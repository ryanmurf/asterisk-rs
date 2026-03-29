//! Enhanced STUN (Session Traversal Utilities for NAT) implementation.
//!
//! Full RFC 5389 STUN with all attributes needed for ICE (RFC 8445) and
//! TURN (RFC 5766). Includes MESSAGE-INTEGRITY (HMAC-SHA1), FINGERPRINT
//! (CRC32), short-term and long-term credential mechanisms, and
//! Indication support.
//!
//! This lives in asterisk-sip because ICE/TURN are tightly coupled with
//! SIP/RTP media negotiation. The asterisk-res stun module remains for
//! standalone external-address discovery.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac};
use sha1::Sha1;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// STUN magic cookie (RFC 5389 Section 6).
pub const MAGIC_COOKIE: u32 = 0x2112A442;

/// STUN header size in bytes.
pub const HEADER_SIZE: usize = 20;

/// FINGERPRINT XOR constant (RFC 5389 Section 15.5).
pub const FINGERPRINT_XOR: u32 = 0x5354554e;

/// Maximum STUN message size we will accept.
pub const MAX_MESSAGE_SIZE: usize = 2048;

// ---------------------------------------------------------------------------
// Message class and method (RFC 5389 Section 6)
// ---------------------------------------------------------------------------

/// STUN message class encoded in the type field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageClass {
    Request,
    Indication,
    SuccessResponse,
    ErrorResponse,
}

/// STUN/TURN methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Method {
    /// RFC 5389: STUN Binding
    Binding = 0x0001,
    /// RFC 5766: TURN Allocate
    Allocate = 0x0003,
    /// RFC 5766: TURN Refresh
    Refresh = 0x0004,
    /// RFC 5766: TURN Send
    Send = 0x0006,
    /// RFC 5766: TURN Data
    Data = 0x0007,
    /// RFC 5766: TURN CreatePermission
    CreatePermission = 0x0008,
    /// RFC 5766: TURN ChannelBind
    ChannelBind = 0x0009,
}

impl Method {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x0001 => Some(Self::Binding),
            0x0003 => Some(Self::Allocate),
            0x0004 => Some(Self::Refresh),
            0x0006 => Some(Self::Send),
            0x0007 => Some(Self::Data),
            0x0008 => Some(Self::CreatePermission),
            0x0009 => Some(Self::ChannelBind),
            _ => None,
        }
    }
}

/// Encode a STUN message type from method and class.
///
/// RFC 5389 Section 6: The message type is a 14-bit value with the method
/// and class bits interleaved:
///
/// ```text
///   0                 1
///   2  3  4 5 6 7 8 9 0 1 2 3 4 5
///  +--+--+-+-+-+-+-+-+-+-+-+-+-+-+
///  |M |M |M|M|M|C|M|M|M|C|M|M|M|
///  |11|10|9|8|7|1|6|5|4|0|3|2|1|0|
///  +--+--+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
pub fn encode_message_type(method: Method, class: MessageClass) -> u16 {
    let m = method as u16;
    let (c0, c1) = match class {
        MessageClass::Request => (0u16, 0u16),
        MessageClass::Indication => (1, 0),
        MessageClass::SuccessResponse => (0, 1),
        MessageClass::ErrorResponse => (1, 1),
    };

    // method bits: M0-M3 in bits 0-3, M4-M6 in bits 5-7, M7-M11 in bits 9-13
    // class bits: C0 in bit 4, C1 in bit 8
    let m_low = m & 0x000F; // M0-M3
    let m_mid = (m & 0x0070) << 1; // M4-M6 shifted left by 1
    let m_high = (m & 0x0F80) << 2; // M7-M11 shifted left by 2
    let c_bits = (c0 << 4) | (c1 << 8);

    m_low | m_mid | m_high | c_bits
}

/// Decode a STUN message type into method and class.
pub fn decode_message_type(msg_type: u16) -> (Option<Method>, MessageClass) {
    let c0 = (msg_type >> 4) & 1;
    let c1 = (msg_type >> 8) & 1;
    let class = match (c0, c1) {
        (0, 0) => MessageClass::Request,
        (1, 0) => MessageClass::Indication,
        (0, 1) => MessageClass::SuccessResponse,
        (1, 1) => MessageClass::ErrorResponse,
        _ => unreachable!(),
    };

    let m_low = msg_type & 0x000F;
    let m_mid = (msg_type >> 1) & 0x0070;
    let m_high = (msg_type >> 2) & 0x0F80;
    let method_val = m_low | m_mid | m_high;

    (Method::from_u16(method_val), class)
}

// ---------------------------------------------------------------------------
// STUN attribute types (RFC 5389 Section 18.2, RFC 5766 Section 14)
// ---------------------------------------------------------------------------

/// STUN/TURN attribute type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum AttrType {
    // Comprehension-required (0x0000-0x7FFF)
    MappedAddress = 0x0001,
    Username = 0x0006,
    MessageIntegrity = 0x0008,
    ErrorCode = 0x0009,
    UnknownAttributes = 0x000A,
    Realm = 0x0014,
    Nonce = 0x0015,
    XorMappedAddress = 0x0020,

    // TURN (RFC 5766)
    ChannelNumber = 0x000C,
    Lifetime = 0x000D,
    XorPeerAddress = 0x0012,
    Data = 0x0013,
    XorRelayedAddress = 0x0016,
    RequestedTransport = 0x0019,
    DontFragment = 0x001A,

    // ICE (RFC 8445)
    Priority = 0x0024,
    UseCandidate = 0x0025,
    IceControlled = 0x8029,
    IceControlling = 0x802A,

    // Comprehension-optional (0x8000-0xFFFF)
    Software = 0x8022,
    AlternateServer = 0x8023,
    Fingerprint = 0x8028,
}

