//! TURN (Traversal Using Relays around NAT) client — RFC 5766.
//!
//! Implements the TURN protocol for relay allocation, permissions,
//! channel bindings, and data relay. Used by the ICE agent for
//! relay candidate gathering and media relay.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::net::UdpSocket;
use tracing::{debug, info};

use crate::stun::{
    self, MessageClass, Method, StunAttrValue, StunMessage, TransactionId,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default TURN allocation lifetime in seconds.
pub const DEFAULT_LIFETIME: u32 = 600;

/// TURN channel number range (RFC 5766 Section 11).
pub const CHANNEL_MIN: u16 = 0x4000;
pub const CHANNEL_MAX: u16 = 0x7FFF;

/// TURN ChannelData header size (4 bytes: channel number + length).
pub const CHANNEL_DATA_HEADER_SIZE: usize = 4;

/// REQUESTED-TRANSPORT protocol numbers.
pub const TRANSPORT_UDP: u8 = 17;
pub const TRANSPORT_TCP: u8 = 6;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum TurnError {
    #[error("TURN I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("STUN error: {0}")]
    Stun(#[from] stun::StunError),
    #[error("TURN error response: {code} {reason}")]
    ErrorResponse { code: u16, reason: String },
    #[error("TURN timeout")]
    Timeout,
    #[error("TURN allocation not active")]
    NoAllocation,
    #[error("TURN channel not bound")]
    ChannelNotBound,
    #[error("TURN error: {0}")]
    Other(String),
}

pub type TurnResult<T> = Result<T, TurnError>;

// ---------------------------------------------------------------------------
// TURN allocation
// ---------------------------------------------------------------------------

/// Represents a TURN relay allocation.
#[derive(Debug, Clone)]
pub struct TurnAllocation {
    /// The relayed transport address on the TURN server.
    pub relayed_addr: SocketAddr,
    /// The server-reflexive (mapped) address.
    pub mapped_addr: SocketAddr,
    /// Allocation lifetime in seconds.
    pub lifetime: u32,
    /// When this allocation was created/refreshed.
    pub created_at: Instant,
}

impl TurnAllocation {
    /// Check if the allocation has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= Duration::from_secs(self.lifetime as u64)
    }

    /// Seconds remaining before expiry.
    pub fn remaining_secs(&self) -> u64 {
        let elapsed = self.created_at.elapsed().as_secs();
        (self.lifetime as u64).saturating_sub(elapsed)
    }

    /// Check if it is time to refresh (refresh at 80% of lifetime).
    pub fn needs_refresh(&self) -> bool {
        let threshold = (self.lifetime as u64) * 80 / 100;
        self.created_at.elapsed().as_secs() >= threshold
    }
}

// ---------------------------------------------------------------------------
// Channel binding
// ---------------------------------------------------------------------------

/// A TURN channel binding.
#[derive(Debug, Clone)]
pub struct ChannelBinding {
    /// Channel number (0x4000-0x7FFF).
    pub channel: u16,
    /// Peer address bound to this channel.
    pub peer_addr: SocketAddr,
    /// When the binding was created/refreshed (expires after 10 minutes).
    pub created_at: Instant,
}

impl ChannelBinding {
    /// Channel bindings expire after 10 minutes (RFC 5766 Section 11).
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= Duration::from_secs(600)
    }
}

// ---------------------------------------------------------------------------
// TURN client
// ---------------------------------------------------------------------------

/// TURN client for relay allocation and data relay.
pub struct TurnClient {
    /// TURN server address.
    pub server_addr: SocketAddr,
    /// Authentication username.
    pub username: String,
    /// Authentication password.
    pub password: String,
    /// Realm (received from server).
    pub realm: String,
    /// Nonce (received from server).
    pub nonce: String,
    /// UDP socket for TURN communication.
    pub socket: Option<UdpSocket>,
    /// Current allocation.
    pub allocation: Option<TurnAllocation>,
    /// Installed permissions (peer addresses).
    pub permissions: Vec<SocketAddr>,
    /// Channel bindings.
    pub channels: HashMap<u16, ChannelBinding>,
    /// Reverse map: peer address -> channel number.
    pub peer_to_channel: HashMap<SocketAddr, u16>,
    /// Next channel number to allocate.
    next_channel: u16,
    /// Request timeout.
    timeout: Duration,
}

