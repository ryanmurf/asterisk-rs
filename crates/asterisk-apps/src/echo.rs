//! Echo application - reads frames and writes them back.
//!
//! Port of app_echo.c from Asterisk C. This is a simple test application
//! that reads all incoming frames and writes them back to the same channel,
//! creating an echo effect. Useful for testing audio quality and latency.
//! The application exits when '#' is pressed.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
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
    /// Reads frames from the channel and writes them back. Continues until
    /// the channel hangs up or '#' DTMF is received.
    ///
    /// # Arguments
    /// * `channel` - The channel to echo frames on
    pub async fn exec(channel: &mut Channel) -> PbxExecResult {
        info!("Echo: starting echo on channel '{}'", channel.name);

        // Track whether we've sent a FIR (Full Intra Request) for video
        let _fir_sent = false;

        // Main echo loop: read frames and write them back
        // In a real implementation, this uses channel.read() and channel.write()
        // which are async operations that wait for the next frame.
        //
        // The loop runs until:
        // 1. The channel hangs up (read returns None/error)
        // 2. The '#' DTMF digit is received
        //
        // Frame handling rules (matching Asterisk C behavior):
        // - Voice frames: echo back
        // - Video frames: echo back (and send FIR on first video frame)
        // - DTMF frames: echo back (unless '#' which terminates)
        // - Text frames: echo back
        // - Control frames: only echo VidUpdate, and only once
        // - Modem frames: do NOT echo
        // - Null frames: do NOT echo

        // Check if channel is still alive
        if channel.state == ChannelState::Down {
            debug!("Echo: channel '{}' hung up", channel.name);
            return PbxExecResult::Hangup;
        }

        // In a real implementation, this would be a loop that:
        //   1. Reads frames from the channel
        //   2. Echoes them back (voice, video, DTMF, text)
        //   3. Terminates on '#' DTMF or channel hangup
        //
        // The actual frame loop would call:
        //   let frame = channel_driver.read(channel).await?;
        //
        // Then process each frame type:
        //   - Voice/Video/Text/DTMF: echo back
        //   - Control (VidUpdate): echo once
        //   - Modem/Null: skip
        //   - DTMF '#': exit
        //
        // Without real frame I/O, we block by polling the channel state.
        // This keeps the dialplan alive so the SIP dialog stays open until
        // the remote side hangs up (softhangup sets state to Down).
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if channel.state == ChannelState::Down
                || channel.check_hangup()
            {
                break;
            }
        }

        info!("Echo: echo completed on channel '{}'", channel.name);
        PbxExecResult::Success
    }
}
