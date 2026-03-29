//! Port of asterisk/tests/test_ari_model.c
//!
//! Tests ARI (Asterisk REST Interface) JSON model validators:
//!
//! - Byte validation (-128..255)
//! - Boolean validation
//! - Int validation (32-bit range)
//! - Long validation (64-bit range)
//! - String validation
//! - Date validation (ISO 8601 format with thorough regex)
//! - List validation (homogeneous arrays)

use regex::Regex;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

fn validate_byte(val: &Value) -> bool {
    match val.as_i64() {
        Some(n) => (-128..=255).contains(&n),
        None => false,
    }
}

fn validate_boolean(val: &Value) -> bool {
    val.is_boolean()
}

fn validate_int(val: &Value) -> bool {
    match val.as_i64() {
        Some(n) => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
        None => false,
    }
}

fn validate_long(val: &Value) -> bool {
    val.as_i64().is_some()
}

fn validate_string(val: &Value) -> bool {
    val.is_string()
}

/// Validate ISO 8601 date format matching the C implementation's regex.
/// The C implementation allows leap seconds (up to :61) and various timezone formats.
fn validate_date(val: &Value) -> bool {
    let s = match val.as_str() {
        Some(s) => s,
        None => return false,
    };
    if s.is_empty() {
        return false;
    }
    // Port of the regex from ari_model_validators.c
    // Seconds can be 00-61 (for leap seconds).
    // Timezone is required when time is present, and can be:
    //   Z, +/-HH, +/-HHMM, +/-HH:MM
    let re = Regex::new(
        r"^\d{4}-[01]\d-[0-3]\d(T[0-2]\d:[0-5]\d(:[0-6]\d(\.\d+)?)?(Z|[+-]\d{2}(:\d{2}|\d{2})?))?$",
    )
    .unwrap();
    re.is_match(s)
}

/// Validate a JSON array where every element passes the given validator.
fn validate_list(val: &Value, validator: fn(&Value) -> bool) -> bool {
    match val.as_array() {
        Some(arr) => arr.iter().all(validator),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(validate_byte).
#[test]
fn test_validate_byte() {
    assert!(validate_byte(&json!(-128)));
    assert!(validate_byte(&json!(0)));
    assert!(validate_byte(&json!(255)));

    assert!(!validate_byte(&json!(-129)));
    assert!(!validate_byte(&json!(256)));

    assert!(!validate_byte(&json!("not a byte")));
    assert!(!validate_byte(&json!("0")));
    assert!(!validate_byte(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_boolean).
#[test]
fn test_validate_boolean() {
    assert!(validate_boolean(&json!(true)));
    assert!(validate_boolean(&json!(false)));

    assert!(!validate_boolean(&json!("not a bool")));
    assert!(!validate_boolean(&json!("true")));
    assert!(!validate_boolean(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_int).
#[test]
fn test_validate_int() {
    assert!(validate_int(&json!(-2_147_483_648_i64)));
    assert!(validate_int(&json!(0)));
    assert!(validate_int(&json!(2_147_483_647_i64)));

    assert!(!validate_int(&json!(-2_147_483_649_i64)));
    assert!(!validate_int(&json!(2_147_483_648_i64)));

    assert!(!validate_int(&json!("not an int")));
    assert!(!validate_int(&json!("0")));
    assert!(!validate_int(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_long).
#[test]
fn test_validate_long() {
    assert!(validate_long(&json!(0)));

    assert!(!validate_long(&json!("not a long")));
    assert!(!validate_long(&json!("0")));
    assert!(!validate_long(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_string).
#[test]
fn test_validate_string() {
    assert!(validate_string(&json!("text")));
    assert!(validate_string(&json!("")));

    assert!(!validate_string(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_date).
///
/// Thorough test of ISO 8601 date validation with many valid and invalid cases.
#[test]
fn test_validate_date() {
    let valid_dates = [
        "2013-06-17",
        "2013-06-17T23:59Z",
        "2013-06-17T23:59:59Z",
        "2013-06-30T23:59:61Z",
        "2013-06-17T23:59:59.999999Z",
        "2013-06-17T23:59-06:00",
        "2013-06-17T23:59:59-06:00",
        "2013-06-30T23:59:61-06:00",
        "2013-06-17T23:59:59.999999-06:00",
        "2013-06-17T23:59+06:30",
        "2013-06-17T23:59:59+06:30",
        "2013-06-30T23:59:61+06:30",
        "2013-06-17T23:59:59.999999+06:30",
        "2013-06-17T23:59-0600",
        "2013-06-17T23:59:59-0600",
        "2013-06-30T23:59:61-0600",
        "2013-06-17T23:59:59.999999-0600",
        "2013-06-17T23:59+0630",
        "2013-06-17T23:59:59+0630",
        "2013-06-30T23:59:61+0630",
        "2013-06-17T23:59:59.999999+0630",
        "9999-12-31T23:59:61.999999Z",
        "2013-06-17T23:59-06",
        "2013-06-17T23:59:59-06",
        "2013-06-30T23:59:61-06",
        "2013-06-17T23:59:59.999999-06",
    ];

    let invalid_dates = [
        "",
        "Not a date",
        "2013-06-17T",
        "2013-06-17T23:59:59.Z",
        "2013-06-17T23:59",
        "2013-06-17T23:59:59.999999",
    ];

    for date in &valid_dates {
        assert!(
            validate_date(&json!(date)),
            "Expected '{}' to be a valid date",
            date
        );
    }

    for date in &invalid_dates {
        assert!(
            !validate_date(&json!(date)),
            "Expected '{}' to be an invalid date",
            date
        );
    }

    assert!(!validate_string(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_list).
///
/// Test list validation with homogeneous type checking.
#[test]
fn test_validate_list() {
    // Empty list passes any validator
    let empty = json!([]);
    assert!(validate_list(&empty, validate_string));
    assert!(validate_list(&empty, validate_int));

    // List with one string: passes string check, fails int check
    let one_str = json!([""]);
    assert!(validate_list(&one_str, validate_string));
    assert!(!validate_list(&one_str, validate_int));

    // Mixed list: fails both
    let mixed = json!(["", 0]);
    assert!(!validate_list(&mixed, validate_string));
    assert!(!validate_list(&mixed, validate_int));

    // Null is not a list
    assert!(!validate_list(&Value::Null, validate_string));
}
