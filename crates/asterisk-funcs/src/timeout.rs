//! TIMEOUT() function - read/write channel timeouts.
//!
//! Port of func_timeout.c from Asterisk C.
//!
//! Usage:
//!   TIMEOUT(absolute) - absolute timeout for the call (seconds)
//!   TIMEOUT(digit)    - inter-digit timeout for DTMF (seconds)
//!   TIMEOUT(response) - response timeout for prompts (seconds)

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Timeout type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutType {
    /// Maximum duration of the entire call.
    Absolute,
    /// Time to wait between DTMF digits.
    Digit,
    /// Time to wait for a response to a prompt.
    Response,
}

impl TimeoutType {
    /// Parse a timeout type from a string argument.
    pub fn from_arg(arg: &str) -> Result<Self, FuncError> {
        match arg.trim().to_lowercase().as_str() {
            "absolute" | "abs" | "a" => Ok(TimeoutType::Absolute),
            "digit" | "dig" | "d" => Ok(TimeoutType::Digit),
            "response" | "resp" | "r" => Ok(TimeoutType::Response),
            other => Err(FuncError::InvalidArgument(format!(
                "TIMEOUT: unknown type '{}', expected absolute|digit|response",
                other
            ))),
        }
    }

    /// Variable name used to store this timeout in channel variables.
    pub fn var_name(&self) -> &'static str {
        match self {
            TimeoutType::Absolute => "__TIMEOUT_ABSOLUTE",
            TimeoutType::Digit => "__TIMEOUT_DIGIT",
            TimeoutType::Response => "__TIMEOUT_RESPONSE",
        }
    }

    /// Default timeout value in seconds.
    pub fn default_seconds(&self) -> f64 {
        match self {
            TimeoutType::Absolute => 0.0, // 0 = no absolute timeout
            TimeoutType::Digit => 5.0,
            TimeoutType::Response => 10.0,
        }
    }
}

/// TIMEOUT() function.
///
/// Reads or writes absolute, digit, and response timeouts on a channel.
///
/// Read usage:  `TIMEOUT(type)` returns the current timeout in seconds.
/// Write usage: `Set(TIMEOUT(type)=seconds)` sets the timeout.
pub struct FuncTimeout;

impl DialplanFunc for FuncTimeout {
    fn name(&self) -> &str {
        "TIMEOUT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let timeout_type = TimeoutType::from_arg(args)?;

        // Read from channel variables; return default if not set
        let value = ctx
            .get_variable(timeout_type.var_name())
            .map(|v| v.clone())
            .unwrap_or_else(|| format!("{:.6}", timeout_type.default_seconds()));

        Ok(value)
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let timeout_type = TimeoutType::from_arg(args)?;

        // Parse the new timeout value (in seconds, may be fractional)
        let seconds: f64 = value.trim().parse().map_err(|_| {
            FuncError::InvalidArgument(format!(
                "TIMEOUT: invalid timeout value '{}', expected a number in seconds",
                value
            ))
        })?;

        if seconds < 0.0 {
            return Err(FuncError::InvalidArgument(
                "TIMEOUT: timeout value cannot be negative".to_string(),
            ));
        }

        ctx.set_variable(timeout_type.var_name(), &format!("{:.6}", seconds));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_default_timeouts() {
        let ctx = FuncContext::new();
        let func = FuncTimeout;

        // Absolute default is 0
        let val = func.read(&ctx, "absolute").unwrap();
        let v: f64 = val.parse().unwrap();
        assert!((v - 0.0).abs() < 0.001);

        // Digit default is 5
        let val = func.read(&ctx, "digit").unwrap();
        let v: f64 = val.parse().unwrap();
        assert!((v - 5.0).abs() < 0.001);

        // Response default is 10
        let val = func.read(&ctx, "response").unwrap();
        let v: f64 = val.parse().unwrap();
        assert!((v - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_write_and_read_timeout() {
        let mut ctx = FuncContext::new();
        let func = FuncTimeout;

        func.write(&mut ctx, "absolute", "30").unwrap();
        let val = func.read(&ctx, "abs").unwrap();
        let v: f64 = val.parse().unwrap();
        assert!((v - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_invalid_timeout_type() {
        let ctx = FuncContext::new();
        let func = FuncTimeout;
        assert!(func.read(&ctx, "invalid").is_err());
    }

    #[test]
    fn test_negative_timeout() {
        let mut ctx = FuncContext::new();
        let func = FuncTimeout;
        assert!(func.write(&mut ctx, "digit", "-5").is_err());
    }
}
