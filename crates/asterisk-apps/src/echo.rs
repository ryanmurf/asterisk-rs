//! Echo application - reads frames and writes them back.
//!
//! Port of app_echo.c from Asterisk C. This is a simple test application
//! that reads all incoming frames and writes them back to the same channel,
//! creating an echo effect. Useful for testing audio quality and latency.
//! The application exits when '#' is pressed.

use std::time::Duration;

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::{ChannelState, Frame};
use tracing::{debug, info};

/// The Echo() dialplan application.
///
/// Echoes back any media or DTMF frames read from the channel.
/// This will not echo CONTROL, MODEM, or NULL frames.
/// If '#' is detected, the application exits.
///
/// Usage: Echo()
///
/// Note: This application does not automatically answer. It should be
/// preceded by Answer() or Progress().
pub struct AppEcho;

impl DialplanApp for AppEcho {
    fn name(&self) -> &str {
        "Echo"
    }

    fn description(&self) -> &str {
        "Echo media, DTMF back to the calling party"
    }
}

impl AppEcho {
    /// Execute the Echo application.
    ///
    /// Reads frames from the channel's technology driver and writes them back,
    /// continuing until the channel hangs up or '#' DTMF is received. This is
    /// the real media pump: for a live inbound call it drives
    /// `read_frame`/`write_frame` on the channel driver, which move RTP frames
    /// to and from the bound socket (issue with no media pump). When the
    /// channel has no media plane (no driver / no RTP session), it degrades to
    /// polling channel state so the dialplan (and thus the SIP dialog) stays
    /// alive until the remote hangs up.
    ///
    /// # Arguments
    /// * `channel` - The channel to echo frames on
    pub async fn exec(channel: &mut Channel) -> PbxExecResult {
        info!("Echo: starting echo on channel '{}'", channel.name);

        // The technology driver is looked up by the channel-name prefix, e.g.
        // "PJSIP/alice-00000001" -> "PJSIP" (Asterisk names channels
        // <tech>/<resource>-<id>). Its read_frame/write_frame proxy to the RTP
        // session bound for this call.
        let tech = channel
            .name
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let driver = asterisk_core::channel::tech_registry::TECH_REGISTRY.find(&tech);

        let chan_name = channel.name.clone();
        loop {
            // Terminate on hangup (local flag, or a hangup set on the shared
            // store copy by AMI / a remote BYE).
            if channel.state == ChannelState::Down || channel.check_hangup() {
                break;
            }
            if let Some(store_chan) = asterisk_core::channel_store::find_by_name(&chan_name) {
                let flags = {
                    let guard = store_chan.lock();
                    if guard.check_hangup() {
                        Some(guard.softhangup_flags)
                    } else {
                        None
                    }
                };
                if let Some(flags) = flags {
                    channel.softhangup(flags);
                    break;
                }
            }

            let Some(driver) = driver.as_ref() else {
                // No technology driver at all: nothing to pump. Poll for hangup.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };

            // Read one frame, bounded so we periodically re-check for hangup
            // even when the media is silent (recv_frame blocks on the socket).
            match tokio::time::timeout(Duration::from_millis(500), driver.read_frame(channel)).await
            {
                Ok(Ok(frame)) => {
                    match &frame {
                        // '#' terminates Echo, matching app_echo.c.
                        Frame::DtmfEnd { digit, .. } | Frame::DtmfBegin { digit } if *digit == '#' => {
                            debug!("Echo: '#' received, exiting on '{}'", chan_name);
                            break;
                        }
                        // Echo media / DTMF / text back to the caller. (Only
                        // voice actually goes on the wire via RTP; other kinds
                        // are accepted and ignored by the driver.)
                        Frame::Voice { .. }
                        | Frame::Video { .. }
                        | Frame::Text { .. }
                        | Frame::DtmfBegin { .. }
                        | Frame::DtmfEnd { .. } => {
                            if let Err(e) = driver.write_frame(channel, &frame).await {
                                debug!("Echo: write_frame error on '{}': {}", chan_name, e);
                            }
                        }
                        // Do not echo Control / Modem / Null frames.
                        _ => {}
                    }
                }
                // Read error (e.g. no RTP session attached for this channel):
                // back off briefly and keep the dialplan alive.
                Ok(Err(_)) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                // Timed out waiting for a frame: loop to re-check hangup.
                Err(_) => {}
            }
        }

        info!("Echo: echo completed on channel '{}'", channel.name);
        PbxExecResult::Success
    }
}
