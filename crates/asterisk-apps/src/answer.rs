//! Answer application - answers an incoming channel.
//!
//! Port of app_answer from Asterisk C. Answers the channel and optionally
//! waits a specified delay before returning control to the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::time::Duration;
use tracing::{debug, warn};

/// The Answer() dialplan application.
///
/// Answers the channel if it is not already answered, then optionally
/// waits for a specified number of milliseconds before returning.
///
/// Usage in dialplan: Answer([delay])
///   delay - optional delay in milliseconds after answering
pub struct AppAnswer;

impl DialplanApp for AppAnswer {
    fn name(&self) -> &str {
        "Answer"
    }

    fn description(&self) -> &str {
        "Answer a channel if ringing"
    }
}

impl AppAnswer {
    /// Execute the Answer application on a channel.
    ///
    /// # Arguments
    /// * `channel` - The channel to answer
    /// * `args` - Optional argument string. If provided, parsed as delay in milliseconds.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let delay_ms: u64 = if args.is_empty() {
            0
        } else {
            match args.trim().parse::<u64>() {
                Ok(d) => d,
                Err(_) => {
                    warn!("Answer: invalid delay argument '{}', using 0", args);
                    0
                }
            }
        };

        // Only answer if the channel is not already answered
        if channel.state != ChannelState::Up {
            debug!(
                "Answer: answering channel '{}' (was in state {:?})",
                channel.name, channel.state
            );
            channel.state = ChannelState::Up;
        } else {
            debug!("Answer: channel '{}' already answered", channel.name);
        }

        // Wait the specified delay if requested
        if delay_ms > 0 {
            debug!("Answer: waiting {}ms after answer", delay_ms);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        PbxExecResult::Success
    }
}
