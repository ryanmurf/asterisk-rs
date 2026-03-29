//! TALK_DETECT() function - talk/silence detection on channels.
//!
//! Port of func_talkdetect.c from Asterisk C.
//!
//! Provides:
//! - TALK_DETECT(action) - enable/disable talk detection
//!
//! When enabled, raises AMI events on talk start/stop transitions.
//! Configurable silence and talking thresholds.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Talk detection configuration.
#[derive(Debug, Clone)]
pub struct TalkDetectConfig {
    /// Silence threshold in ms before talk-stop event (default 2500)
    pub dsp_silence_threshold: u32,
    /// Minimum average magnitude for DSP to consider as talking (default 256)
    pub dsp_talking_threshold: u32,
}

impl Default for TalkDetectConfig {
    fn default() -> Self {
        Self {
            dsp_silence_threshold: 2500,
            dsp_talking_threshold: 256,
        }
    }
}

/// TALK_DETECT() function.
///
/// Enables or disables talk detection on a channel. When enabled,
/// the system monitors audio levels and raises events when talking
/// starts or stops.
///
/// Write usage:
///   Set(TALK_DETECT(set)=)             - enable with defaults
///   Set(TALK_DETECT(set)=2500,256)     - enable with custom thresholds
///   Set(TALK_DETECT(remove)=)          - disable
///
/// The write value for "set" is: [silence_threshold[,talking_threshold]]
///
/// Read returns current state and config as "enabled|silence_ms|talking_mag"
/// or "disabled".
pub struct FuncTalkDetect;

impl DialplanFunc for FuncTalkDetect {
    fn name(&self) -> &str {
        "TALK_DETECT"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        Ok(ctx
            .get_variable("__TALK_DETECT")
            .cloned()
            .unwrap_or_else(|| "disabled".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let action = args.trim().to_lowercase();
        match action.as_str() {
            "set" => {
                let value = value.trim();
                let defaults = TalkDetectConfig::default();
                let (silence_thresh, talking_thresh) = if value.is_empty() {
                    (defaults.dsp_silence_threshold, defaults.dsp_talking_threshold)
                } else {
                    let parts: Vec<&str> = value.splitn(2, ',').collect();
                    let silence: u32 = parts
                        .first()
                        .unwrap_or(&"2500")
                        .trim()
                        .parse()
                        .unwrap_or(defaults.dsp_silence_threshold);
                    let talking: u32 = parts
                        .get(1)
                        .unwrap_or(&"256")
                        .trim()
                        .parse()
                        .unwrap_or(defaults.dsp_talking_threshold);
                    (silence, talking)
                };

                let state = format!("enabled|{}|{}", silence_thresh, talking_thresh);
                ctx.set_variable("__TALK_DETECT", &state);
                Ok(())
            }
            "remove" => {
                ctx.set_variable("__TALK_DETECT", "disabled");
                Ok(())
            }
            other => Err(FuncError::InvalidArgument(format!(
                "TALK_DETECT: action must be 'set' or 'remove', got '{}'",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_talk_detect_default() {
        let ctx = FuncContext::new();
        let func = FuncTalkDetect;
        assert_eq!(func.read(&ctx, "").unwrap(), "disabled");
    }

    #[test]
    fn test_talk_detect_enable_defaults() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetect;
        func.write(&mut ctx, "set", "").unwrap();
        let state = func.read(&ctx, "").unwrap();
        assert!(state.starts_with("enabled"));
        assert!(state.contains("2500"));
        assert!(state.contains("256"));
    }

    #[test]
    fn test_talk_detect_enable_custom() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetect;
        func.write(&mut ctx, "set", "3000,512").unwrap();
        let state = func.read(&ctx, "").unwrap();
        assert!(state.contains("3000"));
        assert!(state.contains("512"));
    }

    #[test]
    fn test_talk_detect_remove() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetect;
        func.write(&mut ctx, "set", "").unwrap();
        func.write(&mut ctx, "remove", "").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "disabled");
    }

    #[test]
    fn test_talk_detect_invalid_action() {
        let mut ctx = FuncContext::new();
        let func = FuncTalkDetect;
        assert!(func.write(&mut ctx, "bogus", "").is_err());
    }
}
