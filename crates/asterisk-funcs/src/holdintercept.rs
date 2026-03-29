//! HOLD_INTERCEPT() function - intercept hold/unhold on a channel.
//!
//! Port of func_holdintercept.c from Asterisk C.
//!
//! Provides:
//! - HOLD_INTERCEPT(action) - intercept hold frames and raise events
//!
//! Actions: "set" to enable, "remove" to disable
//! When enabled, hold/unhold actions from the device are intercepted
//! and raised as AMI/ARI events instead of being processed normally.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// HOLD_INTERCEPT() function.
///
/// Intercepts hold frames on a channel and raises events (AMI/ARI)
/// instead of passing hold/unhold frames through.
///
/// Write-only function:
///   Set(HOLD_INTERCEPT(set)=)   - enable hold interception
///   Set(HOLD_INTERCEPT(remove)=) - disable hold interception
///
/// Read returns current state: "enabled" or "disabled"
pub struct FuncHoldIntercept;

impl DialplanFunc for FuncHoldIntercept {
    fn name(&self) -> &str {
        "HOLD_INTERCEPT"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        Ok(ctx
            .get_variable("__HOLD_INTERCEPT")
            .cloned()
            .unwrap_or_else(|| "disabled".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, _value: &str) -> Result<(), FuncError> {
        let action = args.trim().to_lowercase();
        match action.as_str() {
            "set" => {
                ctx.set_variable("__HOLD_INTERCEPT", "enabled");
                Ok(())
            }
            "remove" => {
                ctx.set_variable("__HOLD_INTERCEPT", "disabled");
                Ok(())
            }
            other => Err(FuncError::InvalidArgument(format!(
                "HOLD_INTERCEPT: action must be 'set' or 'remove', got '{}'",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hold_intercept_default() {
        let ctx = FuncContext::new();
        let func = FuncHoldIntercept;
        assert_eq!(func.read(&ctx, "").unwrap(), "disabled");
    }

    #[test]
    fn test_hold_intercept_enable() {
        let mut ctx = FuncContext::new();
        let func = FuncHoldIntercept;
        func.write(&mut ctx, "set", "").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "enabled");
    }

    #[test]
    fn test_hold_intercept_disable() {
        let mut ctx = FuncContext::new();
        let func = FuncHoldIntercept;
        func.write(&mut ctx, "set", "").unwrap();
        func.write(&mut ctx, "remove", "").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "disabled");
    }

    #[test]
    fn test_hold_intercept_invalid_action() {
        let mut ctx = FuncContext::new();
        let func = FuncHoldIntercept;
        assert!(func.write(&mut ctx, "bogus", "").is_err());
    }
}
