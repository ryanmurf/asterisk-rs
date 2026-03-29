//! Base64 encoding/decoding functions.
//!
//! Port of func_base64.c from Asterisk C.
//!
//! Provides:
//! - BASE64_ENCODE(string) - encode a string to base64
//! - BASE64_DECODE(string) - decode a base64 string

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use base64::{engine::general_purpose::STANDARD, Engine};

/// BASE64_ENCODE() function.
///
/// Encodes a string to base64 representation.
///
/// Usage: BASE64_ENCODE(string)
pub struct FuncBase64Encode;

impl DialplanFunc for FuncBase64Encode {
    fn name(&self) -> &str {
        "BASE64_ENCODE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        if args.is_empty() {
            return Err(FuncError::InvalidArgument(
                "BASE64_ENCODE: data argument is required".to_string(),
            ));
        }
        Ok(STANDARD.encode(args.as_bytes()))
    }
}

/// BASE64_DECODE() function.
///
/// Decodes a base64-encoded string back to plaintext.
///
/// Usage: BASE64_DECODE(string)
pub struct FuncBase64Decode;

impl DialplanFunc for FuncBase64Decode {
    fn name(&self) -> &str {
        "BASE64_DECODE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        if args.is_empty() {
            return Err(FuncError::InvalidArgument(
                "BASE64_DECODE: data argument is required".to_string(),
            ));
        }
        let decoded = STANDARD.decode(args.trim()).map_err(|e| {
            FuncError::InvalidArgument(format!("BASE64_DECODE: invalid base64 input: {}", e))
        })?;
        String::from_utf8(decoded).map_err(|e| {
            FuncError::Internal(format!(
                "BASE64_DECODE: decoded data is not valid UTF-8: {}",
                e
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        let ctx = FuncContext::new();
        let func = FuncBase64Encode;
        assert_eq!(func.read(&ctx, "Hello, World!").unwrap(), "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn test_base64_decode() {
        let ctx = FuncContext::new();
        let func = FuncBase64Decode;
        assert_eq!(
            func.read(&ctx, "SGVsbG8sIFdvcmxkIQ==").unwrap(),
            "Hello, World!"
        );
    }

    #[test]
    fn test_base64_roundtrip() {
        let ctx = FuncContext::new();
        let encode = FuncBase64Encode;
        let decode = FuncBase64Decode;
        let original = "The quick brown fox jumps over the lazy dog";
        let encoded = encode.read(&ctx, original).unwrap();
        let decoded = decode.read(&ctx, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_base64_encode_empty_arg() {
        let ctx = FuncContext::new();
        let func = FuncBase64Encode;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_base64_decode_empty_arg() {
        let ctx = FuncContext::new();
        let func = FuncBase64Decode;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_base64_decode_invalid() {
        let ctx = FuncContext::new();
        let func = FuncBase64Decode;
        assert!(func.read(&ctx, "!!!not-base64!!!").is_err());
    }

    #[test]
    fn test_base64_encode_binary_safe() {
        let ctx = FuncContext::new();
        let encode = FuncBase64Encode;
        // Encode a string with special characters
        let result = encode.read(&ctx, "\x00\x01\x02").unwrap();
        assert_eq!(result, "AAEC");
    }

    #[test]
    fn test_base64_various_lengths() {
        let ctx = FuncContext::new();
        let encode = FuncBase64Encode;
        let decode = FuncBase64Decode;
        // Test various padding scenarios
        for s in &["a", "ab", "abc", "abcd", "abcde"] {
            let encoded = encode.read(&ctx, s).unwrap();
            let decoded = decode.read(&ctx, &encoded).unwrap();
            assert_eq!(&decoded, s);
        }
    }
}
