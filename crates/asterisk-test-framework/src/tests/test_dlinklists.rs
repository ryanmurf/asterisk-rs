//! Port of asterisk/tests/test_dlinklists.c
//!
//! Tests doubly-linked list operations. In Rust, we use VecDeque as the
//! equivalent of the C AST_DLLIST:
//!
//! - INSERT_HEAD: push to front
//! - INSERT_TAIL: push to back
//! - INSERT_AFTER: insert at a specific position
//! - REMOVE_HEAD: pop from front
//! - REMOVE (specific element)
//! - TRAVERSE: forward iteration
//! - TRAVERSE_BACKWARDS: reverse iteration
//! - EMPTY: emptiness check
//! - FIRST / LAST: head/tail access
//! - APPEND_DLLIST: concatenation of two lists
//! - Safe traversal with removal
//! - Safe traversal with insertion before current

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Tests matching the C dll_tests() function
// ---------------------------------------------------------------------------

/// Port of INSERT_HEAD test from dll_tests().
///
/// Insert A, B, C, D at head; list should be A <=> B <=> C <=> D.
#[test]
fn test_dll_insert_head() {
    let mut list: VecDeque<&str> = VecDeque::new();
    list.push_front("D");
    list.push_front("C");
    list.push_front("B");
    list.push_front("A");

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"C", &"D"]);
}

/// Port of EMPTY test from dll_tests().
#[test]
fn test_dll_empty() {
    let list: VecDeque<&str> = VecDeque::new();
    assert!(list.is_empty());
}

/// Port of INSERT_TAIL test from dll_tests().
///
/// Insert A, B, C, D at tail; list should be A <=> B <=> C <=> D.
#[test]
fn test_dll_insert_tail() {
    let mut list: VecDeque<&str> = VecDeque::new();
    list.push_back("A");
    list.push_back("B");
    list.push_back("C");
    list.push_back("D");

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"C", &"D"]);
}

/// Port of INSERT_AFTER test from dll_tests().
///
/// Start with A <=> D, insert B after A, insert C after B.
#[test]
fn test_dll_insert_after() {
    let mut list: VecDeque<&str> = VecDeque::new();
    list.push_back("A");
    list.push_back("D");

    // Insert B after A (index 0 -> insert at index 1)
    list.insert(1, "B");
    // Insert C after B (index 1 -> insert at index 2)
    list.insert(2, "C");

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"C", &"D"]);
}

/// Port of REMOVE_HEAD test from dll_tests().
#[test]
fn test_dll_remove_head() {
    let mut list: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D"]);

    let removed = list.pop_front().unwrap();
    assert_eq!(removed, "A");
    assert_eq!(list.front(), Some(&"B"));
}

/// Port of REMOVE (specific element) from dll_tests().
#[test]
fn test_dll_remove_specific() {
    let mut list: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D"]);

    // Remove "C" (index 2)
    list.remove(2);

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"D"]);
}

/// Port of TRAVERSE_BACKWARDS test from dll_tests().
#[test]
fn test_dll_traverse_backwards() {
    let list: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D"]);

    let items: Vec<&&str> = list.iter().rev().collect();
    assert_eq!(items, vec![&"D", &"C", &"B", &"A"]);
}

/// Port of FIRST / LAST test from dll_tests().
#[test]
fn test_dll_first_last() {
    let list: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D"]);

    assert_eq!(list.front(), Some(&"A"));
    assert_eq!(list.back(), Some(&"D"));
}

/// Port of TRAVERSE_SAFE with removal from dll_tests().
///
/// Remove all elements matching a predicate during traversal.
#[test]
fn test_dll_safe_traverse_remove() {
    let mut list: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D", "E"]);

    // Remove elements B and D (simulate REMOVE_CURRENT)
    let to_remove: Vec<usize> = list
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == "B" || v == "D")
        .map(|(i, _)| i)
        .collect();

    // Remove in reverse order to preserve indices
    for &i in to_remove.iter().rev() {
        list.remove(i);
    }

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"C", &"E"]);
}

/// Port of INSERT_BEFORE_CURRENT from dll_tests().
///
/// Traverse and insert elements before the current position.
#[test]
fn test_dll_insert_before_current() {
    let mut list: VecDeque<&str> = VecDeque::from(["B", "D"]);

    // Insert A before B (index 0)
    list.insert(0, "A");
    // Insert C before D (D is now at index 2)
    list.insert(2, "C");

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"C", &"D"]);
}

/// Port of APPEND_DLLIST from dll_tests().
///
/// Concatenate two lists.
#[test]
fn test_dll_append() {
    let mut list1: VecDeque<&str> = VecDeque::from(["A", "B"]);
    let list2: VecDeque<&str> = VecDeque::from(["C", "D"]);

    list1.extend(list2);

    let items: Vec<&&str> = list1.iter().collect();
    assert_eq!(items, vec![&"A", &"B", &"C", &"D"]);
}

/// Port of MOVE_CURRENT from dll_tests().
///
/// Move elements from one list to another during traversal.
#[test]
fn test_dll_move_current() {
    let mut source: VecDeque<&str> = VecDeque::from(["A", "B", "C", "D"]);
    let mut dest: VecDeque<&str> = VecDeque::new();

    // Move B and D to dest
    let indices: Vec<usize> = source
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == "B" || v == "D")
        .map(|(i, _)| i)
        .collect();

    // Move in reverse to preserve indices
    for &i in indices.iter().rev() {
        let item = source.remove(i).unwrap();
        dest.push_back(item);
    }

    let src_items: Vec<&&str> = source.iter().collect();
    assert_eq!(src_items, vec![&"A", &"C"]);

    // Note: order of moved items depends on traversal direction
    assert_eq!(dest.len(), 2);
    assert!(dest.contains(&"B"));
    assert!(dest.contains(&"D"));
}

/// Test complete drain and rebuild.
#[test]
fn test_dll_drain_rebuild() {
    let mut list: VecDeque<&str> = VecDeque::from(["A", "B", "C"]);

    // Drain all
    while list.pop_front().is_some() {}
    assert!(list.is_empty());

    // Rebuild
    list.push_back("X");
    list.push_back("Y");
    list.push_back("Z");

    let items: Vec<&&str> = list.iter().collect();
    assert_eq!(items, vec![&"X", &"Y", &"Z"]);
}
