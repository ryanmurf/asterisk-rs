//! Port of asterisk/tests/test_http_media_cache.c
//!
//! Tests HTTP media cache operations:
//! - Nominal retrieval
//! - Content-Type based extension detection
//! - URI path parsing for extension
//! - Cache-Control directives (no-cache, must-revalidate)
//! - Cache-Control age (max-age, s-maxage) and precedence
//! - Expires header handling
//! - ETag staleness checks
//! - Nominal creation

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Simulated HTTP media cache
// ---------------------------------------------------------------------------

/// Cache-control directives parsed from response headers.
#[derive(Debug, Clone, Default)]
struct CacheControl {
    max_age: Option<u64>,
    s_maxage: Option<u64>,
    no_cache: bool,
    must_revalidate: bool,
}

/// Metadata for a cached media file.
#[derive(Debug, Clone)]
struct CachedFile {
    uri: String,
    path: String,
    metadata: HashMap<String, String>,
    expires_epoch: Option<u64>,
    etag: Option<String>,
    no_cache: bool,
    must_revalidate: bool,
}

impl CachedFile {
    fn is_stale(&self, current_etag: Option<&str>) -> bool {
        // If no_cache or must_revalidate: always stale UNLESS we have a matching ETag.
        if self.no_cache || self.must_revalidate {
            if let (Some(our_etag), Some(server_etag)) = (&self.etag, current_etag) {
                return our_etag != server_etag;
            }
            return true;
        }
        // If expired, stale (unless ETag matches).
        if let Some(expires) = self.expires_epoch {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now > expires {
                // Expired, but ETag match saves us.
                if let (Some(our_etag), Some(server_etag)) = (&self.etag, current_etag) {
                    return our_etag != server_etag;
                }
                return true;
            }
        }
        false
    }
}

/// Parse extension from a URI, handling query strings.
fn parse_extension(uri: &str) -> Option<String> {
    let path = uri.split('?').next().unwrap_or(uri);
    if let Some(dot) = path.rfind('.') {
        let ext = &path[dot..];
        if !ext.contains('/') {
            return Some(ext.to_string());
        }
    }
    None
}

/// Determine extension from content-type header.
fn extension_from_content_type(ct: &str) -> Option<&'static str> {
    match ct {
        "audio/wav" | "audio/x-wav" => Some(".wav"),
        "audio/mpeg" => Some(".mp3"),
        "audio/ogg" => Some(".ogg"),
        _ => None,
    }
}

/// Compute the effective expiration epoch given cache-control and expires.
fn compute_expires(
    cc: &CacheControl,
    explicit_expires: Option<u64>,
    now: u64,
) -> Option<u64> {
    // s-maxage takes priority over max-age, which takes priority over Expires.
    if let Some(s) = cc.s_maxage {
        return Some(now + s);
    }
    if let Some(m) = cc.max_age {
        return Some(now + m);
    }
    explicit_expires
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(retrieve_nominal) from test_http_media_cache.c.
///
/// Nominal retrieval of a resource (no special headers).
#[test]
fn test_retrieve_nominal() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let cc = CacheControl::default();
    let expires = compute_expires(&cc, None, now);

    let file = CachedFile {
        uri: "http://localhost:8088/test/foo.wav".to_string(),
        path: "/tmp/cached.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: expires,
        etag: None,
        no_cache: false,
        must_revalidate: false,
    };

    assert!(!file.path.is_empty());
    assert!(!file.is_stale(None));
}

/// Port of AST_TEST_DEFINE(retrieve_content_type).
///
/// Content-Type header used to determine file extension.
#[test]
fn test_retrieve_content_type() {
    let ext = extension_from_content_type("audio/wav");
    assert_eq!(ext, Some(".wav"));
}

/// Port of AST_TEST_DEFINE(retrieve_parsed_uri).
///
/// Extension parsed from the path portion of the URI.
#[test]
fn test_retrieve_parsed_uri() {
    let ext = parse_extension("http://localhost:8088/foo.wav?account_id=1234");
    assert_eq!(ext.as_deref(), Some(".wav"));
}

/// No extension in URI.
#[test]
fn test_retrieve_no_extension() {
    let ext = parse_extension("http://localhost:8088/media/get");
    assert!(ext.is_none());
}

