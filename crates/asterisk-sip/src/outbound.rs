//! SIP Outbound support — RFC 5626.
//!
//! Implements the SIP Outbound mechanism for registrations through NATs.
//! Key features:
//! - `Supported: outbound` in REGISTER
//! - `+sip.instance` and `reg-id` in Contact
//! - Flow-based registration (connection reuse)
//! - Flow recovery on connection failure

use crate::parser::{header_names, SipHeader, SipMessage};

/// Configuration for SIP Outbound (RFC 5626).
#[derive(Debug, Clone)]
pub struct OutboundConfig {
    /// Registration ID for this flow (unique per registration binding).
    pub reg_id: u32,
    /// Instance ID in `urn:uuid:<uuid>` format.
    pub instance_id: String,
    /// Whether the registrar supports outbound.
    pub ob_supported: bool,
}

impl OutboundConfig {
    /// Create a new outbound config with the given instance ID.
    pub fn new(instance_id: String) -> Self {
        Self {
            reg_id: 1,
            instance_id,
            ob_supported: false,
        }
    }
}

/// Build a Contact header value with outbound parameters.
///
/// Adds `+sip.instance` and `reg-id` parameters to the Contact header
/// for use in REGISTER requests when SIP Outbound is enabled.
///
/// Example output:
/// `<sip:user@10.0.0.1:5060>;+sip.instance="<urn:uuid:...>";reg-id=1`
pub fn build_outbound_contact(contact_uri: &str, config: &OutboundConfig) -> SipHeader {
    let value = format!(
        "<{}>;+sip.instance=\"<{}>\";reg-id={}",
        contact_uri, config.instance_id, config.reg_id
    );
    SipHeader {
        name: header_names::CONTACT.to_string(),
        value,
    }
}

/// Build the Supported header including `outbound`.
pub fn build_supported_outbound() -> SipHeader {
    SipHeader {
        name: header_names::SUPPORTED.to_string(),
        value: "outbound".to_string(),
    }
}

/// Check if a response requires outbound support.
///
/// Looks for `Require: outbound` in the response headers.
pub fn requires_outbound(msg: &SipMessage) -> bool {
    msg.get_headers(header_names::REQUIRE)
        .iter()
        .any(|v| v.split(',').any(|tok| tok.trim().eq_ignore_ascii_case("outbound")))
}

/// Check if the remote side supports outbound.
///
/// Looks for `outbound` in the Supported header of a response.
pub fn supports_outbound(msg: &SipMessage) -> bool {
    msg.get_headers(header_names::SUPPORTED)
        .iter()
        .any(|v| v.split(',').any(|tok| tok.trim().eq_ignore_ascii_case("outbound")))
}

/// Extract the Flow-Timer value from a 200 OK response to REGISTER.
///
/// The `Flow-Timer` header indicates the recommended keep-alive interval
/// in seconds. Per RFC 5626, the client should send keep-alives at a rate
/// somewhat faster than this value.
pub fn extract_flow_timer(msg: &SipMessage) -> Option<u32> {
    msg.get_header("Flow-Timer")
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// Flow state for connection-oriented registrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowState {
    /// Flow is active and healthy.
    Active,
    /// Flow is suspected to be broken (keepalive timeout).
    Suspect,
    /// Flow has failed; re-registration needed.
    Failed,
}

/// Tracks the state of an outbound registration flow.
#[derive(Debug, Clone)]
pub struct OutboundFlow {
    /// Current flow state.
    pub state: FlowState,
    /// The reg-id associated with this flow.
    pub reg_id: u32,
    /// Instance ID for this flow.
    pub instance_id: String,
    /// Flow-Timer value from registrar (seconds).
    pub flow_timer: Option<u32>,
}

impl OutboundFlow {
    /// Create a new outbound flow.
    pub fn new(config: &OutboundConfig) -> Self {
        Self {
            state: FlowState::Active,
            reg_id: config.reg_id,
            instance_id: config.instance_id.clone(),
            flow_timer: None,
        }
    }

    /// Mark the flow as suspect (keepalive missed).
    pub fn mark_suspect(&mut self) {
        if self.state == FlowState::Active {
            self.state = FlowState::Suspect;
        }
    }

    /// Mark the flow as failed (connection lost or too many missed keepalives).
    pub fn mark_failed(&mut self) {
        self.state = FlowState::Failed;
    }

    /// Mark the flow as recovered (keepalive response received).
    pub fn mark_active(&mut self) {
        self.state = FlowState::Active;
    }

    /// Check if the flow needs re-registration.
    pub fn needs_reregistration(&self) -> bool {
        self.state == FlowState::Failed
    }

    /// Update the flow timer from a registration response.
    pub fn update_flow_timer(&mut self, msg: &SipMessage) {
        self.flow_timer = extract_flow_timer(msg);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_outbound_contact() {
        let config = OutboundConfig::new("urn:uuid:abc-123".to_string());
        let header = build_outbound_contact("sip:user@10.0.0.1:5060", &config);
        assert!(header.value.contains("+sip.instance"));
        assert!(header.value.contains("reg-id=1"));
        assert!(header.value.contains("urn:uuid:abc-123"));
    }

    #[test]
    fn test_requires_outbound() {
        let msg = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>;tag=def\r\n\
              Call-ID: ob-test\r\n\
              CSeq: 1 REGISTER\r\n\
              Require: outbound\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();
        assert!(requires_outbound(&msg));
    }

    #[test]
    fn test_supports_outbound() {
        let msg = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>;tag=def\r\n\
              Call-ID: ob-test\r\n\
              CSeq: 1 REGISTER\r\n\
              Supported: outbound, path\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();
        assert!(supports_outbound(&msg));
    }

    #[test]
    fn test_extract_flow_timer() {
        let msg = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>;tag=def\r\n\
              Call-ID: flow-test\r\n\
              CSeq: 1 REGISTER\r\n\
              Flow-Timer: 120\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();
        assert_eq!(extract_flow_timer(&msg), Some(120));
    }

    #[test]
    fn test_outbound_flow_lifecycle() {
        let config = OutboundConfig::new("urn:uuid:test".to_string());
        let mut flow = OutboundFlow::new(&config);
        assert_eq!(flow.state, FlowState::Active);
        assert!(!flow.needs_reregistration());

        flow.mark_suspect();
        assert_eq!(flow.state, FlowState::Suspect);

        flow.mark_failed();
        assert_eq!(flow.state, FlowState::Failed);
        assert!(flow.needs_reregistration());

        flow.mark_active();
        assert_eq!(flow.state, FlowState::Active);
    }
}
