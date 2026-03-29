//! Port of asterisk/tests/test_stringfields.c
//!
//! Tests string field operations as they apply in Rust:
//! - String allocation and initial values
//! - Set/get operations
//! - String field copy
//! - Growing strings beyond initial capacity
//! - Shrinking strings (reuse of allocated space)
//! - String field reset/clear
//!
//! In Rust, we don't have Asterisk's `AST_STRING_FIELD` system, but we test
//! equivalent behavior using a struct with String fields that mimics the
//! pattern.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// StringFieldSet: Rust equivalent of AST_DECLARE_STRING_FIELDS
// ---------------------------------------------------------------------------

/// A struct with managed string fields, analogous to Asterisk's string fields.
#[derive(Debug, Clone)]
struct StringFieldSet {
    /// Fields stored by name.
    fields: HashMap<String, String>,
}

impl StringFieldSet {
    fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }

    /// Set a string field value (like ast_string_field_set).
    fn set(&mut self, name: &str, value: &str) {
        self.fields.insert(name.to_string(), value.to_string());
    }

    /// Get a string field value (like accessing a string field).
    fn get(&self, name: &str) -> &str {
        self.fields.get(name).map(|s| s.as_str()).unwrap_or("")
    }

    /// Build a string field using format! (like ast_string_field_build).
    fn build(&mut self, name: &str, format: &str) {
        self.fields.insert(name.to_string(), format.to_string());
    }

    /// Reset all fields to empty.
    fn reset(&mut self) {
        for val in self.fields.values_mut() {
            val.clear();
        }
    }

    /// Free all fields.
    fn free(&mut self) {
        self.fields.clear();
    }

    /// Number of fields.
    fn count(&self) -> usize {
        self.fields.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(string_field_test) from test_stringfields.c.
///
/// Test basic string field allocation and setting.
#[test]
fn test_stringfield_init_and_set() {
    let mut sfs = StringFieldSet::new();

    sfs.set("string1", "elephant");
    sfs.set("string2", "hippopotamus");

    assert_eq!(sfs.get("string1"), "elephant");
    assert_eq!(sfs.get("string2"), "hippopotamus");
}

/// Test that shrinking a string field works.
#[test]
fn test_stringfield_shrink() {
    let mut sfs = StringFieldSet::new();

    sfs.set("string1", "elephant");
    assert_eq!(sfs.get("string1"), "elephant");

    sfs.set("string1", "rhino");
    assert_eq!(sfs.get("string1"), "rhino");
}

/// Test that growing a string field beyond initial capacity works.
#[test]
fn test_stringfield_grow() {
    let mut sfs = StringFieldSet::new();

    // Start with a short string.
    sfs.set("string1", "cat");
    assert_eq!(sfs.get("string1"), "cat");

    // Grow to a much longer string.
    let long_string = "A professional panoramic photograph of the majestic elephant bathing itself and its young by the shores of the raging Mississippi River";
    sfs.set("string1", long_string);
    assert_eq!(sfs.get("string1"), long_string);
}

/// Test string field copy between instances.
#[test]
fn test_stringfield_copy() {
    let mut sfs1 = StringFieldSet::new();
    sfs1.set("string1", "hello");
    sfs1.set("string2", "world");

    let sfs2 = sfs1.clone();
    assert_eq!(sfs2.get("string1"), "hello");
    assert_eq!(sfs2.get("string2"), "world");

    // Modifying original should not affect copy.
    sfs1.set("string1", "changed");
    assert_eq!(sfs1.get("string1"), "changed");
    assert_eq!(sfs2.get("string1"), "hello");
}

/// Test string field build (format).
#[test]
fn test_stringfield_build() {
    let mut sfs = StringFieldSet::new();
    sfs.build("string1", &format!("hello {} {}", "beautiful", "world"));
    assert_eq!(sfs.get("string1"), "hello beautiful world");
}

/// Test string field reset.
#[test]
fn test_stringfield_reset() {
    let mut sfs = StringFieldSet::new();
    sfs.set("string1", "hello");
    sfs.set("string2", "world");

    sfs.reset();
    assert_eq!(sfs.get("string1"), "");
    assert_eq!(sfs.get("string2"), "");
    assert_eq!(sfs.count(), 2); // Fields still exist, just empty.
}

/// Test string field free.
#[test]
fn test_stringfield_free() {
    let mut sfs = StringFieldSet::new();
    sfs.set("string1", "hello");
    sfs.set("string2", "world");

    sfs.free();
    assert_eq!(sfs.count(), 0);
    assert_eq!(sfs.get("string1"), ""); // Not found, returns empty.
}

/// Test setting a field multiple times.
#[test]
fn test_stringfield_multiple_sets() {
    let mut sfs = StringFieldSet::new();

    for i in 0..100 {
        sfs.set("counter", &format!("{}", i));
    }
    assert_eq!(sfs.get("counter"), "99");
}

/// Test empty string field.
#[test]
fn test_stringfield_empty() {
    let mut sfs = StringFieldSet::new();
    sfs.set("empty", "");
    assert_eq!(sfs.get("empty"), "");
    assert_eq!(sfs.count(), 1);
}

/// Test many fields.
#[test]
fn test_stringfield_many_fields() {
    let mut sfs = StringFieldSet::new();

    for i in 0..50 {
        sfs.set(&format!("field_{}", i), &format!("value_{}", i));
    }

    assert_eq!(sfs.count(), 50);

    for i in 0..50 {
        assert_eq!(
            sfs.get(&format!("field_{}", i)),
            format!("value_{}", i)
        );
    }
}

/// Test getting non-existent field returns empty string.
#[test]
fn test_stringfield_get_nonexistent() {
    let sfs = StringFieldSet::new();
    assert_eq!(sfs.get("nonexistent"), "");
}

/// Test setting the same field with different-length strings.
#[test]
fn test_stringfield_varying_lengths() {
    let mut sfs = StringFieldSet::new();

    sfs.set("f", "a");
    assert_eq!(sfs.get("f"), "a");

    sfs.set("f", "ab");
    assert_eq!(sfs.get("f"), "ab");

    sfs.set("f", "abc");
    assert_eq!(sfs.get("f"), "abc");

    sfs.set("f", "a");
    assert_eq!(sfs.get("f"), "a");

    sfs.set("f", "");
    assert_eq!(sfs.get("f"), "");

    sfs.set("f", "abcdefghij");
    assert_eq!(sfs.get("f"), "abcdefghij");
}
