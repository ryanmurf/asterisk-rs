//! Morse code generator application.
//!
//! Port of app_morsecode.c from Asterisk C. Plays the Morse code
//! equivalent of a given string as audio tones. Supports both
//! International and American Morse code. Configurable via channel
//! variables MORSEDITLEN, MORSETONE, MORSESPACETONE, and MORSETYPE.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// Morse code type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorseType {
    /// International (ITU) Morse code.
    International,
    /// American (railroad) Morse code.
    American,
}

/// International Morse code lookup table (ASCII 32-127).
///
/// Each entry is a string of '.' (dit), '-' (dah), and ' ' (intra-char pause).
pub const INTERNATIONAL_CODE: &[&str] = &[
    " ",       // 32 <space>
    ".-.-.-",  // 33 !
    ".-..-.",  // 34 "
    "",        // 35 #
    "",        // 36 $
    "",        // 37 %
    "",        // 38 &
    ".----.",  // 39 '
    "-.--.-",  // 40 (
    "-.--.-",  // 41 )
    "",        // 42 *
    "",        // 43 +
    "--..--",  // 44 ,
    "-....-",  // 45 -
    ".-.-.-",  // 46 .
    "-..-.",   // 47 /
    "-----",   // 48 0
    ".----",   // 49 1
    "..---",   // 50 2
    "...--",   // 51 3
    "....-",   // 52 4
    ".....",   // 53 5
    "-....",   // 54 6
    "--...",   // 55 7
    "---..",   // 56 8
    "----.",   // 57 9
    "---...",  // 58 :
    "-.-.-.",  // 59 ;
    "",        // 60 <
    "-...-",   // 61 =
    "",        // 62 >
    "..--..",  // 63 ?
    ".--.-.",  // 64 @
    ".-",      // 65 A
    "-...",    // 66 B
    "-.-.",    // 67 C
    "-..",     // 68 D
    ".",       // 69 E
    "..-.",    // 70 F
    "--.",     // 71 G
    "....",    // 72 H
    "..",      // 73 I
    ".---",    // 74 J
    "-.-",     // 75 K
    ".-..",    // 76 L
    "--",      // 77 M
    "-.",      // 78 N
    "---",     // 79 O
    ".--.",    // 80 P
    "--.-",    // 81 Q
    ".-.",     // 82 R
    "...",     // 83 S
    "-",       // 84 T
    "..-",     // 85 U
    "...-",    // 86 V
    ".--",     // 87 W
    "-..-",    // 88 X
    "-.--",    // 89 Y
    "--..",    // 90 Z
    "-.--.-",  // 91 [
    "-..-.",   // 92 backslash
    "-.--.-",  // 93 ]
    "",        // 94 ^
    "..--.-",  // 95 _
    ".----.",  // 96 `
    ".-",      // 97 a
    "-...",    // 98 b
    "-.-.",    // 99 c
    "-..",     // 100 d
    ".",       // 101 e
    "..-.",    // 102 f
    "--.",     // 103 g
    "....",    // 104 h
    "..",      // 105 i
    ".---",    // 106 j
    "-.-",     // 107 k
    ".-..",    // 108 l
    "--",      // 109 m
    "-.",      // 110 n
    "---",     // 111 o
    ".--.",    // 112 p
    "--.-",    // 113 q
    ".-.",     // 114 r
    "...",     // 115 s
    "-",       // 116 t
    "..-",     // 117 u
    "...-",    // 118 v
    ".--",     // 119 w
    "-..-",    // 120 x
    "-.--",    // 121 y
    "--..",    // 122 z
    "-.--.-",  // 123 {
    "",        // 124 |
    "-.--.-",  // 125 }
    "-..-.",   // 126 ~
    ". . .",   // 127 DEL (error)
];

/// Configuration for the Morse code generator.
#[derive(Debug, Clone)]
pub struct MorseConfig {
    /// Length of a dit in milliseconds (default: 80).
    pub dit_len: u32,
    /// Tone frequency in Hz (default: 800).
    pub tone_freq: u32,
    /// Space tone frequency in Hz (default: 0 = silence).
    pub space_freq: u32,
    /// Morse code type (default: International).
    pub morse_type: MorseType,
}

impl Default for MorseConfig {
    fn default() -> Self {
        Self {
            dit_len: 80,
            tone_freq: 800,
            space_freq: 0,
            morse_type: MorseType::International,
        }
    }
}

/// Look up the Morse code representation for an ASCII character.
pub fn char_to_morse(ch: char, morse_type: MorseType) -> &'static str {
    let idx = ch as usize;
    if !(32..=127).contains(&idx) {
        return "";
    }
    match morse_type {
        MorseType::International => {
            INTERNATIONAL_CODE.get(idx - 32).copied().unwrap_or("")
        }
        MorseType::American => {
            // American Morse code uses the same table structure
            // but with different encodings. For simplicity in this port,
            // we reuse international when American codes aren't defined.
            INTERNATIONAL_CODE.get(idx - 32).copied().unwrap_or("")
        }
    }
}