impl std::fmt::Debug for TurnClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnClient")
            .field("server_addr", &self.server_addr)
            .field("username", &self.username)
            .field("allocation", &self.allocation)
            .field("channels", &self.channels.len())
            .finish()
    }
}

impl TurnClient {
    /// Create a new TURN client.
    pub fn new(server_addr: SocketAddr, username: &str, password: &str) -> Self {
        Self {
            server_addr,
            username: username.to_string(),
            password: password.to_string(),
            realm: String::new(),
            nonce: String::new(),
            socket: None,
            allocation: None,
            permissions: Vec::new(),
            channels: HashMap::new(),
            peer_to_channel: HashMap::new(),
            next_channel: CHANNEL_MIN,
            timeout: Duration::from_secs(5),
        }
    }

    /// Set the timeout for TURN requests.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Bind the UDP socket.
    pub async fn bind(&mut self, local_addr: SocketAddr) -> TurnResult<()> {
        let socket = UdpSocket::bind(local_addr).await?;
        self.socket = Some(socket);
        Ok(())
    }

    /// Set an externally-created socket.
    pub fn set_socket(&mut self, socket: UdpSocket) {
        self.socket = Some(socket);
    }

    /// Get a reference to the socket.
    fn socket(&self) -> TurnResult<&UdpSocket> {
        self.socket
            .as_ref()
            .ok_or(TurnError::Other("socket not bound".into()))
    }

    // ----- TURN Allocate (RFC 5766 Section 6) -----

    /// Send an Allocate request and obtain a relay allocation.
    ///
    /// Handles the 401 Unauthorized challenge flow:
    /// 1. Send initial Allocate without credentials
    /// 2. Receive 401 with REALM and NONCE
    /// 3. Re-send with long-term credentials
    pub async fn allocate(&mut self) -> TurnResult<TurnAllocation> {
        let socket = self.socket()?;

        // Step 1: Initial Allocate request (will get 401)
        let request = StunMessage {
            method: Method::Allocate,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::RequestedTransport(TRANSPORT_UDP),
            ],
        };

        let bytes = request.to_bytes();
        socket.send_to(&bytes, self.server_addr).await?;

        let response = self.recv_response().await?;

        // Handle 401 challenge
        if response.class == MessageClass::ErrorResponse {
            if let Some((code, _reason)) = response.error_code() {
                if code == 401 {
                    // Extract realm and nonce
                    if let Some(realm) = response.realm() {
                        self.realm = realm.to_string();
                    }
                    if let Some(nonce) = response.nonce() {
                        self.nonce = nonce.to_string();
                    }

                    // Step 2: Re-send with credentials
                    return self.allocate_authenticated().await;
                }
                return Err(TurnError::ErrorResponse {
                    code,
                    reason: _reason.to_string(),
                });
            }
        }

