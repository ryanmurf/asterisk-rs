//! Port of asterisk/tests/test_astobj2.c
//!
//! The C `ao2` (Asterisk Object 2) system provides reference-counted containers
//! (hash tables, linked lists, red-black trees). In Rust we use standard
//! collections (HashMap, BTreeMap, Vec) plus Arc for reference counting.
//!
//! This test file ports the behavioral guarantees:
//! - Container create, insert, find, remove
//! - Iteration
//! - Duplicate key handling
//! - Large container operations
//! - Container clone
//! - Callback/filter operations
//! - Iterator with unlink

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Container create, insert, count
// ---------------------------------------------------------------------------

/// Port of container allocation from astobj2_test_1_helper.
/// Verify that a newly created container is empty.
#[test]
fn test_container_create_empty() {
    let map: HashMap<i32, String> = HashMap::new();
    assert_eq!(map.len(), 0);
    assert!(map.is_empty());

    let btree: BTreeMap<i32, String> = BTreeMap::new();
    assert_eq!(btree.len(), 0);
    assert!(btree.is_empty());
}

/// Port of container link operations from astobj2_test_1_helper.
/// Insert N elements and verify count.
#[test]
fn test_container_insert_count() {
    let mut map: HashMap<i32, Arc<TestObj>> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        let obj = Arc::new(TestObj { key: i, dup: 0 });
        map.insert(i, obj);
        assert_eq!(map.len(), (i + 1) as usize);
    }

    assert_eq!(map.len(), limit as usize);
}

#[derive(Debug, Clone)]
struct TestObj {
    key: i32,
    dup: i32,
}

// ---------------------------------------------------------------------------
// Container find operations
// ---------------------------------------------------------------------------

/// Port of test_ao2_find_w_no_flags / test_ao2_find_w_OBJ_KEY.
/// Find random objects by key.
#[test]
fn test_container_find_by_key() {
    let mut map: HashMap<i32, Arc<TestObj>> = HashMap::new();
    let limit = 200;

    for i in 0..limit {
        map.insert(i, Arc::new(TestObj { key: i, dup: 0 }));
    }

    // Find 100 random objects.
    for _ in 0..100 {
        let key = rand::random::<i32>().rem_euclid(limit);
        let obj = map.get(&key);
        assert!(obj.is_some(), "Could not find key {}", key);
        assert_eq!(obj.unwrap().key, key);
    }
}

/// Port of test_ao2_find_w_OBJ_POINTER.
/// Find objects by reference comparison (in Rust, by Arc pointer).
#[test]
fn test_container_find_by_pointer() {
    let mut map: HashMap<i32, Arc<TestObj>> = HashMap::new();
    let mut refs = Vec::new();
    let limit = 50;

    for i in 0..limit {
        let obj = Arc::new(TestObj { key: i, dup: 0 });
        refs.push(Arc::clone(&obj));
        map.insert(i, obj);
    }

    for r in &refs {
        let found = map.get(&r.key).unwrap();
        assert!(Arc::ptr_eq(found, r));
    }
}

/// Port of test_ao2_find_w_OBJ_PARTIAL_KEY.
/// Find objects within a range (partial key match).
#[test]
fn test_container_find_partial_key() {
    let mut btree: BTreeMap<i32, TestObj> = BTreeMap::new();
    let limit = 100;

    for i in 0..limit {
        btree.insert(i, TestObj { key: i, dup: 0 });
    }

    // Find all objects in range [10, 20].
    let range: Vec<_> = btree.range(10..=20).collect();
    assert_eq!(range.len(), 11);
    for (k, v) in &range {
        assert!(**k >= 10 && **k <= 20);
        assert_eq!(v.key, **k);
    }
}

// ---------------------------------------------------------------------------
// Container clone
// ---------------------------------------------------------------------------

/// Port of test_container_clone.
/// Clone a container and verify it has the same elements.
#[test]
fn test_container_clone() {
    let mut orig: HashMap<i32, Arc<TestObj>> = HashMap::new();
    let limit = 50;

    for i in 0..limit {
        orig.insert(i, Arc::new(TestObj { key: i, dup: 0 }));
    }

    let clone = orig.clone();
    assert_eq!(orig.len(), clone.len());

    for (key, obj) in &orig {
        let cloned_obj = clone.get(key).unwrap();
        assert_eq!(obj.key, cloned_obj.key);
        // Arc::clone means same object (reference counted).
        assert!(Arc::ptr_eq(obj, cloned_obj));
    }
}

// ---------------------------------------------------------------------------
// Callback / filter operations
// ---------------------------------------------------------------------------

/// Port of increment_cb test.
/// Iterate all elements and count them via callback.
#[test]
fn test_container_callback_count() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    let mut count = 0;
    for _ in map.values() {
        count += 1;
    }
    assert_eq!(count, limit);
}

/// Port of all_but_one_cb test.
/// Filter all elements except one.
#[test]
fn test_container_filter_all_but_one() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 50;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    // Collect all objects where key != 0.
    let filtered: Vec<_> = map.values().filter(|obj| obj.key != 0).collect();
    assert_eq!(filtered.len(), (limit - 1) as usize);
}

/// Port of multiple_cb test.
/// Find all objects where key < threshold.
#[test]
fn test_container_filter_multiple() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    let threshold = 25;
    let matched: Vec<_> = map
        .values()
        .filter(|obj| obj.key < threshold)
        .collect();
    assert_eq!(matched.len(), threshold as usize);
}

// ---------------------------------------------------------------------------
// Remove operations
// ---------------------------------------------------------------------------