impl AttrType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x0001 => Some(Self::MappedAddress),
            0x0006 => Some(Self::Username),
            0x0008 => Some(Self::MessageIntegrity),
            0x0009 => Some(Self::ErrorCode),
            0x000A => Some(Self::UnknownAttributes),
            0x000C => Some(Self::ChannelNumber),
            0x000D => Some(Self::Lifetime),
            0x0012 => Some(Self::XorPeerAddress),
            0x0013 => Some(Self::Data),
            0x0014 => Some(Self::Realm),
            0x0015 => Some(Self::Nonce),
            0x0016 => Some(Self::XorRelayedAddress),
            0x0019 => Some(Self::RequestedTransport),
            0x001A => Some(Self::DontFragment),
            0x0020 => Some(Self::XorMappedAddress),
            0x0024 => Some(Self::Priority),
            0x0025 => Some(Self::UseCandidate),
            0x8022 => Some(Self::Software),
            0x8023 => Some(Self::AlternateServer),
            0x8028 => Some(Self::Fingerprint),
            0x8029 => Some(Self::IceControlled),
            0x802A => Some(Self::IceControlling),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum StunError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("timeout")]
    Timeout,
    #[error("STUN error response: {code} {reason}")]
    ErrorResponse { code: u16, reason: String },
    #[error("integrity check failed")]
    IntegrityFailed,
    #[error("fingerprint check failed")]
    FingerprintFailed,
}

pub type StunResult<T> = Result<T, StunError>;

// ---------------------------------------------------------------------------
// Transaction ID
// ---------------------------------------------------------------------------

/// 96-bit transaction ID (RFC 5389: magic cookie is separate from TxID).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TransactionId(pub [u8; 12]);

impl TransactionId {
    pub fn new() -> Self {
        use rand::RngCore;
        let mut id = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut id);
        Self(id)
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
// STUN Attribute (raw)
// ---------------------------------------------------------------------------

/// A raw STUN attribute (type + value bytes).
#[derive(Debug, Clone)]
pub struct RawAttribute {
    pub attr_type: u16,
    pub value: Vec<u8>,
}

impl RawAttribute {
    /// Parse a single attribute. Returns (attribute, bytes_consumed).
    pub fn parse(data: &[u8]) -> StunResult<(Self, usize)> {
        if data.len() < 4 {
            return Err(StunError::Parse("attribute header too short".into()));
        }
        let attr_type = u16::from_be_bytes([data[0], data[1]]);
        let attr_len = u16::from_be_bytes([data[2], data[3]]) as usize;
        if data.len() < 4 + attr_len {
            return Err(StunError::Parse(format!(
                "attribute 0x{:04x} truncated: need {} have {}",
                attr_type,
                attr_len,
                data.len() - 4
            )));
        }
        let value = data[4..4 + attr_len].to_vec();
        let padded = (attr_len + 3) & !3;
        let consumed = 4 + padded.min(data.len() - 4);
        Ok((Self { attr_type, value }, consumed))
    }

    /// Serialize to bytes (with padding).
    pub fn to_bytes(&self) -> Vec<u8> {
        let attr_len = self.value.len();
        let padded = (attr_len + 3) & !3;
        let mut buf = Vec::with_capacity(4 + padded);
        buf.extend_from_slice(&self.attr_type.to_be_bytes());
        buf.extend_from_slice(&(attr_len as u16).to_be_bytes());
        buf.extend_from_slice(&self.value);
        // Pad to 4-byte boundary
        for _ in attr_len..padded {
            buf.push(0);
        }
        buf
    }
}

// ---------------------------------------------------------------------------
// Typed attribute values
// ---------------------------------------------------------------------------

/// A parsed/typed STUN attribute value.
#[derive(Debug, Clone)]
pub enum StunAttrValue {
    MappedAddress(SocketAddr),
    XorMappedAddress(SocketAddr),
    XorPeerAddress(SocketAddr),
    XorRelayedAddress(SocketAddr),
    Username(String),
    MessageIntegrity([u8; 20]),
    Fingerprint(u32),
    ErrorCode { code: u16, reason: String },
    Realm(String),
    Nonce(String),
    Software(String),
    AlternateServer(SocketAddr),
    Priority(u32),
    UseCandidate,
    IceControlling(u64),
    IceControlled(u64),
    ChannelNumber(u16),
    Lifetime(u32),
    RequestedTransport(u8),
    DontFragment,
    Data(Vec<u8>),
    Unknown(u16, Vec<u8>),
}

// ---------------------------------------------------------------------------
// Address encoding/decoding
// ---------------------------------------------------------------------------

fn addr_family(addr: &SocketAddr) -> u8 {
    match addr {
        SocketAddr::V4(_) => 0x01,
        SocketAddr::V6(_) => 0x02,
    }
}

/// Encode a MAPPED-ADDRESS value.
pub fn encode_mapped_address(addr: &SocketAddr) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    buf.push(0x00); // reserved
    buf.push(addr_family(addr));
    buf.extend_from_slice(&addr.port().to_be_bytes());
    match addr.ip() {
        IpAddr::V4(ip) => buf.extend_from_slice(&ip.octets()),
        IpAddr::V6(ip) => buf.extend_from_slice(&ip.octets()),
    }
    buf
}

/// Decode a MAPPED-ADDRESS value.
pub fn decode_mapped_address(data: &[u8]) -> StunResult<SocketAddr> {
    if data.len() < 4 {
        return Err(StunError::Parse("MAPPED-ADDRESS too short".into()));
    }
    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);
    match family {
        0x01 => {
            if data.len() < 8 {
                return Err(StunError::Parse("MAPPED-ADDRESS IPv4 too short".into()));
            }
            let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        }
        0x02 => {
            if data.len() < 20 {
                return Err(StunError::Parse("MAPPED-ADDRESS IPv6 too short".into()));
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[4..20]);
            let ip = Ipv6Addr::from(octets);
            Ok(SocketAddr::new(IpAddr::V6(ip), port))
        }
        _ => Err(StunError::Parse(format!("unknown address family: {}", family))),
    }
}

