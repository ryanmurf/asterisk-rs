//! Port of asterisk/tests/test_xml_escape.c
//!
//! Tests XML character escaping:
//! - Standard XML entity encoding (<, &, >, ', ")
//! - Buffer size 0 handling
//! - Truncation of characters
//! - Truncation of entities at the boundary

// ---------------------------------------------------------------------------
// XML escape implementation
// ---------------------------------------------------------------------------

/// Escape XML special characters into a buffer of at most `max_len` bytes.
/// Returns Ok(escaped_string) or Err if the output was truncated or max_len is 0.
fn xml_escape(input: &str, max_len: usize) -> Result<String, String> {
    if max_len == 0 {
        return Err(String::new());
    }

    let mut output = String::new();
    let mut remaining = max_len - 1; // Reserve space for null terminator equivalent.

    for ch in input.chars() {
        let entity = match ch {
            '<' => "&lt;",
            '>' => "&gt;",
            '&' => "&amp;",
            '\'' => "&apos;",
            '"' => "&quot;",
            _ => {
                if remaining >= 1 {
                    output.push(ch);
                    remaining -= 1;
                    continue;
                } else {
                    return Err(output);
                }
            }
        };

        if remaining >= entity.len() {
            output.push_str(entity);
            remaining -= entity.len();
        } else {
            // Not enough room for the entity; truncate.
            return Err(output);
        }
    }

    Ok(output)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(xml_escape_test) from test_xml_escape.c.
///
/// Happy path: all special characters are escaped.
#[test]
fn test_xml_escape_happy_path() {
    let input = "encode me: <&>'\"";
    let expected = "encode me: &lt;&amp;&gt;&apos;&quot;";

    let result = xml_escape(input, 256).unwrap();
    assert_eq!(result, expected);
}

/// Size 0 should fail without producing output.
#[test]
fn test_xml_escape_size_zero() {
    let result = xml_escape("foo", 0);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "");
}

/// Truncation of characters.
#[test]
fn test_xml_escape_truncate_chars() {
    let input = "<truncated>";
    let expected = "&lt;trunc";

    let result = xml_escape(input, 10);
    assert!(result.is_err());
    let actual = result.unwrap_err();
    assert_eq!(actual, expected);
}

/// Truncation at entity boundary.
#[test]
fn test_xml_escape_truncate_entity() {
    let input = "trunc<";
    let expected = "trunc";

    // 9 bytes of output space: "trunc" = 5 chars, "&lt;" = 4 chars, needs 9, but
    // max_len=9 means 8 usable bytes. "trunc" fits (5), "&lt;" (4) would need 9 total > 8.
    let result = xml_escape(input, 9);
    assert!(result.is_err());
    let actual = result.unwrap_err();
    assert_eq!(actual, expected);
}

/// No special characters means no escaping needed.
#[test]
fn test_xml_escape_no_special() {
    let input = "hello world";
    let result = xml_escape(input, 256).unwrap();
    assert_eq!(result, "hello world");
}

/// Empty input produces empty output.
#[test]
fn test_xml_escape_empty() {
    let result = xml_escape("", 256).unwrap();
    assert_eq!(result, "");
}
