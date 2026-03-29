//! BLACKLIST() function - check if caller is on the blacklist.
//!
//! Port of func_blacklist.c from Asterisk C.
//!
//! Provides:
//! - BLACKLIST() - returns "1" if caller is blacklisted, "0" otherwise
//!
//! Uses the internal AstDB "blacklist" family to check if the caller's
//! number or name is present.

use crate::{DialplanFunc, FuncContext, FuncResult};

/// BLACKLIST() function.
///
/// Checks if the current caller's ID number or name is in the "blacklist"
/// database family. Returns "1" if found, "0" otherwise.
///
/// Usage: BLACKLIST()
///
/// The blacklist entries are stored as DB(blacklist/number) or
/// DB(blacklist/name) variables in the context. In production,
/// this queries the AstDB.
pub struct FuncBlacklist;

impl DialplanFunc for FuncBlacklist {
    fn name(&self) -> &str {
        "BLACKLIST"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        // Check caller number against blacklist
        if let Some(number) = &ctx.caller_number {
            if !number.is_empty() {
                let key = format!("__DB_blacklist/{}", number);
                if ctx.get_variable(&key).is_some() {
                    return Ok("1".to_string());
                }
            }
        }

        // Check caller name against blacklist
        if let Some(name) = &ctx.caller_name {
            if !name.is_empty() {
                let key = format!("__DB_blacklist/{}", name);
                if ctx.get_variable(&key).is_some() {
                    return Ok("1".to_string());
                }
            }
        }

        Ok("0".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blacklist_not_listed() {
        let mut ctx = FuncContext::new();
        ctx.caller_number = Some("5551234".to_string());
        let func = FuncBlacklist;
        assert_eq!(func.read(&ctx, "").unwrap(), "0");
    }

    #[test]
    fn test_blacklist_number_listed() {
        let mut ctx = FuncContext::new();
        ctx.caller_number = Some("5551234".to_string());
        ctx.set_variable("__DB_blacklist/5551234", "1");
        let func = FuncBlacklist;
        assert_eq!(func.read(&ctx, "").unwrap(), "1");
    }

    #[test]
    fn test_blacklist_name_listed() {
        let mut ctx = FuncContext::new();
        ctx.caller_name = Some("Spammer".to_string());
        let func = FuncBlacklist;
        assert_eq!(func.read(&ctx, "").unwrap(), "0");

        ctx.set_variable("__DB_blacklist/Spammer", "1");
        assert_eq!(func.read(&ctx, "").unwrap(), "1");
    }

    #[test]
    fn test_blacklist_no_caller() {
        let ctx = FuncContext::new();
        let func = FuncBlacklist;
        assert_eq!(func.read(&ctx, "").unwrap(), "0");
    }
}