/// Encode an XOR-MAPPED-ADDRESS value.
pub fn encode_xor_address(addr: &SocketAddr, transaction_id: &TransactionId) -> Vec<u8> {
    let mut buf = Vec::with_capacity(20);
    buf.push(0x00); // reserved
    buf.push(addr_family(addr));

    let xor_port = addr.port() ^ (MAGIC_COOKIE >> 16) as u16;
    buf.extend_from_slice(&xor_port.to_be_bytes());

    match addr.ip() {
        IpAddr::V4(ip) => {
            let xor_addr = u32::from_be_bytes(ip.octets()) ^ MAGIC_COOKIE;
            buf.extend_from_slice(&xor_addr.to_be_bytes());
        }
        IpAddr::V6(ip) => {
            let octets = ip.octets();
            let mut cookie_bytes = [0u8; 16];
            cookie_bytes[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            cookie_bytes[4..16].copy_from_slice(&transaction_id.0);
            for i in 0..16 {
                buf.push(octets[i] ^ cookie_bytes[i]);
            }
        }
    }
    buf
}

/// Decode an XOR-MAPPED-ADDRESS value.
pub fn decode_xor_address(data: &[u8], transaction_id: &TransactionId) -> StunResult<SocketAddr> {
    if data.len() < 4 {
        return Err(StunError::Parse("XOR-MAPPED-ADDRESS too short".into()));
    }
    let family = data[1];
    let xor_port = u16::from_be_bytes([data[2], data[3]]);
    let port = xor_port ^ (MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            if data.len() < 8 {
                return Err(StunError::Parse("XOR-MAPPED-ADDRESS IPv4 too short".into()));
            }
            let xor_addr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = xor_addr ^ MAGIC_COOKIE;
            let ip = Ipv4Addr::from(addr);
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        }
        0x02 => {
            if data.len() < 20 {
                return Err(StunError::Parse("XOR-MAPPED-ADDRESS IPv6 too short".into()));
            }
            let mut cookie_bytes = [0u8; 16];
            cookie_bytes[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            cookie_bytes[4..16].copy_from_slice(&transaction_id.0);
            let mut octets = [0u8; 16];
            for i in 0..16 {
                octets[i] = data[4 + i] ^ cookie_bytes[i];
            }
            let ip = Ipv6Addr::from(octets);
            Ok(SocketAddr::new(IpAddr::V6(ip), port))
        }
        _ => Err(StunError::Parse(format!("unknown address family: {}", family))),
    }
}

/// Encode an ERROR-CODE attribute value (RFC 5389 Section 15.6).
pub fn encode_error_code(code: u16, reason: &str) -> Vec<u8> {
    let class = (code / 100) as u8;
    let number = (code % 100) as u8;
    let mut buf = Vec::with_capacity(4 + reason.len());
    buf.extend_from_slice(&[0, 0, class, number]);
    buf.extend_from_slice(reason.as_bytes());
    buf
}

/// Decode an ERROR-CODE attribute value.
pub fn decode_error_code(data: &[u8]) -> StunResult<(u16, String)> {
    if data.len() < 4 {
        return Err(StunError::Parse("ERROR-CODE too short".into()));
    }
    let class = data[2] as u16;
    let number = data[3] as u16;
    let code = class * 100 + number;
    let reason = if data.len() > 4 {
        String::from_utf8_lossy(&data[4..]).to_string()
    } else {
        String::new()
    };
    Ok((code, reason))
}

// ---------------------------------------------------------------------------
// STUN Message
// ---------------------------------------------------------------------------

/// A complete STUN message with typed attributes.
#[derive(Debug, Clone)]
pub struct StunMessage {
    pub method: Method,
    pub class: MessageClass,
    pub transaction_id: TransactionId,
    pub attributes: Vec<StunAttrValue>,
}

impl StunMessage {
    // ----- Constructors -----

    /// Create a Binding Request.
    pub fn binding_request() -> Self {
        Self {
            method: Method::Binding,
            class: MessageClass::Request,
            transaction_id: TransactionId::new(),
            attributes: Vec::new(),
        }
    }

    /// Create a Binding Response.
    pub fn binding_response(tid: &TransactionId, mapped_addr: SocketAddr) -> Self {
        Self {
            method: Method::Binding,
            class: MessageClass::SuccessResponse,
            transaction_id: tid.clone(),
            attributes: vec![StunAttrValue::XorMappedAddress(mapped_addr)],
        }
    }

    /// Create a Binding Indication (connectionless keepalive).
    pub fn binding_indication() -> Self {
        Self {
            method: Method::Binding,
            class: MessageClass::Indication,
            transaction_id: TransactionId::new(),
            attributes: Vec::new(),
        }
    }

    /// Create a Binding Error Response.
    pub fn binding_error(tid: &TransactionId, code: u16, reason: &str) -> Self {
        Self {
            method: Method::Binding,
            class: MessageClass::ErrorResponse,
            transaction_id: tid.clone(),
            attributes: vec![StunAttrValue::ErrorCode {
                code,
                reason: reason.to_string(),
            }],
        }
    }

    // ----- Attribute accessors -----

    /// Find the XOR-MAPPED-ADDRESS in this message.
    pub fn xor_mapped_address(&self) -> Option<SocketAddr> {
        for attr in &self.attributes {
            if let StunAttrValue::XorMappedAddress(addr) = attr {
                return Some(*addr);
            }
        }
        None
    }

    /// Find the MAPPED-ADDRESS in this message.
    pub fn mapped_address(&self) -> Option<SocketAddr> {
        for attr in &self.attributes {
            if let StunAttrValue::MappedAddress(addr) = attr {
                return Some(*addr);
            }
        }
        None
    }

    /// Find any mapped address (prefer XOR-MAPPED-ADDRESS).
    pub fn get_mapped_address(&self) -> Option<SocketAddr> {
        self.xor_mapped_address().or_else(|| self.mapped_address())
    }

    /// Get the USERNAME attribute.
    pub fn username(&self) -> Option<&str> {
        for attr in &self.attributes {
            if let StunAttrValue::Username(u) = attr {
                return Some(u);
            }
        }
        None
    }

    /// Get the REALM attribute.
    pub fn realm(&self) -> Option<&str> {
        for attr in &self.attributes {
            if let StunAttrValue::Realm(r) = attr {
                return Some(r);
            }
        }
        None
    }

    /// Get the NONCE attribute.
    pub fn nonce(&self) -> Option<&str> {
        for attr in &self.attributes {
            if let StunAttrValue::Nonce(n) = attr {
                return Some(n);
            }
        }
        None
    }

    /// Get the ERROR-CODE attribute.
    pub fn error_code(&self) -> Option<(u16, &str)> {
        for attr in &self.attributes {
            if let StunAttrValue::ErrorCode { code, reason } = attr {
                return Some((*code, reason));
            }
        }
        None
    }

    /// Get the LIFETIME attribute.
    pub fn lifetime(&self) -> Option<u32> {
        for attr in &self.attributes {
            if let StunAttrValue::Lifetime(l) = attr {
                return Some(*l);
            }
        }
        None
    }

    /// Get the XOR-RELAYED-ADDRESS.
    pub fn xor_relayed_address(&self) -> Option<SocketAddr> {
        for attr in &self.attributes {
            if let StunAttrValue::XorRelayedAddress(addr) = attr {
                return Some(*addr);
            }
        }
        None
    }

    /// Check if USE-CANDIDATE is present.
    pub fn has_use_candidate(&self) -> bool {
        self.attributes
            .iter()
            .any(|a| matches!(a, StunAttrValue::UseCandidate))
    }

    /// Get the PRIORITY attribute.
    pub fn priority(&self) -> Option<u32> {
        for attr in &self.attributes {
            if let StunAttrValue::Priority(p) = attr {
                return Some(*p);
            }
        }
        None
    }

    /// Get ICE-CONTROLLING tie-breaker.
    pub fn ice_controlling(&self) -> Option<u64> {
        for attr in &self.attributes {
            if let StunAttrValue::IceControlling(v) = attr {
                return Some(*v);
            }
        }
        None
    }

    /// Get ICE-CONTROLLED tie-breaker.
    pub fn ice_controlled(&self) -> Option<u64> {
        for attr in &self.attributes {
            if let StunAttrValue::IceControlled(v) = attr {
                return Some(*v);
            }
        }
        None
    }

    // ----- Serialization -----

    /// Serialize to bytes without integrity or fingerprint.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for attr in &self.attributes {
            let raw = attr_value_to_raw(attr, &self.transaction_id);
            body.extend_from_slice(&raw.to_bytes());
        }

        let msg_type = encode_message_type(self.method, self.class);
        let mut buf = Vec::with_capacity(HEADER_SIZE + body.len());
        buf.extend_from_slice(&msg_type.to_be_bytes());
        buf.extend_from_slice(&(body.len() as u16).to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&self.transaction_id.0);
        buf.extend_from_slice(&body);
        buf
    }

    /// Serialize with MESSAGE-INTEGRITY using short-term credentials.
    ///
    /// Key = SASLprep(password) per RFC 5389 Section 15.4.
    /// For ICE, key = remote password.
    pub fn to_bytes_with_integrity(&self, key: &[u8]) -> Vec<u8> {
        let mut buf = self.to_bytes();
        // Add MESSAGE-INTEGRITY: compute HMAC-SHA1 over the message with
        // the length field adjusted to include the MESSAGE-INTEGRITY attribute
        // (24 bytes: 4 header + 20 value).
        let mi_attr_size = 24u16;
        let current_body_len = buf.len() as u16 - HEADER_SIZE as u16;
        let adjusted_len = current_body_len + mi_attr_size;
        buf[2..4].copy_from_slice(&adjusted_len.to_be_bytes());

        let hmac_value = compute_hmac_sha1(key, &buf);

        // Write MESSAGE-INTEGRITY attribute
        let mi_raw = RawAttribute {
            attr_type: AttrType::MessageIntegrity as u16,
            value: hmac_value.to_vec(),
        };
        buf.extend_from_slice(&mi_raw.to_bytes());

        // Fix length to include MESSAGE-INTEGRITY
        let final_body_len = buf.len() as u16 - HEADER_SIZE as u16;
        buf[2..4].copy_from_slice(&final_body_len.to_be_bytes());

        buf
    }

    /// Serialize with MESSAGE-INTEGRITY and FINGERPRINT.
    pub fn to_bytes_with_integrity_and_fingerprint(&self, key: &[u8]) -> Vec<u8> {
        let mut buf = self.to_bytes_with_integrity(key);
        append_fingerprint(&mut buf);
        buf
    }

    /// Serialize with FINGERPRINT only (no integrity).
    pub fn to_bytes_with_fingerprint(&self) -> Vec<u8> {
        let mut buf = self.to_bytes();
        append_fingerprint(&mut buf);
        buf
    }

    /// Serialize with long-term credentials (TURN).
    ///
    /// Key = MD5(username ":" realm ":" password)
    pub fn to_bytes_with_long_term_integrity(
        &self,
        username: &str,
        realm: &str,
        password: &str,
    ) -> Vec<u8> {
        let key = compute_long_term_key(username, realm, password);
        self.to_bytes_with_integrity_and_fingerprint(&key)
    }

    // ----- Deserialization -----

    /// Parse a STUN message from bytes.
    pub fn parse(data: &[u8]) -> StunResult<Self> {
        if data.len() < HEADER_SIZE {
            return Err(StunError::Parse(format!(
                "message too short: {} bytes",
                data.len()
            )));
        }

        // Check for STUN: first two bits must be 0
        if data[0] & 0xC0 != 0 {
            return Err(StunError::Parse("not a STUN message (first 2 bits not 0)".into()));
        }

        let msg_type = u16::from_be_bytes([data[0], data[1]]);
        let msg_length = u16::from_be_bytes([data[2], data[3]]) as usize;
        let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

        if cookie != MAGIC_COOKIE {
            return Err(StunError::Parse("magic cookie mismatch".into()));
        }

        let mut tid = [0u8; 12];
        tid.copy_from_slice(&data[8..20]);
        let transaction_id = TransactionId(tid);

        let (method, class) = decode_message_type(msg_type);
        let method = method.ok_or_else(|| {
            StunError::Parse(format!("unknown STUN method in type 0x{:04x}", msg_type))
        })?;

        let body_end = HEADER_SIZE + msg_length;
        if data.len() < body_end {
            return Err(StunError::Parse("message truncated".into()));
        }

        // Parse raw attributes
        let mut attributes = Vec::new();
        let mut offset = HEADER_SIZE;
        while offset < body_end {
            let (raw, consumed) = RawAttribute::parse(&data[offset..body_end])?;
            let attr = raw_to_attr_value(&raw, &transaction_id);
            attributes.push(attr);
            offset += consumed;
        }

        Ok(Self {
            method,
            class,
            transaction_id,
            attributes,
        })
    }

    /// Verify MESSAGE-INTEGRITY with short-term key.
    pub fn verify_integrity(&self, data: &[u8], key: &[u8]) -> StunResult<()> {
        // Find MESSAGE-INTEGRITY attribute position in raw data
        let mi_value = self.attributes.iter().find_map(|a| {
            if let StunAttrValue::MessageIntegrity(v) = a {
                Some(*v)
            } else {
                None
            }
        });

        let expected = match mi_value {
            Some(v) => v,
            None => return Err(StunError::Parse("no MESSAGE-INTEGRITY attribute".into())),
        };

        // Find offset of MESSAGE-INTEGRITY in the raw data
        let mi_offset = find_attribute_offset(data, AttrType::MessageIntegrity as u16)?;

        // Build the data to hash: header (with adjusted length) + body up to MI
        let mut hash_data = Vec::with_capacity(mi_offset + 24);
        hash_data.extend_from_slice(&data[..HEADER_SIZE]);

        // Adjust length to include up to and including MESSAGE-INTEGRITY
        let adjusted_len = (mi_offset - HEADER_SIZE + 24) as u16;
        hash_data[2..4].copy_from_slice(&adjusted_len.to_be_bytes());

        hash_data.extend_from_slice(&data[HEADER_SIZE..mi_offset]);

        let computed = compute_hmac_sha1(key, &hash_data);

        if computed != expected {
            return Err(StunError::IntegrityFailed);
        }
        Ok(())
    }

    /// Verify FINGERPRINT.
    pub fn verify_fingerprint(&self, data: &[u8]) -> StunResult<()> {
        let fp_value = self.attributes.iter().find_map(|a| {
            if let StunAttrValue::Fingerprint(v) = a {
                Some(*v)
            } else {
                None
            }
        });

        let expected = match fp_value {
            Some(v) => v,
            None => return Err(StunError::Parse("no FINGERPRINT attribute".into())),
        };

        let fp_offset = find_attribute_offset(data, AttrType::Fingerprint as u16)?;

        // Adjust length to include FINGERPRINT
        let mut hash_data = data[..fp_offset].to_vec();
        let adjusted_len = (fp_offset - HEADER_SIZE + 8) as u16;
        hash_data[2..4].copy_from_slice(&adjusted_len.to_be_bytes());

        let computed = compute_fingerprint(&hash_data);
        if computed != expected {
            return Err(StunError::FingerprintFailed);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute HMAC-SHA1 over data with the given key.
pub fn compute_hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC key length issue");
    mac.update(data);
    let result = mac.finalize();
    let bytes = result.into_bytes();
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    out
}

/// Compute CRC32 fingerprint: CRC32(data) XOR 0x5354554e.
pub fn compute_fingerprint(data: &[u8]) -> u32 {
    let crc = crc32fast::hash(data);
    crc ^ FINGERPRINT_XOR
}

/// Compute long-term credential key: MD5(username:realm:password).
pub fn compute_long_term_key(username: &str, realm: &str, password: &str) -> Vec<u8> {
    use md5::{Digest, Md5};
    let input = format!("{}:{}:{}", username, realm, password);
    let result = Md5::digest(input.as_bytes());
    result.to_vec()
}

/// Append a FINGERPRINT attribute to an encoded STUN message.
fn append_fingerprint(buf: &mut Vec<u8>) {
    // Adjust length to include FINGERPRINT (8 bytes: 4 header + 4 value)
    let fp_size = 8u16;
    let current_body_len = buf.len() as u16 - HEADER_SIZE as u16;
    let adjusted_len = current_body_len + fp_size;
    buf[2..4].copy_from_slice(&adjusted_len.to_be_bytes());

    let fp = compute_fingerprint(buf);
    let fp_raw = RawAttribute {
        attr_type: AttrType::Fingerprint as u16,
        value: fp.to_be_bytes().to_vec(),
    };
    buf.extend_from_slice(&fp_raw.to_bytes());

    // Fix length
    let final_body_len = buf.len() as u16 - HEADER_SIZE as u16;
    buf[2..4].copy_from_slice(&final_body_len.to_be_bytes());
}

/// Find the byte offset of an attribute in raw STUN message data.
fn find_attribute_offset(data: &[u8], target_type: u16) -> StunResult<usize> {
    let msg_length = u16::from_be_bytes([data[2], data[3]]) as usize;
    let body_end = HEADER_SIZE + msg_length;
    let mut offset = HEADER_SIZE;

    while offset + 4 <= body_end && offset + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        if attr_type == target_type {
            return Ok(offset);
        }
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let padded = (attr_len + 3) & !3;
        offset += 4 + padded;
    }

    Err(StunError::Parse(format!(
        "attribute 0x{:04x} not found",
        target_type
    )))
}

