//! Port of asterisk/tests/test_vector.c
//!
//! Tests Vec operations (the Rust equivalent of AST_VECTOR):
//! - Push/pop/insert/remove
//! - Sort, filter, dedup
//! - Capacity management
//! - Iterator operations
//! - Ordered and unordered removal
//! - Search/comparison operations

// ---------------------------------------------------------------------------
// Basic operations
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(basic_ops) from test_vector.c.
///
/// Test basic vector append, insert, remove operations.
#[test]
fn test_vector_basic_append() {
    let mut v: Vec<&str> = Vec::with_capacity(3);
    assert_eq!(v.len(), 0);

    v.push("AAA");
    v.push("BBB");
    v.push("CCC");

    assert_eq!(v.len(), 3);
    assert_eq!(v[0], "AAA");
    assert_eq!(v[1], "BBB");
    assert_eq!(v[2], "CCC");
}

/// Test insert at a specific position.
#[test]
fn test_vector_insert_at() {
    let mut v = vec!["AAA", "BBB", "CCC"];

    v.insert(1, "ZZZ");
    assert_eq!(v.len(), 4);
    assert_eq!(v[0], "AAA");
    assert_eq!(v[1], "ZZZ");
    assert_eq!(v[2], "BBB");
    assert_eq!(v[3], "CCC");
}

/// Test insert at beginning.
#[test]
fn test_vector_insert_at_beginning() {
    let mut v = vec!["BBB", "CCC"];
    v.insert(0, "AAA");
    assert_eq!(v, vec!["AAA", "BBB", "CCC"]);
}

/// Test insert at end.
#[test]
fn test_vector_insert_at_end() {
    let mut v = vec!["AAA", "BBB"];
    v.insert(2, "CCC");
    assert_eq!(v, vec!["AAA", "BBB", "CCC"]);
}

// ---------------------------------------------------------------------------
// Remove operations
// ---------------------------------------------------------------------------

/// Port of AST_VECTOR_REMOVE_ORDERED from test_vector.c.
///
/// Test ordered removal (preserving order of remaining elements).
#[test]
fn test_vector_remove_ordered() {
    let mut v = vec!["AAA", "ZZZ", "CCC"];
    let removed = v.remove(1);
    assert_eq!(removed, "ZZZ");
    assert_eq!(v, vec!["AAA", "CCC"]);
}

/// Port of AST_VECTOR_REMOVE_UNORDERED from test_vector.c.
///
/// Test unordered removal (swap with last element).
#[test]
fn test_vector_remove_unordered() {
    let mut v = vec!["AAA", "BBB", "CCC", "DDD"];
    // Remove index 0, swap with last.
    v.swap_remove(0);
    assert_eq!(v.len(), 3);
    assert_eq!(v[0], "DDD");
    assert_eq!(v[1], "BBB");
    assert_eq!(v[2], "CCC");
}

/// Test removing by value (retain).
#[test]
fn test_vector_remove_by_value() {
    let mut v = vec!["AAA", "BBB", "CCC", "BBB", "DDD"];
    v.retain(|&x| x != "BBB");
    assert_eq!(v, vec!["AAA", "CCC", "DDD"]);
}

/// Test pop (remove last).
#[test]
fn test_vector_pop() {
    let mut v = vec!["AAA", "BBB", "CCC"];
    let popped = v.pop();
    assert_eq!(popped, Some("CCC"));
    assert_eq!(v.len(), 2);
}

// ---------------------------------------------------------------------------
// Search / find
// ---------------------------------------------------------------------------

/// Port of AST_VECTOR_GET_CMP from test_vector.c.
///
/// Test finding an element by comparison.
#[test]
fn test_vector_find() {
    let v = vec!["AAA", "BBB", "CCC", "DDD"];

    let found = v.iter().find(|&&x| x == "CCC");
    assert_eq!(found, Some(&"CCC"));

    let not_found = v.iter().find(|&&x| x == "ZZZ");
    assert!(not_found.is_none());
}

/// Test position finding.
#[test]
fn test_vector_position() {
    let v = vec!["AAA", "BBB", "CCC"];

    assert_eq!(v.iter().position(|&x| x == "BBB"), Some(1));
    assert_eq!(v.iter().position(|&x| x == "ZZZ"), None);
}

/// Test contains.
#[test]
fn test_vector_contains() {
    let v = vec!["AAA", "BBB", "CCC"];
    assert!(v.contains(&"BBB"));
    assert!(!v.contains(&"DDD"));
}

// ---------------------------------------------------------------------------
// Sort and filter
// ---------------------------------------------------------------------------

/// Test sorting.
#[test]
fn test_vector_sort() {
    let mut v = vec![5, 3, 1, 4, 2];
    v.sort();
    assert_eq!(v, vec![1, 2, 3, 4, 5]);
}

/// Test sort with custom comparator.
#[test]
fn test_vector_sort_by() {
    let mut v = vec!["banana", "apple", "cherry"];
    v.sort_by(|a, b| a.cmp(b));
    assert_eq!(v, vec!["apple", "banana", "cherry"]);
}

/// Test sort descending.
#[test]
fn test_vector_sort_descending() {
    let mut v = vec![1, 2, 3, 4, 5];
    v.sort_by(|a, b| b.cmp(a));
    assert_eq!(v, vec![5, 4, 3, 2, 1]);
}

