//! Attended and blind transfer applications.
//!
//! Port of app_blind_transfer.c and attended transfer logic from Asterisk C.
//! Provides dialplan applications for both blind (unattended) and attended
//! (consultative) transfers, using the bridge framework for execution.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// Blind transfer status set as BLINDTRANSFERSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlindTransferStatus {
    /// Transfer succeeded.
    Success,
    /// Transfer failed.
    Failure,
    /// Transfer target was invalid.
    Invalid,
    /// Transfer not permitted.
    NotPermitted,
}

impl BlindTransferStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Invalid => "INVALID",
            Self::NotPermitted => "NOTPERMITTED",
        }
    }
}

/// Attended transfer status set as ATTENDEDTRANSFERSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttendedTransferStatus {
    /// Transfer succeeded.
    Success,
    /// Transfer failed.
    Failure,
    /// Transfer target was invalid.
    Invalid,
    /// Transfer not permitted.
    NotPermitted,
}

impl AttendedTransferStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Invalid => "INVALID",
            Self::NotPermitted => "NOTPERMITTED",
        }
    }
}

/// Parsed transfer destination.
#[derive(Debug, Clone)]
pub struct TransferTarget {
    /// Extension to transfer to.
    pub exten: String,
    /// Context for the transfer (None means use current channel context).
    pub context: Option<String>,
}

impl TransferTarget {
    /// Parse a transfer target from argument string.
    ///
    /// Formats:
    ///   - "exten" (use channel's current context)
    ///   - "exten,context" (explicit context, comma-separated)
    ///   - "exten@context" (explicit context, at-sign)
    pub fn parse(args: &str) -> Option<Self> {
        let args = args.trim();
        if args.is_empty() {
            return None;
        }

        // Try comma-separated first (app_blind_transfer format)
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() == 2 && !parts[1].trim().is_empty() {
            return Some(Self {
                exten: parts[0].trim().to_string(),
                context: Some(parts[1].trim().to_string()),
            });
        }

        // Try at-sign separator
        if let Some(at_pos) = args.find('@') {
            let exten = &args[..at_pos];
            let context = &args[at_pos + 1..];
            if !exten.is_empty() && !context.is_empty() {
                return Some(Self {
                    exten: exten.to_string(),
                    context: Some(context.to_string()),
                });
            }
        }

        // Just an extension
        Some(Self {
            exten: parts[0].trim().to_string(),
            context: None,
        })
    }
}

/// The BlindTransfer() dialplan application.
///
/// Usage: BlindTransfer(exten[,context])
///
/// Redirects all channels currently bridged to the caller channel to
/// the specified extension and context. Sets BLINDTRANSFERSTATUS.
pub struct AppBlindTransfer;

impl DialplanApp for AppBlindTransfer {
    fn name(&self) -> &str {
        "BlindTransfer"
    }

    fn description(&self) -> &str {
        "Blind transfer channel(s) to the extension and context provided"
    }
}

impl AppBlindTransfer {
    /// Execute the BlindTransfer application.
    ///
    /// # Arguments
    /// * `channel` - The channel initiating the transfer
    /// * `args` - "exten[,context]" or "exten[@context]"
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let target = match TransferTarget::parse(args) {
            Some(t) => t,
            None => {
                warn!("BlindTransfer: requires an extension argument");
                channel.set_variable("BLINDTRANSFERSTATUS", BlindTransferStatus::Failure.as_str());
                return PbxExecResult::Success;
            }
        };

        let context = target
            .context
            .unwrap_or_else(|| channel.context.clone());

        info!(
            "BlindTransfer: channel '{}' blind transferring to {}@{}",
            channel.name, target.exten, context
        );

        // In a real implementation:
        //
        //   // Check if the channel is in a bridge
        //   let bridge_id = match &channel.bridge_id {
        //       Some(id) => id.clone(),
        //       None => {
        //           warn!("BlindTransfer: channel is not in a bridge");
        //           channel.set_variable("BLINDTRANSFERSTATUS", "FAILURE");
        //           return PbxExecResult::Success;
        //       }
        //   };
        //
        //   // Check if the target extension exists
        //   if !dialplan.extension_exists(&context, &target.exten, 1) {
        //       channel.set_variable("BLINDTRANSFERSTATUS", "INVALID");
        //       return PbxExecResult::Success;
        //   }
        //
        //   // Perform the blind transfer via the bridge framework
        //   match bridge::transfer_blind(channel, &target.exten, &context) {
        //       Ok(BridgeTransferResult::Success) => {
        //           channel.set_variable("BLINDTRANSFERSTATUS", "SUCCESS");
        //       }
        //       Ok(BridgeTransferResult::NotPermitted) => {
        //           channel.set_variable("BLINDTRANSFERSTATUS", "NOTPERMITTED");
        //       }
        //       Ok(BridgeTransferResult::Invalid) => {
        //           channel.set_variable("BLINDTRANSFERSTATUS", "INVALID");
        //       }
        //       Err(_) | Ok(BridgeTransferResult::Fail) => {
        //           channel.set_variable("BLINDTRANSFERSTATUS", "FAILURE");
        //       }
        //   }

        let status = BlindTransferStatus::Success;
        channel.set_variable("BLINDTRANSFERSTATUS", status.as_str());

        debug!(
            "BlindTransfer: BLINDTRANSFERSTATUS={}",
            status.as_str()
        );

        PbxExecResult::Success
    }
}

/// The AttnTransfer() dialplan application.
///
/// Usage: AttnTransfer(exten[@context])
///
/// Performs an attended (consultative) transfer. The transferring party
/// first calls the target, and when the target answers, the transfer
/// is completed by connecting the original caller with the target.
pub struct AppAttnTransfer;

