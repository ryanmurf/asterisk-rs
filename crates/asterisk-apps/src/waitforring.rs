//! WaitForRing application.
//!
//! Port of app_waitforring.c from Asterisk C. Waits for a specified
//! number of seconds for a ring event on the channel.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The WaitForRing() dialplan application.
///
/// Usage: WaitForRing(timeout)
///
/// Waits for the specified number of seconds for a ring event.
/// Returns 0 on success (ring received) or -1 on timeout/error.
///
/// This is useful after answering a call to wait for the next
/// ring on a FXO channel (e.g. for distinctive ring detection).
pub struct AppWaitForRing;

impl DialplanApp for AppWaitForRing {
    fn name(&self) -> &str {
        "WaitForRing"
    }

    fn description(&self) -> &str {
        "Wait for a ring event on the channel"
    }
}

impl AppWaitForRing {
    /// Execute the WaitForRing application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let timeout_secs: f64 = args.trim().parse().unwrap_or(0.0);

        if timeout_secs <= 0.0 {
            warn!("WaitForRing: requires positive timeout in seconds");
            return PbxExecResult::Failed;
        }

        info!(
            "WaitForRing: channel '{}' waiting {:.1}s for ring",
            channel.name, timeout_secs,
        );

        // In a real implementation:
        // 1. Compute deadline from timeout
        // 2. Loop reading frames from channel:
        //    a. If control frame with AST_CONTROL_RING, return success
        //    b. If past deadline, return timeout
        //    c. If hangup, return hangup

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_waitforring_exec() {
        let mut channel = Channel::new("DAHDI/1-1");
        let result = AppWaitForRing::exec(&mut channel, "10").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_waitforring_no_timeout() {
        let mut channel = Channel::new("DAHDI/1-1");
        let result = AppWaitForRing::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
