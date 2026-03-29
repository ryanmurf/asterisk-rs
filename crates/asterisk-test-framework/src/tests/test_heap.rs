//! Port of asterisk/tests/test_heap.c
//!
//! Tests binary heap / priority queue behavior:
//! - Push elements with priorities
//! - Pop returns highest priority first
//! - Peek without removing
//! - Remove specific element
//! - Large heap operations
//!
//! In Rust, we use std::collections::BinaryHeap which is a max-heap.
//! The C Asterisk ast_heap is a max-heap as well (highest priority first).

use std::collections::BinaryHeap;
use std::cmp::Reverse;

// ---------------------------------------------------------------------------
// Basic push and pop
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(heap_test_1) from test_heap.c.
///
/// Push elements with different priorities and verify pop order.
#[test]
fn test_heap_push_pop_max_priority() {
    let mut heap = BinaryHeap::new();

    heap.push(10);
    heap.push(30);
    heap.push(20);
    heap.push(50);
    heap.push(40);

    // Pop should return elements in descending order (max first).
    assert_eq!(heap.pop(), Some(50));
    assert_eq!(heap.pop(), Some(40));
    assert_eq!(heap.pop(), Some(30));
    assert_eq!(heap.pop(), Some(20));
    assert_eq!(heap.pop(), Some(10));
    assert_eq!(heap.pop(), None);
}

/// Test min-heap behavior using Reverse wrapper.
///
/// Port of the priority ordering test with lowest-priority-first.
#[test]
fn test_heap_min_priority() {
    let mut heap = BinaryHeap::new();

    heap.push(Reverse(10));
    heap.push(Reverse(30));
    heap.push(Reverse(20));
    heap.push(Reverse(50));
    heap.push(Reverse(40));

    // Pop should return elements in ascending order (min first).
    assert_eq!(heap.pop(), Some(Reverse(10)));
    assert_eq!(heap.pop(), Some(Reverse(20)));
    assert_eq!(heap.pop(), Some(Reverse(30)));
    assert_eq!(heap.pop(), Some(Reverse(40)));
    assert_eq!(heap.pop(), Some(Reverse(50)));
}

// ---------------------------------------------------------------------------
// Peek without removing
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(heap_test_2) from test_heap.c.
///
/// Test that peek returns the highest priority element without removing it.
#[test]
fn test_heap_peek() {
    let mut heap = BinaryHeap::new();

    heap.push(10);
    heap.push(50);
    heap.push(30);

    // Peek should return 50 (max).
    assert_eq!(heap.peek(), Some(&50));
    assert_eq!(heap.len(), 3); // Not removed.

    // Peek again should return the same.
    assert_eq!(heap.peek(), Some(&50));
}

/// Test peek on empty heap.
#[test]
fn test_heap_peek_empty() {
    let heap: BinaryHeap<i32> = BinaryHeap::new();
    assert_eq!(heap.peek(), None);
}

// ---------------------------------------------------------------------------
// Remove specific element
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(heap_test_3) from test_heap.c.
///
/// Test removing a specific element from the heap. In Rust's BinaryHeap,
/// there's no direct remove-by-value, so we drain-filter.
#[test]
fn test_heap_remove_specific() {
    let mut heap = BinaryHeap::new();

    heap.push(10);
    heap.push(20);
    heap.push(30);
    heap.push(40);
    heap.push(50);

    // Remove the element with value 30.
    let items: Vec<i32> = heap.into_iter().filter(|&x| x != 30).collect();
    heap = BinaryHeap::from(items);

    assert_eq!(heap.len(), 4);

    // Remaining elements should be 50, 40, 20, 10.
    let mut sorted: Vec<i32> = heap.into_sorted_vec();
    sorted.reverse();
    assert_eq!(sorted, vec![50, 40, 20, 10]);
}

/// Test removing the max element.
#[test]
fn test_heap_remove_max() {
    let mut heap = BinaryHeap::from(vec![10, 50, 30, 20, 40]);

    let max = heap.pop().unwrap();
    assert_eq!(max, 50);
    assert_eq!(heap.len(), 4);

    // Next max should be 40.
    assert_eq!(*heap.peek().unwrap(), 40);
}

