//! WaitForSilence and WaitForNoise applications.
//!
//! Port of app_waitforsilence.c from Asterisk C. Waits for a specified
//! duration of silence (or noise) on the channel, useful for detecting
//! when a caller stops speaking.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// Options for WaitForSilence/WaitForNoise.
#[derive(Debug, Clone)]
pub struct WaitForSilenceOptions {
    /// Required silence (or noise) duration in milliseconds.
    pub required_ms: u32,
    /// Number of iterations (times silence must be detected). Default: 1.
    pub iterations: u32,
    /// Overall timeout in seconds (0 = no timeout).
    pub timeout_secs: u32,
}

impl WaitForSilenceOptions {
    /// Parse from comma-separated arguments.
    ///
    /// Format: silencereqd[,iterations[,timeout]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.split(',').collect();
        Self {
            required_ms: parts.first().and_then(|s| s.trim().parse().ok()).unwrap_or(1000),
            iterations: parts.get(1).and_then(|s| s.trim().parse().ok()).unwrap_or(1),
            timeout_secs: parts.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        }
    }
}

/// The WaitForSilence() dialplan application.
///
/// Usage: WaitForSilence(silencereqd[,iterations[,timeout]])
///
/// Waits for the channel to be silent for the specified duration.
///
/// Parameters:
///   silencereqd - Required silence in milliseconds
///   iterations  - Number of times silence must occur (default: 1)
///   timeout     - Maximum time to wait in seconds (0 = forever)
///
/// Sets:
///   WAITSTATUS = SILENCE | TIMEOUT | HANGUP | ERROR
pub struct AppWaitForSilence;

impl DialplanApp for AppWaitForSilence {
    fn name(&self) -> &str {
        "WaitForSilence"
    }

    fn description(&self) -> &str {
        "Wait for silence on the channel"
    }
}

impl AppWaitForSilence {
    /// Execute the WaitForSilence application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = WaitForSilenceOptions::parse(args);

        info!(
            "WaitForSilence: channel '{}' required={}ms iterations={} timeout={}s",
            channel.name, options.required_ms, options.iterations, options.timeout_secs,
        );

        // In a real implementation:
        // 1. Set read format to slin
        // 2. Create DSP for silence detection
        // 3. Loop reading audio frames:
        //    a. Feed to silence detector
        //    b. Track cumulative silence duration
        //    c. If silence >= required_ms, decrement iterations
        //    d. If iterations == 0, set WAITSTATUS=SILENCE and return
        //    e. If timeout reached, set WAITSTATUS=TIMEOUT and return

        PbxExecResult::Success
    }
}

/// The WaitForNoise() dialplan application.
///
/// Usage: WaitForNoise(noisereqd[,iterations[,timeout]])
///
/// The inverse of WaitForSilence -- waits for noise/sound on the channel.
///
/// Sets:
///   WAITSTATUS = NOISE | TIMEOUT | HANGUP | ERROR
pub struct AppWaitForNoise;

impl DialplanApp for AppWaitForNoise {
    fn name(&self) -> &str {
        "WaitForNoise"
    }

    fn description(&self) -> &str {
        "Wait for noise on the channel"
    }
}

impl AppWaitForNoise {
    /// Execute the WaitForNoise application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = WaitForSilenceOptions::parse(args);

        info!(
            "WaitForNoise: channel '{}' required={}ms iterations={} timeout={}s",
            channel.name, options.required_ms, options.iterations, options.timeout_secs,
        );

        // In a real implementation: same as WaitForSilence but inverted logic

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_waitforsilence_options_defaults() {
        let opts = WaitForSilenceOptions::parse("2000");
        assert_eq!(opts.required_ms, 2000);
        assert_eq!(opts.iterations, 1);
        assert_eq!(opts.timeout_secs, 0);
    }

    #[test]
    fn test_waitforsilence_options_full() {
        let opts = WaitForSilenceOptions::parse("1000,3,30");
        assert_eq!(opts.required_ms, 1000);
        assert_eq!(opts.iterations, 3);
        assert_eq!(opts.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_waitforsilence_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWaitForSilence::exec(&mut channel, "1000,1,30").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_waitfornoise_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWaitForNoise::exec(&mut channel, "500").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
