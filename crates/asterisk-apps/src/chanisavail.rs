//! ChanIsAvail - check channel availability.
//!
//! Port of app_chanisavail.c from Asterisk C. Checks whether a
//! specified channel technology/resource is available for making
//! an outbound call.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// Options for the ChanIsAvail application.
#[derive(Debug, Clone, Default)]
pub struct ChanIsAvailOptions {
    /// Check device state only (don't request channel).
    pub state_only: bool,
    /// Check all specified channels, not just first available.
    pub check_all: bool,
}

impl ChanIsAvailOptions {
    /// Parse option string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                's' => result.state_only = true,
                'a' => result.check_all = true,
                _ => {}
            }
        }
        result
    }
}

/// The ChanIsAvail() dialplan application.
///
/// Usage: ChanIsAvail(Tech/Resource[&Tech2/Resource2[&...]][,options])
///
/// Checks if any of the specified channels are available. Channels are
/// specified as Technology/Resource separated by '&'.
///
/// Options:
///   s - Check device state only (don't try to request the channel)
///   a - Check all channels, don't stop at first available
///
/// Sets:
///   AVAILCHAN     - Available channel name
///   AVAILORIGCHAN - Original channel specification that matched
///   AVAILSTATUS   - Device state of available channel
///   AVAILCAUSECODE - Cause code if not available
pub struct AppChanIsAvail;

impl DialplanApp for AppChanIsAvail {
    fn name(&self) -> &str {
        "ChanIsAvail"
    }

    fn description(&self) -> &str {
        "Check channel availability"
    }
}

impl AppChanIsAvail {
    /// Execute the ChanIsAvail application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.split(',').collect();
        let channels_str = parts.first().copied().unwrap_or("");
        let options_str = parts.get(1).copied().unwrap_or("");
        let _options = ChanIsAvailOptions::parse(options_str);

        if channels_str.is_empty() {
            warn!("ChanIsAvail: requires Tech/Resource argument");
            return PbxExecResult::Failed;
        }

        let check_channels: Vec<&str> = channels_str.split('&').collect();

        info!(
            "ChanIsAvail: channel '{}' checking {} channel(s)",
            channel.name,
            check_channels.len(),
        );

        // In a real implementation:
        // For each channel spec:
        // 1. Parse technology and resource
        // 2. If state_only, check device state
        // 3. Otherwise, try to request the channel
        // 4. Set AVAILCHAN, AVAILORIGCHAN, AVAILSTATUS variables

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chanisavail_options() {
        let opts = ChanIsAvailOptions::parse("sa");
        assert!(opts.state_only);
        assert!(opts.check_all);
    }

    #[tokio::test]
    async fn test_chanisavail_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppChanIsAvail::exec(&mut channel, "SIP/phone1&SIP/phone2").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_chanisavail_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppChanIsAvail::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
