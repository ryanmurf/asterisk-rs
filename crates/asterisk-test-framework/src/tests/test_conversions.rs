//! Port of asterisk/tests/test_conversions.c
//!
//! Tests type conversion functions:
//! - String to i32 (ast_str_to_int)
//! - String to u32 (ast_str_to_uint)
//! - String to i64 (ast_str_to_long)
//! - String to u64 (ast_str_to_ulong)
//! - String to i128 (ast_str_to_imax)
//! - String to u128 (ast_str_to_umax)
//! - Boundary values (MAX, MIN, 0)
//! - Invalid input handling (non-numeric, partial, overflow)
//!
//! In Rust we use str::parse::<T>() with trimming, which covers the
//! same behavioral guarantees as the C conversion functions.

/// Strict string-to-integer conversion mirroring Asterisk's ast_str_to_int.
///
/// Returns Ok(value) if the string is a valid integer (with optional leading whitespace).
/// Returns Err(()) for:
/// - Empty strings
/// - Non-numeric content
/// - Partial matches like "7abc"
/// - Overflow
fn str_to_int(s: &str) -> Result<i32, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    trimmed.parse::<i32>().map_err(|_| ())
}

fn str_to_uint(s: &str) -> Result<u32, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    // Reject negative numbers.
    if trimmed.starts_with('-') {
        return Err(());
    }
    trimmed.parse::<u32>().map_err(|_| ())
}

fn str_to_long(s: &str) -> Result<i64, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    trimmed.parse::<i64>().map_err(|_| ())
}

fn str_to_ulong(s: &str) -> Result<u64, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    if trimmed.starts_with('-') {
        return Err(());
    }
    trimmed.parse::<u64>().map_err(|_| ())
}

fn str_to_imax(s: &str) -> Result<i128, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    trimmed.parse::<i128>().map_err(|_| ())
}

fn str_to_umax(s: &str) -> Result<u128, ()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    if trimmed.starts_with('-') {
        return Err(());
    }
    trimmed.parse::<u128>().map_err(|_| ())
}

// ---------------------------------------------------------------------------
// str_to_int tests (port of AST_TEST_DEFINE(str_to_int))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_int_invalid_alpha() {
    assert!(str_to_int("abc").is_err());
}

#[test]
fn test_str_to_int_invalid_partial() {
    assert!(str_to_int("7abc").is_err());
}

#[test]
fn test_str_to_int_negative() {
    assert_eq!(str_to_int("-7"), Ok(-7));
}

#[test]
fn test_str_to_int_negative_spaces() {
    assert_eq!(str_to_int("  -7"), Ok(-7));
}

#[test]
fn test_str_to_int_negative_out_of_range() {
    assert!(str_to_int("-9999999999").is_err());
}

#[test]
fn test_str_to_int_out_of_range() {
    assert!(str_to_int("9999999999").is_err());
}

#[test]
fn test_str_to_int_spaces_only() {
    assert!(str_to_int("  ").is_err());
}

#[test]
fn test_str_to_int_valid() {
    assert_eq!(str_to_int("7"), Ok(7));
}

#[test]
fn test_str_to_int_valid_spaces() {
    assert_eq!(str_to_int("  7"), Ok(7));
}

#[test]
fn test_str_to_int_valid_decimal() {
    // "08" should parse as 8 (base 10), not octal.
    assert_eq!(str_to_int("08"), Ok(8));
}

#[test]
fn test_str_to_int_max() {
    let s = format!("{}", i32::MAX);
    assert_eq!(str_to_int(&s), Ok(i32::MAX));
}

#[test]
fn test_str_to_int_min() {
    let s = format!("{}", i32::MIN);
    assert_eq!(str_to_int(&s), Ok(i32::MIN));
}

#[test]
fn test_str_to_int_empty() {
    assert!(str_to_int("").is_err());
}

#[test]
fn test_str_to_int_zero() {
    assert_eq!(str_to_int("0"), Ok(0));
}

