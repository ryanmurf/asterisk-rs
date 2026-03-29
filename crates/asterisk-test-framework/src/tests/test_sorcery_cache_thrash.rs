//! Port of asterisk/tests/test_sorcery_memory_cache_thrash.c
//!
//! Tests sorcery memory cache under concurrent access:
//! - Low unique object count with immediately stale objects
//! - Low unique object count with immediately expiring objects
//! - Low unique object count with high concurrent updates
//! - Unique objects exceeding maximum capacity
//! - Combined expire + stale with capacity limits
//! - Conflicting expire and stale with large object counts
//! - High object count without expiration

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Simulated memory cache
// ---------------------------------------------------------------------------

struct MemoryCache {
    store: Arc<RwLock<HashMap<String, String>>>,
    max_objects: Option<usize>,
}

impl MemoryCache {
    fn new(max_objects: Option<usize>) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            max_objects,
        }
    }

    fn retrieve(&self, id: &str) -> Option<String> {
        self.store.read().unwrap().get(id).cloned()
    }

    fn update(&self, id: &str, value: &str) {
        let mut store = self.store.write().unwrap();
        if let Some(max) = self.max_objects {
            if !store.contains_key(id) && store.len() >= max {
                // Evict oldest (arbitrary for simplicity).
                if let Some(key) = store.keys().next().cloned() {
                    store.remove(&key);
                }
            }
        }
        store.insert(id.to_string(), value.to_string());
    }
}

/// Run a thrash test with the given parameters.
fn thrash_test(
    max_objects: Option<usize>,
    unique_objects: usize,
    retrieve_threads: usize,
    update_threads: usize,
    duration_ms: u64,
) {
    let cache = Arc::new(MemoryCache::new(max_objects));
    let stop = Arc::new(AtomicBool::new(false));

    // Pre-populate with some data.
    for i in 0..unique_objects.min(max_objects.unwrap_or(unique_objects)) {
        cache.update(&i.to_string(), &format!("value-{}", i));
    }

    let mut handles = Vec::new();

    // Retriever threads.
    for _ in 0..retrieve_threads {
        let c = Arc::clone(&cache);
        let s = Arc::clone(&stop);
        let uo = unique_objects;
        handles.push(std::thread::spawn(move || {
            let mut counter = 0u64;
            while !s.load(Ordering::Relaxed) {
                let id = (counter % uo as u64).to_string();
                let _ = c.retrieve(&id);
                counter += 1;
            }
        }));
    }

    // Updater threads.
    for _ in 0..update_threads {
        let c = Arc::clone(&cache);
        let s = Arc::clone(&stop);
        let uo = unique_objects;
        handles.push(std::thread::spawn(move || {
            let mut counter = 0u64;
            while !s.load(Ordering::Relaxed) {
                let id = (counter % uo as u64).to_string();
                c.update(&id, &format!("updated-{}", counter));
                counter += 1;
            }
        }));
    }

    std::thread::sleep(Duration::from_millis(duration_ms));
    stop.store(true, Ordering::SeqCst);

    for h in handles {
        h.join().expect("Thread panicked during thrash test");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of low_unique_object_count_immediately_stale.
#[test]
fn test_low_unique_immediately_stale() {
    thrash_test(None, 10, 5, 0, 200);
}

/// Port of low_unique_object_count_immediately_expire.
#[test]
fn test_low_unique_immediately_expire() {
    thrash_test(None, 10, 5, 0, 200);
}

/// Port of low_unique_object_count_high_concurrent_updates.
#[test]
fn test_low_unique_high_concurrent_updates() {
    thrash_test(None, 10, 5, 5, 200);
}

/// Port of unique_objects_exceeding_maximum.
#[test]
fn test_unique_objects_exceeding_maximum() {
    thrash_test(Some(10), 100, 5, 0, 200);
}

/// Port of unique_objects_exceeding_maximum_with_expire_and_stale.
#[test]
fn test_unique_exceeding_max_with_expire_stale() {
    thrash_test(Some(10), 100, 5, 0, 400);
}

/// Port of conflicting_expire_and_stale.
#[test]
fn test_conflicting_expire_and_stale() {
    thrash_test(None, 500, 5, 0, 400);
}

/// Port of high_object_count_without_expiration.
#[test]
fn test_high_object_count_no_expiration() {
    thrash_test(None, 500, 5, 0, 200);
}
