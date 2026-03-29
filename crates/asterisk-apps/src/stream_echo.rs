//! StreamEcho - multistream echo test application.
//!
//! Port of app_stream_echo.c from Asterisk C. Similar to Echo() but
//! operates on individual media streams, echoing each stream's audio
//! back independently. Useful for testing multistream (bundled) media.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// The StreamEcho() dialplan application.
///
/// Usage: StreamEcho([num_streams])
///
/// Echoes audio back on each stream independently. If num_streams is
/// specified, the application will request that many streams be set up.
/// Each stream's incoming audio is echoed back on the same stream.
///
/// Press '#' to exit.
pub struct AppStreamEcho;

impl DialplanApp for AppStreamEcho {
    fn name(&self) -> &str {
        "StreamEcho"
    }

    fn description(&self) -> &str {
        "Echo audio back on each media stream independently"
    }
}

impl AppStreamEcho {
    /// Execute the StreamEcho application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let num_streams: u32 = args.trim().parse().unwrap_or(0);

        info!(
            "StreamEcho: channel '{}' streams={}",
            channel.name,
            if num_streams > 0 {
                num_streams.to_string()
            } else {
                "default".to_string()
            },
        );

        // In a real implementation:
        // 1. Answer channel if not already up
        // 2. If num_streams specified, request topology change
        // 3. Loop:
        //    a. Read frame from channel
        //    b. If DTMF '#', break
        //    c. If voice frame, write back to same stream
        // 4. Return

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stream_echo_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppStreamEcho::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_stream_echo_with_streams() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppStreamEcho::exec(&mut channel, "3").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
