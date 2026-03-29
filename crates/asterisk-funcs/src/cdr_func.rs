//! CDR_PROP() function - CDR property control.
//!
//! Port of func_cdr.c (CDR_PROP portion) from Asterisk C.
//!
//! Provides:
//! - CDR_PROP(property) - Read/write CDR properties
//!
//! Properties:
//! - disable: Disable CDR logging for this channel
//! - party_a: Force this channel to be the Party A in CDR

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// CDR_PROP() function.
///
/// Controls CDR behavior properties for the current channel.
///
/// Usage:
///   Set(CDR_PROP(disable)=1)   - Disable CDR for this channel
///   Set(CDR_PROP(party_a)=1)   - Force channel as Party A
///   ${CDR_PROP(disable)}       - Read disable state ("1" or "0")
pub struct FuncCdrProp;

impl DialplanFunc for FuncCdrProp {
    fn name(&self) -> &str {
        "CDR_PROP"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let prop = args.trim().to_lowercase();
        match prop.as_str() {
            "disable" => Ok(ctx
                .get_variable("__CDR_PROP_DISABLE")
                .cloned()
                .unwrap_or_else(|| "0".to_string())),
            "party_a" => Ok(ctx
                .get_variable("__CDR_PROP_PARTY_A")
                .cloned()
                .unwrap_or_else(|| "0".to_string())),
            _ => Err(FuncError::InvalidArgument(format!(
                "CDR_PROP: unknown property '{}', expected 'disable' or 'party_a'",
                prop
            ))),
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let prop = args.trim().to_lowercase();
        let bool_val = match value.trim() {
            "1" | "true" | "yes" | "on" => "1",
            "0" | "false" | "no" | "off" | "" => "0",
            other => {
                return Err(FuncError::InvalidArgument(format!(
                    "CDR_PROP: expected boolean value, got '{}'",
                    other
                )));
            }
        };

        match prop.as_str() {
            "disable" => {
                ctx.set_variable("__CDR_PROP_DISABLE", bool_val);
                Ok(())
            }
            "party_a" => {
                ctx.set_variable("__CDR_PROP_PARTY_A", bool_val);
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "CDR_PROP: unknown property '{}'",
                prop
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdr_prop_default() {
        let ctx = FuncContext::new();
        let func = FuncCdrProp;
        assert_eq!(func.read(&ctx, "disable").unwrap(), "0");
        assert_eq!(func.read(&ctx, "party_a").unwrap(), "0");
    }

    #[test]
    fn test_cdr_prop_disable() {
        let mut ctx = FuncContext::new();
        let func = FuncCdrProp;
        func.write(&mut ctx, "disable", "1").unwrap();
        assert_eq!(func.read(&ctx, "disable").unwrap(), "1");
    }

    #[test]
    fn test_cdr_prop_party_a() {
        let mut ctx = FuncContext::new();
        let func = FuncCdrProp;
        func.write(&mut ctx, "party_a", "yes").unwrap();
        assert_eq!(func.read(&ctx, "party_a").unwrap(), "1");
    }

    #[test]
    fn test_cdr_prop_invalid() {
        let ctx = FuncContext::new();
        let func = FuncCdrProp;
        assert!(func.read(&ctx, "bogus").is_err());
    }
}
