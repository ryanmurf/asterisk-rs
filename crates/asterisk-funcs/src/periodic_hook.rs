//! PERIODIC_HOOK() function - periodic dialplan execution during calls.
//!
//! Port of func_periodic_hook.c from Asterisk C.
//!
//! Provides:
//! - PERIODIC_HOOK(context,extension,interval) - execute dialplan periodically
//!
//! Read returns a hook ID that can be used to disable the hook later.
//! Write with value "off" disables, "on" re-enables.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// PERIODIC_HOOK() function.
///
/// Registers a periodic dialplan hook that fires at a given interval
/// during a call. The hook runs asynchronously on a new channel and
/// injects audio into the call.
///
/// Read usage:  PERIODIC_HOOK(context,extension,interval) -> hook_id
///   Validates arguments and returns a placeholder ID "0".
///   In the real PBX engine, the actual hook is registered via write semantics.
///
/// Write usage: Set(PERIODIC_HOOK(hook_id)=off)  to disable
///              Set(PERIODIC_HOOK(hook_id)=on)   to re-enable
pub struct FuncPeriodicHook;

impl DialplanFunc for FuncPeriodicHook {
    fn name(&self) -> &str {
        "PERIODIC_HOOK"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "PERIODIC_HOOK: requires context,extension,interval arguments".to_string(),
            ));
        }

        let context = parts[0].trim();
        let extension = parts[1].trim();
        let interval_str = parts[2].trim();

        if context.is_empty() || extension.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PERIODIC_HOOK: context and extension cannot be empty".to_string(),
            ));
        }

        let interval: u64 = interval_str.parse().map_err(|_| {
            FuncError::InvalidArgument(format!(
                "PERIODIC_HOOK: invalid interval '{}'",
                interval_str
            ))
        })?;

        if interval == 0 {
            return Err(FuncError::InvalidArgument(
                "PERIODIC_HOOK: interval must be > 0".to_string(),
            ));
        }

        // In the real implementation, this allocates a hook via the PBX engine.
        // Return a placeholder hook ID.
        Ok("0".to_string())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let hook_id = args.trim();
        if hook_id.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PERIODIC_HOOK: hook_id is required for write".to_string(),
            ));
        }

        let cfg_var = format!("__PERIODIC_HOOK_{}", hook_id);

        match value.trim().to_lowercase().as_str() {
            "off" | "on" => {
                let state = value.trim().to_lowercase();
                // Update or create hook state
                if let Some(current) = ctx.get_variable(&cfg_var).cloned() {
                    let mut parts: Vec<String> =
                        current.splitn(4, '|').map(|s| s.to_string()).collect();
                    if parts.len() >= 4 {
                        parts[3] = state;
                        ctx.set_variable(&cfg_var, &parts.join("|"));
                    }
                } else {
                    ctx.set_variable(&cfg_var, &state);
                }
                Ok(())
            }
            other => Err(FuncError::InvalidArgument(format!(
                "PERIODIC_HOOK: value must be 'on' or 'off', got '{}'",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_periodic_hook_read_validates() {
        let ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        let result = func.read(&ctx, "hooks,beep,180");
        assert!(result.is_ok());
    }

    #[test]
    fn test_periodic_hook_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        assert!(func.read(&ctx, "hooks,beep").is_err());
    }

    #[test]
    fn test_periodic_hook_invalid_interval() {
        let ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        assert!(func.read(&ctx, "hooks,beep,abc").is_err());
    }

    #[test]
    fn test_periodic_hook_zero_interval() {
        let ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        assert!(func.read(&ctx, "hooks,beep,0").is_err());
    }

    #[test]
    fn test_periodic_hook_write() {
        let mut ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        ctx.set_variable("__PERIODIC_HOOK_1", "hooks|beep|180|on");
        func.write(&mut ctx, "1", "off").unwrap();
        let val = ctx.get_variable("__PERIODIC_HOOK_1").unwrap();
        assert!(val.contains("off"));
    }

    #[test]
    fn test_periodic_hook_write_invalid_value() {
        let mut ctx = FuncContext::new();
        let func = FuncPeriodicHook;
        assert!(func.write(&mut ctx, "1", "bogus").is_err());
    }
}