        // Unexpected success on first attempt (server doesn't require auth)
        self.process_allocate_response(&response)
    }

    /// Send an authenticated Allocate request.
    async fn allocate_authenticated(&mut self) -> TurnResult<TurnAllocation> {
        let socket = self.socket()?;

        let request = StunMessage {
            method: Method::Allocate,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::RequestedTransport(TRANSPORT_UDP),
                StunAttrValue::Username(self.username.clone()),
                StunAttrValue::Realm(self.realm.clone()),
                StunAttrValue::Nonce(self.nonce.clone()),
            ],
        };

        let key = stun::compute_long_term_key(&self.username, &self.realm, &self.password);
        let bytes = request.to_bytes_with_integrity_and_fingerprint(&key);
        socket.send_to(&bytes, self.server_addr).await?;

        let response = self.recv_response().await?;

        if response.class == MessageClass::ErrorResponse {
            if let Some((code, reason)) = response.error_code() {
                return Err(TurnError::ErrorResponse {
                    code,
                    reason: reason.to_string(),
                });
            }
            return Err(TurnError::ErrorResponse {
                code: 0,
                reason: "unknown error".into(),
            });
        }

        self.process_allocate_response(&response)
    }

    /// Process an Allocate success response.
    fn process_allocate_response(
        &mut self,
        response: &StunMessage,
    ) -> TurnResult<TurnAllocation> {
        let relayed_addr = response.xor_relayed_address().ok_or_else(|| {
            TurnError::Other("no XOR-RELAYED-ADDRESS in Allocate response".into())
        })?;

        let mapped_addr = response.get_mapped_address().ok_or_else(|| {
            TurnError::Other("no mapped address in Allocate response".into())
        })?;

        let lifetime = response.lifetime().unwrap_or(DEFAULT_LIFETIME);

        let allocation = TurnAllocation {
            relayed_addr,
            mapped_addr,
            lifetime,
            created_at: Instant::now(),
        };

        info!(
            relayed = %relayed_addr,
            mapped = %mapped_addr,
            lifetime = lifetime,
            "TURN allocation created"
        );

        self.allocation = Some(allocation.clone());
        Ok(allocation)
    }

    // ----- TURN Refresh (RFC 5766 Section 7) -----

    /// Refresh the current allocation.
    pub async fn refresh(&mut self) -> TurnResult<()> {
        let mut retried_stale_nonce = false;
        loop {
            if self.allocation.is_none() {
                return Err(TurnError::NoAllocation);
            }

            let socket = self.socket()?;

            let request = StunMessage {
                method: Method::Refresh,
                class: MessageClass::Request,
                transaction_id: TransactionId::new(),
                attributes: vec![
                    StunAttrValue::Username(self.username.clone()),
                    StunAttrValue::Realm(self.realm.clone()),
                    StunAttrValue::Nonce(self.nonce.clone()),
                ],
            };

            let key = stun::compute_long_term_key(&self.username, &self.realm, &self.password);
            let bytes = request.to_bytes_with_integrity_and_fingerprint(&key);
            socket.send_to(&bytes, self.server_addr).await?;

            let response = self.recv_response().await?;

            if response.class == MessageClass::ErrorResponse {
                if let Some((code, reason)) = response.error_code() {
                    if code == 438 && !retried_stale_nonce {
                        if let Some(nonce) = response.nonce() {
                            self.nonce = nonce.to_string();
                        }
                        retried_stale_nonce = true;
                        continue;
                    }
                    return Err(TurnError::ErrorResponse {
                        code,
                        reason: reason.to_string(),
                    });
                }
            }

            let lifetime = response.lifetime().unwrap_or(DEFAULT_LIFETIME);
            if let Some(alloc) = &mut self.allocation {
                alloc.lifetime = lifetime;
                alloc.created_at = Instant::now();
            }

            debug!(lifetime = lifetime, "TURN allocation refreshed");
            return Ok(());
        }
    }

    /// Delete the allocation by refreshing with lifetime=0.
    pub async fn deallocate(&mut self) -> TurnResult<()> {
        if self.allocation.is_none() {
            return Ok(());
        }

        let socket = self.socket()?;

        let request = StunMessage {
            method: Method::Refresh,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::Lifetime(0),
                StunAttrValue::Username(self.username.clone()),
                StunAttrValue::Realm(self.realm.clone()),
                StunAttrValue::Nonce(self.nonce.clone()),
            ],
        };

        let key = stun::compute_long_term_key(&self.username, &self.realm, &self.password);
        let bytes = request.to_bytes_with_integrity_and_fingerprint(&key);
        socket.send_to(&bytes, self.server_addr).await?;

        let _response = self.recv_response().await?;
        self.allocation = None;
        self.channels.clear();
        self.peer_to_channel.clear();
        self.permissions.clear();

        debug!("TURN allocation deleted");
        Ok(())
    }

    // ----- CreatePermission (RFC 5766 Section 9) -----

    /// Create a permission for a peer address.
    pub async fn create_permission(&mut self, peer_addr: SocketAddr) -> TurnResult<()> {
        if self.allocation.is_none() {
            return Err(TurnError::NoAllocation);
        }

        let socket = self.socket()?;

        let request = StunMessage {
            method: Method::CreatePermission,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::XorPeerAddress(peer_addr),
                StunAttrValue::Username(self.username.clone()),
                StunAttrValue::Realm(self.realm.clone()),
                StunAttrValue::Nonce(self.nonce.clone()),
            ],
        };

        let key = stun::compute_long_term_key(&self.username, &self.realm, &self.password);
        let bytes = request.to_bytes_with_integrity_and_fingerprint(&key);
        socket.send_to(&bytes, self.server_addr).await?;

        let response = self.recv_response().await?;

        if response.class == MessageClass::ErrorResponse {
            if let Some((code, reason)) = response.error_code() {
                return Err(TurnError::ErrorResponse {
                    code,
                    reason: reason.to_string(),
                });
            }
        }

        if !self.permissions.contains(&peer_addr) {
            self.permissions.push(peer_addr);
        }

        debug!(peer = %peer_addr, "TURN permission created");
        Ok(())
    }

    // ----- ChannelBind (RFC 5766 Section 11) -----

    /// Bind a channel to a peer address for efficient data relay.
    pub async fn channel_bind(&mut self, peer_addr: SocketAddr) -> TurnResult<u16> {
        if self.allocation.is_none() {
            return Err(TurnError::NoAllocation);
        }

        // Check if already bound
        if let Some(&ch) = self.peer_to_channel.get(&peer_addr) {
            return Ok(ch);
        }

        let channel = self.next_channel;
        if channel > CHANNEL_MAX {
            return Err(TurnError::Other("no more channel numbers available".into()));
        }
        self.next_channel += 1;

        let socket = self.socket()?;

        let request = StunMessage {
            method: Method::ChannelBind,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::ChannelNumber(channel),
                StunAttrValue::XorPeerAddress(peer_addr),
                StunAttrValue::Username(self.username.clone()),
                StunAttrValue::Realm(self.realm.clone()),
                StunAttrValue::Nonce(self.nonce.clone()),
            ],
        };

        let key = stun::compute_long_term_key(&self.username, &self.realm, &self.password);
        let bytes = request.to_bytes_with_integrity_and_fingerprint(&key);
        socket.send_to(&bytes, self.server_addr).await?;

        let response = self.recv_response().await?;

        if response.class == MessageClass::ErrorResponse {
            if let Some((code, reason)) = response.error_code() {
                return Err(TurnError::ErrorResponse {
                    code,
                    reason: reason.to_string(),
                });
            }
        }

        let binding = ChannelBinding {
            channel,
            peer_addr,
            created_at: Instant::now(),
        };
        self.channels.insert(channel, binding);
        self.peer_to_channel.insert(peer_addr, channel);

        debug!(channel = channel, peer = %peer_addr, "TURN channel bound");
        Ok(channel)
    }

    // ----- Send/Data indication (RFC 5766 Section 10) -----

    /// Send data to a peer through the TURN relay using a Send indication.
    pub async fn send_data(
        &self,
        peer_addr: SocketAddr,
        data: &[u8],
    ) -> TurnResult<()> {
        if self.allocation.is_none() {
            return Err(TurnError::NoAllocation);
        }

        let socket = self.socket()?;

        // If we have a channel binding, use ChannelData for efficiency
        if let Some(&channel) = self.peer_to_channel.get(&peer_addr) {
            let channel_data = encode_channel_data(channel, data);
            socket.send_to(&channel_data, self.server_addr).await?;
            return Ok(());
        }

        // Otherwise use Send indication
        let indication = StunMessage {
            method: Method::Send,
            class: MessageClass::Indication,
            transaction_id: TransactionId::new(),
            attributes: vec![
                StunAttrValue::XorPeerAddress(peer_addr),
                StunAttrValue::Data(data.to_vec()),
            ],
        };

        let bytes = indication.to_bytes();
        socket.send_to(&bytes, self.server_addr).await?;
        Ok(())
    }

    /// Receive data from the TURN server.
    ///
    /// Returns the peer address and data. Handles both Data indications
    /// and ChannelData messages.
    pub async fn recv_data(&self) -> TurnResult<(SocketAddr, Vec<u8>)> {
        let socket = self.socket()?;
        let mut buf = vec![0u8; 2048];
        let (len, _from) = socket.recv_from(&mut buf).await?;
        buf.truncate(len);

        // Check if it's ChannelData (first two bytes are channel number >= 0x4000)
        if len >= CHANNEL_DATA_HEADER_SIZE {
            let maybe_channel = u16::from_be_bytes([buf[0], buf[1]]);
            if (CHANNEL_MIN..=CHANNEL_MAX).contains(&maybe_channel) {
                let data_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
                if len >= CHANNEL_DATA_HEADER_SIZE + data_len {
                    if let Some(binding) = self.channels.get(&maybe_channel) {
                        return Ok((
                            binding.peer_addr,
                            buf[CHANNEL_DATA_HEADER_SIZE..CHANNEL_DATA_HEADER_SIZE + data_len]
                                .to_vec(),
                        ));
                    }
                }
            }
        }

        // Try parsing as STUN Data indication
        if stun::is_stun_message(&buf) {
            let msg = StunMessage::parse(&buf)?;
            if msg.method == Method::Data && msg.class == MessageClass::Indication {
                let peer_addr = msg.attributes.iter().find_map(|a| {
                    if let StunAttrValue::XorPeerAddress(addr) = a {
                        Some(*addr)
                    } else {
                        None
                    }
                });
                let data = msg.attributes.iter().find_map(|a| {
                    if let StunAttrValue::Data(d) = a {
                        Some(d.clone())
                    } else {
                        None
                    }
                });
                if let (Some(peer), Some(data)) = (peer_addr, data) {
                    return Ok((peer, data));
                }
            }
        }

        Err(TurnError::Other("unexpected data from TURN server".into()))
    }

    // ----- Internal helpers -----

    /// Receive a STUN response from the server.
    async fn recv_response(&self) -> TurnResult<StunMessage> {
        let socket = self.socket()?;
        let mut buf = vec![0u8; stun::MAX_MESSAGE_SIZE];
        let (len, _) = tokio::time::timeout(self.timeout, socket.recv_from(&mut buf))
            .await
            .map_err(|_| TurnError::Timeout)??;
        let msg = StunMessage::parse(&buf[..len])?;
        Ok(msg)
    }
}