// ---------------------------------------------------------------------------
// str_to_uint tests (port of AST_TEST_DEFINE(str_to_uint))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_uint_invalid_alpha() {
    assert!(str_to_uint("abc").is_err());
}

#[test]
fn test_str_to_uint_invalid_partial() {
    assert!(str_to_uint("7abc").is_err());
}

#[test]
fn test_str_to_uint_negative() {
    assert!(str_to_uint("-7").is_err());
}

#[test]
fn test_str_to_uint_negative_spaces() {
    assert!(str_to_uint("  -7").is_err());
}

#[test]
fn test_str_to_uint_out_of_range() {
    assert!(str_to_uint("9999999999").is_err());
}

#[test]
fn test_str_to_uint_spaces() {
    assert!(str_to_uint("  ").is_err());
}

#[test]
fn test_str_to_uint_valid() {
    assert_eq!(str_to_uint("7"), Ok(7));
}

#[test]
fn test_str_to_uint_valid_spaces() {
    assert_eq!(str_to_uint("  7"), Ok(7));
}

#[test]
fn test_str_to_uint_valid_decimal() {
    assert_eq!(str_to_uint("08"), Ok(8));
}

#[test]
fn test_str_to_uint_max() {
    let s = format!("{}", u32::MAX);
    assert_eq!(str_to_uint(&s), Ok(u32::MAX));
}

#[test]
fn test_str_to_uint_zero() {
    assert_eq!(str_to_uint("0"), Ok(0));
}

// ---------------------------------------------------------------------------
// str_to_long tests (port of AST_TEST_DEFINE(str_to_long))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_long_invalid() {
    assert!(str_to_long("abc").is_err());
}

#[test]
fn test_str_to_long_invalid_partial() {
    assert!(str_to_long("7abc").is_err());
}

#[test]
fn test_str_to_long_negative() {
    assert_eq!(str_to_long("-7"), Ok(-7));
}

#[test]
fn test_str_to_long_negative_spaces() {
    assert_eq!(str_to_long("  -7"), Ok(-7));
}

#[test]
fn test_str_to_long_negative_out_of_range() {
    assert!(str_to_long("-99999999999999999999").is_err());
}

#[test]
fn test_str_to_long_out_of_range() {
    assert!(str_to_long("99999999999999999999").is_err());
}

#[test]
fn test_str_to_long_spaces() {
    assert!(str_to_long("  ").is_err());
}

#[test]
fn test_str_to_long_valid() {
    assert_eq!(str_to_long("7"), Ok(7));
}

#[test]
fn test_str_to_long_valid_spaces() {
    assert_eq!(str_to_long("  7"), Ok(7));
}

#[test]
fn test_str_to_long_valid_decimal() {
    assert_eq!(str_to_long("08"), Ok(8));
}

#[test]
fn test_str_to_long_max() {
    let s = format!("{}", i64::MAX);
    assert_eq!(str_to_long(&s), Ok(i64::MAX));
}

#[test]
fn test_str_to_long_min() {
    let s = format!("{}", i64::MIN);
    assert_eq!(str_to_long(&s), Ok(i64::MIN));
}

// ---------------------------------------------------------------------------
// str_to_ulong tests (port of AST_TEST_DEFINE(str_to_ulong))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_ulong_invalid() {
    assert!(str_to_ulong("abc").is_err());
}

#[test]
fn test_str_to_ulong_invalid_partial() {
    assert!(str_to_ulong("7abc").is_err());
}

#[test]
fn test_str_to_ulong_negative() {
    assert!(str_to_ulong("-7").is_err());
}

#[test]
fn test_str_to_ulong_negative_spaces() {
    assert!(str_to_ulong("  -7").is_err());
}

#[test]
fn test_str_to_ulong_out_of_range() {
    assert!(str_to_ulong("99999999999999999999").is_err());
}

