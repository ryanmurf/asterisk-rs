//! Port of asterisk/tests/test_linkedlists.c
//!
//! In Rust we use Vec/VecDeque instead of linked lists. This test file
//! ports the behavioral guarantees from test_linkedlists.c to Rust
//! collection operations:
//! - Insert/remove/iterate
//! - Head/tail access
//! - Empty list operations
//! - Large list performance

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Insert/remove/iterate
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(single_ll_tests) - basic insert and iterate.
///
/// Test that elements can be inserted at the front and iterated in order.
#[test]
fn test_insert_front_iterate() {
    let mut list: VecDeque<i32> = VecDeque::new();

    // Insert at front (like AST_LIST_INSERT_HEAD).
    list.push_front(3);
    list.push_front(2);
    list.push_front(1);

    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

/// Test insert at back (like AST_LIST_INSERT_TAIL).
#[test]
fn test_insert_back_iterate() {
    let mut list: VecDeque<i32> = VecDeque::new();

    list.push_back(1);
    list.push_back(2);
    list.push_back(3);

    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

/// Test insert at specific position (like AST_LIST_INSERT_AFTER).
#[test]
fn test_insert_after() {
    let mut list: VecDeque<i32> = VecDeque::new();

    list.push_back(1);
    list.push_back(3);

    // Insert 2 after position 0 (after element 1).
    list.insert(1, 2);

    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

/// Port of remove operations from test_linkedlists.c.
///
/// Test removing from front, back, and specific position.
#[test]
fn test_remove_front() {
    let mut list: VecDeque<i32> = VecDeque::new();
    list.push_back(1);
    list.push_back(2);
    list.push_back(3);

    let removed = list.pop_front().unwrap();
    assert_eq!(removed, 1);
    assert_eq!(list.len(), 2);

    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![2, 3]);
}

#[test]
fn test_remove_back() {
    let mut list: VecDeque<i32> = VecDeque::new();
    list.push_back(1);
    list.push_back(2);
    list.push_back(3);

    let removed = list.pop_back().unwrap();
    assert_eq!(removed, 3);
    assert_eq!(list.len(), 2);

    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![1, 2]);
}

/// Test remove specific element by value.
///
/// Port of AST_LIST_REMOVE from test_linkedlists.c.
#[test]
fn test_remove_specific() {
    let mut list = vec![10, 20, 30, 40, 50];
    list.retain(|&x| x != 30);
    assert_eq!(list, vec![10, 20, 40, 50]);
}

/// Test remove by index.
#[test]
fn test_remove_by_index() {
    let mut list: VecDeque<i32> = VecDeque::from([10, 20, 30, 40, 50]);
    let removed = list.remove(2).unwrap();
    assert_eq!(removed, 30);
    let collected: Vec<i32> = list.iter().copied().collect();
    assert_eq!(collected, vec![10, 20, 40, 50]);
}

// ---------------------------------------------------------------------------
// Head/tail access
// ---------------------------------------------------------------------------

/// Port of AST_LIST_FIRST / AST_LIST_LAST from test_linkedlists.c.
#[test]
fn test_head_access() {
    let mut list: VecDeque<&str> = VecDeque::new();
    assert!(list.front().is_none());

    list.push_back("first");
    list.push_back("second");
    list.push_back("third");

    assert_eq!(*list.front().unwrap(), "first");
}

#[test]
fn test_tail_access() {
    let mut list: VecDeque<&str> = VecDeque::new();
    assert!(list.back().is_none());

    list.push_back("first");
    list.push_back("second");
    list.push_back("third");

    assert_eq!(*list.back().unwrap(), "third");
}

/// Test that head updates after front removal.
#[test]
fn test_head_after_remove() {
    let mut list: VecDeque<i32> = VecDeque::from([1, 2, 3]);

    list.pop_front();
    assert_eq!(*list.front().unwrap(), 2);

    list.pop_front();
    assert_eq!(*list.front().unwrap(), 3);

    list.pop_front();
    assert!(list.front().is_none());
}

// ---------------------------------------------------------------------------
// Empty list operations
// ---------------------------------------------------------------------------

/// Port of empty list tests from test_linkedlists.c.
///
/// Verify that operations on empty lists don't panic.
#[test]
fn test_empty_list_operations() {
    let mut list: VecDeque<i32> = VecDeque::new();

    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert!(list.front().is_none());
    assert!(list.back().is_none());
    assert!(list.pop_front().is_none());
    assert!(list.pop_back().is_none());

    // Iteration over empty list yields nothing.
    let mut count = 0;
    for _ in list.iter() {
        count += 1;
    }
    assert_eq!(count, 0);
}

/// Test that list becomes empty after removing all elements.
#[test]
fn test_list_drain_to_empty() {
    let mut list: VecDeque<i32> = VecDeque::from([1, 2, 3, 4, 5]);

    while list.pop_front().is_some() {}

    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
}

/// Test clear operation.
#[test]
fn test_list_clear() {
    let mut list: VecDeque<i32> = VecDeque::from([1, 2, 3]);
    assert!(!list.is_empty());

    list.clear();
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
}

// ---------------------------------------------------------------------------
// Large list operations
// ---------------------------------------------------------------------------

/// Port of performance/stress tests from test_linkedlists.c.
///
/// Test that a large number of insertions and removals work correctly.
#[test]
fn test_large_list_insert_remove() {
    let mut list: VecDeque<u64> = VecDeque::new();
    let count = 10_000u64;

    // Insert.
    for i in 0..count {
        list.push_back(i);
    }
    assert_eq!(list.len(), count as usize);

    // Verify order.
    for (idx, &val) in list.iter().enumerate() {
        assert_eq!(val, idx as u64);
    }

    // Remove from front.
    for i in 0..count {
        let val = list.pop_front().unwrap();
        assert_eq!(val, i);
    }
    assert!(list.is_empty());
}

/// Test large list with mixed operations.
#[test]
fn test_large_list_mixed_ops() {
    let mut list: VecDeque<u32> = VecDeque::new();

    // Alternate push_front and push_back.
    for i in 0..1000u32 {
        if i % 2 == 0 {
            list.push_front(i);
        } else {
            list.push_back(i);
        }
    }

    assert_eq!(list.len(), 1000);

    // Drain alternating front and back.
    let mut count = 0;
    while !list.is_empty() {
        if count % 2 == 0 {
            list.pop_front();
        } else {
            list.pop_back();
        }
        count += 1;
    }
    assert!(list.is_empty());
}

/// Test contains/search operation.
#[test]
fn test_list_contains() {
    let list: VecDeque<i32> = VecDeque::from([10, 20, 30, 40, 50]);

    assert!(list.contains(&30));
    assert!(!list.contains(&35));
}

/// Test iteration with filter (port of AST_LIST_TRAVERSE_SAFE_BEGIN pattern).
#[test]
fn test_list_filter_remove() {
    let mut list: Vec<i32> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    // Remove all even numbers (like traversal with removal in C).
    list.retain(|&x| x % 2 != 0);

    assert_eq!(list, vec![1, 3, 5, 7, 9]);
}

/// Test sort operation.
#[test]
fn test_list_sort() {
    let mut list: Vec<i32> = vec![5, 3, 1, 4, 2];
    list.sort();
    assert_eq!(list, vec![1, 2, 3, 4, 5]);
}

/// Test dedup after sort (finding unique elements).
#[test]
fn test_list_dedup() {
    let mut list: Vec<i32> = vec![3, 1, 2, 1, 3, 2, 1];
    list.sort();
    list.dedup();
    assert_eq!(list, vec![1, 2, 3]);
}