/// Port of iterator unlink test.
/// Remove a random object via iteration.
#[test]
fn test_container_remove_by_key() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    let to_remove = 42;
    map.remove(&to_remove);
    assert_eq!(map.len(), (limit - 1) as usize);
    assert!(map.get(&to_remove).is_none());
}

/// Port of OBJ_MULTIPLE | OBJ_UNLINK test.
/// Remove all objects matching a predicate.
#[test]
fn test_container_remove_multiple() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    let threshold = 25;
    let removed: Vec<_> = map
        .iter()
        .filter(|(_, v)| v.key < threshold)
        .map(|(k, _)| *k)
        .collect();

    for k in &removed {
        map.remove(k);
    }

    assert_eq!(removed.len(), threshold as usize);
    assert_eq!(map.len(), (limit - threshold) as usize);
}

/// Re-link removed objects.
#[test]
fn test_container_remove_and_relink() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();
    let limit = 100;

    for i in 0..limit {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    // Remove objects with key < 25.
    let removed: Vec<(i32, TestObj)> = map
        .iter()
        .filter(|(_, v)| v.key < 25)
        .map(|(k, v)| (*k, v.clone()))
        .collect();

    for (k, _) in &removed {
        map.remove(k);
    }
    assert_eq!(map.len(), 75);

    // Re-link them.
    for (k, v) in removed {
        map.insert(k, v);
    }
    assert_eq!(map.len(), limit as usize);
}

// ---------------------------------------------------------------------------
// Duplicate key handling
// ---------------------------------------------------------------------------

/// Port of duplicate key insertion tests from test_astobj2.c.
/// Verify that inserting with the same key replaces the old value.
#[test]
fn test_container_duplicate_key_replaces() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();

    map.insert(1, TestObj { key: 1, dup: 0 });
    map.insert(1, TestObj { key: 1, dup: 1 });

    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&1).unwrap().dup, 1);
}

/// BTreeMap allows ordered duplicates via separate keys.
#[test]
fn test_btree_ordered_iteration() {
    let mut btree: BTreeMap<i32, TestObj> = BTreeMap::new();

    for i in (0..50).rev() {
        btree.insert(i, TestObj { key: i, dup: 0 });
    }

    let keys: Vec<i32> = btree.keys().copied().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "BTreeMap should iterate in sorted order");
}

// ---------------------------------------------------------------------------
// Large container operations
// ---------------------------------------------------------------------------

/// Port of large container test from test_astobj2.c.
/// Test operations on a container with 10,000 elements.
#[test]
fn test_large_container_operations() {
    let mut map: HashMap<i32, TestObj> = HashMap::with_capacity(10_000);

    // Insert 10,000 elements.
    for i in 0..10_000 {
        map.insert(i, TestObj { key: i, dup: 0 });
    }
    assert_eq!(map.len(), 10_000);

    // Find 1000 random elements.
    for _ in 0..1000 {
        let key = rand::random::<i32>().rem_euclid(10_000);
        assert!(map.contains_key(&key));
    }

    // Remove half.
    for i in 0..5_000 {
        map.remove(&i);
    }
    assert_eq!(map.len(), 5_000);

    // Verify remaining.
    for i in 5_000..10_000 {
        assert!(map.contains_key(&i));
    }
    for i in 0..5_000 {
        assert!(!map.contains_key(&i));
    }
}

/// Test large BTreeMap operations.
#[test]
fn test_large_btree_operations() {
    let mut btree: BTreeMap<i32, TestObj> = BTreeMap::new();

    for i in 0..10_000 {
        btree.insert(i, TestObj { key: i, dup: 0 });
    }
    assert_eq!(btree.len(), 10_000);

    // Range query.
    let range: Vec<_> = btree.range(1000..2000).collect();
    assert_eq!(range.len(), 1000);

    // Remove range.
    for i in 0..5_000 {
        btree.remove(&i);
    }
    assert_eq!(btree.len(), 5_000);
    assert_eq!(*btree.keys().next().unwrap(), 5_000);
}

// ---------------------------------------------------------------------------
// Iterator operations
// ---------------------------------------------------------------------------

/// Port of ao2_iterator tests from test_astobj2.c.
/// Test iteration with modification.
#[test]
fn test_container_iteration() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();

    for i in 0..50 {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    let count = map.iter().count();
    assert_eq!(count, 50);

    // Collect keys to remove during "iteration"
    let to_remove: Vec<i32> = map.keys().filter(|&&k| k % 2 == 0).copied().collect();
    for k in to_remove {
        map.remove(&k);
    }
    assert_eq!(map.len(), 25);

    // Verify only odd keys remain.
    for (k, v) in &map {
        assert!(k % 2 != 0);
        assert_eq!(v.key, *k);
    }
}

/// Test Arc reference counting behavior (mirrors ao2 refcount).
#[test]
fn test_arc_refcount() {
    let obj = Arc::new(TestObj { key: 42, dup: 0 });
    assert_eq!(Arc::strong_count(&obj), 1);

    let obj2 = Arc::clone(&obj);
    assert_eq!(Arc::strong_count(&obj), 2);

    drop(obj2);
    assert_eq!(Arc::strong_count(&obj), 1);
}

/// Test container stats (mirrors ao2_container_stats).
#[test]
fn test_container_stats() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();

    assert!(map.is_empty());
    assert_eq!(map.len(), 0);

    for i in 0..100 {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    assert!(!map.is_empty());
    assert_eq!(map.len(), 100);
    assert!(map.capacity() >= 100);
}

/// Test clear operation (mirrors ao2_container_unlink_all).
#[test]
fn test_container_clear() {
    let mut map: HashMap<i32, TestObj> = HashMap::new();

    for i in 0..100 {
        map.insert(i, TestObj { key: i, dup: 0 });
    }

    map.clear();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}
