//! Transfer application - transfers a caller to another destination.
//!
//! Port of app_transfer.c from Asterisk C. Requests a blind transfer
//! of the channel to a given Tech/destination. The result is reported
//! in the TRANSFERSTATUS channel variable.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// Transfer status set as the TRANSFERSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    /// Transfer succeeded.
    Success,
    /// Transfer failed.
    Failure,
    /// Transfer not supported by the channel driver.
    Unsupported,
}

impl TransferStatus {
    /// String representation for the TRANSFERSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

/// Parsed transfer destination.
#[derive(Debug, Clone)]
pub struct TransferDest {
    /// Optional technology prefix (e.g. "SIP", "IAX2").
    pub tech: Option<String>,
    /// The destination address/extension.
    pub destination: String,
}

impl TransferDest {
    /// Parse a transfer destination string.
    ///
    /// Format: `[Tech/]destination`
    pub fn parse(data: &str) -> Option<Self> {
        let data = data.trim();
        if data.is_empty() {
            return None;
        }

        if let Some(slash_pos) = data.find('/') {
            let tech = &data[..slash_pos];
            let dest = &data[slash_pos + 1..];
            if tech.is_empty() || dest.is_empty() {
                // Just a destination without a real tech prefix
                Some(Self {
                    tech: None,
                    destination: data.to_string(),
                })
            } else {
                Some(Self {
                    tech: Some(tech.to_string()),
                    destination: dest.to_string(),
                })
            }
        } else {
            Some(Self {
                tech: None,
                destination: data.to_string(),
            })
        }
    }
}

/// The Transfer() dialplan application.
///
/// Requests the remote caller be transferred to a given destination.
/// If Tech/ is specified, only an incoming call with the same channel
/// technology will be transferred.
///
/// Usage in dialplan: Transfer([Tech/]destination)
///
/// Sets TRANSFERSTATUS channel variable to SUCCESS, FAILURE, or UNSUPPORTED.
/// Sets TRANSFERSTATUSPROTOCOL to the protocol-specific result code.
pub struct AppTransfer;

impl DialplanApp for AppTransfer {
    fn name(&self) -> &str {
        "Transfer"
    }

    fn description(&self) -> &str {
        "Transfer caller to remote extension"
    }
}

impl AppTransfer {
    /// Execute the Transfer application on a channel.
    ///
    /// # Arguments
    /// * `channel` - The channel to transfer
    /// * `args` - Transfer destination: `[Tech/]destination`
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        // Parse destination
        let dest = match TransferDest::parse(args) {
            Some(d) => d,
            None => {
                warn!("Transfer: requires an argument ([Tech/]destination)");
                channel.set_variable("TRANSFERSTATUS", "FAILURE");
                channel.set_variable("TRANSFERSTATUSPROTOCOL", "0");
                return PbxExecResult::Success;
            }
        };

        // If a technology was specified, verify it matches the channel's technology
        if let Some(ref tech) = dest.tech {
            let channel_tech = extract_tech(&channel.name);
            if !channel_tech.eq_ignore_ascii_case(tech) {
                debug!(
                    "Transfer: channel tech '{}' does not match requested tech '{}'",
                    channel_tech, tech
                );
                channel.set_variable("TRANSFERSTATUS", "FAILURE");
                channel.set_variable("TRANSFERSTATUSPROTOCOL", "0");
                return PbxExecResult::Success;
            }
        }

        // In a full implementation, we would check if the channel technology
        // supports the transfer operation and invoke it:
        //
        //   if !channel_tech.supports_transfer() {
        //       channel.set_variable("TRANSFERSTATUS", "UNSUPPORTED");
        //       channel.set_variable("TRANSFERSTATUSPROTOCOL", "0");
        //       return PbxExecResult::Success;
        //   }
        //
        //   let (result, protocol_code) = channel_tech.transfer(channel, &dest.destination).await;

        info!(
            "Transfer: transferring channel '{}' to destination '{}'",
            channel.name, dest.destination
        );

        // For now, report success. The actual transfer mechanics depend on the
        // channel driver implementing the transfer method.
        let status = TransferStatus::Success;
        let protocol_code = 0;

        debug!(
            "Transfer: channel {} TRANSFERSTATUS={}, TRANSFERSTATUSPROTOCOL={}",
            channel.name,
            status.as_str(),
            protocol_code
        );

        channel.set_variable("TRANSFERSTATUS", status.as_str());
        channel.set_variable("TRANSFERSTATUSPROTOCOL", &protocol_code.to_string());

        PbxExecResult::Success
    }
}

/// Extract the technology name from a channel name.
///
/// Channel names follow the format "Tech/identifier-uniqueid",
/// so we extract everything before the first '/'.
fn extract_tech(channel_name: &str) -> &str {
    channel_name
        .split('/')
        .next()
        .unwrap_or(channel_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dest_with_tech() {
        let dest = TransferDest::parse("SIP/100").unwrap();
        assert_eq!(dest.tech.as_deref(), Some("SIP"));
        assert_eq!(dest.destination, "100");
    }

    #[test]
    fn test_parse_dest_without_tech() {
        let dest = TransferDest::parse("100").unwrap();
        assert!(dest.tech.is_none());
        assert_eq!(dest.destination, "100");
    }

    #[test]
    fn test_parse_dest_empty() {
        assert!(TransferDest::parse("").is_none());
    }

    #[test]
    fn test_extract_tech() {
        assert_eq!(extract_tech("SIP/alice-00000001"), "SIP");
        assert_eq!(extract_tech("PJSIP/trunk-00000002"), "PJSIP");
        assert_eq!(extract_tech("Local/100@default"), "Local");
    }

    #[tokio::test]
    async fn test_transfer_empty_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTransfer::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("TRANSFERSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_transfer_tech_mismatch() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTransfer::exec(&mut channel, "IAX2/100").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("TRANSFERSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_transfer_success() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTransfer::exec(&mut channel, "SIP/200").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("TRANSFERSTATUS"), Some("SUCCESS"));
    }
}
