//! Port of asterisk/tests/test_callerid.c
//!
//! Tests caller ID string parsing: "Name" <number>, <number>,
//! number-only, name-only, and presentation values.

/// Parse a caller ID string into (name, number) components.
///
/// This is a Rust port of ast_callerid_parse from callerid.c.
/// Handles formats:
/// - "Name" <number>
/// - Name <number>
/// - <number>
/// - number (digits only)
/// - "Name"
/// - Name
fn parse_callerid(input: &str) -> (Option<String>, Option<String>) {
    let input = input.trim();
    if input.is_empty() {
        return (None, None);
    }

    // Try to find angle brackets for number
    if let Some(lt_pos) = input.find('<') {
        if let Some(gt_pos) = input.find('>') {
            if gt_pos > lt_pos {
                let number_part = input[lt_pos + 1..gt_pos].trim();
                let name_part = input[..lt_pos].trim();

                let number = if number_part.is_empty() {
                    None
                } else {
                    Some(number_part.to_string())
                };

                let name = if name_part.is_empty() {
                    None
                } else {
                    // Strip quotes from name
                    let name_str = strip_quotes(name_part);
                    if name_str.is_empty() {
                        None
                    } else {
                        Some(name_str)
                    }
                };

                return (name, number);
            }
        }
        // Unmatched '<' -- treat as name <number without closing >
        let number_part = input[lt_pos + 1..].trim();
        let name_part = input[..lt_pos].trim();
        let name = if name_part.is_empty() {
            None
        } else {
            Some(strip_quotes(name_part))
        };
        let number = if number_part.is_empty() {
            None
        } else {
            Some(number_part.to_string())
        };
        return (name, number);
    }

    // No angle brackets -- check if it's a quoted name
    if input.starts_with('"') {
        let unquoted = strip_quotes(input);
        if !unquoted.is_empty() {
            return (Some(unquoted), None);
        }
        return (None, None);
    }

    // Check if it looks like a number (all digits)
    if input.chars().all(|c| c.is_ascii_digit()) {
        return (None, Some(input.to_string()));
    }

    // Otherwise treat as name
    (Some(input.to_string()), None)
}

/// Strip surrounding double quotes, handling escaped quotes.
fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        // Unescape \" within the string
        inner.replace("\\\"", "\"")
    } else if s.starts_with('"') {
        // Unmatched opening quote -- strip it
        let inner = &s[1..];
        inner.replace("\\\"", "\"")
    } else {
        s.to_string()
    }
}

/// Port of AST_TEST_DEFINE(parse_nominal) from test_callerid.c.
///
/// Tests parsing of nominal callerid strings.
#[test]
fn test_parse_callerid_nominal() {
    // Test cases from the C code
    let test_cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("\"name\" <number>", Some("name"), Some("number")),
        ("\"   name  \" <number>", Some("   name  "), Some("number")),
        ("name <number>", Some("name"), Some("number")),
        ("         name     <number>", Some("name"), Some("number")),
        ("\"\" <number>", None, Some("number")),
        ("<number>", None, Some("number")),
        ("name", Some("name"), None),
        (" name", Some("name"), None),
        ("\"name\"", Some("name"), None),
        ("\"*10\"", Some("*10"), None),
        (" \"*10\"", Some("*10"), None),
        ("\"name\" <>", Some("name"), None),
        ("name <>", Some("name"), None),
        ("1234", None, Some("1234")),
        (" 1234", None, Some("1234")),
    ];

    for (i, (input, expected_name, expected_number)) in test_cases.iter().enumerate() {
        let (name, number) = parse_callerid(input);
        assert_eq!(
            name.as_deref(),
            *expected_name,
            "Test case {} - name mismatch for input '{}': got {:?}, expected {:?}",
            i, input, name, expected_name
        );
        assert_eq!(
            number.as_deref(),
            *expected_number,
            "Test case {} - number mismatch for input '{}': got {:?}, expected {:?}",
            i, input, number, expected_number
        );
    }
}

/// Port of AST_TEST_DEFINE(parse_off_nominal) from test_callerid.c.
///
/// Tests parsing of off-nominal (edge case) callerid strings.
#[test]
fn test_parse_callerid_off_nominal() {
    let test_cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("\"name <number>\"", Some("name"), Some("number")),
    ];

    for (i, (input, expected_name, expected_number)) in test_cases.iter().enumerate() {
        let (name, number) = parse_callerid(input);
        assert_eq!(
            name.as_deref(),
            *expected_name,
            "Off-nominal test {} - name mismatch for '{}': got {:?}",
            i, input, name
        );
        assert_eq!(
            number.as_deref(),
            *expected_number,
            "Off-nominal test {} - number mismatch for '{}': got {:?}",
            i, input, number
        );
    }
}

/// Test CallerID presentation values.
///
/// Port of party presentation constants from party.h.
#[test]
fn test_callerid_presentation_values() {
    use asterisk_types::party::presentation;

    assert_eq!(presentation::ALLOWED, 0x00);
    assert_eq!(presentation::RESTRICTED, 0x20);
    assert_eq!(presentation::UNAVAILABLE, 0x43);

    // Verify they don't overlap
    assert_ne!(presentation::ALLOWED, presentation::RESTRICTED);
    assert_ne!(presentation::RESTRICTED, presentation::UNAVAILABLE);
    assert_ne!(presentation::ALLOWED, presentation::UNAVAILABLE);
}

/// Test party number and name validation.
///
/// Verifies the structure of PartyId, PartyName, PartyNumber.
#[test]
fn test_party_number_name() {
    use asterisk_types::CallerId;

    let mut cid = CallerId::default();
    cid.id.name.name = "John Doe".to_string();
    cid.id.name.valid = true;
    cid.id.number.number = "5551234".to_string();
    cid.id.number.valid = true;

    assert_eq!(cid.id.name.name, "John Doe");
    assert!(cid.id.name.valid);
    assert_eq!(cid.id.number.number, "5551234");
    assert!(cid.id.number.valid);

    // ANI
    cid.ani.number.number = "5559999".to_string();
    assert_eq!(cid.ani.number.number, "5559999");
}

/// Test empty / default party information.
#[test]
fn test_party_defaults() {
    use asterisk_types::CallerId;

    let cid = CallerId::default();
    assert!(cid.id.name.name.is_empty());
    assert!(!cid.id.name.valid);
    assert!(cid.id.number.number.is_empty());
    assert!(!cid.id.number.valid);
    assert_eq!(cid.ani2, 0);
}

/// Test parsing various caller ID formats used in real configurations.
#[test]
fn test_callerid_real_world_formats() {
    // SIP-style
    let (name, number) = parse_callerid("\"Alice Smith\" <sip:alice@example.com>");
    assert_eq!(name.as_deref(), Some("Alice Smith"));
    assert_eq!(number.as_deref(), Some("sip:alice@example.com"));

    // Just a number
    let (name, number) = parse_callerid("18005551234");
    assert_eq!(name, None);
    assert_eq!(number.as_deref(), Some("18005551234"));

    // Just a name
    let (name, number) = parse_callerid("Reception Desk");
    assert_eq!(name.as_deref(), Some("Reception Desk"));
    assert_eq!(number, None);

    // Empty string
    let (name, number) = parse_callerid("");
    assert_eq!(name, None);
    assert_eq!(number, None);
}
