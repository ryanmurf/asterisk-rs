//! Extended ENUM lookup functions - TXTCIDNAME() and additional ENUM utilities.
//!
//! Extensions to func_enum.c providing:
//! - TXTCIDNAME(number) - DNS TXT record lookup for Caller ID name
//! - ENUMLOOKUP_EXT(number,zone,service) - Extended ENUM with explicit service

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use crate::enum_func::FuncEnumLookup;

/// TXTCIDNAME() function.
///
/// Performs a DNS TXT record lookup to resolve a phone number to a
/// Caller ID name. This is a simplified alternative to full ENUM.
///
/// Usage: TXTCIDNAME(number)
///
/// In this port, lookups are simulated via context variables.
pub struct FuncTxtCidName;

impl FuncTxtCidName {
    /// Convert number to DNS TXT lookup domain.
    /// E.g., +15551234567 -> 7.6.5.4.3.2.1.5.5.5.1.e164.arpa (same as ENUM)
    pub fn number_to_txt_domain(number: &str) -> String {
        FuncEnumLookup::number_to_domain(number, "e164.arpa")
    }
}

impl DialplanFunc for FuncTxtCidName {
    fn name(&self) -> &str {
        "TXTCIDNAME"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let number = args.trim();
        if number.is_empty() {
            return Err(FuncError::InvalidArgument(
                "TXTCIDNAME: number argument is required".to_string(),
            ));
        }

        let domain = Self::number_to_txt_domain(number);
        let var_key = format!("__TXTCID_{}", domain);
        Ok(ctx.get_variable(&var_key).cloned().unwrap_or_default())
    }
}

/// ENUMLOOKUP_EXT() - Extended ENUM lookup with additional parameters.
///
/// Usage: ENUMLOOKUP_EXT(number,zone,service)
pub struct FuncEnumLookupExt;

impl DialplanFunc for FuncEnumLookupExt {
    fn name(&self) -> &str {
        "ENUMLOOKUP_EXT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.is_empty() || parts[0].trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "ENUMLOOKUP_EXT: number is required".to_string(),
            ));
        }

        let number = parts[0].trim();
        let zone = if parts.len() > 1 && !parts[1].trim().is_empty() {
            parts[1].trim()
        } else {
            "e164.arpa"
        };
        let _service = if parts.len() > 2 { parts[2].trim() } else { "E2U+sip" };

        let domain = FuncEnumLookup::number_to_domain(number, zone);
        let var_key = format!("__ENUM_RESULT_{}", domain);
        Ok(ctx.get_variable(&var_key).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_txtcidname_empty() {
        let ctx = FuncContext::new();
        let func = FuncTxtCidName;
        let result = func.read(&ctx, "+15551234567").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_txtcidname_with_stored() {
        let mut ctx = FuncContext::new();
        let domain = FuncTxtCidName::number_to_txt_domain("+15551234567");
        ctx.set_variable(&format!("__TXTCID_{}", domain), "John Doe");
        let func = FuncTxtCidName;
        assert_eq!(func.read(&ctx, "+15551234567").unwrap(), "John Doe");
    }

    #[test]
    fn test_txtcidname_missing_number() {
        let ctx = FuncContext::new();
        assert!(FuncTxtCidName.read(&ctx, "").is_err());
    }

    #[test]
    fn test_enumlookup_ext() {
        let ctx = FuncContext::new();
        let func = FuncEnumLookupExt;
        let result = func.read(&ctx, "+15551234567,e164.arpa,E2U+sip").unwrap();
        assert!(result.is_empty());
    }
}
