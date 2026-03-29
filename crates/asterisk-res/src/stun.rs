//! STUN (Session Traversal Utilities for NAT) client.
//!
//! Port of `main/stun.c` and `res/res_stun_monitor.c`. Implements STUN
//! Binding Request/Response for external IP discovery and basic NAT traversal,
//! as specified in RFC 3489 (classic STUN) and RFC 5389 (updated STUN).

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;
use tokio::net::UdpSocket;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Standard STUN port.
pub const STUN_DEFAULT_PORT: u16 = 3478;

/// STUN magic cookie (RFC 5389).
pub const STUN_MAGIC_COOKIE: u32 = 0x2112A442;

/// Maximum STUN retries.
pub const STUN_MAX_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// STUN message types (RFC 3489 sec 11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StunMessageType {
    BindingRequest = 0x0001,
    BindingResponse = 0x0101,
    BindingErrorResponse = 0x0111,
    SharedSecretRequest = 0x0002,
    SharedSecretResponse = 0x0102,
    SharedSecretErrorResponse = 0x0112,
}

impl StunMessageType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x0001 => Some(Self::BindingRequest),
            0x0101 => Some(Self::BindingResponse),
            0x0111 => Some(Self::BindingErrorResponse),
            0x0002 => Some(Self::SharedSecretRequest),
            0x0102 => Some(Self::SharedSecretResponse),
            0x0112 => Some(Self::SharedSecretErrorResponse),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::BindingRequest => "Binding Request",
            Self::BindingResponse => "Binding Response",
            Self::BindingErrorResponse => "Binding Error Response",
            Self::SharedSecretRequest => "Shared Secret Request",
            Self::SharedSecretResponse => "Shared Secret Response",
            Self::SharedSecretErrorResponse => "Shared Secret Error Response",
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute types
// ---------------------------------------------------------------------------

/// STUN attribute types (RFC 3489 sec 11.2, RFC 5389 sec 18.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StunAttrType {
    MappedAddress = 0x0001,
    ResponseAddress = 0x0002,
    ChangeRequest = 0x0003,
    SourceAddress = 0x0004,
    ChangedAddress = 0x0005,
    Username = 0x0006,
    Password = 0x0007,
    MessageIntegrity = 0x0008,
    ErrorCode = 0x0009,
    UnknownAttributes = 0x000A,
    ReflectedFrom = 0x000B,
    // RFC 5389 attributes:
    XorMappedAddress = 0x0020,
    Software = 0x8022,
    Fingerprint = 0x8028,
}

impl StunAttrType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x0001 => Some(Self::MappedAddress),
            0x0002 => Some(Self::ResponseAddress),
            0x0003 => Some(Self::ChangeRequest),
            0x0004 => Some(Self::SourceAddress),
            0x0005 => Some(Self::ChangedAddress),
            0x0006 => Some(Self::Username),
            0x0007 => Some(Self::Password),
            0x0008 => Some(Self::MessageIntegrity),
            0x0009 => Some(Self::ErrorCode),
            0x000A => Some(Self::UnknownAttributes),
            0x000B => Some(Self::ReflectedFrom),
            0x0020 => Some(Self::XorMappedAddress),
            0x8022 => Some(Self::Software),
            0x8028 => Some(Self::Fingerprint),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum StunError {
    #[error("STUN I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("STUN parse error: {0}")]
    Parse(String),
    #[error("STUN timeout")]
    Timeout,
    #[error("STUN error response: {code} {reason}")]
    ErrorResponse { code: u16, reason: String },
}

pub type StunResult<T> = Result<T, StunError>;

// ---------------------------------------------------------------------------
// Transaction ID
// ---------------------------------------------------------------------------

/// STUN transaction ID (128 bits / 16 bytes).
///
/// In RFC 5389, the first 4 bytes are the magic cookie (0x2112A442) and the
/// remaining 12 bytes are the transaction ID. In RFC 3489, all 16 bytes are
/// the transaction ID.
#[derive(Clone, PartialEq, Eq)]
pub struct TransactionId(pub [u8; 16]);

impl TransactionId {
    /// Generate a new random transaction ID (RFC 5389 format with magic cookie).
    pub fn new() -> Self {
        use rand::RngCore;
        let mut id = [0u8; 16];
        // Set magic cookie in first 4 bytes.
        id[0..4].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        // Random transaction ID in remaining 12 bytes.
        rand::thread_rng().fill_bytes(&mut id[4..]);
        Self(id)
    }

