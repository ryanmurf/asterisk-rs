//! Port of asterisk/tests/test_uuid.c
//!
//! Tests UUID generation and operations:
//! - Generate UUID and convert to string
//! - Parse UUID string back to UUID
//! - UUID comparison (equality)
//! - UUID copy
//! - Nil UUID detection
//! - UUID uniqueness

/// Port of AST_TEST_DEFINE(uuid) from test_uuid.c.
///
/// Exercises the full UUID lifecycle: generate, stringify, parse,
/// compare, copy, and nil detection.
#[test]
fn test_uuid_lifecycle() {
    // Generate a UUID string directly.
    let uuid_str = uuid::Uuid::new_v4().to_string();
    assert_eq!(uuid_str.len(), 36); // Standard UUID string length.

    // Parse the string back.
    let uuid1 = uuid::Uuid::parse_str(&uuid_str).unwrap();
    assert!(!uuid1.is_nil());

    // Convert back to string and verify roundtrip.
    let uuid1_str = uuid1.to_string();
    assert_eq!(uuid1_str, uuid_str);

    // Parse from the string again and compare.
    let uuid2 = uuid::Uuid::parse_str(&uuid1_str).unwrap();
    assert_eq!(uuid1, uuid2);

    // Copy and compare.
    let uuid3 = uuid1;
    assert_eq!(uuid1, uuid3);
    assert_eq!(uuid2, uuid3);

    // Nil UUID.
    let nil = uuid::Uuid::nil();
    assert!(nil.is_nil());
    assert_ne!(uuid1, nil);
}

/// Test that two generated UUIDs are unique.
#[test]
fn test_uuid_uniqueness() {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    assert_ne!(a, b);
}

/// Test UUID string format (8-4-4-4-12).
#[test]
fn test_uuid_string_format() {
    let uuid = uuid::Uuid::new_v4();
    let s = uuid.to_string();

    let parts: Vec<&str> = s.split('-').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0].len(), 8);
    assert_eq!(parts[1].len(), 4);
    assert_eq!(parts[2].len(), 4);
    assert_eq!(parts[3].len(), 4);
    assert_eq!(parts[4].len(), 12);
}

/// Test parsing invalid UUID strings.
#[test]
fn test_uuid_parse_invalid() {
    assert!(uuid::Uuid::parse_str("not-a-uuid").is_err());
    assert!(uuid::Uuid::parse_str("").is_err());
    assert!(uuid::Uuid::parse_str("12345678-1234-1234-1234-12345678901").is_err());
}

/// Test nil UUID is all zeros.
#[test]
fn test_uuid_nil() {
    let nil = uuid::Uuid::nil();
    assert!(nil.is_nil());
    assert_eq!(nil.to_string(), "00000000-0000-0000-0000-000000000000");
}
