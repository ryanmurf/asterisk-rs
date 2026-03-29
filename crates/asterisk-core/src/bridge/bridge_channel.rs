//! Extended bridge channel operations.
//!
//! Port of bridge_channel.c from Asterisk C.
//!
//! Provides extended bridge channel operations beyond the basic BridgeChannel
//! struct: DTMF feature processing during bridged calls, blind and attended
//! transfer initiation from within a bridge, hold/unhold management, and
//! interval hook processing.

use crate::channel::ChannelId;
use std::collections::VecDeque;
use tracing::{debug, info};

/// DTMF digit buffered during bridge operation.
#[derive(Debug, Clone)]
pub struct DtmfDigit {
    pub digit: char,
    pub begin: bool, // true = begin, false = end
}

/// Transfer types that can be initiated from a bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTransferType {
    /// Blind (unattended) transfer
    Blind,
    /// Attended (consultative) transfer
    Attended,
}

/// Result of a bridge transfer operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTransferResult {
    Success,
    /// Transfer failed - return to bridge
    Fail,
    /// Invalid transfer destination
    InvalidDest,
    /// Transfer not permitted
    NotPermitted,
}

impl BridgeTransferResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Fail => "FAIL",
            Self::InvalidDest => "INVALID",
            Self::NotPermitted => "NOT_PERMITTED",
        }
    }
}

/// Actions that can be queued for a bridge channel.
#[derive(Debug, Clone)]
pub enum BridgeChannelAction {
    /// Write a DTMF digit to the bridge
    DtmfStream(String),
    /// Initiate a blind transfer to destination
    BlindTransfer { destination: String, context: String },
    /// Initiate an attended transfer
    AttendedTransfer { destination: String, context: String },
    /// Place the channel on hold
    Hold,
    /// Take the channel off hold
    Unhold,
    /// Run an interval callback
    IntervalHook { hook_id: u32 },
    /// Indicate ringing to the bridge peer
    Ringing,
    /// Bridge channel should leave the bridge
    Leave,
}

/// Extended bridge channel operations manager.
///
/// Manages per-channel state within a bridge including DTMF buffering,
/// queued actions, and transfer state.
#[derive(Debug)]
pub struct BridgeChannelOps {
    /// Channel identifier
    channel_id: ChannelId,
    /// Queued actions for this channel
    action_queue: VecDeque<BridgeChannelAction>,
    /// DTMF digit buffer for feature detection
    dtmf_buffer: String,
    /// Whether the channel is currently on hold
    on_hold: bool,
    /// Current transfer state
    transfer_state: Option<BridgeTransferType>,
    /// Number of frames written through this bridge channel
    frames_written: u64,
    /// Number of frames read through this bridge channel
    frames_read: u64,
}

