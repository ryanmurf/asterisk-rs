//! SendURL application.
//!
//! Port of app_url.c from Asterisk (if present). Sends a URL to the
//! channel for display. Typically used with SIP channels that support
//! the NOTIFY method with URL content, or with channels that have
//! a display capable of rendering URLs.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{info, warn};

/// Options for SendURL.
#[derive(Debug, Clone, Default)]
pub struct SendUrlOptions {
    /// Wait for the channel to acknowledge the URL.
    pub wait: bool,
}

impl SendUrlOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            if ch == 'w' { result.wait = true }
        }
        result
    }
}

/// The SendURL() dialplan application.
///
/// Usage: SendURL(url[,options])
///
/// Sends a URL to the channel. If the channel does not support receiving
/// URLs, the application returns and execution continues normally.
///
/// Options:
///   w - Wait for the channel to acknowledge receipt of the URL
pub struct AppSendUrl;

impl DialplanApp for AppSendUrl {
    fn name(&self) -> &str {
        "SendURL"
    }

    fn description(&self) -> &str {
        "Send a URL"
    }
}

impl AppSendUrl {
    /// Execute the SendURL application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let url = match parts.first() {
            Some(u) if !u.trim().is_empty() => u.trim(),
            _ => {
                warn!("SendURL: requires a URL argument");
                return PbxExecResult::Failed;
            }
        };

        let options = parts
            .get(1)
            .map(|o| SendUrlOptions::parse(o.trim()))
            .unwrap_or_default();

        info!(
            "SendURL: channel '{}' sending URL '{}' (wait={})",
            channel.name, url, options.wait,
        );

        // Answer the channel if not up
        if channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   // Check if the channel supports HTML/URL
        //   if !channel_supports_html(channel) {
        //       // Channel doesn't support URLs - just continue
        //       return PbxExecResult::Success;
        //   }
        //
        //   // Send the URL
        //   send_html(channel, HtmlSubType::Url, url).await;
        //
        //   if options.wait {
        //       // Wait for acknowledgment or hangup
        //       loop {
        //           let frame = read_frame(channel).await;
        //           match frame {
        //               None => return PbxExecResult::Hangup,
        //               Some(Frame::Html(HtmlSubType::LoadComplete)) => break,
        //               Some(Frame::Dtmf(_)) => break,
        //               _ => continue,
        //           }
        //       }
        //   }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sendurl_options() {
        let opts = SendUrlOptions::parse("w");
        assert!(opts.wait);
    }

    #[tokio::test]
    async fn test_sendurl_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendUrl::exec(&mut channel, "https://example.com").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_sendurl_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendUrl::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
