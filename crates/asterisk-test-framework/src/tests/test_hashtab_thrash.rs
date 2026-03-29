//! Port of asterisk/tests/test_hashtab_thrash.c
//!
//! Tests concurrent hash table operations: simultaneous grow, shrink,
//! lookup, and count operations from multiple threads to verify
//! correctness under contention.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

const MAX_HASH_ENTRIES: usize = 3000;

/// Create a key string from an integer.
fn ht_key(i: i64) -> String {
    format!("key{:08x}", i as u32)
}

/// Port of AST_TEST_DEFINE(hash_test) from test_hashtab_thrash.c.
///
/// Spawns grow, shrink, lookup, and count threads against a shared
/// hash map to verify consistency under concurrent access.
#[test]
fn test_hashtab_thrash() {
    let preload = MAX_HASH_ENTRIES / 2;
    let max_grow = MAX_HASH_ENTRIES - preload;

    let map: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let grow_count = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Preload with negative-keyed entries to be deleted by shrink thread.
    {
        let mut m = map.write().unwrap();
        for i in 1..preload {
            let key = ht_key(-(i as i64));
            m.insert(key.clone(), key);
        }
    }

    let map_grow = Arc::clone(&map);
    let gc = Arc::clone(&grow_count);
    let grow_handle = std::thread::spawn(move || {
        for i in 0..max_grow {
            let key = ht_key(i as i64);
            map_grow.write().unwrap().insert(key.clone(), key);
            gc.fetch_add(1, Ordering::SeqCst);
        }
    });

    let map_lookup = Arc::clone(&map);
    let gc_lookup = Arc::clone(&grow_count);
    let stop_lookup = Arc::clone(&stop);
    let lookup_handle = std::thread::spawn(move || {
        let mut rng_state: u32 = 42;
        while !stop_lookup.load(Ordering::SeqCst) {
            let max = gc_lookup.load(Ordering::SeqCst);
            if max == 0 {
                std::thread::yield_now();
                continue;
            }
            // Simple pseudo-random
            rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
            let i = (rng_state as usize) % max;
            let key = ht_key(i as i64);
            let found = map_lookup.read().unwrap().contains_key(&key);
            assert!(found, "key unexpectedly missing: {}", key);
        }
    });

    let map_shrink = Arc::clone(&map);
    let shrink_handle = std::thread::spawn(move || {
        for i in 1..preload {
            let key = ht_key(-(i as i64));
            let removed = map_shrink.write().unwrap().remove(&key);
            assert!(removed.is_some(), "could not delete object: {}", key);
        }
    });

    let map_count = Arc::clone(&map);
    let gc_count = Arc::clone(&grow_count);
    let stop_count = Arc::clone(&stop);
    let count_handle = std::thread::spawn(move || {
        let mut last_count = 0usize;
        while gc_count.load(Ordering::SeqCst) < max_grow {
            if stop_count.load(Ordering::SeqCst) {
                break;
            }
            let m = map_count.read().unwrap();
            let count = m.keys().filter(|k| k.starts_with("key0")).count();
            drop(m);
            assert!(
                count >= last_count,
                "hashtab unexpectedly shrank: {} < {}",
                count,
                last_count
            );
            last_count = count;
            std::thread::sleep(Duration::from_micros(1));
        }
    });

    grow_handle.join().expect("grow thread panicked");
    shrink_handle.join().expect("shrink thread panicked");
    stop.store(true, Ordering::SeqCst);
    lookup_handle.join().expect("lookup thread panicked");
    count_handle.join().expect("count thread panicked");

    let final_size = map.read().unwrap().len();
    assert_eq!(
        final_size, max_grow,
        "Expected {} entries, got {}",
        max_grow, final_size
    );
}