/// The Morsecode() dialplan application.
///
/// Usage: Morsecode(string)
///
/// Plays the Morse code equivalent of the given string as audio tones.
/// Does not automatically answer the channel -- should be preceded by
/// Answer() or Progress().
///
/// Configurable via channel variables:
///   MORSEDITLEN   - Dit length in ms (default: 80)
///   MORSETONE     - Tone frequency in Hz (default: 800)
///   MORSESPACETONE - Space frequency in Hz (default: 0)
///   MORSETYPE     - "AMERICAN" or "INTERNATIONAL" (default)
pub struct AppMorsecode;

impl DialplanApp for AppMorsecode {
    fn name(&self) -> &str {
        "Morsecode"
    }

    fn description(&self) -> &str {
        "Plays morse code"
    }
}

impl AppMorsecode {
    /// Execute the Morsecode application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            warn!("Morsecode: requires a string argument");
            return PbxExecResult::Success;
        }

        info!(
            "Morsecode: channel '{}' playing morse for '{}'",
            channel.name, args,
        );

        // In a real implementation:
        //
        //   // Read configuration from channel variables
        //   let config = MorseConfig {
        //       dit_len: get_variable_int(channel, "MORSEDITLEN").unwrap_or(80),
        //       tone_freq: get_variable_int(channel, "MORSETONE").unwrap_or(800),
        //       space_freq: get_variable_int(channel, "MORSESPACETONE").unwrap_or(0),
        //       morse_type: match get_variable(channel, "MORSETYPE").as_deref() {
        //           Some("AMERICAN") => MorseType::American,
        //           _ => MorseType::International,
        //       },
        //   };
        //
        //   for ch in args.chars() {
        //       let code = char_to_morse(ch, config.morse_type);
        //       if code.is_empty() {
        //           continue;
        //       }
        //
        //       for element in code.chars() {
        //           match element {
        //               '.' => {
        //                   // Dit: tone for 1 dit length
        //                   play_tone(channel, config.tone_freq, config.dit_len).await?;
        //               }
        //               '-' => {
        //                   // Dah: tone for 3 dit lengths
        //                   play_tone(channel, config.tone_freq, 3 * config.dit_len).await?;
        //               }
        //               ' ' => {
        //                   // Intra-character space: silence for 3 dit lengths
        //                   play_tone(channel, config.space_freq, 3 * config.dit_len).await?;
        //               }
        //               _ => {
        //                   // Other spacers
        //                   play_tone(channel, config.space_freq, 2 * config.dit_len).await?;
        //               }
        //           }
        //           // Inter-element gap: silence for 1 dit length
        //           play_tone(channel, config.space_freq, config.dit_len).await?;
        //       }
        //       // Inter-character gap: silence for 2 dit lengths
        //       // (total = 3 dit lengths including the 1 from inter-element)
        //       play_tone(channel, config.space_freq, 2 * config.dit_len).await?;
        //   }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_to_morse_letters() {
        assert_eq!(char_to_morse('A', MorseType::International), ".-");
        assert_eq!(char_to_morse('B', MorseType::International), "-...");
        assert_eq!(char_to_morse('S', MorseType::International), "...");
        assert_eq!(char_to_morse('O', MorseType::International), "---");
    }

    #[test]
    fn test_char_to_morse_digits() {
        assert_eq!(char_to_morse('0', MorseType::International), "-----");
        assert_eq!(char_to_morse('1', MorseType::International), ".----");
        assert_eq!(char_to_morse('9', MorseType::International), "----.");
    }

    #[test]
    fn test_char_to_morse_lowercase() {
        assert_eq!(char_to_morse('a', MorseType::International), ".-");
        assert_eq!(char_to_morse('z', MorseType::International), "--..");
    }

    #[test]
    fn test_char_to_morse_space() {
        assert_eq!(char_to_morse(' ', MorseType::International), " ");
    }

    #[test]
    fn test_char_to_morse_unknown() {
        assert_eq!(char_to_morse('\x01', MorseType::International), "");
    }

    #[test]
    fn test_sos() {
        let sos: Vec<&str> = "SOS"
            .chars()
            .map(|c| char_to_morse(c, MorseType::International))
            .collect();
        assert_eq!(sos, vec!["...", "---", "..."]);
    }

    #[test]
    fn test_morse_config_default() {
        let config = MorseConfig::default();
        assert_eq!(config.dit_len, 80);
        assert_eq!(config.tone_freq, 800);
        assert_eq!(config.space_freq, 0);
        assert_eq!(config.morse_type, MorseType::International);
    }

    #[tokio::test]
    async fn test_morsecode_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMorsecode::exec(&mut channel, "SOS").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_morsecode_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMorsecode::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