    /// Generate an RFC 3489 (classic) transaction ID (all random).
    pub fn new_classic() -> Self {
        use rand::RngCore;
        let mut id = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut id);
        Self(id)
    }

    /// Check if this transaction ID uses the RFC 5389 magic cookie.
    pub fn has_magic_cookie(&self) -> bool {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]]) == STUN_MAGIC_COOKIE
    }
}

impl Default for TransactionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TxId(")?;
        for b in &self.0 {
            write!(f, "{:02x}", b)?;
        }
        write!(f, ")")
    }
}

// ---------------------------------------------------------------------------
// STUN header
// ---------------------------------------------------------------------------

/// STUN message header (20 bytes).
///
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |      STUN Message Type        |       Message Length          |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                Transaction ID (128 bits)                      |
/// |                                                               |
/// |                                                               |
/// |                                                               |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
#[derive(Debug, Clone)]
pub struct StunHeader {
    pub msg_type: u16,
    pub msg_length: u16,
    pub transaction_id: TransactionId,
}

impl StunHeader {
    pub const SIZE: usize = 20;

    pub fn parse(data: &[u8]) -> StunResult<Self> {
        if data.len() < Self::SIZE {
            return Err(StunError::Parse(format!(
                "STUN header too short: {} bytes",
                data.len()
            )));
        }

        let msg_type = u16::from_be_bytes([data[0], data[1]]);
        let msg_length = u16::from_be_bytes([data[2], data[3]]);
        let mut tid = [0u8; 16];
        tid.copy_from_slice(&data[4..20]);

        Ok(Self {
            msg_type,
            msg_length,
            transaction_id: TransactionId(tid),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..2].copy_from_slice(&self.msg_type.to_be_bytes());
        buf[2..4].copy_from_slice(&self.msg_length.to_be_bytes());
        buf[4..20].copy_from_slice(&self.transaction_id.0);
        buf
    }
}

// ---------------------------------------------------------------------------
// STUN attribute
// ---------------------------------------------------------------------------

/// A parsed STUN attribute.
#[derive(Debug, Clone)]
pub struct StunAttribute {
    pub attr_type: u16,
    pub value: Bytes,
}

impl StunAttribute {
    /// Parse a single STUN attribute from `data`. Returns the attribute and
    /// bytes consumed (including padding to 4-byte boundary).
    pub fn parse(data: &[u8]) -> StunResult<(Self, usize)> {
        if data.len() < 4 {
            return Err(StunError::Parse("STUN attribute too short".into()));
        }

        let attr_type = u16::from_be_bytes([data[0], data[1]]);
        let attr_len = u16::from_be_bytes([data[2], data[3]]) as usize;

        if data.len() < 4 + attr_len {
            return Err(StunError::Parse(format!(
                "STUN attribute {} truncated: need {} have {}",
                attr_type,
                attr_len,
                data.len() - 4
            )));
        }

        let value = Bytes::copy_from_slice(&data[4..4 + attr_len]);

        // Attributes are padded to 4-byte boundaries.
        let padded_len = (attr_len + 3) & !3;
        let consumed = 4 + padded_len.min(data.len() - 4);

        Ok((Self { attr_type, value }, consumed))
    }

    pub fn to_bytes(&self) -> BytesMut {
        let attr_len = self.value.len().min(u16::MAX as usize);
        let padded = (attr_len + 3) & !3;
        let mut buf = BytesMut::with_capacity(4 + padded);
        buf.put_u16(self.attr_type);
        buf.put_u16(attr_len as u16);
        buf.put_slice(&self.value[..attr_len]);
        // Pad to 4-byte boundary.
        for _ in attr_len..padded {
            buf.put_u8(0);
        }
        buf
    }
}

/// Parse all attributes from the body of a STUN message.
pub fn parse_attributes(data: &[u8]) -> StunResult<Vec<StunAttribute>> {
    let mut attrs = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (attr, consumed) = StunAttribute::parse(&data[offset..])?;
        attrs.push(attr);
        offset += consumed;
    }
    Ok(attrs)
}

// ---------------------------------------------------------------------------
// Address parsing
// ---------------------------------------------------------------------------

/// Address family values.
const STUN_ADDR_FAMILY_IPV4: u8 = 0x01;
const STUN_ADDR_FAMILY_IPV6: u8 = 0x02;

