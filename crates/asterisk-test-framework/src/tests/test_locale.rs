//! Port of asterisk/tests/test_locale.c
//!
//! Tests locale-independent number formatting, decimal separator handling,
//! and locale-safe string operations. In Asterisk, locale issues can cause
//! number formatting to use commas instead of dots for decimal separators,
//! which breaks SIP/SDP parsing. These tests verify that our formatting
//! functions are locale-independent.

// ---------------------------------------------------------------------------
// Locale-independent number formatting
// ---------------------------------------------------------------------------

/// Port of test_locale CLI command from test_locale.c.
///
/// Test that formatting floating-point numbers always uses '.' as the
/// decimal separator, regardless of locale.
#[test]
fn test_float_formatting_dot_separator() {
    let values: Vec<f64> = vec![0.0, 1.0, 1.5, 100.123, -42.7, 0.001, f64::MAX, f64::MIN];

    for val in &values {
        let formatted = format!("{}", val);
        // Should never contain comma as decimal separator.
        // (Commas are acceptable in large number grouping only if we use them,
        // but standard Rust format!() does not.)
        assert!(
            !formatted.contains(','),
            "Formatted '{}' as '{}' which contains comma",
            val,
            formatted
        );

        // If the number has a fractional part, it should contain a dot.
        if val.fract() != 0.0 {
            assert!(
                formatted.contains('.'),
                "Formatted '{}' as '{}' which should contain a dot",
                val,
                formatted
            );
        }
    }
}

/// Test that format! with explicit precision uses dot separator.
#[test]
fn test_precision_formatting() {
    let val = 3.14159;
    let formatted = format!("{:.2}", val);
    assert_eq!(formatted, "3.14");

    let formatted = format!("{:.6}", val);
    assert!(formatted.starts_with("3.14159"));
}

// ---------------------------------------------------------------------------
// Locale-safe string operations
// ---------------------------------------------------------------------------

/// Test that to_uppercase/to_lowercase are locale-independent for ASCII.
#[test]
fn test_ascii_case_conversion() {
    let input = "Hello World 123!";
    assert_eq!(input.to_uppercase(), "HELLO WORLD 123!");
    assert_eq!(input.to_lowercase(), "hello world 123!");

    // Turkish dotted/dotless I issue: in Rust, to_lowercase/to_uppercase
    // are always Unicode-aware but for ASCII it's always predictable.
    assert_eq!("I".to_lowercase(), "i");
    assert_eq!("i".to_uppercase(), "I");
}

/// Test that string comparison is locale-independent.
#[test]
fn test_string_comparison_locale_independent() {
    // In some locales, sorting order differs. Rust's Ord for strings
    // is always byte-wise, which is what we want.
    assert!("a" < "b");
    assert!("A" < "a"); // Uppercase ASCII values are lower.
    assert!("abc" < "abd");
    assert!("abc" < "abcd");
}

// ---------------------------------------------------------------------------
// Number parsing (locale-independent)
// ---------------------------------------------------------------------------

/// Test that parsing floating-point strings uses dot separator.
#[test]
fn test_float_parsing() {
    let val: f64 = "3.14".parse().unwrap();
    assert!((val - 3.14).abs() < f64::EPSILON);

    let val: f64 = "0.001".parse().unwrap();
    assert!((val - 0.001).abs() < f64::EPSILON);

    // Comma should NOT be accepted as decimal separator.
    let result: Result<f64, _> = "3,14".parse();
    assert!(result.is_err());
}

/// Test integer parsing.
#[test]
fn test_integer_parsing() {
    let val: i64 = "42".parse().unwrap();
    assert_eq!(val, 42);

    let val: i64 = "-100".parse().unwrap();
    assert_eq!(val, -100);

    let val: i64 = "0".parse().unwrap();
    assert_eq!(val, 0);
}

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

/// Test that time formatting is consistent.
#[test]
fn test_time_formatting() {
    // Format a duration in seconds.
    let seconds = 3661u64; // 1 hour, 1 minute, 1 second
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    let formatted = format!("{:02}:{:02}:{:02}", hours, minutes, secs);
    assert_eq!(formatted, "01:01:01");
}

/// Test formatting of large numbers.
#[test]
fn test_large_number_formatting() {
    let val: u64 = 1_000_000;
    let formatted = format!("{}", val);
    assert_eq!(formatted, "1000000");
    // No locale-specific grouping separators.
    assert!(!formatted.contains(','));
    assert!(!formatted.contains('.'));
}

// ---------------------------------------------------------------------------
// Roundtrip format-parse
// ---------------------------------------------------------------------------

/// Test that format/parse roundtrip preserves values.
#[test]
fn test_format_parse_roundtrip() {
    let values: Vec<f64> = vec![0.0, 1.0, -1.0, 3.14, 100.5, 0.001];

    for &val in &values {
        let formatted = format!("{}", val);
        let parsed: f64 = formatted.parse().unwrap();
        assert!(
            (val - parsed).abs() < 1e-10,
            "Roundtrip failed for {}: formatted='{}', parsed={}",
            val,
            formatted,
            parsed
        );
    }
}

/// Test that integer format/parse roundtrip works.
#[test]
fn test_integer_roundtrip() {
    for val in -1000..=1000 {
        let formatted = format!("{}", val);
        let parsed: i32 = formatted.parse().unwrap();
        assert_eq!(val, parsed);
    }
}

// ---------------------------------------------------------------------------
// Special float values
// ---------------------------------------------------------------------------

/// Test formatting of special float values.
#[test]
fn test_special_float_formatting() {
    let inf = f64::INFINITY;
    let neg_inf = f64::NEG_INFINITY;
    let nan = f64::NAN;

    let inf_str = format!("{}", inf);
    let neg_inf_str = format!("{}", neg_inf);
    let nan_str = format!("{}", nan);

    assert!(inf_str.to_lowercase().contains("inf"));
    assert!(neg_inf_str.to_lowercase().contains("inf"));
    assert!(nan_str.to_lowercase().contains("nan"));
}
