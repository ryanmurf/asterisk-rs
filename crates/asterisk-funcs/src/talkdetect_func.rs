//! Additional talk detect configuration functions.
//!
//! Extensions to func_talkdetect.c - provides fine-grained talk detection
//! configuration beyond the basic TALK_DETECT() function.
//!
//! - TALK_DETECT_CONFIG(setting) - Read/write individual talk detection settings

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// TALK_DETECT_CONFIG() function.
///
/// Provides granular configuration for talk detection:
///   ${TALK_DETECT_CONFIG(dsp_silence_threshold)} - Silence threshold in ms
///   ${TALK_DETECT_CONFIG(dsp_talking_threshold)} - Talking magnitude threshold
///   ${TALK_DETECT_CONFIG(event_type)}            - Event type ("ami" or "stasis")
///   ${TALK_DETECT_CONFIG(report_interval)}       - Reporting interval in ms
pub struct FuncTalkDetectConfig;

impl DialplanFunc for FuncTalkDetectConfig {
    fn name(&self) -> &str {
        "TALK_DETECT_CONFIG"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let setting = args.trim().to_lowercase();
        let key = format!("__TALK_DETECT_CFG_{}", setting.to_uppercase());
        let default = match setting.as_str() {
            "dsp_silence_threshold" => "2500",
            "dsp_talking_threshold" => "256",
            "event_type" => "ami",
            "report_interval" => "0",
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
            "dsp_silence_threshold" | "dsp_talking_threshold" | "report_interval" => {
                let _: u32 = value.trim().parse().map_err(|_| {
                    FuncError::InvalidArgument(format!(
                        "TALK_DETECT_CONFIG({}): expected numeric value, got '{}'",
                        setting, value
                    ))
                })?;
                let key = format!("__TALK_DETECT_CFG_{}", setting.to_uppercase());
                ctx.set_variable(&key, value.trim());
                Ok(())
            }
            "event_type" => {
                let val = value.trim().to_lowercase();
                if val != "ami" && val != "stasis" {
                    return Err(FuncError::InvalidArgument(format!(
                        "TALK_DETECT_CONFIG(event_type): expected 'ami' or 'stasis', got '{}'",
                        val
                    )));
                }
                ctx.set_variable("__TALK_DETECT_CFG_EVENT_TYPE", &val);
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "TALK_DETECT_CONFIG: unknown setting '{}'",
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
        let func = FuncTalkDetectConfig;
        assert_eq!(func.read(&ctx, "dsp_silence_threshold").unwrap(), "2500");
        assert_eq!(func.read(&ctx, "dsp_talking_threshold").unwrap(), "256");
        assert_eq!(func.read(&ctx, "event_type").unwrap(), "ami");
    }

    #[test]
    fn test_set_threshold() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectConfig;
        func.write(&mut ctx, "dsp_silence_threshold", "3000").unwrap();
        assert_eq!(func.read(&ctx, "dsp_silence_threshold").unwrap(), "3000");
    }

    #[test]
    fn test_set_event_type() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectConfig;
        func.write(&mut ctx, "event_type", "stasis").unwrap();
        assert_eq!(func.read(&ctx, "event_type").unwrap(), "stasis");
    }

    #[test]
    fn test_invalid_event_type() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectConfig;
        assert!(func.write(&mut ctx, "event_type", "bogus").is_err());
    }

    #[test]
    fn test_invalid_setting() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetectConfig;
        assert!(func.write(&mut ctx, "nonexistent", "1").is_err());
    }
}
