//! MGCP channel driver - Media Gateway Control Protocol.
//!
//! Port of chan_mgcp.c from Asterisk C.
//!
//! MGCP (RFC 3435) uses a centralized call-control model where a Call Agent
//! (Asterisk) controls media gateways. The protocol uses UDP text messages
//! for signaling. Key concepts:
//!
//! - Endpoint: a logical entity on a gateway (e.g., "aaln/1@gw.example.com")
//! - Connection: an RTP media path associated with an endpoint
//! - Commands: CRCX (create), MDCX (modify), DLCX (delete), RQNT (request notify), NTFY (notify)

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

// ---------------------------------------------------------------------------
// MGCP protocol constants
// ---------------------------------------------------------------------------

/// Default MGCP Call Agent port.
pub const MGCP_CA_PORT: u16 = 2727;

/// Default MGCP Gateway port.
pub const MGCP_GW_PORT: u16 = 2427;

/// MGCP protocol version.
pub const MGCP_VERSION: &str = "MGCP 1.0";

// ---------------------------------------------------------------------------
// MGCP commands
// ---------------------------------------------------------------------------

/// MGCP command/verb types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgcpCommand {
    /// Create Connection - establishes an RTP stream
    Crcx,
    /// Modify Connection - changes connection parameters
    Mdcx,
    /// Delete Connection - tears down an RTP stream
    Dlcx,
    /// Request Notification - ask gateway to watch for events
    Rqnt,
    /// Notify - gateway reports observed events
    Ntfy,
    /// Audit Endpoint - query endpoint capabilities
    Auep,
    /// Audit Connection - query connection state
    Aucx,
    /// Restart In Progress - gateway restarting
    Rsip,
    /// Endpoint Configuration - codec/endpoint setup
    Epcf,
}

impl MgcpCommand {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Crcx => "CRCX",
            Self::Mdcx => "MDCX",
            Self::Dlcx => "DLCX",
            Self::Rqnt => "RQNT",
            Self::Ntfy => "NTFY",
            Self::Auep => "AUEP",
            Self::Aucx => "AUCX",
            Self::Rsip => "RSIP",
            Self::Epcf => "EPCF",
        }
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "CRCX" => Some(Self::Crcx),
            "MDCX" => Some(Self::Mdcx),
            "DLCX" => Some(Self::Dlcx),
            "RQNT" => Some(Self::Rqnt),
            "NTFY" => Some(Self::Ntfy),
            "AUEP" => Some(Self::Auep),
            "AUCX" => Some(Self::Aucx),
            "RSIP" => Some(Self::Rsip),
            "EPCF" => Some(Self::Epcf),
            _ => None,
        }
    }
}

/// MGCP response codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgcpResponseCode {
    /// Transaction being executed (provisional)
    Trying = 100,
    /// Transaction executed normally
    Ok = 200,
    /// Connection was deleted
    ConnectionDeleted = 250,
    /// Transient error
    TransientError = 400,
    /// Endpoint not ready
    EndpointNotReady = 401,
    /// Insufficient resources
    InsufficientResources = 403,
    /// Endpoint unknown
    EndpointUnknown = 500,
    /// Endpoint not available
    EndpointNotAvailable = 501,
    /// No connection for that connection ID
    NoConnection = 515,
    /// Protocol error
    ProtocolError = 510,
    /// Unsupported mode
    UnsupportedMode = 517,
    /// Internal inconsistency
    InternalError = 520,
}

/// MGCP connection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgcpConnectionMode {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
    Conference,
}

impl MgcpConnectionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SendRecv => "sendrecv",
            Self::SendOnly => "sendonly",
            Self::RecvOnly => "recvonly",
            Self::Inactive => "inactive",
            Self::Conference => "confrnce",
        }
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sendrecv" => Some(Self::SendRecv),
            "sendonly" => Some(Self::SendOnly),
            "recvonly" => Some(Self::RecvOnly),
            "inactive" => Some(Self::Inactive),
            "confrnce" | "conference" => Some(Self::Conference),
            _ => None,
        }
    }
}

