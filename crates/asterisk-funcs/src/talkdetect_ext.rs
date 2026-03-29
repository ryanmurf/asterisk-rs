//! TALK_DETECT() extended settings.
//!
//! Additional talk detection configuration beyond the base talkdetect module.
//!
//! - TALK_DETECT_EXT(setting) - Extended talk detection parameters

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// TALK_DETECT_EXT() function.
///
/// Extended talk detection settings:
///   ${TALK_DETECT_EXT(min_talk_ms)}    - Minimum talk duration to trigger (ms)
///   ${TALK_DETECT_EXT(max_silence_ms)} - Max silence before stop event (ms)
///   ${TALK_DETECT_EXT(energy_threshold)} - Energy threshold for detection
///   ${TALK_DETECT_EXT(mode)}           - Detection mode ("dsp" or "energy")
pub struct FuncTalkDetectExt;

impl DialplanFunc for FuncTalkDetectExt {
    fn name(&self) -> &str {
        "TALK_DETECT_EXT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let setting = args.trim().to_lowercase();
        let key = format!("__TALK_DETECT_EXT_{}", setting.to_uppercase());
        let default = match setting.as_str() {
            "min_talk_ms" => "300",
            "max_silence_ms" => "2500",
            "energy_threshold" => "256",
            "mode" => "dsp",
            _ => "",
        };
        Ok(ctx
            .get_variable(&key)
            .cloned()
            .unwrap_or_else(|| default.to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let setting = args.trim().to_lowercase();
        match setting.as_str() {
            "min_talk_ms" | "max_silence_ms" | "energy_threshold" => {
                let _: u32 = value.trim().parse().map_err(|_| {
                    FuncError::InvalidArgument(format!(
                        "TALK_DETECT_EXT({}): expected numeric value, got '{}'",
                        setting, value
                    ))
                })?;
                let key = format!("__TALK_DETECT_EXT_{}", setting.to_uppercase());
                ctx.set_variable(&key, value.trim());
                Ok(())
            }
            "mode" => {
                let mode = value.trim().to_lowercase();
                if mode != "dsp" && mode != "energy" {
                    return Err(FuncError::InvalidArgument(format!(
                        "TALK_DETECT_EXT(mode): expected 'dsp' or 'energy', got '{}'",
                        mode
                    )));
                }
                ctx.set_variable("__TALK_DETECT_EXT_MODE", &mode);
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "TALK_DETECT_EXT: unknown setting '{}'",
                setting
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let ctx = FuncContext::new();
        let func = FuncTalkDetectExt;
        assert_eq!(func.read(&ctx, "min_talk_ms").unwrap(), "300");
        assert_eq!(func.read(&ctx, "mode").unwrap(), "dsp");
    }

    #[test]
    fn test_set_min_talk() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectExt;
        func.write(&mut ctx, "min_talk_ms", "500").unwrap();
        assert_eq!(func.read(&ctx, "min_talk_ms").unwrap(), "500");
    }

    #[test]
    fn test_set_mode() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectExt;
        func.write(&mut ctx, "mode", "energy").unwrap();
        assert_eq!(func.read(&ctx, "mode").unwrap(), "energy");
    }

    #[test]
    fn test_invalid_mode() {
        let mut ctx = FuncContext::new();
        assert!(FuncTalkDetectExt.write(&mut ctx, "mode", "bogus").is_err());
    }
}
