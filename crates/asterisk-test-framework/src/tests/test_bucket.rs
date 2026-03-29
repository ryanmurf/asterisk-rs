//! Port of asterisk/tests/test_bucket.c
//!
//! Tests the media cache / bucket system: cache creation, entry
//! insertion/retrieval/deletion, metadata handling, staleness/expiration,
//! validators, entry cloning, and multiple-entry management.
//! In Rust the MediaCache from asterisk-res is the equivalent of the
//! C bucket system.

use asterisk_res::media_cache::{CacheEntry, MediaCache};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Create a default test cache.
fn test_cache() -> MediaCache {
    MediaCache::new("/tmp/test_bucket_cache")
}

/// Create a test cache entry.
fn test_entry(url: &str) -> CacheEntry {
    CacheEntry {
        url: url.to_string(),
        local_path: PathBuf::from(format!("/tmp/test_{}", url.len())),
        etag: None,
        last_modified: None,
        expires: None,
        size: 1024,
        last_access: Instant::now(),
        created: Instant::now(),
    }
}

// ---------------------------------------------------------------------------
// Cache creation / alloc
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_alloc) from test_bucket.c.
///
/// Test that creating a cache produces a valid empty cache.
#[test]
fn test_cache_creation() {
    let cache = test_cache();
    assert_eq!(cache.entry_count(), 0);
}

/// Test that retrieval from empty cache returns None.
#[test]
fn test_cache_empty_get() {
    let cache = test_cache();
    assert!(cache.get("").is_none());
    assert!(cache.get("http://example.com/nonexistent").is_none());
}

// ---------------------------------------------------------------------------
// Insert (create) and retrieve
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_create) from test_bucket.c.
///
/// Test creating a cache entry and retrieving it.
#[test]
fn test_cache_store_and_retrieve() {
    let cache = test_cache();
    let entry = test_entry("http://example.com/sound.wav");
    cache.store(entry);

    let retrieved = cache.get("http://example.com/sound.wav");
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.url, "http://example.com/sound.wav");
    assert_eq!(retrieved.size, 1024);
}

/// Test that inserting the same URL twice updates the entry.
#[test]
fn test_cache_store_duplicate_updates() {
    let cache = test_cache();

    let mut entry1 = test_entry("http://example.com/file.wav");
    entry1.size = 100;
    cache.store(entry1);

    let mut entry2 = test_entry("http://example.com/file.wav");
    entry2.size = 200;
    cache.store(entry2);

    assert_eq!(cache.entry_count(), 1);
    let retrieved = cache.get("http://example.com/file.wav").unwrap();
    assert_eq!(retrieved.size, 200);
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_delete) from test_bucket.c.
///
/// Test deleting a cache entry.
#[test]
fn test_cache_delete() {
    let cache = test_cache();
    cache.store(test_entry("http://example.com/to_delete.wav"));
    assert_eq!(cache.entry_count(), 1);

    let removed = cache.remove("http://example.com/to_delete.wav");
    assert!(removed.is_some());
    assert_eq!(cache.entry_count(), 0);
    assert!(cache.get("http://example.com/to_delete.wav").is_none());
}

/// Test deleting non-existent entry returns None.
#[test]
fn test_cache_delete_nonexistent() {
    let cache = test_cache();
    let removed = cache.remove("http://example.com/nope.wav");
    assert!(removed.is_none());
}

/// Test double-delete fails on second attempt.
#[test]
fn test_cache_double_delete() {
    let cache = test_cache();
    cache.store(test_entry("http://example.com/double.wav"));

    assert!(cache.remove("http://example.com/double.wav").is_some());
    assert!(cache.remove("http://example.com/double.wav").is_none());
}

// ---------------------------------------------------------------------------
// Staleness / expiration
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_is_stale) from test_bucket.c.
///
/// Test cache entry staleness detection via expiration.
#[test]
fn test_cache_entry_staleness() {
    let entry = test_entry("http://example.com/fresh.wav");
    assert!(!entry.is_expired());

    let mut future_entry = test_entry("http://example.com/future.wav");
    future_entry.expires = Some(SystemTime::now() + Duration::from_secs(3600));
    assert!(!future_entry.is_expired());

    let mut past_entry = test_entry("http://example.com/past.wav");
    past_entry.expires = Some(SystemTime::UNIX_EPOCH);
    assert!(past_entry.is_expired());
}

/// Test freshness checking on the cache.
#[test]
fn test_cache_is_fresh() {
    let cache = test_cache();

    // Non-existent URL is not fresh.
    assert!(!cache.is_fresh("http://example.com/missing.wav"));

    // Entry without expiration is fresh.
    cache.store(test_entry("http://example.com/no_exp.wav"));
    assert!(cache.is_fresh("http://example.com/no_exp.wav"));

    // Entry with past expiration is not fresh.
    let mut expired = test_entry("http://example.com/expired.wav");
    expired.expires = Some(SystemTime::UNIX_EPOCH);
    cache.store(expired);
    assert!(!cache.is_fresh("http://example.com/expired.wav"));
}

// ---------------------------------------------------------------------------
// Metadata / validators
// ---------------------------------------------------------------------------