/// Test filter operation.
#[test]
fn test_vector_filter() {
    let v = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let evens: Vec<i32> = v.into_iter().filter(|&x| x % 2 == 0).collect();
    assert_eq!(evens, vec![2, 4, 6, 8, 10]);
}

/// Test dedup after sort.
#[test]
fn test_vector_dedup() {
    let mut v = vec![3, 1, 2, 1, 3, 2, 1];
    v.sort();
    v.dedup();
    assert_eq!(v, vec![1, 2, 3]);
}

// ---------------------------------------------------------------------------
// Capacity management
// ---------------------------------------------------------------------------

/// Test that capacity grows as needed.
#[test]
fn test_vector_capacity_growth() {
    let mut v: Vec<i32> = Vec::with_capacity(3);
    assert!(v.capacity() >= 3);

    for i in 0..100 {
        v.push(i);
    }

    assert_eq!(v.len(), 100);
    assert!(v.capacity() >= 100);
}

/// Test reserve and shrink.
#[test]
fn test_vector_reserve_shrink() {
    let mut v: Vec<i32> = Vec::new();
    v.reserve(100);
    assert!(v.capacity() >= 100);

    v.push(1);
    v.push(2);
    v.shrink_to_fit();
    assert!(v.capacity() >= 2);
    assert_eq!(v.len(), 2);
}

/// Test clear.
#[test]
fn test_vector_clear() {
    let mut v = vec![1, 2, 3, 4, 5];
    v.clear();
    assert!(v.is_empty());
    assert_eq!(v.len(), 0);
}

/// Test with_capacity and initial size 0.
#[test]
fn test_vector_init_empty() {
    let v: Vec<i32> = Vec::with_capacity(0);
    assert_eq!(v.len(), 0);
    assert!(v.is_empty());
}

// ---------------------------------------------------------------------------
// Iterator operations
// ---------------------------------------------------------------------------

/// Test map iterator.
#[test]
fn test_vector_iter_map() {
    let v = vec![1, 2, 3, 4, 5];
    let doubled: Vec<i32> = v.iter().map(|&x| x * 2).collect();
    assert_eq!(doubled, vec![2, 4, 6, 8, 10]);
}

/// Test fold/reduce.
#[test]
fn test_vector_iter_fold() {
    let v = vec![1, 2, 3, 4, 5];
    let sum: i32 = v.iter().sum();
    assert_eq!(sum, 15);
}

/// Test enumerate.
#[test]
fn test_vector_iter_enumerate() {
    let v = vec!["a", "b", "c"];
    for (i, &val) in v.iter().enumerate() {
        match i {
            0 => assert_eq!(val, "a"),
            1 => assert_eq!(val, "b"),
            2 => assert_eq!(val, "c"),
            _ => panic!("unexpected index"),
        }
    }
}

/// Test any and all.
#[test]
fn test_vector_iter_any_all() {
    let v = vec![2, 4, 6, 8];
    assert!(v.iter().all(|&x| x % 2 == 0));
    assert!(!v.iter().any(|&x| x % 2 != 0));

    let v2 = vec![1, 2, 3];
    assert!(v2.iter().any(|&x| x > 2));
    assert!(!v2.iter().all(|&x| x > 2));
}

// ---------------------------------------------------------------------------
// Replace operations
// ---------------------------------------------------------------------------

/// Port of AST_VECTOR_REPLACE from test_vector.c.
///
/// Test replacing elements at specific indices.
#[test]
fn test_vector_replace() {
    let mut v = vec!["AAA", "BBB", "CCC"];

    // Replace index 1.
    v[1] = "ZZZ";
    assert_eq!(v, vec!["AAA", "ZZZ", "CCC"]);
}

/// Test extending a vector.
#[test]
fn test_vector_extend() {
    let mut v1 = vec![1, 2, 3];
    let v2 = vec![4, 5, 6];
    v1.extend(v2);
    assert_eq!(v1, vec![1, 2, 3, 4, 5, 6]);
}

/// Test truncate.
#[test]
fn test_vector_truncate() {
    let mut v = vec![1, 2, 3, 4, 5];
    v.truncate(3);
    assert_eq!(v, vec![1, 2, 3]);
}

/// Test drain.
#[test]
fn test_vector_drain() {
    let mut v = vec![1, 2, 3, 4, 5];
    let drained: Vec<i32> = v.drain(1..3).collect();
    assert_eq!(drained, vec![2, 3]);
    assert_eq!(v, vec![1, 4, 5]);
}

// ---------------------------------------------------------------------------
// Callback-based cleanup (port of AST_VECTOR_CALLBACK_VOID)
// ---------------------------------------------------------------------------

/// Test that dropping elements invokes cleanup.
#[test]
fn test_vector_cleanup_on_clear() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let count = Arc::new(AtomicUsize::new(0));
    let mut v: Vec<Arc<AtomicUsize>> = Vec::new();

    for _ in 0..5 {
        v.push(Arc::clone(&count));
    }

    // Arc strong count should be 6 (1 original + 5 in vector).
    assert_eq!(Arc::strong_count(&count), 6);

    v.clear();

    // After clear, strong count should be back to 1.
    assert_eq!(Arc::strong_count(&count), 1);
}
