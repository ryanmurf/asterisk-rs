//! VM_INFO() function - voicemail information query.
//!
//! Provides:
//! - VM_INFO(mailbox[@context],attribute) - Query voicemail info
//!
//! Attributes: count, email, fullname, locale, pager, password, tz, exists, newcount, oldcount

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// VM_INFO() function.
///
/// Returns voicemail information for a specified mailbox.
///
/// Usage:
///   ${VM_INFO(1001@default,count)}     - Total message count
///   ${VM_INFO(1001@default,newcount)}   - New (unread) messages
///   ${VM_INFO(1001@default,oldcount)}   - Old (read) messages
///   ${VM_INFO(1001@default,email)}      - Email address
///   ${VM_INFO(1001@default,fullname)}   - Full name
///   ${VM_INFO(1001@default,locale)}     - Locale
///   ${VM_INFO(1001@default,pager)}      - Pager address
///   ${VM_INFO(1001@default,tz)}         - Timezone
///   ${VM_INFO(1001@default,password)}   - Password (hashed)
///   ${VM_INFO(1001@default,exists)}     - "1" if mailbox exists
///
/// In this port, data is retrieved from context variables.
pub struct FuncVmInfo;

impl FuncVmInfo {
    fn parse_args(args: &str) -> Result<(&str, &str), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "VM_INFO: requires mailbox,attribute arguments".to_string(),
            ));
        }
        let mailbox = parts[0].trim();
        let attribute = parts[1].trim();
        if mailbox.is_empty() || attribute.is_empty() {
            return Err(FuncError::InvalidArgument(
                "VM_INFO: mailbox and attribute cannot be empty".to_string(),
            ));
        }
        Ok((mailbox, attribute))
    }
}

impl DialplanFunc for FuncVmInfo {
    fn name(&self) -> &str {
        "VM_INFO"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (mailbox, attribute) = Self::parse_args(args)?;

        let valid_attrs = [
            "count", "newcount", "oldcount", "email", "fullname",
            "locale", "pager", "password", "tz", "exists",
        ];

        let attr_lower = attribute.to_lowercase();
        if !valid_attrs.contains(&attr_lower.as_str()) {
            return Err(FuncError::InvalidArgument(format!(
                "VM_INFO: unknown attribute '{}'",
                attribute
            )));
        }

        // Normalize mailbox (add @default if no context)
        let normalized = if mailbox.contains('@') {
            mailbox.to_string()
        } else {
            format!("{}@default", mailbox)
        };

        let key = format!("__VM_INFO_{}_{}", normalized, attr_lower.to_uppercase());
        let default = match attr_lower.as_str() {
            "count" | "newcount" | "oldcount" => "0",
            "exists" => "0",
            _ => "",
        };
        Ok(ctx
            .get_variable(&key)
            .cloned()
            .unwrap_or_else(|| default.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_info_defaults() {
        let ctx = FuncContext::new();
        let func = FuncVmInfo;
        assert_eq!(func.read(&ctx, "1001@default,count").unwrap(), "0");
        assert_eq!(func.read(&ctx, "1001@default,exists").unwrap(), "0");
        assert_eq!(func.read(&ctx, "1001@default,email").unwrap(), "");
    }

    #[test]
    fn test_vm_info_with_data() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__VM_INFO_1001@default_COUNT", "5");
        ctx.set_variable("__VM_INFO_1001@default_NEWCOUNT", "3");
        ctx.set_variable("__VM_INFO_1001@default_EMAIL", "alice@example.com");
        ctx.set_variable("__VM_INFO_1001@default_FULLNAME", "Alice Smith");
        ctx.set_variable("__VM_INFO_1001@default_EXISTS", "1");

        let func = FuncVmInfo;
        assert_eq!(func.read(&ctx, "1001@default,count").unwrap(), "5");
        assert_eq!(func.read(&ctx, "1001@default,newcount").unwrap(), "3");
        assert_eq!(func.read(&ctx, "1001@default,email").unwrap(), "alice@example.com");
        assert_eq!(func.read(&ctx, "1001@default,fullname").unwrap(), "Alice Smith");
        assert_eq!(func.read(&ctx, "1001@default,exists").unwrap(), "1");
    }

    #[test]
    fn test_vm_info_implicit_context() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__VM_INFO_1001@default_COUNT", "2");
        let func = FuncVmInfo;
        assert_eq!(func.read(&ctx, "1001,count").unwrap(), "2");
    }

    #[test]
    fn test_vm_info_invalid_attribute() {
        let ctx = FuncContext::new();
        let func = FuncVmInfo;
        assert!(func.read(&ctx, "1001@default,bogus").is_err());
    }

    #[test]
    fn test_vm_info_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncVmInfo;
        assert!(func.read(&ctx, "1001").is_err());
    }
}