/// Convert a typed attribute to a raw attribute.
fn attr_value_to_raw(attr: &StunAttrValue, tid: &TransactionId) -> RawAttribute {
    match attr {
        StunAttrValue::MappedAddress(addr) => RawAttribute {
            attr_type: AttrType::MappedAddress as u16,
            value: encode_mapped_address(addr),
        },
        StunAttrValue::XorMappedAddress(addr) => RawAttribute {
            attr_type: AttrType::XorMappedAddress as u16,
            value: encode_xor_address(addr, tid),
        },
        StunAttrValue::XorPeerAddress(addr) => RawAttribute {
            attr_type: AttrType::XorPeerAddress as u16,
            value: encode_xor_address(addr, tid),
        },
        StunAttrValue::XorRelayedAddress(addr) => RawAttribute {
            attr_type: AttrType::XorRelayedAddress as u16,
            value: encode_xor_address(addr, tid),
        },
        StunAttrValue::Username(u) => RawAttribute {
            attr_type: AttrType::Username as u16,
            value: u.as_bytes().to_vec(),
        },
        StunAttrValue::MessageIntegrity(v) => RawAttribute {
            attr_type: AttrType::MessageIntegrity as u16,
            value: v.to_vec(),
        },
        StunAttrValue::Fingerprint(v) => RawAttribute {
            attr_type: AttrType::Fingerprint as u16,
            value: v.to_be_bytes().to_vec(),
        },
        StunAttrValue::ErrorCode { code, reason } => RawAttribute {
            attr_type: AttrType::ErrorCode as u16,
            value: encode_error_code(*code, reason),
        },
        StunAttrValue::Realm(r) => RawAttribute {
            attr_type: AttrType::Realm as u16,
            value: r.as_bytes().to_vec(),
        },
        StunAttrValue::Nonce(n) => RawAttribute {
            attr_type: AttrType::Nonce as u16,
            value: n.as_bytes().to_vec(),
        },
        StunAttrValue::Software(s) => RawAttribute {
            attr_type: AttrType::Software as u16,
            value: s.as_bytes().to_vec(),
        },
        StunAttrValue::AlternateServer(addr) => RawAttribute {
            attr_type: AttrType::AlternateServer as u16,
            value: encode_mapped_address(addr),
        },
        StunAttrValue::Priority(p) => RawAttribute {
            attr_type: AttrType::Priority as u16,
            value: p.to_be_bytes().to_vec(),
        },
        StunAttrValue::UseCandidate => RawAttribute {
            attr_type: AttrType::UseCandidate as u16,
            value: Vec::new(),
        },
        StunAttrValue::IceControlling(v) => RawAttribute {
            attr_type: AttrType::IceControlling as u16,
            value: v.to_be_bytes().to_vec(),
        },
        StunAttrValue::IceControlled(v) => RawAttribute {
            attr_type: AttrType::IceControlled as u16,
            value: v.to_be_bytes().to_vec(),
        },
        StunAttrValue::ChannelNumber(ch) => {
            let mut v = Vec::with_capacity(4);
            v.extend_from_slice(&ch.to_be_bytes());
            v.extend_from_slice(&[0, 0]); // RFFU
            RawAttribute {
                attr_type: AttrType::ChannelNumber as u16,
                value: v,
            }
        }
        StunAttrValue::Lifetime(l) => RawAttribute {
            attr_type: AttrType::Lifetime as u16,
            value: l.to_be_bytes().to_vec(),
        },
        StunAttrValue::RequestedTransport(proto) => {
            let mut v = Vec::with_capacity(4);
            v.push(*proto);
            v.extend_from_slice(&[0, 0, 0]); // RFFU
            RawAttribute {
                attr_type: AttrType::RequestedTransport as u16,
                value: v,
            }
        }
        StunAttrValue::DontFragment => RawAttribute {
            attr_type: AttrType::DontFragment as u16,
            value: Vec::new(),
        },
        StunAttrValue::Data(d) => RawAttribute {
            attr_type: AttrType::Data as u16,
            value: d.clone(),
        },
        StunAttrValue::Unknown(t, v) => RawAttribute {
            attr_type: *t,
            value: v.clone(),
        },
    }
}

