//! HTTP media cache.
//!
//! Port of `res/res_http_media_cache.c`. Provides a local file cache for
//! media fetched over HTTP, with support for ETag and Last-Modified
//! freshness checks and configurable eviction policies.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum MediaCacheError {
    #[error("cache entry not found for URL: {0}")]
    NotFound(String),
    #[error("fetch failed for URL {url}: {reason}")]
    FetchFailed { url: String, reason: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cache error: {0}")]
    Other(String),
}

pub type MediaCacheResult<T> = Result<T, MediaCacheError>;

// ---------------------------------------------------------------------------
// Cache entry
// ---------------------------------------------------------------------------

/// A single cached media file.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The source URL.
    pub url: String,
    /// Local file path where the content is stored.
    pub local_path: PathBuf,
    /// HTTP ETag for conditional requests.
    pub etag: Option<String>,
    /// HTTP Last-Modified value.
    pub last_modified: Option<String>,
    /// Expiry time (if the server provided Cache-Control or Expires).
    pub expires: Option<SystemTime>,
    /// Size in bytes on disk.
    pub size: u64,
    /// When this entry was last accessed.
    pub last_access: Instant,
    /// When this entry was first created.
    pub created: Instant,
}

impl CacheEntry {
    /// Whether this entry has expired based on its `expires` field.
    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires {
            SystemTime::now() > exp
        } else {
            false
        }
    }

    /// Whether the entry has freshness information that can be validated.
    pub fn has_validators(&self) -> bool {
        self.etag.is_some() || self.last_modified.is_some()
    }

    /// Age of this cache entry.
    pub fn age(&self) -> Duration {
        self.created.elapsed()
    }

    /// Time since last access.
    pub fn idle_time(&self) -> Duration {
        self.last_access.elapsed()
    }
}

// ---------------------------------------------------------------------------
// Eviction policy
// ---------------------------------------------------------------------------

/// Cache eviction policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Least Recently Used: evict the entry that was accessed longest ago.
    Lru,
    /// Time To Live: evict entries older than a configured duration.
    Ttl,
}

// ---------------------------------------------------------------------------
// Media cache
// ---------------------------------------------------------------------------

/// HTTP media cache manager.
///
/// Maintains a mapping from URLs to locally cached files. Supports
/// conditional HTTP requests (ETag/Last-Modified) and eviction.
pub struct MediaCache {
    /// Base directory for cached files.
    pub cache_dir: PathBuf,
    /// Entries keyed by URL.
    entries: RwLock<HashMap<String, CacheEntry>>,
    /// Maximum cache size in bytes (0 = unlimited).
    pub max_size: u64,
    /// Maximum number of entries (0 = unlimited).
    pub max_entries: usize,
    /// TTL for entries when using TTL eviction policy.
    pub ttl: Duration,
    /// Eviction policy.
    pub eviction_policy: EvictionPolicy,
}

