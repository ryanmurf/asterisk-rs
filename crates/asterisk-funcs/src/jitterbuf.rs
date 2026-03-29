//! JITTERBUFFER() function - jitter buffer control on channels.
//!
//! Port of func_jitterbuffer.c from Asterisk C.
//!
//! Provides:
//! - JITTERBUFFER(type) - set jitter buffer on the read side of a channel
//!
//! Types: fixed, adaptive, disabled
//! Parameters (write value): max_size,resync_threshold,target_extra
//!   or "default" for default values

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Jitter buffer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitterBufferType {
    /// Fixed-size jitter buffer
    Fixed,
    /// Adaptive jitter buffer
    Adaptive,
    /// Disable jitter buffer
    Disabled,
}

impl JitterBufferType {
    fn from_str(s: &str) -> Result<Self, FuncError> {
        match s.to_lowercase().as_str() {
            "fixed" => Ok(Self::Fixed),
            "adaptive" => Ok(Self::Adaptive),
            "disabled" => Ok(Self::Disabled),
            other => Err(FuncError::InvalidArgument(format!(
                "JITTERBUFFER: unknown type '{}' (use fixed, adaptive, or disabled)",
                other
            ))),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Adaptive => "adaptive",
            Self::Disabled => "disabled",
        }
    }
}

/// Jitter buffer configuration.
#[derive(Debug, Clone)]
pub struct JitterBufferConfig {
    /// Buffer type
    pub jb_type: JitterBufferType,
    /// Maximum buffer size in ms (default 200)
    pub max_size: u32,
    /// Resync threshold in ms (default 1000)
    pub resync_threshold: u32,
    /// Extra target delay for adaptive jitter buffer (default 40)
    pub target_extra: u32,
}

impl Default for JitterBufferConfig {
    fn default() -> Self {
        Self {
            jb_type: JitterBufferType::Fixed,
            max_size: 200,
            resync_threshold: 1000,
            target_extra: 40,
        }
    }
}

/// JITTERBUFFER() function.
///
/// Sets a jitter buffer on the read side of a channel to dejitter
/// the audio stream before it reaches the Asterisk core.
///
/// Write usage: Set(JITTERBUFFER(fixed)=default)
///              Set(JITTERBUFFER(adaptive)=200,1000,40)
///              Set(JITTERBUFFER(disabled)=)
///
/// Read usage:  ${JITTERBUFFER(type)} -> current jitter buffer type
pub struct FuncJitterBuffer;

impl DialplanFunc for FuncJitterBuffer {
    fn name(&self) -> &str {
        "JITTERBUFFER"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let _jb_type = JitterBufferType::from_str(args.trim())?;
        // Return current jitter buffer config from context
        Ok(ctx
            .get_variable("__JITTERBUFFER_TYPE")
            .cloned()
            .unwrap_or_else(|| "disabled".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let jb_type = JitterBufferType::from_str(args.trim())?;

        if jb_type == JitterBufferType::Disabled {
            ctx.set_variable("__JITTERBUFFER_TYPE", "disabled");
            ctx.set_variable("__JITTERBUFFER_MAX_SIZE", "0");
            ctx.set_variable("__JITTERBUFFER_RESYNC", "0");
            ctx.set_variable("__JITTERBUFFER_TARGET_EXTRA", "0");
            return Ok(());
        }

        let value = value.trim();
        let (max_size, resync_threshold, target_extra) = if value.eq_ignore_ascii_case("default")
            || value.is_empty()
        {
            let defaults = JitterBufferConfig::default();
            (defaults.max_size, defaults.resync_threshold, defaults.target_extra)
        } else {
            let parts: Vec<&str> = value.splitn(4, ',').collect();
            let max_size: u32 = parts
                .first()
                .unwrap_or(&"200")
                .trim()
                .parse()
                .unwrap_or(200);
            let resync: u32 = parts
                .get(1)
                .unwrap_or(&"1000")
                .trim()
                .parse()
                .unwrap_or(1000);
            let target: u32 = parts
                .get(2)
                .unwrap_or(&"40")
                .trim()
                .parse()
                .unwrap_or(40);
            (max_size, resync, target)
        };

        ctx.set_variable("__JITTERBUFFER_TYPE", jb_type.as_str());
        ctx.set_variable("__JITTERBUFFER_MAX_SIZE", &max_size.to_string());
        ctx.set_variable("__JITTERBUFFER_RESYNC", &resync_threshold.to_string());
        ctx.set_variable("__JITTERBUFFER_TARGET_EXTRA", &target_extra.to_string());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jitterbuffer_set_fixed_default() {
        let mut ctx = FuncContext::new();
        let func = FuncJitterBuffer;
        func.write(&mut ctx, "fixed", "default").unwrap();
        assert_eq!(ctx.get_variable("__JITTERBUFFER_TYPE").unwrap(), "fixed");
        assert_eq!(ctx.get_variable("__JITTERBUFFER_MAX_SIZE").unwrap(), "200");
    }

    #[test]
    fn test_jitterbuffer_set_adaptive_custom() {
        let mut ctx = FuncContext::new();
        let func = FuncJitterBuffer;
        func.write(&mut ctx, "adaptive", "300,2000,60").unwrap();
        assert_eq!(ctx.get_variable("__JITTERBUFFER_TYPE").unwrap(), "adaptive");
        assert_eq!(ctx.get_variable("__JITTERBUFFER_MAX_SIZE").unwrap(), "300");
        assert_eq!(ctx.get_variable("__JITTERBUFFER_RESYNC").unwrap(), "2000");
        assert_eq!(
            ctx.get_variable("__JITTERBUFFER_TARGET_EXTRA").unwrap(),
            "60"
        );
    }

    #[test]
    fn test_jitterbuffer_disable() {
        let mut ctx = FuncContext::new();
        let func = FuncJitterBuffer;
        func.write(&mut ctx, "fixed", "default").unwrap();
        func.write(&mut ctx, "disabled", "").unwrap();
        assert_eq!(
            ctx.get_variable("__JITTERBUFFER_TYPE").unwrap(),
            "disabled"
        );
    }

    #[test]
    fn test_jitterbuffer_read_default() {
        let ctx = FuncContext::new();
        let func = FuncJitterBuffer;
        assert_eq!(func.read(&ctx, "fixed").unwrap(), "disabled");
    }

    #[test]
    fn test_jitterbuffer_invalid_type() {
        let ctx = FuncContext::new();
        let func = FuncJitterBuffer;
        assert!(func.read(&ctx, "bogus").is_err());
    }
}