/// An MGCP message (request or response).
#[derive(Debug, Clone)]
pub struct MgcpMessage {
    /// Transaction ID
    pub transaction_id: u32,
    /// Command verb (for requests)
    pub command: Option<MgcpCommand>,
    /// Response code (for responses)
    pub response_code: Option<u32>,
    /// Endpoint identifier
    pub endpoint: String,
    /// Connection ID (if applicable)
    pub connection_id: Option<String>,
    /// Call ID
    pub call_id: Option<String>,
    /// Local connection options
    pub local_options: Option<String>,
    /// Connection mode
    pub mode: Option<MgcpConnectionMode>,
    /// SDP body
    pub sdp: Option<String>,
    /// Request/notification events (e.g., "L/hd", "D/[0-9#*]")
    pub requested_events: Option<String>,
    /// Observed events for NTFY
    pub observed_events: Option<String>,
    /// Signal requests (e.g., "L/rg" for ring)
    pub signal_requests: Option<String>,
}

impl MgcpMessage {
    /// Format as a MGCP request line.
    pub fn format_request(&self) -> String {
        let cmd = self.command.map(|c| c.as_str()).unwrap_or("UNKNOWN");
        let mut msg = format!("{} {} {} {}\r\n", cmd, self.transaction_id, self.endpoint, MGCP_VERSION);

        if let Some(ref call_id) = self.call_id {
            msg.push_str(&format!("C: {}\r\n", call_id));
        }
        if let Some(ref conn_id) = self.connection_id {
            msg.push_str(&format!("I: {}\r\n", conn_id));
        }
        if let Some(ref mode) = self.mode {
            msg.push_str(&format!("M: {}\r\n", mode.as_str()));
        }
        if let Some(ref events) = self.requested_events {
            msg.push_str(&format!("R: {}\r\n", events));
        }
        if let Some(ref signals) = self.signal_requests {
            msg.push_str(&format!("S: {}\r\n", signals));
        }
        if let Some(ref sdp) = self.sdp {
            msg.push_str("\r\n");
            msg.push_str(sdp);
        }
        msg
    }

    /// Create a CRCX (Create Connection) request.
    pub fn crcx(transaction_id: u32, endpoint: &str, call_id: &str, mode: MgcpConnectionMode) -> Self {
        Self {
            transaction_id,
            command: Some(MgcpCommand::Crcx),
            response_code: None,
            endpoint: endpoint.to_string(),
            connection_id: None,
            call_id: Some(call_id.to_string()),
            local_options: None,
            mode: Some(mode),
            sdp: None,
            requested_events: None,
            observed_events: None,
            signal_requests: None,
        }
    }

    /// Create a DLCX (Delete Connection) request.
    pub fn dlcx(transaction_id: u32, endpoint: &str, connection_id: &str) -> Self {
        Self {
            transaction_id,
            command: Some(MgcpCommand::Dlcx),
            response_code: None,
            endpoint: endpoint.to_string(),
            connection_id: Some(connection_id.to_string()),
            call_id: None,
            local_options: None,
            mode: None,
            sdp: None,
            requested_events: None,
            observed_events: None,
            signal_requests: None,
        }
    }

    /// Create an RQNT (Request Notification) - ask gateway to detect events.
    pub fn rqnt(transaction_id: u32, endpoint: &str, events: &str, signal: Option<&str>) -> Self {
        Self {
            transaction_id,
            command: Some(MgcpCommand::Rqnt),
            response_code: None,
            endpoint: endpoint.to_string(),
            connection_id: None,
            call_id: None,
            local_options: None,
            mode: None,
            sdp: None,
            requested_events: Some(events.to_string()),
            observed_events: None,
            signal_requests: signal.map(|s| s.to_string()),
        }
    }
}

/// MGCP endpoint state.
#[derive(Debug, Clone)]
pub struct MgcpEndpoint {
    /// Endpoint name (e.g., "aaln/1@gateway.example.com")
    pub name: String,
    /// Gateway address
    pub gateway_addr: Option<SocketAddr>,
    /// Active connection ID
    pub connection_id: Option<String>,
    /// Current call ID
    pub call_id: Option<String>,
    /// Is the endpoint in use?
    pub in_use: bool,
}

