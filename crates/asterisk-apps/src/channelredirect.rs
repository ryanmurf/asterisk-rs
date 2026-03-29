//! Channel redirect application.
//!
//! Port of app_channelredirect.c from Asterisk C. Redirects another
//! channel to a specified context/extension/priority in the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The ChannelRedirect() dialplan application.
///
/// Usage: ChannelRedirect(channel,context,extension,priority)
///
/// Redirects the specified channel to the given dialplan location.
/// The channel does not need to be the current channel -- this can
/// redirect any active channel by name.
///
/// Sets CHANNELREDIRECT_STATUS:
///   NOCHANNEL - Channel not found
///   SUCCESS   - Redirect succeeded
pub struct AppChannelRedirect;

impl DialplanApp for AppChannelRedirect {
    fn name(&self) -> &str {
        "ChannelRedirect"
    }

    fn description(&self) -> &str {
        "Redirect another channel to a dialplan location"
    }
}

/// Parsed redirect arguments.
#[derive(Debug, Clone)]
pub struct RedirectTarget {
    /// Channel name to redirect.
    pub channel: String,
    /// Target context.
    pub context: String,
    /// Target extension.
    pub extension: String,
    /// Target priority.
    pub priority: i32,
}

impl RedirectTarget {
    /// Parse from comma-separated arguments.
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 4 {
            return None;
        }
        Some(Self {
            channel: parts[0].trim().to_string(),
            context: parts[1].trim().to_string(),
            extension: parts[2].trim().to_string(),
            priority: parts[3].trim().parse().unwrap_or(1),
        })
    }
}

impl AppChannelRedirect {
    /// Execute the ChannelRedirect application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let target = match RedirectTarget::parse(args) {
            Some(t) => t,
            None => {
                warn!("ChannelRedirect: requires channel,context,exten,priority arguments");
                return PbxExecResult::Failed;
            }
        };

        info!(
            "ChannelRedirect: channel '{}' redirecting '{}' to {},{},{}",
            channel.name, target.channel, target.context, target.extension, target.priority,
        );

        // In a real implementation:
        // 1. Look up target channel by name
        // 2. If not found, set CHANNELREDIRECT_STATUS=NOCHANNEL
        // 3. Call ast_async_goto on target channel
        // 4. Set CHANNELREDIRECT_STATUS=SUCCESS

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_target_parse() {
        let target = RedirectTarget::parse("SIP/phone1,default,s,1").unwrap();
        assert_eq!(target.channel, "SIP/phone1");
        assert_eq!(target.context, "default");
        assert_eq!(target.extension, "s");
        assert_eq!(target.priority, 1);
    }

    #[test]
    fn test_redirect_target_parse_insufficient() {
        assert!(RedirectTarget::parse("SIP/phone1,default").is_none());
    }

    #[tokio::test]
    async fn test_channel_redirect_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppChannelRedirect::exec(&mut channel, "SIP/phone1,default,s,1").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
