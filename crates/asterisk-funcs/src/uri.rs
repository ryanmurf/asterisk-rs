//! URI encoding/decoding functions.
//!
//! Port of func_uri.c from Asterisk C.
//!
//! Provides:
//! - URIENCODE(string) - percent-encode a string per RFC 2396/3986
//! - URIDECODE(string) - percent-decode a string

use crate::{DialplanFunc, FuncContext, FuncResult};

/// Characters that are unreserved in RFC 3986 and do NOT need encoding.
fn is_unreserved(b: u8) -> bool {
    matches!(b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
        b'-' | b'_' | b'.' | b'~'
    )
}

/// Percent-encode a string per RFC 3986.
///
/// All characters except unreserved characters (A-Z, a-z, 0-9, -, _, ., ~)
/// are encoded as %XX where XX is the hexadecimal representation.
pub fn uri_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        if is_unreserved(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{:02X}", byte));
        }
    }
    encoded
}

/// Percent-decode a string.
///
/// Sequences of %XX are decoded to the corresponding byte value.
/// Invalid sequences are passed through unchanged.
pub fn uri_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1];
            let lo = bytes[i + 2];
            if let (Some(h), Some(l)) = (hex_digit(hi), hex_digit(lo)) {
                decoded.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

/// Convert a hex digit character to its numeric value.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// URIENCODE() function.
///
/// Percent-encodes a string according to RFC 3986.
///
/// Usage: URIENCODE(string)
pub struct FuncUriEncode;

impl DialplanFunc for FuncUriEncode {
    fn name(&self) -> &str {
        "URIENCODE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(uri_encode(args))
    }
}

/// URIDECODE() function.
///
/// Percent-decodes a URI-encoded string.
///
/// Usage: URIDECODE(string)
pub struct FuncUriDecode;

impl DialplanFunc for FuncUriDecode {
    fn name(&self) -> &str {
        "URIDECODE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(uri_decode(args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_encode_basic() {
        assert_eq!(uri_encode("hello world"), "hello%20world");
    }

    #[test]
    fn test_uri_encode_special_chars() {
        assert_eq!(uri_encode("a=b&c=d"), "a%3Db%26c%3Dd");
    }

    #[test]
    fn test_uri_encode_unreserved() {
        assert_eq!(uri_encode("abc-123_XYZ.~"), "abc-123_XYZ.~");
    }

    #[test]
    fn test_uri_encode_empty() {
        assert_eq!(uri_encode(""), "");
    }

    #[test]
    fn test_uri_encode_unicode() {
        // UTF-8 bytes of 'e' with accent (U+00E9) are 0xC3 0xA9
        let encoded = uri_encode("\u{00E9}");
        assert_eq!(encoded, "%C3%A9");
    }

    #[test]
    fn test_uri_decode_basic() {
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

    #[test]
    fn test_uri_decode_invalid_sequence() {
        // Invalid hex digits - pass through unchanged
        assert_eq!(uri_decode("hello%ZZworld"), "hello%ZZworld");
    }

    #[test]
    fn test_uri_decode_truncated() {
        // Truncated percent sequence - pass through
        assert_eq!(uri_decode("hello%2"), "hello%2");
        assert_eq!(uri_decode("hello%"), "hello%");
    }

    #[test]
    fn test_roundtrip() {
        let original = "Hello, World! foo@bar.com/path?q=1&r=2#frag";
        let encoded = uri_encode(original);
        let decoded = uri_decode(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_func_uriencode() {
        let ctx = FuncContext::new();
        let func = FuncUriEncode;
        assert_eq!(func.read(&ctx, "hello world").unwrap(), "hello%20world");
    }

    #[test]
    fn test_func_uridecode() {
        let ctx = FuncContext::new();
        let func = FuncUriDecode;
        assert_eq!(func.read(&ctx, "hello%20world").unwrap(), "hello world");
    }

    #[test]
    fn test_uri_decode_lowercase_hex() {
        assert_eq!(uri_decode("hello%2fworld"), "hello/world");
    }

    #[test]
    fn test_uri_encode_slash() {
        assert_eq!(uri_encode("/"), "%2F");
    }
}
