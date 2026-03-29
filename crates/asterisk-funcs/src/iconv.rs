//! ICONV() function - character set conversion.
//!
//! Port of func_iconv.c from Asterisk C.
//!
//! Provides:
//! - ICONV(in_charset,out_charset,string) - convert string between character sets
//!
//! Usage:
//!   Set(result=${ICONV(ISO-8859-1,UTF-8,${mystring})})
//!
//! This is a stub implementation. Full iconv requires libiconv or glibc iconv.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// ICONV() function.
pub struct FuncIconv;

impl DialplanFunc for FuncIconv {
    fn name(&self) -> &str {
        "ICONV"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "ICONV requires in_charset, out_charset, and string arguments".into(),
            ));
        }

        let in_charset = parts[0].trim();
        let out_charset = parts[1].trim();
        let input_string = parts[2];

        if in_charset.is_empty() || out_charset.is_empty() {
            return Err(FuncError::InvalidArgument(
                "ICONV: charset arguments cannot be empty".into(),
            ));
        }

        // If both charsets are the same, or both are UTF-8, pass through
        if in_charset.eq_ignore_ascii_case(out_charset)
            || (in_charset.eq_ignore_ascii_case("UTF-8")
                && out_charset.eq_ignore_ascii_case("UTF-8"))
        {
            return Ok(input_string.to_string());
        }

        // Stub: real conversion requires libiconv
        // Return the input as-is with a warning (Rust strings are UTF-8)
        Ok(input_string.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iconv_same_charset() {
        let ctx = FuncContext::new();
        let func = FuncIconv;
        let result = func.read(&ctx, "UTF-8,UTF-8,hello world").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_iconv_passthrough() {
        let ctx = FuncContext::new();
        let func = FuncIconv;
        let result = func.read(&ctx, "ISO-8859-1,UTF-8,test").unwrap();
        assert_eq!(result, "test");
    }

    #[test]
    fn test_iconv_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncIconv;
        assert!(func.read(&ctx, "UTF-8,UTF-8").is_err());
    }

    #[test]
    fn test_iconv_empty_charset() {
        let ctx = FuncContext::new();
        let func = FuncIconv;
        assert!(func.read(&ctx, ",UTF-8,hello").is_err());
    }
}
