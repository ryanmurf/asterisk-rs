//! Port of asterisk/tests/test_strings.c
//!
//! Tests string manipulation: trim, strip, escape, URI encode/decode,
//! and base64 encode/decode roundtrips.

/// Test string trim (whitespace removal).
///
/// Port of basic string trimming operations from Asterisk.
#[test]
fn test_string_trim() {
    // Leading whitespace
    assert_eq!("  hello  ".trim(), "hello");
    assert_eq!("hello".trim(), "hello");
    assert_eq!("   ".trim(), "");
    assert_eq!("".trim(), "");

    // Trim start only
    assert_eq!("  hello".trim_start(), "hello");
    assert_eq!("hello  ".trim_start(), "hello  ");

    // Trim end only
    assert_eq!("hello  ".trim_end(), "hello");
    assert_eq!("  hello".trim_end(), "  hello");
}

/// Test string strip (removing specific characters).
///
/// Port of ast_strip_quoted behavior.
#[test]
fn test_string_strip_quotes() {
    // Strip surrounding quotes
    fn strip_quotes(s: &str) -> &str {
        let s = s.trim();
        if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
            &s[1..s.len() - 1]
        } else {
            s
        }
    }

    assert_eq!(strip_quotes("\"hello\""), "hello");
    assert_eq!(strip_quotes("hello"), "hello");
    assert_eq!(strip_quotes("\"\""), "");
    assert_eq!(strip_quotes("\"hello"), "\"hello"); // unmatched
    assert_eq!(strip_quotes("  \"test\"  "), "test");
}

/// Test string escape for quoted strings.
///
/// Port of ast_escape_quoted from test_utils.c / test_strings.c.
/// In Asterisk, this escapes " and \ characters for use in quoted strings.
#[test]
fn test_escape_quoted() {
    fn escape_quoted(input: &str) -> String {
        let mut result = String::with_capacity(input.len() * 2);
        for ch in input.chars() {
            match ch {
                '"' | '\\' => {
                    result.push('\\');
                    result.push(ch);
                }
                _ => result.push(ch),
            }
        }
        result
    }

    // Port of quoted_escape_test from test_utils.c
    let input = "a\"bcdefg\"hijkl\\mnopqrs tuv\twxyz";
    let expected = "a\\\"bcdefg\\\"hijkl\\\\mnopqrs tuv\twxyz";
    assert_eq!(escape_quoted(input), expected);

    // Edge cases
    assert_eq!(escape_quoted(""), "");
    assert_eq!(escape_quoted("no special chars"), "no special chars");
    assert_eq!(escape_quoted("\\\\"), "\\\\\\\\");
    assert_eq!(escape_quoted("\"\""), "\\\"\\\"");
}

/// Test URI encode/decode roundtrip.
///
/// Port of uri_encode_decode_test from test_utils.c.
/// Tests that encoding special characters and decoding produces the original.
#[test]
fn test_uri_encode_decode_roundtrip() {
    fn uri_encode(input: &str) -> String {
        let mut result = String::new();
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                _ => {
                    result.push_str(&format!("%{:02X}", byte));
                }
            }
        }
        result
    }

    fn uri_decode(input: &str) -> String {
        let mut result = Vec::new();
        let bytes = input.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(byte) = u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                    16,
                ) {
                    result.push(byte);
                    i += 3;
                    continue;
                }
            } else if bytes[i] == b'+' {
                result.push(b' ');
                i += 1;
                continue;
            }
            result.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(result).unwrap_or_default()
    }

    // Test roundtrip with special characters
    let inputs = [
        "hello world",
        "a@b.com",
        "foo&bar=baz",
        "special!@#$%^&*()",
        "path/to/file",
        "",
        "no_special_chars",
        "spaces   and\ttabs",
    ];

    for input in &inputs {
        let encoded = uri_encode(input);
        let decoded = uri_decode(&encoded);
        assert_eq!(&decoded, *input, "Roundtrip failed for: {}", input);
    }
}

/// Test base64 encode/decode roundtrip.
///
/// Port of base64_test from test_utils.c.
#[test]
fn test_base64_encode_decode_roundtrip() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    // Test data from test_utils.c
    let test_cases = [
        ("giraffe", "Z2lyYWZmZQ=="),
        ("platypus", "cGxhdHlwdXM="),
        (
            "ParastratiosphecomyiaStratiosphecomyioides",
            "UGFyYXN0cmF0aW9zcGhlY29teWlhU3RyYXRpb3NwaGVjb215aW9pZGVz",
        ),
    ];

    for (input, expected_b64) in &test_cases {
        // Encode
        let encoded = STANDARD.encode(input.as_bytes());
        assert_eq!(
            &encoded, *expected_b64,
            "Base64 encode mismatch for '{}'",
            input
        );

        // Decode
        let decoded_bytes = STANDARD.decode(expected_b64).unwrap();
        let decoded = std::str::from_utf8(&decoded_bytes).unwrap();
        assert_eq!(
            decoded, *input,
            "Base64 decode mismatch for '{}'",
            expected_b64
        );
    }
}

/// Test base64 roundtrip with arbitrary binary data.
#[test]
fn test_base64_roundtrip_binary() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let binary_data: Vec<u8> = (0..=255).collect();
    let encoded = STANDARD.encode(&binary_data);
    let decoded = STANDARD.decode(&encoded).unwrap();
    assert_eq!(decoded, binary_data);
}

/// Test string set and append operations.
///
/// Port of str_test from test_strings.c which tests dynamic string
/// operations. In Rust we use String directly.
#[test]
fn test_string_set_and_append() {
    let short1 = "apple";
    let short2 = "banana";

    // Set
    let mut s = String::new();
    s.push_str(short1);
    assert_eq!(s, short1);

    // Append
    s.push_str(short2);
    assert_eq!(s, "applebanana");

    // Clear
    s.clear();
    assert_eq!(s.len(), 0);
    assert!(s.is_empty());
}

/// Test dynamic string with long strings.
///
/// Port of Part 2 of str_test which tests strings larger than initial allocation.
#[test]
fn test_long_string_operations() {
    let long1 = "applebananapeachmangocherrypeargrapeplumlimetangerinepomegranategravel";
    let long2 = "passionuglinectarinepineapplekiwilemonpaintthinner";

    let mut s = String::with_capacity(15); // small initial capacity

    // Set with long string -- should grow
    s.push_str(long1);
    assert_eq!(s, long1);

    // Append another long string
    s.push_str(long2);
    let expected = format!("{}{}", long1, long2);
    assert_eq!(s, expected);
}

/// Test string field operations (split by delimiter).
///
/// Port of string field behavior in Asterisk.
#[test]
fn test_string_field_operations() {
    let csv = "field1,field2,field3,field4";
    let fields: Vec<&str> = csv.split(',').collect();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0], "field1");
    assert_eq!(fields[1], "field2");
    assert_eq!(fields[2], "field3");
    assert_eq!(fields[3], "field4");

    // With spaces
    let spaced = " a , b , c ";
    let fields: Vec<&str> = spaced.split(',').map(|s| s.trim()).collect();
    assert_eq!(fields, vec!["a", "b", "c"]);
}