// ---------------------------------------------------------------------------
// Large heap operations
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(heap_test_large) from test_heap.c.
///
/// Test heap operations with a large number of elements.
#[test]
fn test_heap_large_operations() {
    let mut heap = BinaryHeap::new();

    // Push 10,000 elements in random-ish order.
    for i in (0..10_000).rev() {
        heap.push(i);
    }

    assert_eq!(heap.len(), 10_000);
    assert_eq!(*heap.peek().unwrap(), 9999);

    // Pop all elements -- should come out in descending order.
    let mut prev = 10_000i64;
    while let Some(val) = heap.pop() {
        assert!(val < prev as i32, "Heap order violated");
        prev = val as i64;
    }

    assert!(heap.is_empty());
}

/// Test large heap with mixed push/pop operations.
#[test]
fn test_heap_large_mixed() {
    let mut heap = BinaryHeap::new();

    // Push 1000, pop 500, push 500 more, pop all.
    for i in 0..1000 {
        heap.push(i);
    }
    assert_eq!(heap.len(), 1000);

    for _ in 0..500 {
        heap.pop();
    }
    assert_eq!(heap.len(), 500);

    for i in 1000..1500 {
        heap.push(i);
    }
    assert_eq!(heap.len(), 1000);

    // Pop all -- should be in order.
    let mut prev = i64::MAX;
    while let Some(val) = heap.pop() {
        assert!((val as i64) <= prev);
        prev = val as i64;
    }
}

// ---------------------------------------------------------------------------
// Additional behavioral tests
// ---------------------------------------------------------------------------

/// Test heap with duplicate values.
#[test]
fn test_heap_duplicates() {
    let mut heap = BinaryHeap::new();

    heap.push(10);
    heap.push(10);
    heap.push(20);
    heap.push(20);
    heap.push(30);

    assert_eq!(heap.len(), 5);

    assert_eq!(heap.pop(), Some(30));
    assert_eq!(heap.pop(), Some(20));
    assert_eq!(heap.pop(), Some(20));
    assert_eq!(heap.pop(), Some(10));
    assert_eq!(heap.pop(), Some(10));
}

/// Test heap with custom struct (simulating Asterisk's heap with opaque data).
#[test]
fn test_heap_custom_struct() {
    #[derive(Debug, Eq, PartialEq)]
    struct TimerEntry {
        priority: u32,
        name: String,
    }

    impl Ord for TimerEntry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.priority.cmp(&other.priority)
        }
    }

    impl PartialOrd for TimerEntry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut heap = BinaryHeap::new();

    heap.push(TimerEntry {
        priority: 10,
        name: "low".to_string(),
    });
    heap.push(TimerEntry {
        priority: 50,
        name: "high".to_string(),
    });
    heap.push(TimerEntry {
        priority: 30,
        name: "medium".to_string(),
    });

    let top = heap.pop().unwrap();
    assert_eq!(top.name, "high");
    assert_eq!(top.priority, 50);

    let next = heap.pop().unwrap();
    assert_eq!(next.name, "medium");

    let last = heap.pop().unwrap();
    assert_eq!(last.name, "low");
}

/// Test into_sorted_vec.
#[test]
fn test_heap_sorted_vec() {
    let heap = BinaryHeap::from(vec![5, 3, 1, 4, 2]);
    let sorted = heap.into_sorted_vec();
    assert_eq!(sorted, vec![1, 2, 3, 4, 5]);
}

/// Test building heap from iterator.
#[test]
fn test_heap_from_iter() {
    let data = vec![100, 50, 75, 25, 90];
    let heap: BinaryHeap<i32> = data.into_iter().collect();

    assert_eq!(heap.len(), 5);
    assert_eq!(*heap.peek().unwrap(), 100);
}

/// Test heap capacity/reserve.
#[test]
fn test_heap_capacity() {
    let mut heap = BinaryHeap::with_capacity(100);
    assert!(heap.capacity() >= 100);
    assert!(heap.is_empty());

    for i in 0..50 {
        heap.push(i);
    }
    assert_eq!(heap.len(), 50);
}

/// Test drain operation.
#[test]
fn test_heap_drain() {
    let mut heap = BinaryHeap::from(vec![1, 2, 3, 4, 5]);
    let drained: Vec<i32> = heap.drain().collect();
    assert_eq!(drained.len(), 5);
    assert!(heap.is_empty());
}
