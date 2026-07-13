//! Ringing application - indicate ringing to the calling channel.
//!
//! Port of Asterisk's `Ringing()` builtin (`pbx_builtin_ringing`): sends
//! AST_CONTROL_RINGING through the channel technology — for SIP channels the
//! driver emits `180 Ringing` on the pending INVITE — and moves the channel
//! to the Ringing state (issue #57).

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::{ChannelState, ControlFrame};
use tracing::{debug, warn};

/// The Ringing() dialplan application.
///
/// Usage in dialplan: Ringing()
pub struct AppRinging;

impl DialplanApp for AppRinging {
    fn name(&self) -> &str {
        "Ringing"
    }

    fn description(&self) -> &str {
        "Indicate ringing tone"
    }
}

impl AppRinging {
    /// Execute the Ringing application on a channel.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        // Indicate through the technology driver, looked up by the
        // channel-name prefix (e.g. "PJSIP/alice-00000001" -> "PJSIP"),
        // exactly like Echo()/Playback(). For SIP this sends 180 Ringing.
        let tech = channel.name.split('/').next().unwrap_or("").to_string();
        match asterisk_core::channel::tech_registry::TECH_REGISTRY.find(&tech) {
            Some(driver) => {
                if let Err(e) = driver
                    .indicate(channel, ControlFrame::Ringing as i32, &[])
                    .await
                {
                    // Mirrors Asterisk: a failed indication is logged, not
                    // fatal — the dialplan continues.
                    warn!(
                        "Ringing: indicate failed on channel '{}': {}",
                        channel.name, e
                    );
                }
            }
            None => {
                debug!(
                    "Ringing: channel '{}' has no '{}' driver; state change only",
                    channel.name, tech
                );
            }
        }

        // Move the channel to Ringing pre-answer. Update the store copy too,
        // so observers (AMI, other apps polling the store) see the state.
        if channel.state != ChannelState::Up {
            channel.set_state(ChannelState::Ringing);
            if let Some(store_chan) =
                asterisk_core::channel_store::find_by_name(&channel.name)
            {
                let mut ch = store_chan.lock();
                if ch.state != ChannelState::Up {
                    ch.set_state(ChannelState::Ringing);
                }
            }
        }

        PbxExecResult::Success
    }
}