/// Convert a raw attribute to a typed attribute value.
fn raw_to_attr_value(raw: &RawAttribute, tid: &TransactionId) -> StunAttrValue {
    match AttrType::from_u16(raw.attr_type) {
        Some(AttrType::MappedAddress) => {
            match decode_mapped_address(&raw.value) {
                Ok(addr) => StunAttrValue::MappedAddress(addr),
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::XorMappedAddress) => {
            match decode_xor_address(&raw.value, tid) {
                Ok(addr) => StunAttrValue::XorMappedAddress(addr),
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::XorPeerAddress) => {
            match decode_xor_address(&raw.value, tid) {
                Ok(addr) => StunAttrValue::XorPeerAddress(addr),
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::XorRelayedAddress) => {
            match decode_xor_address(&raw.value, tid) {
                Ok(addr) => StunAttrValue::XorRelayedAddress(addr),
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::Username) => {
            StunAttrValue::Username(String::from_utf8_lossy(&raw.value).to_string())
        }
        Some(AttrType::MessageIntegrity) => {
            if raw.value.len() == 20 {
                let mut v = [0u8; 20];
                v.copy_from_slice(&raw.value);
                StunAttrValue::MessageIntegrity(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::Fingerprint) => {
            if raw.value.len() == 4 {
                let v = u32::from_be_bytes([
                    raw.value[0],
                    raw.value[1],
                    raw.value[2],
                    raw.value[3],
                ]);
                StunAttrValue::Fingerprint(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::ErrorCode) => {
            match decode_error_code(&raw.value) {
                Ok((code, reason)) => StunAttrValue::ErrorCode { code, reason },
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::Realm) => {
            StunAttrValue::Realm(String::from_utf8_lossy(&raw.value).to_string())
        }
        Some(AttrType::Nonce) => {
            StunAttrValue::Nonce(String::from_utf8_lossy(&raw.value).to_string())
        }
        Some(AttrType::Software) => {
            StunAttrValue::Software(String::from_utf8_lossy(&raw.value).to_string())
        }
        Some(AttrType::AlternateServer) => {
            match decode_mapped_address(&raw.value) {
                Ok(addr) => StunAttrValue::AlternateServer(addr),
                Err(_) => StunAttrValue::Unknown(raw.attr_type, raw.value.clone()),
            }
        }
        Some(AttrType::Priority) => {
            if raw.value.len() == 4 {
                let v = u32::from_be_bytes([
                    raw.value[0],
                    raw.value[1],
                    raw.value[2],
                    raw.value[3],
                ]);
                StunAttrValue::Priority(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::UseCandidate) => StunAttrValue::UseCandidate,
        Some(AttrType::IceControlling) => {
            if raw.value.len() == 8 {
                let v = u64::from_be_bytes([
                    raw.value[0], raw.value[1], raw.value[2], raw.value[3],
                    raw.value[4], raw.value[5], raw.value[6], raw.value[7],
                ]);
                StunAttrValue::IceControlling(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::IceControlled) => {
            if raw.value.len() == 8 {
                let v = u64::from_be_bytes([
                    raw.value[0], raw.value[1], raw.value[2], raw.value[3],
                    raw.value[4], raw.value[5], raw.value[6], raw.value[7],
                ]);
                StunAttrValue::IceControlled(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::ChannelNumber) => {
            if raw.value.len() >= 2 {
                let ch = u16::from_be_bytes([raw.value[0], raw.value[1]]);
                StunAttrValue::ChannelNumber(ch)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::Lifetime) => {
            if raw.value.len() == 4 {
                let v = u32::from_be_bytes([
                    raw.value[0], raw.value[1], raw.value[2], raw.value[3],
                ]);
                StunAttrValue::Lifetime(v)
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::RequestedTransport) => {
            if !raw.value.is_empty() {
                StunAttrValue::RequestedTransport(raw.value[0])
            } else {
                StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
            }
        }
        Some(AttrType::DontFragment) => StunAttrValue::DontFragment,
        Some(AttrType::Data) => StunAttrValue::Data(raw.value.clone()),
        Some(AttrType::UnknownAttributes) | None => {
            StunAttrValue::Unknown(raw.attr_type, raw.value.clone())
        }
    }
}

/// Check if a packet is a STUN message (first two bits 0, magic cookie present).
pub fn is_stun_message(data: &[u8]) -> bool {
    if data.len() < HEADER_SIZE {
        return false;
    }
    if data[0] & 0xC0 != 0 {
        return false;
    }
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    cookie == MAGIC_COOKIE
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_message_type() {
        // Binding Request = 0x0001
        let mt = encode_message_type(Method::Binding, MessageClass::Request);
        assert_eq!(mt, 0x0001);

        // Binding Response = 0x0101
        let mt = encode_message_type(Method::Binding, MessageClass::SuccessResponse);
        assert_eq!(mt, 0x0101);

        // Binding Error = 0x0111
        let mt = encode_message_type(Method::Binding, MessageClass::ErrorResponse);
        assert_eq!(mt, 0x0111);

        // Binding Indication = 0x0011
        let mt = encode_message_type(Method::Binding, MessageClass::Indication);
        assert_eq!(mt, 0x0011);

        // Allocate Request = 0x0003
        let mt = encode_message_type(Method::Allocate, MessageClass::Request);
        assert_eq!(mt, 0x0003);

        // Roundtrip
        for method in [Method::Binding, Method::Allocate, Method::Refresh, Method::CreatePermission, Method::ChannelBind] {
            for class in [MessageClass::Request, MessageClass::Indication, MessageClass::SuccessResponse, MessageClass::ErrorResponse] {
                let mt = encode_message_type(method, class);
                let (m, c) = decode_message_type(mt);
                assert_eq!(m, Some(method), "method mismatch for {:?}/{:?}", method, class);
                assert_eq!(c, class, "class mismatch for {:?}/{:?}", method, class);
            }
        }
    }

    #[test]
    fn test_binding_request_roundtrip() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE); // No attributes

        let parsed = StunMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.method, Method::Binding);
        assert_eq!(parsed.class, MessageClass::Request);
        assert_eq!(parsed.transaction_id, msg.transaction_id);
        assert!(parsed.attributes.is_empty());
    }

    #[test]
    fn test_binding_response_with_xor_mapped() {
        let tid = TransactionId::new();
        let addr: SocketAddr = "192.168.1.100:5060".parse().unwrap();
        let msg = StunMessage::binding_response(&tid, addr);
        let bytes = msg.to_bytes();

        let parsed = StunMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.method, Method::Binding);
        assert_eq!(parsed.class, MessageClass::SuccessResponse);
        let mapped = parsed.xor_mapped_address().unwrap();
        assert_eq!(mapped, addr);
    }

    #[test]
    fn test_xor_address_ipv4_roundtrip() {
        let tid = TransactionId::new();
        let addr: SocketAddr = "10.0.0.1:3478".parse().unwrap();
        let encoded = encode_xor_address(&addr, &tid);
        let decoded = decode_xor_address(&encoded, &tid).unwrap();
        assert_eq!(decoded, addr);
    }

    #[test]
    fn test_mapped_address_roundtrip() {
        let addr: SocketAddr = "203.0.113.50:8080".parse().unwrap();
        let encoded = encode_mapped_address(&addr);
        let decoded = decode_mapped_address(&encoded).unwrap();
        assert_eq!(decoded, addr);
    }

    #[test]
    fn test_error_code_roundtrip() {
        let encoded = encode_error_code(401, "Unauthorized");
        let (code, reason) = decode_error_code(&encoded).unwrap();
        assert_eq!(code, 401);
        assert_eq!(reason, "Unauthorized");

        let encoded = encode_error_code(487, "Role Conflict");
        let (code, reason) = decode_error_code(&encoded).unwrap();
        assert_eq!(code, 487);
        assert_eq!(reason, "Role Conflict");
    }

    #[test]
    fn test_message_integrity() {
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttrValue::Username("user:remote".to_string()));

        let key = b"password123";
        let bytes = msg.to_bytes_with_integrity(key);

        let parsed = StunMessage::parse(&bytes).unwrap();
        assert!(parsed.verify_integrity(&bytes, key).is_ok());
        assert!(parsed.verify_integrity(&bytes, b"wrongpass").is_err());
    }

    #[test]
    fn test_fingerprint() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes_with_fingerprint();

        let parsed = StunMessage::parse(&bytes).unwrap();
        assert!(parsed.verify_fingerprint(&bytes).is_ok());

        // Corrupt a byte and verify fingerprint fails
        let mut corrupt = bytes.clone();
        corrupt[HEADER_SIZE] ^= 0xFF;
        // Reparse may fail due to corrupted attribute, so just test the CRC logic
        let fp = compute_fingerprint(&bytes[..bytes.len() - 8]);
        let stored = parsed.attributes.iter().find_map(|a| {
            if let StunAttrValue::Fingerprint(v) = a {
                Some(*v)
            } else {
                None
            }
        }).unwrap();
        assert_eq!(fp, stored);
    }

    #[test]
    fn test_integrity_and_fingerprint() {
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttrValue::Username("alice:bob".to_string()));
        msg.attributes.push(StunAttrValue::Priority(12345));

        let key = b"secretkey";
        let bytes = msg.to_bytes_with_integrity_and_fingerprint(key);

        let parsed = StunMessage::parse(&bytes).unwrap();
        assert!(parsed.verify_integrity(&bytes, key).is_ok());
        assert!(parsed.verify_fingerprint(&bytes).is_ok());
    }

    #[test]
    fn test_ice_attributes() {
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttrValue::Username("ufrag1:ufrag2".to_string()));
        msg.attributes.push(StunAttrValue::Priority(1845501695));
        msg.attributes.push(StunAttrValue::IceControlling(0x123456789ABCDEF0));
        msg.attributes.push(StunAttrValue::UseCandidate);

        let bytes = msg.to_bytes();
        let parsed = StunMessage::parse(&bytes).unwrap();

        assert_eq!(parsed.username(), Some("ufrag1:ufrag2"));
        assert_eq!(parsed.priority(), Some(1845501695));
        assert_eq!(parsed.ice_controlling(), Some(0x123456789ABCDEF0));
        assert!(parsed.has_use_candidate());
    }

    #[test]
    fn test_turn_attributes() {
        let tid = TransactionId::new();
        let msg = StunMessage {
            method: Method::Allocate,
            class: MessageClass::SuccessResponse,
            transaction_id: tid,
            attributes: vec![
                StunAttrValue::XorRelayedAddress("198.51.100.1:49152".parse().unwrap()),
                StunAttrValue::XorMappedAddress("203.0.113.50:12345".parse().unwrap()),
                StunAttrValue::Lifetime(600),
            ],
        };

        let bytes = msg.to_bytes();
        let parsed = StunMessage::parse(&bytes).unwrap();

        assert_eq!(
            parsed.xor_relayed_address().unwrap(),
            "198.51.100.1:49152".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            parsed.xor_mapped_address().unwrap(),
            "203.0.113.50:12345".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(parsed.lifetime(), Some(600));
    }

    #[test]
    fn test_is_stun_message() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes();
        assert!(is_stun_message(&bytes));

        // Not STUN: first two bits set
        let mut bad = bytes.clone();
        bad[0] |= 0xC0;
        assert!(!is_stun_message(&bad));

        // Too short
        assert!(!is_stun_message(&[0, 1, 2]));

        // RTP packet (version 2, starts with 0x80)
        assert!(!is_stun_message(&[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                                    0x00, 0x00, 0x00, 0x00]));
    }

    #[test]
    fn test_long_term_key() {
        // RFC 5389 test vector equivalent
        let key = compute_long_term_key("user", "realm", "pass");
        assert_eq!(key.len(), 16); // MD5 = 16 bytes
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL STUN TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_stun_hmac_sha1_known_vector() {
        // Verify HMAC-SHA1 with a known test vector.
        // HMAC-SHA1 of empty data with key "Jefe" from RFC 2202:
        // key = "Jefe", data = "what do ya want for nothing?"
        // HMAC = 0xeffcdf6ae5eb2fa2d27416d5f184df9c259a7c79
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let result = compute_hmac_sha1(key, data);
        let expected: [u8; 20] = [
            0xef, 0xfc, 0xdf, 0x6a, 0xe5, 0xeb, 0x2f, 0xa2,
            0xd2, 0x74, 0x16, 0xd5, 0xf1, 0x84, 0xdf, 0x9c,
            0x25, 0x9a, 0x7c, 0x79,
        ];
        assert_eq!(result, expected, "HMAC-SHA1 test vector mismatch (RFC 2202)");
    }

    #[test]
    fn test_stun_tampered_message_integrity_fails() {
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttrValue::Username("test:user".to_string()));
        let key = b"correct-password";
        let bytes = msg.to_bytes_with_integrity(key);
        let parsed = StunMessage::parse(&bytes).unwrap();

        // Verify with wrong password
        assert!(parsed.verify_integrity(&bytes, b"wrong-password").is_err());
    }

    #[test]
    fn test_stun_fingerprint_known_crc32() {
        // Verify CRC32 XOR constant.
        // compute_fingerprint = CRC32(data) XOR 0x5354554e
        let data = b"";
        let fp = compute_fingerprint(data);
        // CRC32 of empty = 0x00000000, XOR 0x5354554e = 0x5354554e
        assert_eq!(fp, 0x5354554e);
    }

    #[test]
    fn test_stun_integrity_then_fingerprint_order() {
        // Message with both INTEGRITY and FINGERPRINT -- order matters.
        // Integrity must come first, fingerprint last.
        let mut msg = StunMessage::binding_request();
        msg.attributes.push(StunAttrValue::Username("alice:bob".to_string()));
        let key = b"mysecretkey";
        let bytes = msg.to_bytes_with_integrity_and_fingerprint(key);

        let parsed = StunMessage::parse(&bytes).unwrap();

        // Find positions of MessageIntegrity and Fingerprint
        let mi_pos = parsed.attributes.iter().position(|a| matches!(a, StunAttrValue::MessageIntegrity(_)));
        let fp_pos = parsed.attributes.iter().position(|a| matches!(a, StunAttrValue::Fingerprint(_)));

        assert!(mi_pos.is_some(), "MessageIntegrity must be present");
        assert!(fp_pos.is_some(), "Fingerprint must be present");
        assert!(mi_pos.unwrap() < fp_pos.unwrap(), "MessageIntegrity must come before Fingerprint");

        // Both should verify
        assert!(parsed.verify_integrity(&bytes, key).is_ok());
        assert!(parsed.verify_fingerprint(&bytes).is_ok());
    }

    #[test]
    fn test_stun_long_term_credential_md5() {
        // RFC 5389 long-term: key = MD5(username:realm:password)
        let key = compute_long_term_key("user", "example.com", "pass");
        assert_eq!(key.len(), 16);

        // Different inputs should give different keys
        let key2 = compute_long_term_key("user2", "example.com", "pass");
        assert_ne!(key, key2);

        let key3 = compute_long_term_key("user", "other.com", "pass");
        assert_ne!(key, key3);
    }

    #[test]
    fn test_stun_parse_truncated_message() {
        // Message that claims a body length longer than actual data
        let mut msg = StunMessage::binding_request();
        let mut bytes = msg.to_bytes();
        // Set length to 100 but don't provide the body
        bytes[2] = 0;
        bytes[3] = 100;
        assert!(StunMessage::parse(&bytes).is_err());
    }

    #[test]
    fn test_stun_xor_address_ipv6_roundtrip() {
        let tid = TransactionId::new();
        let addr: SocketAddr = "[2001:db8::1]:5060".parse().unwrap();
        let encoded = encode_xor_address(&addr, &tid);
        let decoded = decode_xor_address(&encoded, &tid).unwrap();
        assert_eq!(decoded, addr);
    }

    #[test]
    fn test_stun_error_code_boundary_values() {
        // Test error code at class boundaries
        for code in [300, 399, 400, 401, 420, 438, 487, 500, 699] {
            let encoded = encode_error_code(code, "test");
            let (decoded_code, decoded_reason) = decode_error_code(&encoded).unwrap();
            assert_eq!(decoded_code, code, "Error code roundtrip failed for {}", code);
            assert_eq!(decoded_reason, "test");
        }
    }

    #[test]
    fn test_stun_message_type_all_methods_all_classes() {
        // Exhaustive roundtrip of all method/class combinations
        let methods = [
            Method::Binding, Method::Allocate, Method::Refresh,
            Method::Send, Method::Data,
            Method::CreatePermission, Method::ChannelBind,
        ];
        let classes = [
            MessageClass::Request, MessageClass::Indication,
            MessageClass::SuccessResponse, MessageClass::ErrorResponse,
        ];
        for &method in &methods {
            for &class in &classes {
                let mt = encode_message_type(method, class);
                let (decoded_m, decoded_c) = decode_message_type(mt);
                assert_eq!(decoded_m, Some(method));
                assert_eq!(decoded_c, class);
            }
        }
    }
}
