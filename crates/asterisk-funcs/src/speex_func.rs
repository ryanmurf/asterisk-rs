//! Speex preprocessor functions - DENOISE() and AGC().
//!
//! Port of func_speex.c from Asterisk C.
//!
//! Provides:
//! - DENOISE() - Enable/disable Speex noise reduction on a channel
//! - AGC() - Enable/disable Speex Automatic Gain Control
//!
//! These are stubs since actual Speex DSP is not linked. The functions
//! set channel variables that would be read by the audio pipeline.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// DENOISE() function.
///
/// Write usage: Set(DENOISE()=on) or Set(DENOISE()=off)
/// Read returns current state: "on" or "off"
pub struct FuncDenoise;

impl DialplanFunc for FuncDenoise {
    fn name(&self) -> &str {
        "DENOISE"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        Ok(ctx
            .get_variable("__DENOISE")
            .cloned()
            .unwrap_or_else(|| "off".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, _args: &str, value: &str) -> Result<(), FuncError> {
        let state = match value.trim().to_lowercase().as_str() {
            "on" | "1" | "true" | "yes" => "on",
            "off" | "0" | "false" | "no" => "off",
            other => {
                return Err(FuncError::InvalidArgument(format!(
                    "DENOISE: expected on/off, got '{}'",
                    other
                )));
            }
        };
        ctx.set_variable("__DENOISE", state);
        Ok(())
    }
}

/// AGC() function - Automatic Gain Control.
///
/// Write usage: Set(AGC()=on) or Set(AGC(rx)=8000) for target level
/// Read returns current state/level.
pub struct FuncAgc;

impl DialplanFunc for FuncAgc {
    fn name(&self) -> &str {
        "AGC"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let direction = if args.trim().is_empty() { "tx" } else { args.trim() };
        let key = format!("__AGC_{}", direction.to_uppercase());
        Ok(ctx
            .get_variable(&key)
            .cloned()
            .unwrap_or_else(|| "off".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let direction = if args.trim().is_empty() { "tx" } else { args.trim() };
        let key = format!("__AGC_{}", direction.to_uppercase());

        match value.trim().to_lowercase().as_str() {
            "off" | "0" | "false" | "no" => {
                ctx.set_variable(&key, "off");
            }
            other => {
                // Accept a numeric target level or "on"
                if other == "on" || other == "1" || other == "true" || other == "yes" {
                    ctx.set_variable(&key, "8000"); // default target level
                } else if let Ok(level) = other.parse::<u32>() {
                    ctx.set_variable(&key, &level.to_string());
                } else {
                    return Err(FuncError::InvalidArgument(format!(
                        "AGC: expected on/off/level, got '{}'",
                        other
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_denoise_default() {
        let ctx = FuncContext::new();
        assert_eq!(FuncDenoise.read(&ctx, "").unwrap(), "off");
    }

    #[test]
    fn test_denoise_on_off() {
        let mut ctx = FuncContext::new();
        FuncDenoise.write(&mut ctx, "", "on").unwrap();
        assert_eq!(FuncDenoise.read(&ctx, "").unwrap(), "on");
        FuncDenoise.write(&mut ctx, "", "off").unwrap();
        assert_eq!(FuncDenoise.read(&ctx, "").unwrap(), "off");
    }

    #[test]
    fn test_agc_default() {
        let ctx = FuncContext::new();
        assert_eq!(FuncAgc.read(&ctx, "").unwrap(), "off");
    }

    #[test]
    fn test_agc_on() {
        let mut ctx = FuncContext::new();
        FuncAgc.write(&mut ctx, "tx", "on").unwrap();
        assert_eq!(FuncAgc.read(&ctx, "tx").unwrap(), "8000");
    }

    #[test]
    fn test_agc_level() {
        let mut ctx = FuncContext::new();
        FuncAgc.write(&mut ctx, "rx", "4000").unwrap();
        assert_eq!(FuncAgc.read(&ctx, "rx").unwrap(), "4000");
    }
}
