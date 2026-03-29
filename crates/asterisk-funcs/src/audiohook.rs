//! AUDIOHOOK_INHERIT() function - inherit audiohooks across channels.
//!
//! Port of func_audiohookinherit.c from Asterisk C.
//!
//! Provides:
//! - AUDIOHOOK_INHERIT(source) - set whether audiohooks from a given source
//!   should be inherited when a channel is masqueraded (e.g., during transfer).
//!
//! Usage:
//!   Set(AUDIOHOOK_INHERIT(MixMonitor)=yes)  - inherit MixMonitor audiohook
//!   Set(AUDIOHOOK_INHERIT(MixMonitor)=no)   - don't inherit

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// AUDIOHOOK_INHERIT() function.
pub struct FuncAudiohookInherit;

impl DialplanFunc for FuncAudiohookInherit {
    fn name(&self) -> &str {
        "AUDIOHOOK_INHERIT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let source = args.trim();
        if source.is_empty() {
            return Err(FuncError::InvalidArgument(
                "AUDIOHOOK_INHERIT requires a source argument".into(),
            ));
        }
        let var = format!("__AUDIOHOOK_INHERIT_{}", source.to_uppercase());
        Ok(ctx.get_variable(&var).cloned().unwrap_or_else(|| "no".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let source = args.trim();
        if source.is_empty() {
            return Err(FuncError::InvalidArgument(
                "AUDIOHOOK_INHERIT requires a source argument".into(),
            ));
        }

        let normalized = match value.trim().to_lowercase().as_str() {
            "yes" | "1" | "true" | "on" => "yes",
            "no" | "0" | "false" | "off" => "no",
            _ => return Err(FuncError::InvalidArgument(format!(
                "AUDIOHOOK_INHERIT: value must be yes/no, got '{}'", value
            ))),
        };

        let var = format!("__AUDIOHOOK_INHERIT_{}", source.to_uppercase());
        ctx.set_variable(&var, normalized);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audiohook_inherit_write_read() {
        let mut ctx = FuncContext::new();
        let func = FuncAudiohookInherit;
        func.write(&mut ctx, "MixMonitor", "yes").unwrap();
        let result = func.read(&ctx, "MixMonitor").unwrap();
        assert_eq!(result, "yes");
    }

    #[test]
    fn test_audiohook_inherit_default() {
        let ctx = FuncContext::new();
        let func = FuncAudiohookInherit;
        let result = func.read(&ctx, "MixMonitor").unwrap();
        assert_eq!(result, "no");
    }

    #[test]
    fn test_audiohook_inherit_no() {
        let mut ctx = FuncContext::new();
        let func = FuncAudiohookInherit;
        func.write(&mut ctx, "ChanSpy", "no").unwrap();
        let result = func.read(&ctx, "ChanSpy").unwrap();
        assert_eq!(result, "no");
    }

    #[test]
    fn test_audiohook_inherit_empty_source() {
        let ctx = FuncContext::new();
        let func = FuncAudiohookInherit;
        assert!(func.read(&ctx, "").is_err());
    }
}