/// Port of AST_TEST_DEFINE(retrieve_cache_control_directives).
///
/// no-cache and must-revalidate make a resource stale unless ETag matches.
#[test]
fn test_cache_control_no_cache() {
    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: None,
        etag: None,
        no_cache: true,
        must_revalidate: false,
    };
    assert!(file.is_stale(None));
}

#[test]
fn test_cache_control_no_cache_with_etag() {
    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: None,
        etag: Some("123456789".to_string()),
        no_cache: true,
        must_revalidate: false,
    };
    // Matching ETag means NOT stale.
    assert!(!file.is_stale(Some("123456789")));
    // Different ETag means stale.
    assert!(file.is_stale(Some("999999")));
}

#[test]
fn test_cache_control_must_revalidate() {
    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: None,
        etag: None,
        no_cache: false,
        must_revalidate: true,
    };
    assert!(file.is_stale(None));
}

#[test]
fn test_cache_control_must_revalidate_with_etag() {
    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: None,
        etag: Some("123456789".to_string()),
        no_cache: false,
        must_revalidate: true,
    };
    assert!(!file.is_stale(Some("123456789")));
}

/// Port of AST_TEST_DEFINE(retrieve_cache_control_age).
///
/// max-age and s-maxage control expiration; s-maxage takes precedence.
#[test]
fn test_cache_control_max_age() {
    let now = 1000u64;
    let cc = CacheControl {
        max_age: Some(300),
        ..Default::default()
    };
    let exp = compute_expires(&cc, None, now);
    assert_eq!(exp, Some(1300));
}

#[test]
fn test_cache_control_s_maxage() {
    let now = 1000u64;
    let cc = CacheControl {
        s_maxage: Some(300),
        ..Default::default()
    };
    let exp = compute_expires(&cc, None, now);
    assert_eq!(exp, Some(1300));
}

#[test]
fn test_cache_control_s_maxage_over_max_age() {
    let now = 1000u64;
    let cc = CacheControl {
        max_age: Some(300),
        s_maxage: Some(600),
        ..Default::default()
    };
    let exp = compute_expires(&cc, None, now);
    // s-maxage wins.
    assert_eq!(exp, Some(1600));
}

#[test]
fn test_cache_control_max_age_over_expires() {
    let now = 1000u64;
    let cc = CacheControl {
        max_age: Some(300),
        ..Default::default()
    };
    let exp = compute_expires(&cc, Some(now + 3000), now);
    // max-age wins over explicit Expires.
    assert_eq!(exp, Some(1300));
}

/// Port of AST_TEST_DEFINE(retrieve_expires).
///
/// Expires header controls staleness when no cache-control age is present.
#[test]
fn test_expires_not_expired() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: Some(now + 3000),
        etag: None,
        no_cache: false,
        must_revalidate: false,
    };
    assert!(!file.is_stale(None));
}

#[test]
fn test_expires_expired() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: Some(now.saturating_sub(10)),
        etag: None,
        no_cache: false,
        must_revalidate: false,
    };
    assert!(file.is_stale(None));
}

/// Port of AST_TEST_DEFINE(retrieve_etag).
///
/// ETag matching prevents staleness even when expired.
#[test]
fn test_etag_expired_but_matching() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let file = CachedFile {
        uri: "test".to_string(),
        path: "/tmp/f.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: Some(now.saturating_sub(10)),
        etag: Some("123456789".to_string()),
        no_cache: false,
        must_revalidate: false,
    };
    // Matching etag = not stale.
    assert!(!file.is_stale(Some("123456789")));
    // Different etag = stale.
    assert!(file.is_stale(Some("99999999")));
}

/// Port of AST_TEST_DEFINE(create_nominal).
///
/// Nominal creation of a bucket file resource.
#[test]
fn test_create_nominal() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cc = CacheControl::default();
    let exp = compute_expires(&cc, None, now);

    let file = CachedFile {
        uri: "http://localhost:8088/foo.wav".to_string(),
        path: "/tmp/created.wav".to_string(),
        metadata: HashMap::new(),
        expires_epoch: exp,
        etag: None,
        no_cache: false,
        must_revalidate: false,
    };
    assert!(!file.path.is_empty());
    assert!(!file.uri.is_empty());
}