impl MediaCache {
    /// Create a new media cache.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            entries: RwLock::new(HashMap::new()),
            max_size: 0,
            max_entries: 500,
            ttl: Duration::from_secs(86400), // 24 hours
            eviction_policy: EvictionPolicy::Lru,
        }
    }

    /// Store a cached entry (typically after a successful fetch).
    pub fn store(&self, entry: CacheEntry) {
        let url = entry.url.clone();
        debug!(url = %url, path = %entry.local_path.display(), "Media cache store");
        self.entries.write().insert(url, entry);
    }

    /// Look up a cached entry by URL, updating its last access time.
    pub fn get(&self, url: &str) -> Option<CacheEntry> {
        let mut entries = self.entries.write();
        if let Some(entry) = entries.get_mut(url) {
            entry.last_access = Instant::now();
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Check whether a URL is cached and still fresh.
    pub fn is_fresh(&self, url: &str) -> bool {
        let entries = self.entries.read();
        entries.get(url).map_or(false, |e| !e.is_expired())
    }

    /// Remove a cache entry by URL.
    pub fn remove(&self, url: &str) -> Option<CacheEntry> {
        let removed = self.entries.write().remove(url);
        if removed.is_some() {
            debug!(url, "Media cache entry removed");
        }
        removed
    }

    /// Build a conditional request check. Returns `(etag, last_modified)` if
    /// validators are available for the given URL.
    pub fn conditional_headers(&self, url: &str) -> Option<(Option<String>, Option<String>)> {
        let entries = self.entries.read();
        entries.get(url).and_then(|e| {
            if e.has_validators() {
                Some((e.etag.clone(), e.last_modified.clone()))
            } else {
                None
            }
        })
    }

    /// Run eviction, removing stale or excess entries.
    ///
    /// Returns the number of entries evicted.
    pub fn evict(&self) -> usize {
        let mut entries = self.entries.write();
        let before = entries.len();

        match self.eviction_policy {
            EvictionPolicy::Ttl => {
                entries.retain(|_url, entry| entry.age() < self.ttl);
            }
            EvictionPolicy::Lru => {
                // Evict expired entries first.
                entries.retain(|_url, entry| !entry.is_expired());

                // If still over max_entries, remove least recently used.
                if self.max_entries > 0 && entries.len() > self.max_entries {
                    let mut by_access: Vec<(String, Instant)> = entries
                        .iter()
                        .map(|(url, e)| (url.clone(), e.last_access))
                        .collect();
                    by_access.sort_by_key(|(_, t)| *t);

                    let to_remove = entries.len() - self.max_entries;
                    for (url, _) in by_access.into_iter().take(to_remove) {
                        entries.remove(&url);
                    }
                }
            }
        }

        let evicted = before - entries.len();
        if evicted > 0 {
            debug!(evicted, remaining = entries.len(), "Media cache eviction");
        }
        evicted
    }

    /// Total number of cached entries.
    pub fn entry_count(&self) -> usize {
        self.entries.read().len()
    }

    /// Total cached size in bytes.
    pub fn total_size(&self) -> u64 {
        self.entries.read().values().map(|e| e.size).sum()
    }

    /// List all cached URLs.
    pub fn cached_urls(&self) -> Vec<String> {
        self.entries.read().keys().cloned().collect()
    }

    /// Clear the entire cache.
    pub fn clear(&self) {
        self.entries.write().clear();
        info!("Media cache cleared");
    }
}

impl fmt::Debug for MediaCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaCache")
            .field("cache_dir", &self.cache_dir)
            .field("entries", &self.entries.read().len())
            .field("eviction_policy", &self.eviction_policy)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(url: &str) -> CacheEntry {
        CacheEntry {
            url: url.to_string(),
            local_path: PathBuf::from(format!("/tmp/cache/{}", url.len())),
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 01 Jan 2025 00:00:00 GMT".to_string()),
            expires: None,
            size: 1024,
            last_access: Instant::now(),
            created: Instant::now(),
        }
    }

    #[test]
    fn test_store_and_get() {
        let cache = MediaCache::new("/tmp/media_cache");
        let entry = make_entry("http://example.com/sound.wav");
        cache.store(entry);

        let retrieved = cache.get("http://example.com/sound.wav");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().url, "http://example.com/sound.wav");
    }

    #[test]
    fn test_get_missing() {
        let cache = MediaCache::new("/tmp/media_cache");
        assert!(cache.get("http://example.com/missing.wav").is_none());
    }

    #[test]
    fn test_remove() {
        let cache = MediaCache::new("/tmp/media_cache");
        cache.store(make_entry("http://example.com/a.wav"));
        assert_eq!(cache.entry_count(), 1);
        cache.remove("http://example.com/a.wav");
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_is_fresh_no_expiry() {
        let cache = MediaCache::new("/tmp/media_cache");
        cache.store(make_entry("http://example.com/a.wav"));
        // No expiry set -> always fresh.
        assert!(cache.is_fresh("http://example.com/a.wav"));
    }

    #[test]
    fn test_is_fresh_expired() {
        let cache = MediaCache::new("/tmp/media_cache");
        let mut entry = make_entry("http://example.com/old.wav");
        entry.expires = Some(SystemTime::now() - Duration::from_secs(3600));
        cache.store(entry);
        assert!(!cache.is_fresh("http://example.com/old.wav"));
    }

    #[test]
    fn test_conditional_headers() {
        let cache = MediaCache::new("/tmp/media_cache");
        cache.store(make_entry("http://example.com/a.wav"));
        let headers = cache.conditional_headers("http://example.com/a.wav");
        assert!(headers.is_some());
        let (etag, lm) = headers.unwrap();
        assert_eq!(etag, Some("\"abc123\"".to_string()));
        assert!(lm.is_some());
    }

    #[test]
    fn test_evict_ttl() {
        let mut cache = MediaCache::new("/tmp/media_cache");
        cache.eviction_policy = EvictionPolicy::Ttl;
        cache.ttl = Duration::from_millis(0); // immediate expiry
        cache.store(make_entry("http://example.com/a.wav"));
        std::thread::sleep(Duration::from_millis(1));
        let evicted = cache.evict();
        assert_eq!(evicted, 1);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_total_size() {
        let cache = MediaCache::new("/tmp/media_cache");
        cache.store(make_entry("http://example.com/a.wav"));
        cache.store(make_entry("http://example.com/b.wav"));
        assert_eq!(cache.total_size(), 2048);
    }

    #[test]
    fn test_clear() {
        let cache = MediaCache::new("/tmp/media_cache");
        cache.store(make_entry("http://example.com/a.wav"));
        cache.store(make_entry("http://example.com/b.wav"));
        cache.clear();
        assert_eq!(cache.entry_count(), 0);
    }
}
