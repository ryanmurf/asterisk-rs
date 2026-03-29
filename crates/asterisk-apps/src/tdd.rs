//! TDD/TTY application for hearing-impaired communication.
//!
//! Port of app_tdd.c from Asterisk C. Provides text telephone
//! (TDD/TTY) support using Baudot code (45.45 baud, 5-bit) encoding
//! and decoding for hearing-impaired callers.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// Baudot code shift state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaudotShift {
    /// Letters mode (LTRS).
    Letters,
    /// Figures/numbers mode (FIGS).
    Figures,
}

/// Baudot encoding constants.
pub mod baudot {
    /// Baud rate for TDD (45.45 baud).
    pub const BAUD_RATE: f64 = 45.45;
    /// Bits per character (5 data bits).
    pub const BITS_PER_CHAR: u8 = 5;
    /// Mark tone frequency (Hz).
    pub const MARK_FREQ: u32 = 1400;
    /// Space tone frequency (Hz).
    pub const SPACE_FREQ: u32 = 1800;
    /// Shift to Letters code.
    pub const LTRS: u8 = 0x1F;
    /// Shift to Figures code.
    pub const FIGS: u8 = 0x1B;
}

/// Encode a character to Baudot code (stub).
///
/// Returns (baudot_code, shift_needed) where shift_needed indicates
/// if a LTRS or FIGS shift character must be sent first.
pub fn char_to_baudot(ch: char, current_shift: BaudotShift) -> Option<(u8, Option<BaudotShift>)> {
    let ch = ch.to_ascii_uppercase();
    match ch {
        'A'..='Z' => {
            let code = match ch {
                'A' => 0x03, 'B' => 0x19, 'C' => 0x0E, 'D' => 0x09,
                'E' => 0x01, 'F' => 0x0D, 'G' => 0x1A, 'H' => 0x14,
                'I' => 0x06, 'J' => 0x0B, 'K' => 0x0F, 'L' => 0x12,
                'M' => 0x1C, 'N' => 0x0C, 'O' => 0x18, 'P' => 0x16,
                'Q' => 0x17, 'R' => 0x0A, 'S' => 0x05, 'T' => 0x10,
                'U' => 0x07, 'V' => 0x1E, 'W' => 0x13, 'X' => 0x1D,
                'Y' => 0x15, 'Z' => 0x11,
                _ => return None,
            };
            let shift = if current_shift != BaudotShift::Letters {
                Some(BaudotShift::Letters)
            } else {
                None
            };
            Some((code, shift))
        }
        ' ' => Some((0x04, None)), // Space is same in both shifts
        '\n' => Some((0x02, None)), // Line feed
        '\r' => Some((0x08, None)), // Carriage return
        _ => None,
    }
}

/// The TDD() dialplan application (stub).
///
/// Usage: TDD(mode)
///
/// Enables TDD/TTY mode on the channel for hearing-impaired callers.
/// Mode: send | receive | mate
///
/// In send mode, text is converted to Baudot FSK and sent as audio.
/// In receive mode, audio is decoded from Baudot FSK to text.
pub struct AppTdd;

impl DialplanApp for AppTdd {
    fn name(&self) -> &str {
        "TDD"
    }

    fn description(&self) -> &str {
        "Enable TDD/TTY mode for hearing-impaired communication"
    }
}

impl AppTdd {
    /// Execute the TDD application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let mode = args.trim().to_lowercase();

        info!("TDD: channel '{}' mode='{}'", channel.name, mode);

        // In a real implementation:
        // 1. Set up FSK modem at 45.45 baud (mark=1400Hz, space=1800Hz)
        // 2. Based on mode:
        //    - send: encode text to Baudot, generate FSK audio frames
        //    - receive: demodulate audio, decode Baudot to text
        //    - mate: bidirectional TDD relay
        // 3. Bridge text <-> audio until hangup

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_to_baudot_letter() {
        let (code, shift) = char_to_baudot('A', BaudotShift::Letters).unwrap();
        assert_eq!(code, 0x03);
        assert!(shift.is_none());
    }

    #[test]
    fn test_char_to_baudot_shift_needed() {
        let (code, shift) = char_to_baudot('A', BaudotShift::Figures).unwrap();
        assert_eq!(code, 0x03);
        assert_eq!(shift, Some(BaudotShift::Letters));
    }

    #[test]
    fn test_char_to_baudot_space() {
        let (code, shift) = char_to_baudot(' ', BaudotShift::Letters).unwrap();
        assert_eq!(code, 0x04);
        assert!(shift.is_none());
    }

    #[test]
    fn test_baudot_constants() {
        assert_eq!(baudot::MARK_FREQ, 1400);
        assert_eq!(baudot::SPACE_FREQ, 1800);
    }

    #[tokio::test]
    async fn test_tdd_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTdd::exec(&mut channel, "receive").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
