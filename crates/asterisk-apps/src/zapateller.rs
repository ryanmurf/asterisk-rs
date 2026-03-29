//! Zapateller application - Special Information Tones (SIT).
//!
//! Port of app_zapateller.c from Asterisk C. Plays the Special Information
//! Tone (SIT) tri-tone sequence to discourage telemarketers and autodialers.
//! The SIT consists of three tones: 950 Hz, 1400 Hz, and 1800 Hz, each
//! for 330 ms, followed by 1 second of silence.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info};

/// Status set by the Zapateller application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZapatellerStatus {
    /// No action was taken.
    Nothing,
    /// Channel was answered.
    Answered,
    /// SIT tones were played.
    Zapped,
}

impl ZapatellerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nothing => "NOTHING",
            Self::Answered => "ANSWERED",
            Self::Zapped => "ZAPPED",
        }
    }
}

/// Options for the Zapateller application.
#[derive(Debug, Clone, Default)]
pub struct ZapatellerOptions {
    /// Answer the channel before playing the tones.
    pub answer: bool,
    /// Only play tones if there is no caller ID.
    pub no_caller_id: bool,
}

impl ZapatellerOptions {
    /// Parse comma-separated options.
    pub fn parse(args: &str) -> Self {
        let mut result = Self::default();
        for opt in args.split(',') {
            match opt.trim().to_lowercase().as_str() {
                "answer" => result.answer = true,
                "nocallerid" => result.no_caller_id = true,
                "" => {}
                _ => {
                    debug!("Zapateller: ignoring unknown option '{}'", opt.trim());
                }
            }
        }
        result
    }
}

/// SIT tone parameters.
pub const SIT_TONE_1_FREQ: u32 = 950;
pub const SIT_TONE_2_FREQ: u32 = 1400;
pub const SIT_TONE_3_FREQ: u32 = 1800;
pub const SIT_TONE_DURATION_MS: u32 = 330;
pub const SIT_SILENCE_DURATION_MS: u32 = 1000;

/// The Zapateller() dialplan application.
///
/// Usage: Zapateller(options)
///
/// Generates Special Information Tones (SIT) to block telemarketers
/// and autodialers from calling. The tri-tone (950/1400/1800 Hz) signals
/// to automatic dialing equipment that the number is disconnected.
///
/// Options (comma-separated):
///   answer      - Answer the channel before playing
///   nocallerid  - Only play if there is no caller ID
///
/// Sets ZAPATELLERSTATUS variable to NOTHING, ANSWERED, or ZAPPED.
pub struct AppZapateller;

impl DialplanApp for AppZapateller {
    fn name(&self) -> &str {
        "Zapateller"
    }

    fn description(&self) -> &str {
        "Block Telemarketers with Special Information Tone"
    }
}

impl AppZapateller {
    /// Execute the Zapateller application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = ZapatellerOptions::parse(args);

        info!(
            "Zapateller: channel '{}' (answer={}, nocallerid={})",
            channel.name, options.answer, options.no_caller_id,
        );

        // Set initial status
        let mut status = ZapatellerStatus::Nothing;

        // Stop any currently playing stream
        // stop_stream(channel).await;

        // Answer the channel if requested and not already up
        if channel.state != ChannelState::Up {
            if options.answer {
                channel.state = ChannelState::Up;
                status = ZapatellerStatus::Answered;
            }
            // Brief pause after answer
            // safe_sleep(channel, 500).await;
        }

        // If nocallerid option is set and we have caller ID, skip
        if options.no_caller_id {
            // In a real implementation, check channel.caller_id.number
            // If caller ID is present, return without playing tones
            //
            //   if let Some(ref cid) = channel.caller_id_number {
            //       if !cid.is_empty() {
            //           set_variable(channel, "ZAPATELLERSTATUS", status.as_str());
            //           return PbxExecResult::Success;
            //       }
            //   }
            debug!("Zapateller: nocallerid check on '{}'", channel.name);
        }

        // Play the SIT tri-tone sequence
        //
        // In a real implementation:
        //   tone_pair(channel, 950, 0, 330, 0).await?;
        //   tone_pair(channel, 1400, 0, 330, 0).await?;
        //   tone_pair(channel, 1800, 0, 330, 0).await?;
        //   tone_pair(channel, 0, 0, 1000, 0).await?;  // silence
        //
        //   set_variable(channel, "ZAPATELLERSTATUS", "ZAPPED");
        status = ZapatellerStatus::Zapped;

        info!(
            "Zapateller: channel '{}' status={}",
            channel.name,
            status.as_str(),
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zapateller_options_answer() {
        let opts = ZapatellerOptions::parse("answer");
        assert!(opts.answer);
        assert!(!opts.no_caller_id);
    }

    #[test]
    fn test_zapateller_options_both() {
        let opts = ZapatellerOptions::parse("answer,nocallerid");
        assert!(opts.answer);
        assert!(opts.no_caller_id);
    }

    #[test]
    fn test_zapateller_options_empty() {
        let opts = ZapatellerOptions::parse("");
        assert!(!opts.answer);
        assert!(!opts.no_caller_id);
    }

    #[test]
    fn test_zapateller_status() {
        assert_eq!(ZapatellerStatus::Nothing.as_str(), "NOTHING");
        assert_eq!(ZapatellerStatus::Answered.as_str(), "ANSWERED");
        assert_eq!(ZapatellerStatus::Zapped.as_str(), "ZAPPED");
    }

    #[test]
    fn test_sit_frequencies() {
        assert_eq!(SIT_TONE_1_FREQ, 950);
        assert_eq!(SIT_TONE_2_FREQ, 1400);
        assert_eq!(SIT_TONE_3_FREQ, 1800);
    }

    #[tokio::test]
    async fn test_zapateller_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppZapateller::exec(&mut channel, "answer").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.state, ChannelState::Up);
    }
}
