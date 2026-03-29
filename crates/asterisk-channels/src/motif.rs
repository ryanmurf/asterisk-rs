//! Motif/Jingle channel driver (stub).
//!
//! Port of chan_motif.c from Asterisk C.
//!
//! Provides an XMPP Jingle session channel driver for Google Talk
//! and standard Jingle (XEP-0166) voice calls. This is a stub
//! implementation providing the session framework types.

use std::fmt;

use async_trait::async_trait;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

/// Jingle session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JingleSessionState {
    /// Session initiate sent/received
    Pending,
    /// Session accepted
    Active,
    /// Session being terminated
    Terminating,
    /// Session ended
    Ended,
}

/// Jingle transport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JingleTransport {
    /// ICE-UDP transport (XEP-0176)
    IceUdp,
    /// Raw UDP transport (XEP-0177)
    RawUdp,
    /// Google transport-info (legacy)
    GoogleV1,
}

impl JingleTransport {
    pub fn namespace(&self) -> &'static str {
        match self {
            Self::IceUdp => "urn:xmpp:jingle:transports:ice-udp:1",
            Self::RawUdp => "urn:xmpp:jingle:transports:raw-udp:1",
            Self::GoogleV1 => "http://www.google.com/transport/p2p",
        }
    }
}

/// Jingle session description.
#[derive(Debug, Clone)]
pub struct JingleSession {
    /// Session ID (SID)
    pub sid: String,
    /// Initiator JID
    pub initiator: String,
    /// Responder JID
    pub responder: String,
    /// Session state
    pub state: JingleSessionState,
    /// Transport type
    pub transport: JingleTransport,
}

/// Jingle action types (XEP-0166).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JingleAction {
    SessionInitiate,
    SessionAccept,
    SessionTerminate,
    SessionInfo,
    ContentAdd,
    ContentModify,
    ContentRemove,
    TransportInfo,
    TransportAccept,
    TransportReject,
    TransportReplace,
}

impl JingleAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionInitiate => "session-initiate",
            Self::SessionAccept => "session-accept",
            Self::SessionTerminate => "session-terminate",
            Self::SessionInfo => "session-info",
            Self::ContentAdd => "content-add",
            Self::ContentModify => "content-modify",
            Self::ContentRemove => "content-remove",
            Self::TransportInfo => "transport-info",
            Self::TransportAccept => "transport-accept",
            Self::TransportReject => "transport-reject",
            Self::TransportReplace => "transport-replace",
        }
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "session-initiate" => Some(Self::SessionInitiate),
            "session-accept" => Some(Self::SessionAccept),
            "session-terminate" => Some(Self::SessionTerminate),
            "session-info" => Some(Self::SessionInfo),
            "content-add" => Some(Self::ContentAdd),
            "content-modify" => Some(Self::ContentModify),
            "content-remove" => Some(Self::ContentRemove),
            "transport-info" => Some(Self::TransportInfo),
            "transport-accept" => Some(Self::TransportAccept),
            "transport-reject" => Some(Self::TransportReject),
            "transport-replace" => Some(Self::TransportReplace),
            _ => None,
        }
    }
}

/// Session termination reasons (XEP-0166).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateReason {
    Busy,
    Cancel,
    Decline,
    Expired,
    GeneralError,
    Gone,
    Success,
    Timeout,
    UnsupportedTransports,
}

impl TerminateReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Cancel => "cancel",
            Self::Decline => "decline",
            Self::Expired => "expired",
            Self::GeneralError => "general-error",
            Self::Gone => "gone",
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::UnsupportedTransports => "unsupported-transports",
        }
    }
}

/// Motif/Jingle channel driver.
///
/// Stub implementation for XMPP Jingle voice calls.
pub struct MotifDriver;

impl fmt::Debug for MotifDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MotifDriver").finish()
    }
}

impl MotifDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MotifDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for MotifDriver {
    fn name(&self) -> &str {
        "Motif"
    }

    fn description(&self) -> &str {
        "Motif Jingle Channel Driver (XMPP/Jingle)"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        // dest format: "endpoint/jid"
        let chan_name = format!("Motif/{}", dest);
        let channel = Channel::new(chan_name);
        info!(dest, "Motif/Jingle channel created (stub)");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        // Would send session-initiate Jingle IQ
        info!(channel = %channel.name, "Motif channel call (stub)");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        // Would send session-accept Jingle IQ
        channel.answer();
        info!(channel = %channel.name, "Motif channel answered (stub)");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        // Would send session-terminate Jingle IQ
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "Motif channel hungup (stub)");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        Err(AsteriskError::NotSupported("Motif read_frame stub".into()))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jingle_actions() {
        assert_eq!(JingleAction::SessionInitiate.as_str(), "session-initiate");
        assert_eq!(
            JingleAction::from_str_name("session-accept"),
            Some(JingleAction::SessionAccept)
        );
        assert_eq!(JingleAction::from_str_name("bogus"), None);
    }

    #[test]
    fn test_transport_namespace() {
        assert!(JingleTransport::IceUdp.namespace().contains("ice-udp"));
    }

    #[test]
    fn test_terminate_reasons() {
        assert_eq!(TerminateReason::Busy.as_str(), "busy");
        assert_eq!(TerminateReason::Success.as_str(), "success");
    }

    #[test]
    fn test_driver_name() {
        let driver = MotifDriver::new();
        assert_eq!(driver.name(), "Motif");
    }
}