/// Parse a MAPPED-ADDRESS attribute value into a socket address.
///
/// Format (RFC 3489):
/// ```text
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |x x x x x x x x|    Family     |         Port                |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                         Address (32 bits)                    |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
pub fn parse_mapped_address(data: &[u8]) -> StunResult<SocketAddr> {
    if data.len() < 8 {
        return Err(StunError::Parse("MAPPED-ADDRESS too short".into()));
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        STUN_ADDR_FAMILY_IPV4 => {
            let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        STUN_ADDR_FAMILY_IPV6 => Err(StunError::Parse("IPv6 MAPPED-ADDRESS not yet supported".into())),
        _ => Err(StunError::Parse(format!(
            "Unknown address family: {}",
            family
        ))),
    }
}

/// Parse an XOR-MAPPED-ADDRESS attribute value (RFC 5389).
///
/// The address and port are XORed with the magic cookie and transaction ID.
pub fn parse_xor_mapped_address(
    data: &[u8],
    _transaction_id: &TransactionId,
) -> StunResult<SocketAddr> {
    if data.len() < 8 {
        return Err(StunError::Parse("XOR-MAPPED-ADDRESS too short".into()));
    }

    let family = data[1];
    let xored_port = u16::from_be_bytes([data[2], data[3]]);
    let magic_hi = (STUN_MAGIC_COOKIE >> 16) as u16;
    let port = xored_port ^ magic_hi;

    match family {
        STUN_ADDR_FAMILY_IPV4 => {
            let xored_addr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = xored_addr ^ STUN_MAGIC_COOKIE;
            let ip = Ipv4Addr::from(addr);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        STUN_ADDR_FAMILY_IPV6 => {
            Err(StunError::Parse("IPv6 XOR-MAPPED-ADDRESS not yet supported".into()))
        }
        _ => Err(StunError::Parse(format!(
            "Unknown address family: {}",
            family
        ))),
    }
}

// ---------------------------------------------------------------------------
// STUN message
// ---------------------------------------------------------------------------

/// A complete parsed STUN message.
#[derive(Debug, Clone)]
pub struct StunMessage {
    pub header: StunHeader,
    pub attributes: Vec<StunAttribute>,
}

impl StunMessage {
    /// Parse a complete STUN message from a byte buffer.
    pub fn parse(data: &[u8]) -> StunResult<Self> {
        let header = StunHeader::parse(data)?;
        let body_end = StunHeader::SIZE + header.msg_length as usize;
        if data.len() < body_end {
            return Err(StunError::Parse("STUN message truncated".into()));
        }
        let attributes = parse_attributes(&data[StunHeader::SIZE..body_end])?;
        Ok(Self { header, attributes })
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Bytes {
        let mut body = BytesMut::new();
        for attr in &self.attributes {
            body.extend_from_slice(&attr.to_bytes());
        }

        let mut header = self.header.clone();
        if body.len() > u16::MAX as usize {
            tracing::warn!(
                body_len = body.len(),
                "STUN message body exceeds u16::MAX; truncating length field"
            );
        }
        header.msg_length = body.len().min(u16::MAX as usize) as u16;

        let mut buf = BytesMut::with_capacity(StunHeader::SIZE + body.len());
        buf.put_slice(&header.to_bytes());
        buf.put_slice(&body);
        buf.freeze()
    }

    /// Create a Binding Request message.
    pub fn binding_request() -> Self {
        Self {
            header: StunHeader {
                msg_type: StunMessageType::BindingRequest as u16,
                msg_length: 0,
                transaction_id: TransactionId::new(),
            },
            attributes: Vec::new(),
        }
    }

    /// Find the mapped address (MAPPED-ADDRESS or XOR-MAPPED-ADDRESS) in a
    /// binding response.
    pub fn mapped_address(&self) -> StunResult<Option<SocketAddr>> {
        // Prefer XOR-MAPPED-ADDRESS.
        for attr in &self.attributes {
            if attr.attr_type == StunAttrType::XorMappedAddress as u16 {
                return Ok(Some(parse_xor_mapped_address(
                    &attr.value,
                    &self.header.transaction_id,
                )?));
            }
        }
        // Fall back to MAPPED-ADDRESS.
        for attr in &self.attributes {
            if attr.attr_type == StunAttrType::MappedAddress as u16 {
                return Ok(Some(parse_mapped_address(&attr.value)?));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// NAT type detection
// ---------------------------------------------------------------------------

/// NAT type as determined by STUN tests (RFC 3489 sec 10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// No NAT (open Internet).
    Open,
    /// Full Cone NAT.
    FullCone,
    /// Restricted Cone NAT.
    RestrictedCone,
    /// Port Restricted Cone NAT.
    PortRestrictedCone,
    /// Symmetric NAT.
    Symmetric,
    /// Symmetric UDP Firewall.
    SymmetricFirewall,
    /// UDP Blocked.
    Blocked,
    /// Unknown / could not determine.
    Unknown,
}

// ---------------------------------------------------------------------------
// STUN client
// ---------------------------------------------------------------------------

/// STUN client for NAT traversal and external IP discovery.
///
/// Port of `main/stun.c` and `res/res_stun_monitor.c`.
pub struct StunClient {
    /// STUN server address.
    server_addr: SocketAddr,
    /// UDP socket for STUN communication.
    socket: Option<UdpSocket>,
    /// Last known external address.
    external_addr: Option<SocketAddr>,
    /// Request timeout in milliseconds.
    timeout_ms: u64,
}

impl fmt::Debug for StunClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StunClient")
            .field("server_addr", &self.server_addr)
            .field("external_addr", &self.external_addr)
            .finish()
    }
}

impl StunClient {
    /// Create a new STUN client targeting the specified server.
    pub fn new(server_addr: SocketAddr) -> Self {
        Self {
            server_addr,
            socket: None,
            external_addr: None,
            timeout_ms: 3000,
        }
    }

    /// Set the request timeout.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Bind the UDP socket.
    pub async fn bind(&mut self, local_addr: SocketAddr) -> StunResult<()> {
        let socket = UdpSocket::bind(local_addr).await?;
        self.socket = Some(socket);
        Ok(())
    }

    /// Send a Binding Request and return the external (mapped) address.
    pub async fn discover_external_addr(&mut self) -> StunResult<SocketAddr> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| StunError::Parse("Socket not bound".into()))?;

        let request = StunMessage::binding_request();
        let request_bytes = request.to_bytes();
        let tid = request.header.transaction_id.clone();

        // Send request.
        socket.send_to(&request_bytes, self.server_addr).await?;
        debug!(server = %self.server_addr, "STUN Binding Request sent");

        // Receive response with timeout.
        let mut buf = vec![0u8; 1024];
        let timeout = tokio::time::Duration::from_millis(self.timeout_ms);

        let (len, _from) = tokio::time::timeout(timeout, socket.recv_from(&mut buf))
            .await
            .map_err(|_| StunError::Timeout)??;

        let response = StunMessage::parse(&buf[..len])?;

        // Verify transaction ID matches.
        if response.header.transaction_id != tid {
            return Err(StunError::Parse("Transaction ID mismatch".into()));
        }

        let msg_type = StunMessageType::from_u16(response.header.msg_type);
        match msg_type {
            Some(StunMessageType::BindingResponse) => {
                let addr = response
                    .mapped_address()?
                    .ok_or_else(|| StunError::Parse("No mapped address in response".into()))?;

                self.external_addr = Some(addr);
                info!(external = %addr, "STUN external address discovered");
                Ok(addr)
            }
            Some(StunMessageType::BindingErrorResponse) => {
                Err(StunError::ErrorResponse {
                    code: 0,
                    reason: "Binding Error Response".into(),
                })
            }
            _ => Err(StunError::Parse(format!(
                "Unexpected STUN response type: 0x{:04x}",
                response.header.msg_type
            ))),
        }
    }

    /// Get the last known external address.
    pub fn external_addr(&self) -> Option<SocketAddr> {
        self.external_addr
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stun_header_roundtrip() {
        let header = StunHeader {
            msg_type: StunMessageType::BindingRequest as u16,
            msg_length: 0,
            transaction_id: TransactionId::new(),
        };
        let bytes = header.to_bytes();
        let parsed = StunHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.msg_type, StunMessageType::BindingRequest as u16);
        assert_eq!(parsed.msg_length, 0);
        assert_eq!(parsed.transaction_id, header.transaction_id);
    }

    #[test]
    fn test_transaction_id_magic_cookie() {
        let tid = TransactionId::new();
        assert!(tid.has_magic_cookie());

        let classic = TransactionId::new_classic();
        // Classic might coincidentally have the magic cookie, but very unlikely.
        // Just test it doesn't panic.
        let _ = classic.has_magic_cookie();
    }

    #[test]
    fn test_mapped_address_parse() {
        // IPv4 address 192.168.1.100, port 5060
        let data = [
            0x00, // unused
            0x01, // IPv4
            0x13, 0xC4, // port 5060
            0xC0, 0xA8, 0x01, 0x64, // 192.168.1.100
        ];
        let addr = parse_mapped_address(&data).unwrap();
        assert_eq!(addr, "192.168.1.100:5060".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn test_xor_mapped_address_parse() {
        // XOR-MAPPED-ADDRESS for 192.168.1.100:5060
        // Port XOR: 5060 ^ (0x2112A442 >> 16) = 5060 ^ 0x2112 = 0x32D6
        // Addr XOR: 0xC0A80164 ^ 0x2112A442 = 0xE1BAA526
        let xor_port = 5060u16 ^ 0x2112;
        let xor_addr = 0xC0A80164u32 ^ STUN_MAGIC_COOKIE;

        let mut data = [0u8; 8];
        data[1] = 0x01; // IPv4
        data[2..4].copy_from_slice(&xor_port.to_be_bytes());
        data[4..8].copy_from_slice(&xor_addr.to_be_bytes());

        let tid = TransactionId::new();
        let addr = parse_xor_mapped_address(&data, &tid).unwrap();
        assert_eq!(addr, "192.168.1.100:5060".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn test_stun_message_binding_request() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes();

        // Should be 20 bytes (header only, no attributes).
        assert_eq!(bytes.len(), 20);

        // Parse it back.
        let parsed = StunMessage::parse(&bytes).unwrap();
        assert_eq!(
            parsed.header.msg_type,
            StunMessageType::BindingRequest as u16
        );
        assert_eq!(parsed.header.msg_length, 0);
        assert!(parsed.attributes.is_empty());
    }

    #[test]
    fn test_stun_attribute_roundtrip() {
        let attr = StunAttribute {
            attr_type: StunAttrType::Username as u16,
            value: Bytes::from("testuser"),
        };
        let bytes = attr.to_bytes();
        let (parsed, consumed) = StunAttribute::parse(&bytes).unwrap();
        assert_eq!(parsed.attr_type, StunAttrType::Username as u16);
        assert_eq!(&parsed.value[..], b"testuser");
        // "testuser" is 8 bytes, already 4-byte aligned.
        assert_eq!(consumed, 4 + 8);
    }

    #[test]
    fn test_stun_attribute_padding() {
        let attr = StunAttribute {
            attr_type: StunAttrType::Username as u16,
            value: Bytes::from("abc"), // 3 bytes -> padded to 4
        };
        let bytes = attr.to_bytes();
        assert_eq!(bytes.len(), 4 + 4); // 4 header + 4 padded value
        let (parsed, consumed) = StunAttribute::parse(&bytes).unwrap();
        assert_eq!(&parsed.value[..], b"abc");
        assert_eq!(consumed, 8);
    }

    #[test]
    fn test_stun_message_with_attributes() {
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttribute {
            attr_type: StunAttrType::Username as u16,
            value: Bytes::from("user1"),
        });

        let bytes = msg.to_bytes();
        let parsed = StunMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.attributes.len(), 1);
        assert_eq!(&parsed.attributes[0].value[..], b"user1");
    }

    #[test]
    fn test_binding_response_mapped_address() {
        // Build a fake binding response with a MAPPED-ADDRESS attribute.
        let mut msg = StunMessage {
            header: StunHeader {
                msg_type: StunMessageType::BindingResponse as u16,
                msg_length: 0,
                transaction_id: TransactionId::new(),
            },
            attributes: vec![StunAttribute {
                attr_type: StunAttrType::MappedAddress as u16,
                value: Bytes::from_static(&[
                    0x00, 0x01, // IPv4
                    0x13, 0xC4, // port 5060
                    0x0A, 0x00, 0x00, 0x01, // 10.0.0.1
                ]),
            }],
        };

        let bytes = msg.to_bytes();
        let parsed = StunMessage::parse(&bytes).unwrap();
        let addr = parsed.mapped_address().unwrap().unwrap();
        assert_eq!(addr, "10.0.0.1:5060".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn test_nat_type_debug() {
        // Just ensure NatType can be printed.
        assert_eq!(format!("{:?}", NatType::FullCone), "FullCone");
        assert_eq!(format!("{:?}", NatType::Symmetric), "Symmetric");
    }
}