/// MGCP channel driver.
pub struct MgcpDriver {
    endpoints: RwLock<HashMap<String, MgcpEndpoint>>,
    next_transaction_id: AtomicU32,
}

impl fmt::Debug for MgcpDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MgcpDriver")
            .field("endpoints", &self.endpoints.read().len())
            .finish()
    }
}

impl MgcpDriver {
    pub fn new() -> Self {
        Self {
            endpoints: RwLock::new(HashMap::new()),
            next_transaction_id: AtomicU32::new(1),
        }
    }

    /// Register an endpoint.
    pub fn add_endpoint(&self, name: &str, gateway_addr: Option<SocketAddr>) {
        self.endpoints.write().insert(
            name.to_string(),
            MgcpEndpoint {
                name: name.to_string(),
                gateway_addr,
                connection_id: None,
                call_id: None,
                in_use: false,
            },
        );
    }

    /// Get the next transaction ID.
    pub fn next_txn_id(&self) -> u32 {
        self.next_transaction_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for MgcpDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for MgcpDriver {
    fn name(&self) -> &str {
        "MGCP"
    }

    fn description(&self) -> &str {
        "Media Gateway Control Protocol Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let chan_name = format!("MGCP/{}", dest);
        let channel = Channel::new(chan_name);
        info!(endpoint = dest, "MGCP channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        // Would send RQNT with ring signal
        info!(channel = %channel.name, "MGCP channel ringing");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        info!(channel = %channel.name, "MGCP channel answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        // Would send DLCX to tear down connection
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "MGCP channel hungup");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        Err(AsteriskError::NotSupported("MGCP read_frame stub".into()))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mgcp_commands() {
        assert_eq!(MgcpCommand::Crcx.as_str(), "CRCX");
        assert_eq!(MgcpCommand::from_str_name("DLCX"), Some(MgcpCommand::Dlcx));
        assert_eq!(MgcpCommand::from_str_name("bogus"), None);
    }

    #[test]
    fn test_connection_mode() {
        assert_eq!(MgcpConnectionMode::SendRecv.as_str(), "sendrecv");
        assert_eq!(
            MgcpConnectionMode::from_str_name("recvonly"),
            Some(MgcpConnectionMode::RecvOnly)
        );
    }

    #[test]
    fn test_crcx_message() {
        let msg = MgcpMessage::crcx(1, "aaln/1@gw.test", "C001", MgcpConnectionMode::SendRecv);
        let formatted = msg.format_request();
        assert!(formatted.starts_with("CRCX 1"));
        assert!(formatted.contains("aaln/1@gw.test"));
        assert!(formatted.contains("M: sendrecv"));
        assert!(formatted.contains("C: C001"));
    }

    #[test]
    fn test_dlcx_message() {
        let msg = MgcpMessage::dlcx(2, "aaln/1@gw.test", "I001");
        let formatted = msg.format_request();
        assert!(formatted.starts_with("DLCX 2"));
        assert!(formatted.contains("I: I001"));
    }

    #[test]
    fn test_rqnt_message() {
        let msg = MgcpMessage::rqnt(3, "aaln/1@gw.test", "L/hd(N)", Some("L/rg"));
        let formatted = msg.format_request();
        assert!(formatted.starts_with("RQNT 3"));
        assert!(formatted.contains("R: L/hd(N)"));
        assert!(formatted.contains("S: L/rg"));
    }

    #[test]
    fn test_endpoint_management() {
        let driver = MgcpDriver::new();
        driver.add_endpoint("aaln/1@gw.test", None);
        assert!(driver.endpoints.read().contains_key("aaln/1@gw.test"));
    }

    #[test]
    fn test_transaction_ids() {
        let driver = MgcpDriver::new();
        let id1 = driver.next_txn_id();
        let id2 = driver.next_txn_id();
        assert_eq!(id2, id1 + 1);
    }
}
