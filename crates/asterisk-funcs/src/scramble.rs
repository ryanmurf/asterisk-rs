//! SCRAMBLE() function - audio scrambling for privacy.
//!
//! Port of func_scramble.c from Asterisk C.
//!
//! Provides:
//! - SCRAMBLE([direction]) - scramble audio on a channel
//!
//! Uses whole-spectrum frequency inversion to render audio unintelligible.
//! This is NOT encryption; it is a simple privacy enhancement.
//!
//! Usage:
//!   Set(SCRAMBLE()=both)    - scramble TX and RX
//!   Set(SCRAMBLE()=TX)      - scramble only transmit
//!   Set(SCRAMBLE()=RX)      - scramble only receive
//!   Set(SCRAMBLE()=remove)  - remove scrambler

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Scramble direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrambleDirection {
    /// Scramble both TX and RX.
    Both,
    /// Scramble TX only.
    Tx,
    /// Scramble RX only.
    Rx,
    /// Remove the scrambler.
    Remove,
}

/// SCRAMBLE() function.
pub struct FuncScramble;

impl DialplanFunc for FuncScramble {
    fn name(&self) -> &str {
        "SCRAMBLE"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        Ok(ctx.get_variable("__SCRAMBLE").cloned().unwrap_or_else(|| "off".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, _args: &str, value: &str) -> Result<(), FuncError> {
        let direction = match value.trim().to_lowercase().as_str() {
            "both" | "" => "both",
            "tx" => "tx",
            "rx" => "rx",
            "remove" | "off" => "off",
            other => return Err(FuncError::InvalidArgument(format!(
                "SCRAMBLE: direction must be TX, RX, both, or remove, got '{}'", other
            ))),
        };

        ctx.set_variable("__SCRAMBLE", direction);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scramble_both() {
        let mut ctx = FuncContext::new();
        let func = FuncScramble;
        func.write(&mut ctx, "", "both").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "both");
    }

    #[test]
    fn test_scramble_tx() {
        let mut ctx = FuncContext::new();
        let func = FuncScramble;
        func.write(&mut ctx, "", "TX").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "tx");
    }

    #[test]
    fn test_scramble_remove() {
        let mut ctx = FuncContext::new();
        let func = FuncScramble;
        func.write(&mut ctx, "", "both").unwrap();
        func.write(&mut ctx, "", "remove").unwrap();
        assert_eq!(func.read(&ctx, "").unwrap(), "off");
    }

    #[test]
    fn test_scramble_default() {
        let ctx = FuncContext::new();
        let func = FuncScramble;
        assert_eq!(func.read(&ctx, "").unwrap(), "off");
    }

    #[test]
    fn test_scramble_invalid() {
        let mut ctx = FuncContext::new();
        let func = FuncScramble;
        assert!(func.write(&mut ctx, "", "invalid").is_err());
    }
}
