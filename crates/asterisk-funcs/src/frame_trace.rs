//! FRAME_TRACE() function - trace internal frames on a channel.
//!
//! Port of func_frame_trace.c from Asterisk C.
//!
//! Provides:
//! - FRAME_TRACE(filter_type) - enable/disable frame tracing on a channel
//!
//! Filter type can be "white" or "black" followed by frame type names.
//! Frame types: DTMF_BEGIN, DTMF_END, VOICE, VIDEO, CONTROL, NULL,
//! IAX, TEXT, TEXT_DATA, IMAGE, HTML, CNG, MODEM.
//!
//! Usage:
//!   Set(FRAME_TRACE(white)=DTMF_BEGIN,DTMF_END)  - trace only DTMF frames
//!   Set(FRAME_TRACE(black)=VOICE)                 - trace everything except voice

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Known frame types that can be filtered.
pub const FRAME_TYPES: &[&str] = &[
    "DTMF_BEGIN", "DTMF_END", "VOICE", "VIDEO", "CONTROL", "NULL",
    "IAX", "TEXT", "TEXT_DATA", "IMAGE", "HTML", "CNG", "MODEM",
];

/// Filter list type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    /// Whitelist: only trace listed frame types.
    White,
    /// Blacklist: trace everything except listed frame types.
    Black,
}

/// FRAME_TRACE() function.
pub struct FuncFrameTrace;

impl DialplanFunc for FuncFrameTrace {
    fn name(&self) -> &str {
        "FRAME_TRACE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let _ = args;
        // Read current trace configuration
        Ok(ctx.get_variable("__FRAME_TRACE").cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let filter_type = match args.trim().to_lowercase().as_str() {
            "white" | "" => "white",
            "black" => "black",
            other => return Err(FuncError::InvalidArgument(format!(
                "FRAME_TRACE: filter type must be 'white' or 'black', got '{}'", other
            ))),
        };

        // Store trace configuration as "type:frame1,frame2,..."
        let config = format!("{}:{}", filter_type, value.trim());
        ctx.set_variable("__FRAME_TRACE", &config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_trace_write_read() {
        let mut ctx = FuncContext::new();
        let func = FuncFrameTrace;
        func.write(&mut ctx, "white", "DTMF_BEGIN,DTMF_END").unwrap();
        let val = func.read(&ctx, "").unwrap();
        assert!(val.contains("DTMF_BEGIN"));
    }

    #[test]
    fn test_frame_trace_black_filter() {
        let mut ctx = FuncContext::new();
        let func = FuncFrameTrace;
        func.write(&mut ctx, "black", "VOICE").unwrap();
        let val = func.read(&ctx, "").unwrap();
        assert!(val.starts_with("black:"));
    }

    #[test]
    fn test_frame_trace_invalid_filter() {
        let mut ctx = FuncContext::new();
        let func = FuncFrameTrace;
        assert!(func.write(&mut ctx, "invalid", "VOICE").is_err());
    }
}
