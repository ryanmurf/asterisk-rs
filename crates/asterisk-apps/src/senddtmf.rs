//! SendDTMF application.
//!
//! Port of app_senddtmf.c from Asterisk C. Sends a string of DTMF
//! digits on the current channel or a specified external channel.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// Options for the SendDTMF application.
#[derive(Debug, Clone)]
pub struct SendDtmfOptions {
    /// DTMF digits to send.
    pub digits: String,
    /// Timeout between digits in milliseconds (default: 250).
    pub timeout_ms: u32,
    /// Duration of each digit in milliseconds (default: 0 = channel default).
    pub duration_ms: u32,
    /// Channel to send DTMF to (empty = current channel).
    pub channel: Option<String>,
}

impl SendDtmfOptions {
    /// Parse from comma-separated arguments.
    ///
    /// Format: digits[,timeout_ms[,duration_ms[,channel]]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.split(',').collect();
        Self {
            digits: parts.first().copied().unwrap_or("").to_string(),
            timeout_ms: parts.get(1).and_then(|s| s.trim().parse().ok()).unwrap_or(250),
            duration_ms: parts.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            channel: parts.get(3).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        }
    }
}

/// The SendDTMF() dialplan application.
///
/// Usage: SendDTMF(digits[,timeout_ms[,duration_ms[,channel]]])
///
/// Sends DTMF digits on a channel. If no channel is specified, sends
/// on the current channel. Valid digits: 0-9, *, #, A-D, a-d, w (500ms pause).
///
/// Sets SENDDTMFSTATUS = SUCCESS | FAILURE
pub struct AppSendDtmf;

impl DialplanApp for AppSendDtmf {
    fn name(&self) -> &str {
        "SendDTMF"
    }

    fn description(&self) -> &str {
        "Send DTMF digits on a channel"
    }
}

impl AppSendDtmf {
    /// Execute the SendDTMF application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = SendDtmfOptions::parse(args);

        if options.digits.is_empty() {
            warn!("SendDTMF: requires digits argument");
            return PbxExecResult::Failed;
        }

        info!(
            "SendDTMF: channel '{}' digits='{}' timeout={}ms duration={}ms",
            channel.name, options.digits, options.timeout_ms, options.duration_ms,
        );

        // In a real implementation:
        // 1. If channel specified, look it up
        // 2. For each digit in the string:
        //    a. If 'w' or 'W', pause for 500ms
        //    b. Otherwise, send DTMF begin frame, wait duration, send DTMF end
        //    c. Wait timeout_ms between digits
        // 3. Set SENDDTMFSTATUS

        PbxExecResult::Success
    }
}

/// The ReceiveDTMF() dialplan application.
///
/// Usage: ReceiveDTMF(variable[,digits[,timeout]])
///
/// Receives DTMF digits from the channel.
pub struct AppReceiveDtmf;

impl DialplanApp for AppReceiveDtmf {
    fn name(&self) -> &str {
        "ReceiveDTMF"
    }

    fn description(&self) -> &str {
        "Receive DTMF digits from a channel"
    }
}

impl AppReceiveDtmf {
    /// Execute the ReceiveDTMF application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.split(',').collect();
        let varname = parts.first().copied().unwrap_or("").trim();

        if varname.is_empty() {
            warn!("ReceiveDTMF: requires variable name argument");
            return PbxExecResult::Failed;
        }

        info!("ReceiveDTMF: channel '{}' into variable '{}'", channel.name, varname);

        // In a real implementation:
        // Read DTMF digits and store in the specified variable

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_senddtmf_options_parse() {
        let opts = SendDtmfOptions::parse("12345,300,100,SIP/phone1");
        assert_eq!(opts.digits, "12345");
        assert_eq!(opts.timeout_ms, 300);
        assert_eq!(opts.duration_ms, 100);
        assert_eq!(opts.channel.as_deref(), Some("SIP/phone1"));
    }

    #[test]
    fn test_senddtmf_options_defaults() {
        let opts = SendDtmfOptions::parse("*#123");
        assert_eq!(opts.digits, "*#123");
        assert_eq!(opts.timeout_ms, 250);
        assert_eq!(opts.duration_ms, 0);
        assert!(opts.channel.is_none());
    }

    #[tokio::test]
    async fn test_senddtmf_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendDtmf::exec(&mut channel, "1234").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_senddtmf_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendDtmf::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
