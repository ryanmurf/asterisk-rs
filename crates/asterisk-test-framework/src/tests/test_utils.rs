//! Port of asterisk/tests/test_utils.c
//!
//! Tests utility functions: URI encode/decode, base64 roundtrip,
//! MD5 hash computation, and SHA1 hash computation.

/// Port of AST_TEST_DEFINE(uri_encode_decode_test) from test_utils.c.
///
/// Test that URI encoding of special characters produces expected output
/// and that decoding restores the original.
#[test]
fn test_uri_encode_special_characters() {
    fn uri_encode_http(input: &str) -> String {
        let mut result = String::new();
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
                | b'*' | b'(' | b')' | b'\'' => {
                    result.push(byte as char);
                }
                b' ' => result.push_str("%20"),
                _ => result.push_str(&format!("%{:02X}", byte)),
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
            }
            result.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(result).unwrap_or_default()
    }

    // Test basic encoding
    let input = "hello world";
    let encoded = uri_encode_http(input);
    assert_eq!(encoded, "hello%20world");

    // Roundtrip
    let decoded = uri_decode(&encoded);
    assert_eq!(decoded, input);

    // Test with all printable ASCII
    let all_alpha = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let encoded_alpha = uri_encode_http(all_alpha);
    // Alphanumeric chars should not be encoded
    assert_eq!(encoded_alpha, all_alpha);

    // Test special characters
    let special = "@#$%^&";
    let encoded_special = uri_encode_http(special);
    // Each non-safe char should be percent-encoded
    assert!(encoded_special.contains("%40")); // @
    assert!(encoded_special.contains("%23")); // #
    assert!(encoded_special.contains("%24")); // $
    assert!(encoded_special.contains("%25")); // %
    assert!(encoded_special.contains("%5E")); // ^

    // Roundtrip special
    let decoded_special = uri_decode(&encoded_special);
    assert_eq!(decoded_special, special);
}

/// Test URI encode with empty string.
#[test]
fn test_uri_encode_empty() {
    fn uri_encode(input: &str) -> String {
        let mut result = String::new();
        for byte in input.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                _ => result.push_str(&format!("%{:02X}", byte)),
            }
        }
        result
    }

    assert_eq!(uri_encode(""), "");
}

/// Port of AST_TEST_DEFINE(base64_test) from test_utils.c.
///
/// Test base64 encode and decode with known test vectors.
#[test]
fn test_base64_roundtrip() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let test_cases = [
        ("giraffe", "Z2lyYWZmZQ=="),
        ("platypus", "cGxhdHlwdXM="),
        (
            "ParastratiosphecomyiaStratiosphecomyioides",
            "UGFyYXN0cmF0aW9zcGhlY29teWlhU3RyYXRpb3NwaGVjb215aW9pZGVz",
        ),
    ];

    for (input, expected_encoded) in &test_cases {
        // Encode
        let encoded = STANDARD.encode(input.as_bytes());
        assert_eq!(
            &encoded, *expected_encoded,
            "Base64 encode failed for '{}'",
            input
        );

        // Decode
        let decoded_bytes = STANDARD.decode(expected_encoded).unwrap();
        let decoded = String::from_utf8(decoded_bytes).unwrap();
        assert_eq!(
            &decoded, *input,
            "Base64 decode failed for '{}'",
            expected_encoded
        );
    }
}

/// Port of AST_TEST_DEFINE(md5_test) from test_utils.c.
///
/// Test MD5 hash computation with known test vectors.
#[test]
fn test_md5_hash() {
    use md5::{Digest, Md5};

    let test_cases = [
        ("apples", "daeccf0ad3c1fc8c8015205c332f5b42"),
        ("bananas", "ec121ff80513ae58ed478d5c5787075b"),
        (
            "reallylongstringaboutgoatcheese",
            "0a2d9280d37e2e37545cfef6e7e4e890",
        ),
    ];

    for (input, expected_hash) in &test_cases {
        let mut hasher = Md5::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        let hash_hex = hex::encode(result);
        assert_eq!(
            hash_hex, *expected_hash,
            "MD5 hash mismatch for '{}'",
            input
        );
    }
}

/// Port of AST_TEST_DEFINE(sha1_test) from test_utils.c.
///
/// Test SHA1 hash computation with known test vectors.
#[test]
fn test_sha1_hash() {
    use sha1::{Digest, Sha1};

    let test_cases = [
        ("giraffe", "fac8f1a31d2998734d6a5253e49876b8e6a08239"),
        ("platypus", "1dfb21b7a4d35e90d943e3a16107ccbfabd064d5"),
        (
            "ParastratiosphecomyiaStratiosphecomyioides",
            "58af4e8438676f2bd3c4d8df9e00ee7fe06945bb",
        ),
    ];

    for (input, expected_hash) in &test_cases {
        let mut hasher = Sha1::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        let hash_hex = hex::encode(result);
        assert_eq!(
            hash_hex, *expected_hash,
            "SHA1 hash mismatch for '{}'",
            input
        );
    }
}

/// Test MD5 with empty input.
#[test]
fn test_md5_empty() {
    use md5::{Digest, Md5};

    let mut hasher = Md5::new();
    hasher.update(b"");
    let result = hasher.finalize();
    let hash_hex = hex::encode(result);
    assert_eq!(hash_hex, "d41d8cd98f00b204e9800998ecf8427e"); // Known MD5 of empty string
}

/// Test SHA1 with empty input.
#[test]
fn test_sha1_empty() {
    use sha1::{Digest, Sha1};

    let mut hasher = Sha1::new();
    hasher.update(b"");
    let result = hasher.finalize();
    let hash_hex = hex::encode(result);
    assert_eq!(hash_hex, "da39a3ee5e6b4b0d3255bfef95601890afd80709"); // Known SHA1 of empty string
}

/// Test MD5 with binary data.
#[test]
fn test_md5_binary() {
    use md5::{Digest, Md5};

    let data: Vec<u8> = (0..=255).collect();
    let mut hasher = Md5::new();
    hasher.update(&data);
    let result = hasher.finalize();
    let hash_hex = hex::encode(result);
    // Just verify it produces a valid 32-char hex string
    assert_eq!(hash_hex.len(), 32);
    assert!(hash_hex.chars().all(|c| c.is_ascii_hexdigit()));
}

/// Port of quoted_escape_test from test_utils.c.
///
/// Test escaping quoted strings (escaping " and \).
#[test]
fn test_quoted_escape() {
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

    let input = "a\"bcdefg\"hijkl\\mnopqrs tuv\twxyz";
    let expected = "a\\\"bcdefg\\\"hijkl\\\\mnopqrs tuv\twxyz";
    assert_eq!(escape_quoted(input), expected);
}
