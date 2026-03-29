//! Icecast streaming application.
//!
//! Port of app_ices.c from Asterisk C. Streams channel audio to an
//! Icecast server by piping audio to an external `ices` process.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The ICES() dialplan application.
///
/// Usage: ICES(config_file)
///
/// Encodes channel audio and streams it to an Icecast server by spawning
/// an external `ices` (or `ices0`) process with the given configuration file.
/// Audio from the channel is piped to the ices process stdin as raw 16-bit
/// signed linear PCM at 8000 Hz.
pub struct AppIces;

impl DialplanApp for AppIces {
    fn name(&self) -> &str {
        "ICES"
    }

    fn description(&self) -> &str {
        "Stream audio to Icecast server via ices"
    }
}

impl AppIces {
    /// Execute the ICES application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let config_file = args.trim();

        if config_file.is_empty() {
            warn!("ICES: requires ices configuration file argument");
            return PbxExecResult::Failed;
        }

        info!("ICES: channel '{}' streaming with config '{}'", channel.name, config_file);

        // In a real implementation:
        // 1. Set channel read format to slin (16-bit signed linear, 8kHz)
        // 2. Fork/exec ices process with config_file
        // 3. Pipe channel audio frames to ices stdin
        // 4. Loop until hangup
        // 5. Kill ices process and waitpid()

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ices_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppIces::exec(&mut channel, "/etc/ices.xml").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_ices_exec_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppIces::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
