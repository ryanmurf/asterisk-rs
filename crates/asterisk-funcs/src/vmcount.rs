//! VMCOUNT() function - count voicemail messages.
//!
//! Port of func_vmcount.c from Asterisk C.
//!
//! Provides:
//! - VMCOUNT(mailbox[@context][&mailbox2[@context2]...][,folder])
//!
//! Counts messages in a voicemail mailbox. Multiple mailboxes can be
//! specified separated by '&'. The folder defaults to "INBOX" if not
//! specified.
//!
//! Usage:
//!   Set(count=${VMCOUNT(100@default)})
//!   Set(count=${VMCOUNT(100@default,Old)})
//!   Set(count=${VMCOUNT(100@default&200@default)})

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Default voicemail folder if none specified.
pub const DEFAULT_FOLDER: &str = "INBOX";

/// VMCOUNT() function.
pub struct FuncVmCount;

impl FuncVmCount {
    /// Parse the mailbox specification into (mailboxes, folder).
    fn parse_args(args: &str) -> Result<(Vec<String>, String), FuncError> {
        let parts: Vec<&str> = args.split(',').collect();
        let mailbox_spec = parts.first().map(|s| s.trim()).unwrap_or("");

        if mailbox_spec.is_empty() {
            return Err(FuncError::InvalidArgument(
                "VMCOUNT requires a mailbox argument".into(),
            ));
        }

        let folder = parts.get(1)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| DEFAULT_FOLDER.to_string());

        // Split multiple mailboxes by '&'
        let mailboxes: Vec<String> = mailbox_spec
            .split('&')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok((mailboxes, folder))
    }
}

impl DialplanFunc for FuncVmCount {
    fn name(&self) -> &str {
        "VMCOUNT"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let (mailboxes, _folder) = Self::parse_args(args)?;

        // Stub: would query voicemail backend for message count
        // Returns "0" for each mailbox (no voicemail system connected)
        let _count = mailboxes.len();
        Ok("0".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vmcount_simple() {
        let ctx = FuncContext::new();
        let func = FuncVmCount;
        let result = func.read(&ctx, "100@default").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_vmcount_with_folder() {
        let ctx = FuncContext::new();
        let func = FuncVmCount;
        let result = func.read(&ctx, "100@default,Old").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_vmcount_multiple() {
        let ctx = FuncContext::new();
        let func = FuncVmCount;
        let result = func.read(&ctx, "100@default&200@default").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_vmcount_empty() {
        let ctx = FuncContext::new();
        let func = FuncVmCount;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_parse_args() {
        let (mailboxes, folder) = FuncVmCount::parse_args("100@default&200@ctx,Old").unwrap();
        assert_eq!(mailboxes, vec!["100@default", "200@ctx"]);
        assert_eq!(folder, "Old");
    }

    #[test]
    fn test_parse_args_default_folder() {
        let (mailboxes, folder) = FuncVmCount::parse_args("100@default").unwrap();
        assert_eq!(mailboxes, vec!["100@default"]);
        assert_eq!(folder, "INBOX");
    }
}