impl BridgeChannelOps {
    /// Create a new bridge channel operations manager.
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            action_queue: VecDeque::new(),
            dtmf_buffer: String::new(),
            on_hold: false,
            transfer_state: None,
            frames_written: 0,
            frames_read: 0,
        }
    }

    /// Queue an action for this bridge channel.
    pub fn queue_action(&mut self, action: BridgeChannelAction) {
        debug!(channel = %self.channel_id, action = ?action, "Queuing bridge channel action");
        self.action_queue.push_back(action);
    }

    /// Dequeue the next action.
    pub fn next_action(&mut self) -> Option<BridgeChannelAction> {
        self.action_queue.pop_front()
    }

    /// Check if there are pending actions.
    pub fn has_pending_actions(&self) -> bool {
        !self.action_queue.is_empty()
    }

    /// Buffer a DTMF digit for feature detection.
    pub fn buffer_dtmf(&mut self, digit: char) {
        self.dtmf_buffer.push(digit);
    }

    /// Get the current DTMF buffer contents.
    pub fn dtmf_buffer(&self) -> &str {
        &self.dtmf_buffer
    }

    /// Clear the DTMF buffer.
    pub fn clear_dtmf_buffer(&mut self) {
        self.dtmf_buffer.clear();
    }

    /// Initiate a blind transfer from within the bridge.
    ///
    /// The transferring channel will be removed from the bridge and the
    /// peer channel will be redirected to the transfer destination.
    pub fn initiate_blind_transfer(&mut self, destination: &str, context: &str) -> BridgeTransferResult {
        if destination.is_empty() {
            return BridgeTransferResult::InvalidDest;
        }
        info!(
            channel = %self.channel_id,
            dest = destination,
            context = context,
            "Initiating blind transfer from bridge"
        );
        self.transfer_state = Some(BridgeTransferType::Blind);
        self.queue_action(BridgeChannelAction::BlindTransfer {
            destination: destination.to_string(),
            context: context.to_string(),
        });
        BridgeTransferResult::Success
    }

    /// Initiate an attended transfer from within the bridge.
    pub fn initiate_attended_transfer(&mut self, destination: &str, context: &str) -> BridgeTransferResult {
        if destination.is_empty() {
            return BridgeTransferResult::InvalidDest;
        }
        info!(
            channel = %self.channel_id,
            dest = destination,
            context = context,
            "Initiating attended transfer from bridge"
        );
        self.transfer_state = Some(BridgeTransferType::Attended);
        self.queue_action(BridgeChannelAction::AttendedTransfer {
            destination: destination.to_string(),
            context: context.to_string(),
        });
        BridgeTransferResult::Success
    }

    /// Place the channel on hold within the bridge.
    pub fn hold(&mut self) {
        if !self.on_hold {
            self.on_hold = true;
            self.queue_action(BridgeChannelAction::Hold);
            debug!(channel = %self.channel_id, "Bridge channel placed on hold");
        }
    }

    /// Take the channel off hold.
    pub fn unhold(&mut self) {
        if self.on_hold {
            self.on_hold = false;
            self.queue_action(BridgeChannelAction::Unhold);
            debug!(channel = %self.channel_id, "Bridge channel taken off hold");
        }
    }

    /// Check if the channel is on hold.
    pub fn is_on_hold(&self) -> bool {
        self.on_hold
    }

    /// Record a frame write.
    pub fn record_write(&mut self) {
        self.frames_written += 1;
    }

    /// Record a frame read.
    pub fn record_read(&mut self) {
        self.frames_read += 1;
    }

    /// Get frame statistics.
    pub fn frame_stats(&self) -> (u64, u64) {
        (self.frames_read, self.frames_written)
    }

    /// Get current transfer state.
    pub fn transfer_state(&self) -> Option<BridgeTransferType> {
        self.transfer_state
    }

    /// Clear transfer state.
    pub fn clear_transfer_state(&mut self) {
        self.transfer_state = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_channel_id() -> ChannelId {
        ChannelId::from_name("SIP/alice-001")
    }

    #[test]
    fn test_action_queue() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        assert!(!ops.has_pending_actions());

        ops.queue_action(BridgeChannelAction::Hold);
        ops.queue_action(BridgeChannelAction::Ringing);
        assert!(ops.has_pending_actions());

        let action = ops.next_action().unwrap();
        assert!(matches!(action, BridgeChannelAction::Hold));

        let action = ops.next_action().unwrap();
        assert!(matches!(action, BridgeChannelAction::Ringing));

        assert!(!ops.has_pending_actions());
    }

    #[test]
    fn test_dtmf_buffer() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        ops.buffer_dtmf('#');
        ops.buffer_dtmf('1');
        assert_eq!(ops.dtmf_buffer(), "#1");
        ops.clear_dtmf_buffer();
        assert!(ops.dtmf_buffer().is_empty());
    }

    #[test]
    fn test_blind_transfer() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        let result = ops.initiate_blind_transfer("100", "default");
        assert_eq!(result, BridgeTransferResult::Success);
        assert_eq!(ops.transfer_state(), Some(BridgeTransferType::Blind));
        assert!(ops.has_pending_actions());
    }

    #[test]
    fn test_blind_transfer_invalid_dest() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        let result = ops.initiate_blind_transfer("", "default");
        assert_eq!(result, BridgeTransferResult::InvalidDest);
    }

    #[test]
    fn test_hold_unhold() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        assert!(!ops.is_on_hold());
        ops.hold();
        assert!(ops.is_on_hold());
        ops.unhold();
        assert!(!ops.is_on_hold());
    }

    #[test]
    fn test_frame_stats() {
        let mut ops = BridgeChannelOps::new(test_channel_id());
        ops.record_read();
        ops.record_read();
        ops.record_write();
        assert_eq!(ops.frame_stats(), (2, 1));
    }

    #[test]
    fn test_transfer_result_str() {
        assert_eq!(BridgeTransferResult::Success.as_str(), "SUCCESS");
        assert_eq!(BridgeTransferResult::Fail.as_str(), "FAIL");
    }
}
