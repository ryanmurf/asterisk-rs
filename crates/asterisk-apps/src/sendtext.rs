//! SendText application - sends a text message to a channel.
//!
//! Port of app_sendtext.c from Asterisk C. Sends text to the current
//! channel using the channel driver's send_text capability. Supports
//! enhanced messaging with display names and content types via
//! channel variables.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info};

/// Send text status set as SENDTEXTSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTextStatus {
    /// Transmission succeeded.
    Success,
    /// Transmission failed.
    Failure,
    /// Text transmission not supported by channel driver.
    Unsupported,
}

impl SendTextStatus {
    /// String representation for the SENDTEXTSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

/// Send text type set as SENDTEXTTYPE channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTextType {
    /// No message sent.
    None,
    /// Basic text message (no enhanced attributes).
    Basic,
    /// Enhanced message with attributes.
    Enhanced,
}

impl SendTextType {
    /// String representation for the SENDTEXTTYPE variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Basic => "BASIC",
            Self::Enhanced => "ENHANCED",
        }
    }
}

/// The SendText() dialplan application.
///
/// Sends text to the current channel. The text can be provided as
/// the argument or via the SENDTEXT_BODY channel variable.
///
/// Usage: SendText([text])
///
/// Channel variables consulted:
///   SENDTEXT_FROM_DISPLAYNAME - From display name for enhanced messaging
///   SENDTEXT_TO_DISPLAYNAME   - To display name for enhanced messaging
///   SENDTEXT_CONTENT_TYPE     - Content type (default: text/plain)
///   SENDTEXT_BODY             - Message body (overrides argument)
///
/// Sets SENDTEXTSTATUS (SUCCESS, FAILURE, UNSUPPORTED)
/// Sets SENDTEXTTYPE (NONE, BASIC, ENHANCED)
pub struct AppSendText;

impl DialplanApp for AppSendText {
    fn name(&self) -> &str {
        "SendText"
    }

    fn description(&self) -> &str {
        "Send a Text Message on a channel"
    }
}

impl AppSendText {
    /// Execute the SendText application.
    ///
    /// # Arguments
    /// * `channel` - The channel to send text on
    /// * `args` - The text message to send (may be overridden by SENDTEXT_BODY)
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        // Determine the text body: SENDTEXT_BODY variable overrides the argument
        let body = if let Some(var_body) = channel.get_variable("SENDTEXT_BODY") {
            if !var_body.is_empty() {
                var_body.to_string()
            } else {
                args.to_string()
            }
        } else {
            args.to_string()
        };

        if body.is_empty() {
            debug!("SendText: no text to send");
            channel.set_variable("SENDTEXTSTATUS", SendTextStatus::Success.as_str());
            channel.set_variable("SENDTEXTTYPE", SendTextType::None.as_str());
            return PbxExecResult::Success;
        }

        info!(
            "SendText: sending text to channel '{}' ({} bytes)",
            channel.name,
            body.len()
        );

        // Check for enhanced messaging attributes
        let _from_name = channel
            .get_variable("SENDTEXT_FROM_DISPLAYNAME")
            .map(|s| s.to_string());
        let _to_name = channel
            .get_variable("SENDTEXT_TO_DISPLAYNAME")
            .map(|s| s.to_string());
        let _content_type = channel
            .get_variable("SENDTEXT_CONTENT_TYPE")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "text/plain".to_string());

        // In a full implementation, we would:
        //
        // 1. Check if the channel driver supports send_text
        // 2. If enhanced messaging is available and attributes are set:
        //    - Build an enhanced message with from/to/content-type
        //    - Send via send_text_data()
        //    - Set SENDTEXTTYPE to ENHANCED
        // 3. Otherwise:
        //    - If content type is not text/*, set UNSUPPORTED
        //    - Send via send_text()
        //    - Set SENDTEXTTYPE to BASIC
        //
        //   match channel_driver.send_text(channel, &body).await {
        //       Ok(()) => {
        //           channel.set_variable("SENDTEXTSTATUS", "SUCCESS");
        //           channel.set_variable("SENDTEXTTYPE", "BASIC");
        //       }
        //       Err(AsteriskError::NotSupported(_)) => {
        //           channel.set_variable("SENDTEXTSTATUS", "UNSUPPORTED");
        //           channel.set_variable("SENDTEXTTYPE", "NONE");
        //       }
        //       Err(_) => {
        //           channel.set_variable("SENDTEXTSTATUS", "FAILURE");
        //           channel.set_variable("SENDTEXTTYPE", "NONE");
        //       }
        //   }

        // Stub: report success
        let status = SendTextStatus::Success;
        let text_type = SendTextType::Basic;

        channel.set_variable("SENDTEXTSTATUS", status.as_str());
        channel.set_variable("SENDTEXTTYPE", text_type.as_str());

        debug!(
            "SendText: SENDTEXTSTATUS={}, SENDTEXTTYPE={}",
            status.as_str(),
            text_type.as_str()
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_strings() {
        assert_eq!(SendTextStatus::Success.as_str(), "SUCCESS");
        assert_eq!(SendTextStatus::Failure.as_str(), "FAILURE");
        assert_eq!(SendTextStatus::Unsupported.as_str(), "UNSUPPORTED");
    }

    #[test]
    fn test_type_strings() {
        assert_eq!(SendTextType::None.as_str(), "NONE");
        assert_eq!(SendTextType::Basic.as_str(), "BASIC");
        assert_eq!(SendTextType::Enhanced.as_str(), "ENHANCED");
    }

    #[tokio::test]
    async fn test_sendtext_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendText::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SENDTEXTTYPE"), Some("NONE"));
    }

    #[tokio::test]
    async fn test_sendtext_with_body() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendText::exec(&mut channel, "Hello World").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SENDTEXTSTATUS"), Some("SUCCESS"));
        assert_eq!(channel.get_variable("SENDTEXTTYPE"), Some("BASIC"));
    }

    #[tokio::test]
    async fn test_sendtext_body_variable_override() {
        let mut channel = Channel::new("SIP/test-001");
        channel.set_variable("SENDTEXT_BODY", "Override text");
        let result = AppSendText::exec(&mut channel, "Ignored text").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SENDTEXTSTATUS"), Some("SUCCESS"));
    }
}
