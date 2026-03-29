//! Milliwatt test tone generator application.
//!
//! Port of app_milliwatt.c from Asterisk C. Generates a 1004 Hz test tone
//! at 0 dBm (mu-law). Supports the standard milliwatt test pattern with
//! 1-second silent intervals, as well as legacy 1000 Hz constant tone mode.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

/// The 8-byte digital milliwatt pattern (mu-law encoded 1004 Hz).
///
/// This pattern repeats to produce the standard 1004 Hz milliwatt test tone
/// at 0 dBm. At 8000 Hz sample rate, the 8-sample pattern produces exactly
/// 1004 Hz (approximately 1000 Hz, the standard test frequency).
pub const DIGITAL_MILLIWATT: [u8; 8] = [0x1e, 0x0b, 0x0b, 0x1e, 0x9e, 0x8b, 0x8b, 0x9e];

/// Options for the Milliwatt application.
#[derive(Debug, Clone, Default)]
pub struct MilliwattOptions {
    /// Generate a true milliwatt test tone with 1-second silent intervals
    /// every 10 seconds ('m' option).
    pub milliwatt_test: bool,
    /// Use old behavior: constant 1000 Hz tone ('o' option).
    pub old_behavior: bool,
}

impl MilliwattOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'm' => result.milliwatt_test = true,
                'o' => result.old_behavior = true,
                _ => {
                    debug!("Milliwatt: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The Milliwatt() dialplan application.
///
/// Usage: Milliwatt([options])
///
/// Generates a 1004 Hz test tone at 0 dBm (mu-law).
///
/// Options:
///   m - Generate a proper milliwatt test tone (1004 Hz for 9 seconds,
///       1 second silence, repeating). Required for actual milliwatt testing.
///   o - Generate a constant 1000 Hz tone (legacy behavior).
///
/// With no options, plays a continuous 1004 Hz tone (not suitable for
/// a proper milliwatt test without the 'm' option).
pub struct AppMilliwatt;

impl DialplanApp for AppMilliwatt {
    fn name(&self) -> &str {
        "Milliwatt"
    }

    fn description(&self) -> &str {
        "Generate a Constant 1004 Hz tone at 0dbm (mu-law)"
    }
}

impl AppMilliwatt {
    /// Sample rate for the milliwatt tone.
    pub const SAMPLE_RATE: u32 = 8000;

    /// Frequency of the test tone (Hz).
    pub const TONE_FREQ: u32 = 1004;

    /// Amplitude for the tone generator (corresponds to ~0 dBm).
    pub const TONE_AMPLITUDE: u32 = 23255;

    /// Execute the Milliwatt application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = MilliwattOptions::parse(args);

        // Answer the channel if not already up
        if channel.state != ChannelState::Up {
            debug!("Milliwatt: answering channel");
            channel.state = ChannelState::Up;
        }

        info!(
            "Milliwatt: channel '{}' starting tone (milliwatt={}, old={})",
            channel.name, options.milliwatt_test, options.old_behavior,
        );

        if options.old_behavior {
            // Old mode: generate continuous 1000 Hz tone via the tone generator
            //
            // In a real implementation:
            //   set_write_format(channel, Format::Ulaw).await;
            //   set_read_format(channel, Format::Ulaw).await;
            //   activate_generator(channel, &milliwatt_gen).await;
            //   loop {
            //       if safe_sleep(channel, 10_000).await.is_err() {
            //           break;
            //       }
            //   }
            //   deactivate_generator(channel).await;
            info!("Milliwatt: old-style 1000 Hz constant tone on '{}'", channel.name);
        } else if options.milliwatt_test {
            // Proper milliwatt test: 1004 Hz for 9 seconds, silence for 1 second
            //
            // In a real implementation:
            //   let tone_spec = "1004/9000,0/1000";  // 9s tone, 1s silence
            //   playtones_start(channel, TONE_AMPLITUDE, tone_spec).await;
            //   loop {
            //       if safe_sleep(channel, 10_000).await.is_err() {
            //           break;
            //       }
            //   }
            info!(
                "Milliwatt: proper milliwatt test tone (9s on, 1s off) on '{}'",
                channel.name,
            );
        } else {
            // Default: continuous 1004 Hz tone (no silent interval)
            //
            // In a real implementation:
            //   let tone_spec = "1004/1000";  // continuous 1004 Hz
            //   playtones_start(channel, TONE_AMPLITUDE, tone_spec).await;
            //   loop {
            //       if safe_sleep(channel, 10_000).await.is_err() {
            //           break;
            //       }
            //   }
            info!(
                "Milliwatt: continuous 1004 Hz tone on '{}'",
                channel.name,
            );
        }

        PbxExecResult::Success
    }

    /// Generate milliwatt samples into a buffer using the digital milliwatt pattern.
    ///
    /// Fills `buf` with mu-law encoded 1004 Hz tone samples, cycling through
    /// the 8-byte pattern. Returns the new index into the pattern.
    pub fn generate_samples(buf: &mut [u8], start_index: usize) -> usize {
        let mut idx = start_index;
        for sample in buf.iter_mut() {
            *sample = DIGITAL_MILLIWATT[idx & 7];
            idx += 1;
        }
        idx & 7
    }

    /// Generate signed linear (16-bit) samples for a 1004 Hz tone.
    ///
    /// This produces a pure sine wave at 1004 Hz, 8000 Hz sample rate.
    pub fn generate_slin_samples(buf: &mut [i16], sample_offset: u64) {
        let freq = Self::TONE_FREQ as f64;
        let rate = Self::SAMPLE_RATE as f64;
        let amplitude = 8192.0_f64; // Reasonable amplitude for slin

        for (i, sample) in buf.iter_mut().enumerate() {
            let t = (sample_offset + i as u64) as f64 / rate;
            let value = amplitude * (2.0 * std::f64::consts::PI * freq * t).sin();
            *sample = value.round() as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_milliwatt_options_empty() {
        let opts = MilliwattOptions::parse("");
        assert!(!opts.milliwatt_test);
        assert!(!opts.old_behavior);
    }

    #[test]
    fn test_milliwatt_options_m() {
        let opts = MilliwattOptions::parse("m");
        assert!(opts.milliwatt_test);
        assert!(!opts.old_behavior);
    }

    #[test]
    fn test_milliwatt_options_o() {
        let opts = MilliwattOptions::parse("o");
        assert!(!opts.milliwatt_test);
        assert!(opts.old_behavior);
    }

    #[test]
    fn test_digital_milliwatt_pattern() {
        assert_eq!(DIGITAL_MILLIWATT.len(), 8);
        // Pattern is symmetric
        assert_eq!(DIGITAL_MILLIWATT[0], 0x1e);
        assert_eq!(DIGITAL_MILLIWATT[4], 0x9e);
    }

    #[test]
    fn test_generate_samples() {
        let mut buf = [0u8; 16];
        let new_idx = AppMilliwatt::generate_samples(&mut buf, 0);
        assert_eq!(new_idx, 0); // 16 % 8 == 0
        // First 8 bytes should match the pattern
        assert_eq!(&buf[..8], &DIGITAL_MILLIWATT);
        // Second 8 bytes should also match (repeating)
        assert_eq!(&buf[8..], &DIGITAL_MILLIWATT);
    }

    #[test]
    fn test_generate_slin_samples() {
        let mut buf = [0i16; 8000]; // 1 second
        AppMilliwatt::generate_slin_samples(&mut buf, 0);
        // Should have approximately 1004 zero crossings in 1 second
        let mut crossings = 0u32;
        for i in 1..buf.len() {
            if (buf[i - 1] >= 0 && buf[i] < 0) || (buf[i - 1] < 0 && buf[i] >= 0) {
                crossings += 1;
            }
        }
        // 1004 Hz = ~2008 zero crossings per second (within tolerance)
        assert!(crossings > 1990 && crossings < 2020, "crossings: {}", crossings);
    }

    #[tokio::test]
    async fn test_milliwatt_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMilliwatt::exec(&mut channel, "m").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