#[test]
fn test_str_to_ulong_spaces() {
    assert!(str_to_ulong("  ").is_err());
}

#[test]
fn test_str_to_ulong_valid() {
    assert_eq!(str_to_ulong("7"), Ok(7));
}

#[test]
fn test_str_to_ulong_valid_spaces() {
    assert_eq!(str_to_ulong("  7"), Ok(7));
}

#[test]
fn test_str_to_ulong_valid_decimal() {
    assert_eq!(str_to_ulong("08"), Ok(8));
}

#[test]
fn test_str_to_ulong_max() {
    let s = format!("{}", u64::MAX);
    assert_eq!(str_to_ulong(&s), Ok(u64::MAX));
}

// ---------------------------------------------------------------------------
// str_to_imax tests (port of AST_TEST_DEFINE(str_to_imax))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_imax_invalid() {
    assert!(str_to_imax("abc").is_err());
}

#[test]
fn test_str_to_imax_invalid_partial() {
    assert!(str_to_imax("7abc").is_err());
}

#[test]
fn test_str_to_imax_negative() {
    assert_eq!(str_to_imax("-7"), Ok(-7));
}

#[test]
fn test_str_to_imax_negative_spaces() {
    assert_eq!(str_to_imax("  -7"), Ok(-7));
}

#[test]
fn test_str_to_imax_negative_out_of_range() {
    assert!(
        str_to_imax("-99999999999999999999999999999999999999999999999999").is_err()
    );
}

#[test]
fn test_str_to_imax_out_of_range() {
    assert!(
        str_to_imax("99999999999999999999999999999999999999999999999999").is_err()
    );
}

#[test]
fn test_str_to_imax_spaces() {
    assert!(str_to_imax("  ").is_err());
}

#[test]
fn test_str_to_imax_valid() {
    assert_eq!(str_to_imax("7"), Ok(7));
}

#[test]
fn test_str_to_imax_valid_spaces() {
    assert_eq!(str_to_imax("  7"), Ok(7));
}

#[test]
fn test_str_to_imax_valid_decimal() {
    assert_eq!(str_to_imax("08"), Ok(8));
}

#[test]
fn test_str_to_imax_max() {
    let s = format!("{}", i128::MAX);
    assert_eq!(str_to_imax(&s), Ok(i128::MAX));
}

#[test]
fn test_str_to_imax_min() {
    let s = format!("{}", i128::MIN);
    assert_eq!(str_to_imax(&s), Ok(i128::MIN));
}

// ---------------------------------------------------------------------------
// str_to_umax tests (port of AST_TEST_DEFINE(str_to_umax))
// ---------------------------------------------------------------------------

#[test]
fn test_str_to_umax_invalid() {
    assert!(str_to_umax("abc").is_err());
}

#[test]
fn test_str_to_umax_invalid_partial() {
    assert!(str_to_umax("7abc").is_err());
}

#[test]
fn test_str_to_umax_negative() {
    assert!(str_to_umax("-7").is_err());
}

#[test]
fn test_str_to_umax_negative_spaces() {
    assert!(str_to_umax("  -7").is_err());
}

#[test]
fn test_str_to_umax_out_of_range() {
    assert!(
        str_to_umax("99999999999999999999999999999999999999999999999999").is_err()
    );
}

#[test]
fn test_str_to_umax_spaces() {
    assert!(str_to_umax("  ").is_err());
}

#[test]
fn test_str_to_umax_valid() {
    assert_eq!(str_to_umax("7"), Ok(7));
}

#[test]
fn test_str_to_umax_valid_spaces() {
    assert_eq!(str_to_umax("  7"), Ok(7));
}

#[test]
fn test_str_to_umax_valid_decimal() {
    assert_eq!(str_to_umax("08"), Ok(8));
}

#[test]
fn test_str_to_umax_max() {
    let s = format!("{}", u128::MAX);
    assert_eq!(str_to_umax(&s), Ok(u128::MAX));
}
