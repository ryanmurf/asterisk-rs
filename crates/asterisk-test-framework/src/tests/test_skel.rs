//! Port of asterisk/tests/test_skel.c
//!
//! Skeleton test template demonstrating the basic structure of a unit test.
//! Tests that memory allocation works and basic assertions pass.

/// Port of AST_TEST_DEFINE(sample_test) from test_skel.c.
///
/// This demonstrates the basic structure of a test: allocate resources,
/// validate conditions, clean up.
#[test]
fn test_sample() {
    // Simulate ast_malloc: allocate some heap memory.
    let ptr: Box<[u8; 8]> = Box::new([0u8; 8]);
    assert_eq!(ptr.len(), 8);

    let ptr2: Box<[u8; 8]> = Box::new([0u8; 8]);
    assert_eq!(ptr2.len(), 8);

    // Both allocations succeeded. In the C test, this is the
    // minimal skeleton that ensures the test framework works.
}

/// Test that Vec allocation works (additional skeleton exercise).
#[test]
fn test_sample_vec() {
    let v: Vec<u8> = vec![0; 1024];
    assert_eq!(v.len(), 1024);
    assert!(v.iter().all(|&b| b == 0));
}

/// Test string allocation.
#[test]
fn test_sample_string() {
    let s = String::from("Hello, Asterisk test framework!");
    assert!(!s.is_empty());
    assert!(s.contains("Asterisk"));
}
