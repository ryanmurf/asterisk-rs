//! VOLUME() function - technology-independent volume control.
//!
//! Port of func_volume.c from Asterisk C.
//!
//! Provides:
//! - VOLUME(direction) - set/get TX or RX volume adjustment in dB
//!
//! Usage:
//!   Set(VOLUME(TX)=3)   - increase transmit volume by 3dB
//!   Set(VOLUME(RX)=-4)  - decrease receive volume by 4dB
//!   Set(VOLUME(RX)=0)   - reset to normal

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Audio direction for volume adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeDirection {
    /// Transmit direction (audio going to the channel)
    Tx,
    /// Receive direction (audio coming from the channel)
    Rx,
}

/// VOLUME() function.
///
/// Adjusts the TX or RX audio volume of a channel.
///
/// Read:  VOLUME(direction) -> current gain in dB as float string
/// Write: Set(VOLUME(direction)=value) where value is dB gain (float)
pub struct FuncVolume;

impl FuncVolume {
    /// Parse direction argument, ignoring trailing options after comma.
    fn parse_direction(args: &str) -> Result<VolumeDirection, FuncError> {
        let args = args.trim();
        // direction may be followed by ",options" (e.g. "TX,p")
        let direction_str = args.split(',').next().unwrap_or("").trim();
        match direction_str.to_uppercase().as_str() {
            "TX" => Ok(VolumeDirection::Tx),
            "RX" => Ok(VolumeDirection::Rx),
            "" => Err(FuncError::InvalidArgument(
                "VOLUME: direction (TX or RX) is required".to_string(),
            )),
            other => Err(FuncError::InvalidArgument(format!(
                "VOLUME: direction must be TX or RX, got '{}'",
                other
            ))),
        }
    }

    fn var_name(dir: VolumeDirection) -> &'static str {
        match dir {
            VolumeDirection::Tx => "__VOLUME_TX",
            VolumeDirection::Rx => "__VOLUME_RX",
        }
    }
}

impl DialplanFunc for FuncVolume {
    fn name(&self) -> &str {
        "VOLUME"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let dir = Self::parse_direction(args)?;
        let var = Self::var_name(dir);
        Ok(ctx.get_variable(var).cloned().unwrap_or_else(|| "0".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let dir = Self::parse_direction(args)?;
        let _gain: f64 = value.trim().parse().map_err(|_| {
            FuncError::InvalidArgument(format!("VOLUME: invalid gain value '{}'", value))
        })?;
        let var = Self::var_name(dir);
        ctx.set_variable(var, value.trim());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_write_and_read() {
        let mut ctx = FuncContext::new();
        let func = FuncVolume;
        func.write(&mut ctx, "TX", "3.5").unwrap();
        assert_eq!(func.read(&ctx, "TX").unwrap(), "3.5");
    }

    #[test]
    fn test_volume_rx() {
        let mut ctx = FuncContext::new();
        let func = FuncVolume;
        func.write(&mut ctx, "RX", "-2").unwrap();
        assert_eq!(func.read(&ctx, "RX").unwrap(), "-2");
    }

    #[test]
    fn test_volume_default_zero() {
        let ctx = FuncContext::new();
        let func = FuncVolume;
        assert_eq!(func.read(&ctx, "TX").unwrap(), "0");
    }

    #[test]
    fn test_volume_invalid_direction() {
        let ctx = FuncContext::new();
        let func = FuncVolume;
        assert!(func.read(&ctx, "BLAH").is_err());
    }

    #[test]
    fn test_volume_case_insensitive() {
        let mut ctx = FuncContext::new();
        let func = FuncVolume;
        func.write(&mut ctx, "tx", "1").unwrap();
        assert_eq!(func.read(&ctx, "Tx").unwrap(), "1");
    }

    #[test]
    fn test_volume_with_options() {
        let mut ctx = FuncContext::new();
        let func = FuncVolume;
        func.write(&mut ctx, "TX,p", "5").unwrap();
        assert_eq!(func.read(&ctx, "TX").unwrap(), "5");
    }
}
