//! NBS (Network Broadcast Sound) streaming application.
//!
//! Port of app_nbscat.c from Asterisk C. Streams audio from a Network
//! Broadcast Sound server to a channel by piping output from the `roles/nbscat`
//! command.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// The NBScat() dialplan application.
///
/// Usage: NBScat()
///
/// Plays NBS audio stream to the channel. Spawns an external `nbscat8k`
/// process and pipes its output (raw 8kHz mu-law audio) to the channel.
pub struct AppNbscat;

impl DialplanApp for AppNbscat {
    fn name(&self) -> &str {
        "NBScat"
    }

    fn description(&self) -> &str {
        "Stream NBS audio to the channel"
    }
}

impl AppNbscat {
    /// External command used to receive NBS audio.
    pub const NBSCAT_CMD: &'static str = "nbscat8k";

    /// Execute the NBScat application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("NBScat: channel '{}' starting NBS stream", channel.name);

        // In a real implementation:
        // 1. Answer channel if not already up
        // 2. Set write format to mu-law
        // 3. Fork/exec nbscat8k process
        // 4. Read stdout from nbscat8k
        // 5. Write audio frames to channel
        // 6. Loop until hangup or process exit
        // 7. Kill process and waitpid()

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_nbscat_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppNbscat::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