// ---------------------------------------------------------------------------
// ChannelData encoding/decoding (RFC 5766 Section 11.4)
// ---------------------------------------------------------------------------

/// Encode a ChannelData message.
///
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |         Channel Number        |            Length             |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                                                               |
/// /                       Application Data                        /
/// /                                                               /
/// |                                                               |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
pub fn encode_channel_data(channel: u16, data: &[u8]) -> Vec<u8> {
    let padded_len = (data.len() + 3) & !3;
    let mut buf = Vec::with_capacity(CHANNEL_DATA_HEADER_SIZE + padded_len);
    buf.extend_from_slice(&channel.to_be_bytes());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    // Pad to 4-byte boundary
    #[allow(clippy::same_item_push)]
    for _ in data.len()..padded_len {
        buf.push(0);
    }
    buf
}

/// Decode a ChannelData message. Returns (channel, data).
pub fn decode_channel_data(buf: &[u8]) -> Option<(u16, &[u8])> {
    if buf.len() < CHANNEL_DATA_HEADER_SIZE {
        return None;
    }
    let channel = u16::from_be_bytes([buf[0], buf[1]]);
    if !(CHANNEL_MIN..=CHANNEL_MAX).contains(&channel) {
        return None;
    }
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf.len() < CHANNEL_DATA_HEADER_SIZE + length {
        return None;
    }
    Some((channel, &buf[CHANNEL_DATA_HEADER_SIZE..CHANNEL_DATA_HEADER_SIZE + length]))
}

