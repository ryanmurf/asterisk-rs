//! Port of asterisk/tests/test_uri.c
//!
//! Tests URI parsing/encoding:
//! - Parse SIP URI components
//! - URI encode special characters
//! - URI decode percent-encoded strings
//! - Roundtrip encode/decode
//!
//! Uses the uri module from asterisk-funcs.

use asterisk_funcs::uri::{uri_decode, uri_encode};

// ---------------------------------------------------------------------------
// URI encode special characters
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(uri_encode_test) from test_uri.c.
///
/// Test that special characters are percent-encoded correctly.
#[test]
fn test_uri_encode_space() {
    assert_eq!(uri_encode("hello world"), "hello%20world");
}

#[test]
fn test_uri_encode_ampersand_equals() {
    assert_eq!(uri_encode("a=b&c=d"), "a%3Db%26c%3Dd");
}

#[test]
fn test_uri_encode_unreserved_passthrough() {
    // RFC 3986 unreserved characters should pass through unchanged.
    assert_eq!(uri_encode("abc-123_XYZ.~"), "abc-123_XYZ.~");
}

#[test]
fn test_uri_encode_empty() {
    assert_eq!(uri_encode(""), "");
}

#[test]
fn test_uri_encode_all_alpha() {
    let alpha = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    assert_eq!(uri_encode(alpha), alpha);
}

#[test]
fn test_uri_encode_slash() {
    assert_eq!(uri_encode("/"), "%2F");
}

#[test]
fn test_uri_encode_at_sign() {
    assert_eq!(uri_encode("user@host"), "user%40host");
}

#[test]
fn test_uri_encode_colon() {
    assert_eq!(uri_encode("host:5060"), "host%3A5060");
}

/// Test encoding of unicode characters (multi-byte UTF-8).
#[test]
fn test_uri_encode_unicode() {
    // U+00E9 (e with accent) is 0xC3 0xA9 in UTF-8.
    let encoded = uri_encode("\u{00E9}");
    assert_eq!(encoded, "%C3%A9");
}

/// Test encoding of hash and question mark.
#[test]
fn test_uri_encode_hash_question() {
    assert_eq!(uri_encode("#"), "%23");
    assert_eq!(uri_encode("?"), "%3F");
}

// ---------------------------------------------------------------------------
// URI decode percent-encoded strings
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(uri_decode_test) from test_uri.c.
#[test]
fn test_uri_decode_space() {
    assert_eq!(uri_decode("hello%20world"), "hello world");
}

#[test]
fn test_uri_decode_special_chars() {
    assert_eq!(uri_decode("a%3Db%26c%3Dd"), "a=b&c=d");
}

#[test]
fn test_uri_decode_passthrough() {
    assert_eq!(uri_decode("hello"), "hello");
}

#[test]
fn test_uri_decode_empty() {
    assert_eq!(uri_decode(""), "");
}

/// Test decoding with lowercase hex digits.
#[test]
fn test_uri_decode_lowercase_hex() {
    assert_eq!(uri_decode("hello%2fworld"), "hello/world");
}

/// Test decoding invalid percent sequences (pass through unchanged).
#[test]
fn test_uri_decode_invalid_sequence() {
    assert_eq!(uri_decode("hello%ZZworld"), "hello%ZZworld");
}

/// Test decoding truncated percent sequences.
#[test]
fn test_uri_decode_truncated() {
    assert_eq!(uri_decode("hello%2"), "hello%2");
    assert_eq!(uri_decode("hello%"), "hello%");
}

/// Test decoding percent at end of string.
#[test]
fn test_uri_decode_percent_at_end() {
    assert_eq!(uri_decode("test%"), "test%");
}

/// Test decoding multiple consecutive encoded characters.
#[test]
fn test_uri_decode_consecutive_encoded() {
    assert_eq!(uri_decode("%20%20%20"), "   ");
}

// ---------------------------------------------------------------------------
// Roundtrip encode/decode
// ---------------------------------------------------------------------------

/// Port of the roundtrip test from test_uri.c.
#[test]
fn test_uri_roundtrip_simple() {
    let original = "Hello, World!";
    let encoded = uri_encode(original);
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, original);
}

#[test]
fn test_uri_roundtrip_complex() {
    let original = "foo@bar.com/path?q=1&r=2#frag";
    let encoded = uri_encode(original);
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, original);
}