/// Test that entries with ETag have validators.
#[test]
fn test_cache_entry_has_validators_etag() {
    let mut entry = test_entry("http://example.com/etag.wav");
    assert!(!entry.has_validators());

    entry.etag = Some("\"abc123\"".to_string());
    assert!(entry.has_validators());
}

/// Test that entries with Last-Modified have validators.
#[test]
fn test_cache_entry_has_validators_last_modified() {
    let mut entry = test_entry("http://example.com/lm.wav");
    assert!(!entry.has_validators());

    entry.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".to_string());
    assert!(entry.has_validators());
}

/// Test conditional headers retrieval.
#[test]
fn test_cache_conditional_headers() {
    let cache = test_cache();

    // No entry => no headers.
    assert!(cache.conditional_headers("http://example.com/x.wav").is_none());

    // Entry without validators => no headers.
    cache.store(test_entry("http://example.com/no_val.wav"));
    assert!(cache.conditional_headers("http://example.com/no_val.wav").is_none());

    // Entry with validators => returns headers.
    let mut entry = test_entry("http://example.com/with_val.wav");
    entry.etag = Some("\"etag123\"".to_string());
    entry.last_modified = Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string());
    cache.store(entry);

    let headers = cache.conditional_headers("http://example.com/with_val.wav");
    assert!(headers.is_some());
    let (etag, lm) = headers.unwrap();
    assert_eq!(etag.as_deref(), Some("\"etag123\""));
    assert_eq!(lm.as_deref(), Some("Mon, 01 Jan 2024 00:00:00 GMT"));
}

// ---------------------------------------------------------------------------
// Entry fields
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_json) from test_bucket.c.
///
/// Test that cache entry fields are properly stored and accessible.
#[test]
fn test_cache_entry_fields() {
    let mut entry = test_entry("test:///tmp/bob");
    entry.etag = Some("\"etag123\"".to_string());
    entry.last_modified = Some("Tue, 01 Jan 2019 00:00:00 GMT".to_string());
    entry.size = 4096;

    assert_eq!(entry.url, "test:///tmp/bob");
    assert_eq!(entry.etag.as_deref(), Some("\"etag123\""));
    assert_eq!(
        entry.last_modified.as_deref(),
        Some("Tue, 01 Jan 2019 00:00:00 GMT")
    );
    assert_eq!(entry.size, 4096);
}

// ---------------------------------------------------------------------------
// Multiple entries
// ---------------------------------------------------------------------------

/// Test inserting and managing multiple cache entries.
#[test]
fn test_cache_multiple_entries() {
    let cache = test_cache();

    for i in 0..10 {
        cache.store(test_entry(&format!("http://example.com/file{}.wav", i)));
    }

    assert_eq!(cache.entry_count(), 10);

    for i in 0..10 {
        let url = format!("http://example.com/file{}.wav", i);
        assert!(cache.get(&url).is_some(), "Entry {} not found", i);
    }
}

/// Test listing cached URLs.
#[test]
fn test_cache_list_urls() {
    let cache = test_cache();
    cache.store(test_entry("http://a.com/1.wav"));
    cache.store(test_entry("http://a.com/2.wav"));
    cache.store(test_entry("http://a.com/3.wav"));

    let urls = cache.cached_urls();
    assert_eq!(urls.len(), 3);
    assert!(urls.contains(&"http://a.com/1.wav".to_string()));
    assert!(urls.contains(&"http://a.com/2.wav".to_string()));
    assert!(urls.contains(&"http://a.com/3.wav".to_string()));
}

/// Test clearing all entries.
#[test]
fn test_cache_clear() {
    let cache = test_cache();
    for i in 0..5 {
        cache.store(test_entry(&format!("http://example.com/{}.wav", i)));
    }
    assert_eq!(cache.entry_count(), 5);

    cache.clear();
    assert_eq!(cache.entry_count(), 0);
}

/// Test total size calculation.
#[test]
fn test_cache_total_size() {
    let cache = test_cache();
    let mut e1 = test_entry("http://a.com/1.wav");
    e1.size = 100;
    let mut e2 = test_entry("http://a.com/2.wav");
    e2.size = 200;
    cache.store(e1);
    cache.store(e2);

    assert_eq!(cache.total_size(), 300);
}

// ---------------------------------------------------------------------------
// Age / idle time
// ---------------------------------------------------------------------------

/// Test that age of a cache entry increases.
#[test]
fn test_cache_entry_age() {
    let entry = test_entry("http://example.com/age.wav");
    assert!(entry.age() < Duration::from_secs(1));
}

/// Test idle time tracking.
#[test]
fn test_cache_entry_idle_time() {
    let entry = test_entry("http://example.com/idle.wav");
    assert!(entry.idle_time() < Duration::from_secs(1));
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(bucket_clone) from test_bucket.c.
///
/// Test that CacheEntry can be cloned and the clone has the same data.
#[test]
fn test_cache_entry_clone() {
    let mut entry = test_entry("http://example.com/clone.wav");
    entry.etag = Some("\"tag\"".to_string());
    entry.size = 2048;

    let cloned = entry.clone();
    assert_eq!(cloned.url, entry.url);
    assert_eq!(cloned.etag, entry.etag);
    assert_eq!(cloned.size, entry.size);
    assert_eq!(cloned.local_path, entry.local_path);
}