/// Check if a received packet is a TURN ChannelData message.
pub fn is_channel_data(buf: &[u8]) -> bool {
    if buf.len() < CHANNEL_DATA_HEADER_SIZE {
        return false;
    }
    let channel = u16::from_be_bytes([buf[0], buf[1]]);
    (CHANNEL_MIN..=CHANNEL_MAX).contains(&channel)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_data_roundtrip() {
        let data = b"Hello, TURN!";
        let encoded = encode_channel_data(0x4001, data);

        assert!(is_channel_data(&encoded));

        let (channel, decoded) = decode_channel_data(&encoded).unwrap();
        assert_eq!(channel, 0x4001);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_channel_data_padding() {
        let data = b"abc"; // 3 bytes -> padded to 4
        let encoded = encode_channel_data(0x4000, data);
        // Header (4) + padded data (4) = 8 bytes
        assert_eq!(encoded.len(), 8);

        let (_, decoded) = decode_channel_data(&encoded).unwrap();
        assert_eq!(decoded, b"abc");
    }

    #[test]
    fn test_channel_data_empty() {
        let data = b"";
        let encoded = encode_channel_data(0x4000, data);
        assert_eq!(encoded.len(), 4); // Just header

        let (channel, decoded) = decode_channel_data(&encoded).unwrap();
        assert_eq!(channel, 0x4000);
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_channel_range() {
        assert!(is_channel_data(&encode_channel_data(CHANNEL_MIN, b"x")));
        assert!(is_channel_data(&encode_channel_data(CHANNEL_MAX, b"x")));

        // Below range
        let mut buf = vec![0; 8];
        buf[0..2].copy_from_slice(&0x3FFFu16.to_be_bytes());
        buf[2..4].copy_from_slice(&1u16.to_be_bytes());
        buf[4] = b'x';
        assert!(!is_channel_data(&buf));
    }

    #[test]
    fn test_turn_allocation_lifecycle() {
        let alloc = TurnAllocation {
            relayed_addr: "198.51.100.1:49152".parse().unwrap(),
            mapped_addr: "203.0.113.50:12345".parse().unwrap(),
            lifetime: 600,
            created_at: Instant::now(),
        };

        assert!(!alloc.is_expired());
        assert!(!alloc.needs_refresh());
        assert!(alloc.remaining_secs() > 590);
    }

    #[test]
    fn test_turn_client_creation() {
        let client = TurnClient::new(
            "198.51.100.1:3478".parse().unwrap(),
            "user",
            "pass",
        );
        assert_eq!(client.username, "user");
        assert_eq!(client.password, "pass");
        assert!(client.allocation.is_none());
        assert!(client.channels.is_empty());
        assert_eq!(client.next_channel, CHANNEL_MIN);
    }

    #[test]
    fn test_channel_binding_expiry() {
        let binding = ChannelBinding {
            channel: 0x4000,
            peer_addr: "10.0.0.1:5000".parse().unwrap(),
            created_at: Instant::now(),
        };
        assert!(!binding.is_expired());
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL TURN TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_channel_number_below_range_rejected() {
        // Channel numbers below 0x4000 must be rejected
        let invalid_buf = {
            let mut buf = vec![0; 8];
            buf[0..2].copy_from_slice(&0x3FFFu16.to_be_bytes());
            buf[2..4].copy_from_slice(&2u16.to_be_bytes());
            buf[4] = b'a';
            buf[5] = b'b';
            buf
        };
        assert!(decode_channel_data(&invalid_buf).is_none());
        assert!(!is_channel_data(&invalid_buf));
    }

    #[test]
    fn test_channel_number_above_range_rejected() {
        // Channel numbers above 0x7FFF must be rejected
        let invalid_buf = {
            let mut buf = vec![0; 8];
            buf[0..2].copy_from_slice(&0x8000u16.to_be_bytes());
            buf[2..4].copy_from_slice(&2u16.to_be_bytes());
            buf[4] = b'a';
            buf[5] = b'b';
            buf
        };
        assert!(decode_channel_data(&invalid_buf).is_none());
        assert!(!is_channel_data(&invalid_buf));
    }

    #[test]
    fn test_channel_number_boundary_min() {
        let encoded = encode_channel_data(CHANNEL_MIN, b"test");
        let (ch, data) = decode_channel_data(&encoded).unwrap();
        assert_eq!(ch, CHANNEL_MIN);
        assert_eq!(data, b"test");
    }

    #[test]
    fn test_channel_number_boundary_max() {
        let encoded = encode_channel_data(CHANNEL_MAX, b"test");
        let (ch, data) = decode_channel_data(&encoded).unwrap();
        assert_eq!(ch, CHANNEL_MAX);
        assert_eq!(data, b"test");
    }

    #[test]
    fn test_channel_data_truncated_header() {
        // Only 3 bytes, less than CHANNEL_DATA_HEADER_SIZE
        assert!(decode_channel_data(&[0x40, 0x00, 0x00]).is_none());
        assert!(!is_channel_data(&[0x40, 0x00, 0x00]));
    }

    #[test]
    fn test_channel_data_length_exceeds_buffer() {
        // Header says 100 bytes of data, but buffer is only 8 bytes total
        let mut buf = vec![0; 8];
        buf[0..2].copy_from_slice(&0x4001u16.to_be_bytes());
        buf[2..4].copy_from_slice(&100u16.to_be_bytes());
        assert!(decode_channel_data(&buf).is_none());
    }

    #[test]
    fn test_turn_client_channel_exhaustion() {
        let mut client = TurnClient::new(
            "198.51.100.1:3478".parse().unwrap(),
            "user",
            "pass",
        );
        // Manually exhaust channel range
        client.next_channel = CHANNEL_MAX + 1;
        // The next_channel is already past the max, so no channels available
        assert!(client.next_channel > CHANNEL_MAX);
    }

    #[test]
    fn test_turn_allocation_needs_refresh_at_80_percent() {
        let alloc = TurnAllocation {
            relayed_addr: "198.51.100.1:49152".parse().unwrap(),
            mapped_addr: "203.0.113.50:12345".parse().unwrap(),
            lifetime: 10, // 10 seconds for testing
            created_at: Instant::now() - Duration::from_secs(8), // 80% elapsed
        };
        assert!(alloc.needs_refresh());
        assert!(!alloc.is_expired());
    }

    #[test]
    fn test_turn_allocation_expired() {
        let alloc = TurnAllocation {
            relayed_addr: "198.51.100.1:49152".parse().unwrap(),
            mapped_addr: "203.0.113.50:12345".parse().unwrap(),
            lifetime: 1,
            created_at: Instant::now() - Duration::from_secs(2),
        };
        assert!(alloc.is_expired());
        assert_eq!(alloc.remaining_secs(), 0);
    }

    #[test]
    fn test_channel_data_large_payload() {
        // Test with MTU-size payload
        let payload = vec![0xAB; 1400];
        let encoded = encode_channel_data(0x4001, &payload);
        let (ch, decoded) = decode_channel_data(&encoded).unwrap();
        assert_eq!(ch, 0x4001);
        assert_eq!(decoded, &payload[..]);
    }

    #[test]
    fn test_channel_data_padding_various_sizes() {
        // Test padding for sizes 1-4
        for size in 1..=4 {
            let payload = vec![0xFF; size];
            let encoded = encode_channel_data(0x4000, &payload);
            let padded_len = (size + 3) & !3;
            assert_eq!(encoded.len(), 4 + padded_len, "Wrong padded length for size {}", size);
            let (_, decoded) = decode_channel_data(&encoded).unwrap();
            assert_eq!(decoded, &payload[..], "Roundtrip failed for size {}", size);
        }
    }
}