#[test]
fn test_uri_roundtrip_special_chars() {
    let original = "!@#$%^&*(){}[]|\\:;\"'<>,./? \t\n";
    let encoded = uri_encode(original);
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, original);
}

#[test]
fn test_uri_roundtrip_empty() {
    let original = "";
    let encoded = uri_encode(original);
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, original);
}

#[test]
fn test_uri_roundtrip_unreserved_only() {
    let original = "abcXYZ012-_.~";
    let encoded = uri_encode(original);
    // Unreserved chars should not be changed.
    assert_eq!(encoded, original);
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, original);
}

// ---------------------------------------------------------------------------
// Parse SIP URI components
// ---------------------------------------------------------------------------

/// Port of SIP URI parsing tests from test_uri.c.
///
/// Parse a SIP URI into its components. This is a simplified parser
/// for testing purposes.
fn parse_sip_uri(uri: &str) -> Option<SipUri> {
    let uri = uri.trim();

    // Extract scheme.
    let (scheme, rest) = if let Some(pos) = uri.find(':') {
        let s = &uri[..pos];
        if s == "sip" || s == "sips" {
            (s.to_string(), &uri[pos + 1..])
        } else {
            return None;
        }
    } else {
        return None;
    };

    // Strip leading slashes if present.
    let rest = rest.trim_start_matches('/');

    // Extract user@host:port.
    let (user, host_part) = if let Some(at_pos) = rest.find('@') {
        (Some(rest[..at_pos].to_string()), &rest[at_pos + 1..])
    } else {
        (None, rest)
    };

    // Extract host and port.
    let (host, port) = if let Some(colon_pos) = host_part.find(':') {
        let h = &host_part[..colon_pos];
        let p_str = &host_part[colon_pos + 1..];
        // Port might be followed by parameters.
        let p_end = p_str.find(';').unwrap_or(p_str.len());
        let p: u16 = p_str[..p_end].parse().unwrap_or(0);
        (h.to_string(), Some(p))
    } else {
        let h_end = host_part.find(';').unwrap_or(host_part.len());
        (host_part[..h_end].to_string(), None)
    };

    Some(SipUri {
        scheme,
        user,
        host,
        port,
    })
}

#[derive(Debug, PartialEq)]
struct SipUri {
    scheme: String,
    user: Option<String>,
    host: String,
    port: Option<u16>,
}

/// Test parsing a full SIP URI.
#[test]
fn test_parse_sip_uri_full() {
    let uri = parse_sip_uri("sip:alice@example.com:5060").unwrap();
    assert_eq!(uri.scheme, "sip");
    assert_eq!(uri.user, Some("alice".to_string()));
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, Some(5060));
}

/// Test parsing SIP URI without user.
#[test]
fn test_parse_sip_uri_no_user() {
    let uri = parse_sip_uri("sip:example.com:5060").unwrap();
    assert_eq!(uri.scheme, "sip");
    assert_eq!(uri.user, None);
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, Some(5060));
}

/// Test parsing SIP URI without port.
#[test]
fn test_parse_sip_uri_no_port() {
    let uri = parse_sip_uri("sip:alice@example.com").unwrap();
    assert_eq!(uri.scheme, "sip");
    assert_eq!(uri.user, Some("alice".to_string()));
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, None);
}

/// Test parsing SIPS URI.
#[test]
fn test_parse_sips_uri() {
    let uri = parse_sip_uri("sips:bob@secure.example.com:5061").unwrap();
    assert_eq!(uri.scheme, "sips");
    assert_eq!(uri.user, Some("bob".to_string()));
    assert_eq!(uri.host, "secure.example.com");
    assert_eq!(uri.port, Some(5061));
}

/// Test parsing invalid scheme returns None.
#[test]
fn test_parse_invalid_scheme() {
    assert!(parse_sip_uri("http:example.com").is_none());
    assert!(parse_sip_uri("ftp:example.com").is_none());
}

/// Test parsing minimal SIP URI (host only).
#[test]
fn test_parse_sip_uri_minimal() {
    let uri = parse_sip_uri("sip:example.com").unwrap();
    assert_eq!(uri.scheme, "sip");
    assert_eq!(uri.user, None);
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, None);
}
