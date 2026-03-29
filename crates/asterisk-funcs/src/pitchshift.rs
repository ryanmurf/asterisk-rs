//! PITCH_SHIFT() function - audio pitch shifting.
//!
//! Port of func_pitchshift.c from Asterisk C.
//!
//! Provides:
//! - PITCH_SHIFT(direction,amount) - shift audio pitch
//!
//! Direction: TX (transmit) or RX (receive)
//! Amount: pitch factor where 1.0 = no change, 0.5 = octave down, 2.0 = octave up
//!
//! The original C implementation uses STFT-based pitch shifting (PSOLA).
//! This Rust port stores the configuration; the actual DSP would be
//! performed by the audio pipeline.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// PITCH_SHIFT() function.
///
/// Shifts the pitch of audio on a channel in the TX or RX direction.
///
/// Write usage: Set(PITCH_SHIFT(TX)=1.5)  - shift TX pitch up by 50%
///              Set(PITCH_SHIFT(RX)=0.8)  - shift RX pitch down by 20%
///
/// The pitch factor must be between 0.1 and 4.0.
/// A value of 1.0 means no pitch change.
/// Values < 1.0 lower pitch, values > 1.0 raise pitch.
///
/// Read returns the current pitch shift factor for the given direction.
pub struct FuncPitchShift;

impl FuncPitchShift {
    fn parse_direction(args: &str) -> Result<&'static str, FuncError> {
        match args.trim().to_uppercase().as_str() {
            "TX" => Ok("TX"),
            "RX" => Ok("RX"),
            "" => Err(FuncError::InvalidArgument(
                "PITCH_SHIFT: direction (TX or RX) is required".to_string(),
            )),
            other => Err(FuncError::InvalidArgument(format!(
                "PITCH_SHIFT: direction must be TX or RX, got '{}'",
                other
            ))),
        }
    }

    fn var_name(direction: &str) -> String {
        format!("__PITCH_SHIFT_{}", direction)
    }
}

impl DialplanFunc for FuncPitchShift {
    fn name(&self) -> &str {
        "PITCH_SHIFT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let dir = Self::parse_direction(args)?;
        let var = Self::var_name(dir);
        Ok(ctx
            .get_variable(&var)
            .cloned()
            .unwrap_or_else(|| "1.0".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let dir = Self::parse_direction(args)?;
        let factor: f64 = value.trim().parse().map_err(|_| {
            FuncError::InvalidArgument(format!("PITCH_SHIFT: invalid pitch factor '{}'", value))
        })?;

        if !(0.1..=4.0).contains(&factor) {
            return Err(FuncError::InvalidArgument(format!(
                "PITCH_SHIFT: factor must be between 0.1 and 4.0, got {}",
                factor
            )));
        }

        let var = Self::var_name(dir);
        ctx.set_variable(&var, value.trim());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pitchshift_default() {
        let ctx = FuncContext::new();
        let func = FuncPitchShift;
        assert_eq!(func.read(&ctx, "TX").unwrap(), "1.0");
        assert_eq!(func.read(&ctx, "RX").unwrap(), "1.0");
    }

    #[test]
    fn test_pitchshift_set_tx() {
        let mut ctx = FuncContext::new();
        let func = FuncPitchShift;
        func.write(&mut ctx, "TX", "1.5").unwrap();
        assert_eq!(func.read(&ctx, "TX").unwrap(), "1.5");
    }

    #[test]
    fn test_pitchshift_set_rx() {
        let mut ctx = FuncContext::new();
        let func = FuncPitchShift;
        func.write(&mut ctx, "RX", "0.5").unwrap();
        assert_eq!(func.read(&ctx, "RX").unwrap(), "0.5");
    }

    #[test]
    fn test_pitchshift_invalid_direction() {
        let ctx = FuncContext::new();
        let func = FuncPitchShift;
        assert!(func.read(&ctx, "BLAH").is_err());
    }

    #[test]
    fn test_pitchshift_out_of_range() {
        let mut ctx = FuncContext::new();
        let func = FuncPitchShift;
        assert!(func.write(&mut ctx, "TX", "0.01").is_err());
        assert!(func.write(&mut ctx, "TX", "5.0").is_err());
    }

    #[test]
    fn test_pitchshift_octave_down() {
        let mut ctx = FuncContext::new();
        let func = FuncPitchShift;
        func.write(&mut ctx, "TX", "0.5").unwrap();
        assert_eq!(func.read(&ctx, "TX").unwrap(), "0.5");
    }

    #[test]
    fn test_pitchshift_octave_up() {
        let mut ctx = FuncContext::new();
        let func = FuncPitchShift;
        func.write(&mut ctx, "RX", "2.0").unwrap();
        assert_eq!(func.read(&ctx, "RX").unwrap(), "2.0");
    }
}