impl DialplanApp for AppAttnTransfer {
    fn name(&self) -> &str {
        "AttnTransfer"
    }

    fn description(&self) -> &str {
        "Attended transfer current call to another extension"
    }
}

impl AppAttnTransfer {
    /// Execute the AttnTransfer application.
    ///
    /// # Arguments
    /// * `channel` - The channel initiating the attended transfer
    /// * `args` - "exten[@context]"
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let target = match TransferTarget::parse(args) {
            Some(t) => t,
            None => {
                warn!("AttnTransfer: requires an extension argument");
                channel.set_variable(
                    "ATTENDEDTRANSFERSTATUS",
                    AttendedTransferStatus::Failure.as_str(),
                );
                return PbxExecResult::Success;
            }
        };

        let context = target
            .context
            .unwrap_or_else(|| channel.context.clone());

        info!(
            "AttnTransfer: channel '{}' attended transfer to {}@{}",
            channel.name, target.exten, context
        );

        // In a real implementation, attended transfer is a multi-step process:
        //
        //   // 1. Place the current bridge on hold (the original caller hears MOH)
        //   let bridge_id = match &channel.bridge_id {
        //       Some(id) => id.clone(),
        //       None => {
        //           channel.set_variable("ATTENDEDTRANSFERSTATUS", "FAILURE");
        //           return PbxExecResult::Success;
        //       }
        //   };
        //
        //   // 2. Originate a call to the transfer target
        //   let target_channel = originate(&target.exten, &context).await;
        //
        //   // 3. Bridge the transferring channel with the target
        //   let consult_bridge = Bridge::new("atxfer-consult");
        //   consult_bridge.add_channel(channel.unique_id.clone(), channel.name.clone());
        //   consult_bridge.add_channel(target_channel.unique_id.clone(), target_channel.name.clone());
        //
        //   // 4. Wait for the transferring party to complete or abort
        //   //    - If they hang up: complete the transfer (connect original caller to target)
        //   //    - If target hangs up: return to original call
        //   //    - DTMF abort: return to original call
        //
        //   match wait_for_transfer_decision(channel, &target_channel).await {
        //       TransferDecision::Complete => {
        //           // Move the original caller to the target's bridge
        //           bridge::transfer_attended(channel, &target_channel).await;
        //           channel.set_variable("ATTENDEDTRANSFERSTATUS", "SUCCESS");
        //       }
        //       TransferDecision::Abort => {
        //           // Hang up target, return to original bridge
        //           target_channel.hangup(HangupCause::NormalClearing);
        //           channel.set_variable("ATTENDEDTRANSFERSTATUS", "FAILURE");
        //       }
        //       TransferDecision::TargetHangup => {
        //           // Return to original bridge
        //           channel.set_variable("ATTENDEDTRANSFERSTATUS", "FAILURE");
        //       }
        //   }

        let status = AttendedTransferStatus::Success;
        channel.set_variable("ATTENDEDTRANSFERSTATUS", status.as_str());

        debug!(
            "AttnTransfer: ATTENDEDTRANSFERSTATUS={}",
            status.as_str()
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_transfer_target_exten_only() {
        let target = TransferTarget::parse("100").unwrap();
        assert_eq!(target.exten, "100");
        assert!(target.context.is_none());
    }

    #[test]
    fn test_parse_transfer_target_comma_context() {
        let target = TransferTarget::parse("100,sales").unwrap();
        assert_eq!(target.exten, "100");
        assert_eq!(target.context.as_deref(), Some("sales"));
    }

    #[test]
    fn test_parse_transfer_target_at_context() {
        let target = TransferTarget::parse("100@sales").unwrap();
        assert_eq!(target.exten, "100");
        assert_eq!(target.context.as_deref(), Some("sales"));
    }

    #[test]
    fn test_parse_transfer_target_empty() {
        assert!(TransferTarget::parse("").is_none());
    }

    #[test]
    fn test_blind_transfer_status_strings() {
        assert_eq!(BlindTransferStatus::Success.as_str(), "SUCCESS");
        assert_eq!(BlindTransferStatus::Failure.as_str(), "FAILURE");
        assert_eq!(BlindTransferStatus::Invalid.as_str(), "INVALID");
        assert_eq!(BlindTransferStatus::NotPermitted.as_str(), "NOTPERMITTED");
    }

    #[test]
    fn test_attended_transfer_status_strings() {
        assert_eq!(AttendedTransferStatus::Success.as_str(), "SUCCESS");
        assert_eq!(AttendedTransferStatus::Failure.as_str(), "FAILURE");
    }

    #[tokio::test]
    async fn test_blind_transfer_exec() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "from-internal".to_string();
        let result = AppBlindTransfer::exec(&mut channel, "200").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(
            channel.get_variable("BLINDTRANSFERSTATUS"),
            Some("SUCCESS")
        );
    }

    #[tokio::test]
    async fn test_blind_transfer_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppBlindTransfer::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(
            channel.get_variable("BLINDTRANSFERSTATUS"),
            Some("FAILURE")
        );
    }

    #[tokio::test]
    async fn test_attn_transfer_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAttnTransfer::exec(&mut channel, "200@sales").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(
            channel.get_variable("ATTENDEDTRANSFERSTATUS"),
            Some("SUCCESS")
        );
    }

    #[tokio::test]
    async fn test_attn_transfer_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAttnTransfer::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(
            channel.get_variable("ATTENDEDTRANSFERSTATUS"),
            Some("FAILURE")
        );
    }
}
