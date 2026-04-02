//! IAX2 (Inter-Asterisk eXchange v2) channel driver.
//!
//! Port of `channels/chan_iax2.c` and associated `iax2/` headers. Implements
//! the IAX2 protocol as specified in RFC 5456.
//!
//! IAX2 multiplexes signaling and media over a single UDP connection (default
//! port 4569). Frame formats:
//!
//! - **Full frame**: reliable, 12-byte header + IE data.
//! - **Mini frame**: unreliable voice shorthand, 4-byte header.
//! - **Meta frame**: trunk/video multiplexing.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use parking_lot::RwLock;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{
    AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame,
};

// ---------------------------------------------------------------------------
// Constants (from iax2.h)
// ---------------------------------------------------------------------------

/// Default IAX2 UDP port.
pub const IAX2_DEFAULT_PORT: u16 = 4569;

/// IAX protocol version.
pub const IAX_PROTO_VERSION: u16 = 2;

/// Maximum call numbers (protocol limit is 32768 -- 15 bits).
pub const IAX_MAX_CALLS: u16 = 32768;

/// High bit set means this is a full frame.
pub const IAX_FLAG_FULL: u16 = 0x8000;

/// High bit set on dcallno means retransmission.
pub const IAX_FLAG_RETRANS: u16 = 0x8000;

// ---------------------------------------------------------------------------
// IAX2 frame types (matches AST_FRAME_* numbering for on-wire compat)
// ---------------------------------------------------------------------------

/// IAX2 on-wire frame type values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Iax2FrameType {
    DtmfEnd = 1,
    Voice = 2,
    Video = 3,
    Control = 4,
    Null = 5,
    Iax = 6,
    Text = 7,
    Image = 8,
    Html = 9,
    Cng = 10,
    Modem = 11,
    DtmfBegin = 12,
}

impl Iax2FrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::DtmfEnd),
            2 => Some(Self::Voice),
            3 => Some(Self::Video),
            4 => Some(Self::Control),
            5 => Some(Self::Null),
            6 => Some(Self::Iax),
            7 => Some(Self::Text),
            8 => Some(Self::Image),
            9 => Some(Self::Html),
            10 => Some(Self::Cng),
            11 => Some(Self::Modem),
            12 => Some(Self::DtmfBegin),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// IAX2 commands (subclass of Iax frame type)
// ---------------------------------------------------------------------------

/// IAX command subclasses (when frame type == IAX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum IaxCommand {
    New = 1,
    Ping = 2,
    Pong = 3,
    Ack = 4,
    Hangup = 5,
    Reject = 6,
    Accept = 7,
    AuthReq = 8,
    AuthRep = 9,
    Inval = 10,
    LagRq = 11,
    LagRp = 12,
    RegReq = 13,
    RegAuth = 14,
    RegAck = 15,
    RegRej = 16,
    RegRel = 17,
    Vnak = 18,
    DpReq = 19,
    DpRep = 20,
    Dial = 21,
    TxReq = 22,
    TxCnt = 23,
    TxAcc = 24,
    TxReady = 25,
    TxRel = 26,
    TxRej = 27,
    Quelch = 28,
    Unquelch = 29,
    Poke = 30,
    Page = 31,
    Mwi = 32,
    Unsupport = 33,
    Transfer = 34,
    Provision = 35,
    FwDownl = 36,
    FwData = 37,
    TxMedia = 38,
    RtKey = 39,
    CallToken = 40,
}

impl IaxCommand {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::New),
            2 => Some(Self::Ping),
            3 => Some(Self::Pong),
            4 => Some(Self::Ack),
            5 => Some(Self::Hangup),
            6 => Some(Self::Reject),
            7 => Some(Self::Accept),
            8 => Some(Self::AuthReq),
            9 => Some(Self::AuthRep),
            10 => Some(Self::Inval),
            11 => Some(Self::LagRq),
            12 => Some(Self::LagRp),
            13 => Some(Self::RegReq),
            14 => Some(Self::RegAuth),
            15 => Some(Self::RegAck),
            16 => Some(Self::RegRej),
            17 => Some(Self::RegRel),
            18 => Some(Self::Vnak),
            19 => Some(Self::DpReq),
            20 => Some(Self::DpRep),
            21 => Some(Self::Dial),
            22 => Some(Self::TxReq),
            23 => Some(Self::TxCnt),
            24 => Some(Self::TxAcc),
            25 => Some(Self::TxReady),
            26 => Some(Self::TxRel),
            27 => Some(Self::TxRej),
            28 => Some(Self::Quelch),
            29 => Some(Self::Unquelch),
            30 => Some(Self::Poke),
            31 => Some(Self::Page),
            32 => Some(Self::Mwi),
            33 => Some(Self::Unsupport),
            34 => Some(Self::Transfer),
            35 => Some(Self::Provision),
            36 => Some(Self::FwDownl),
            37 => Some(Self::FwData),
            38 => Some(Self::TxMedia),
            39 => Some(Self::RtKey),
            40 => Some(Self::CallToken),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// IAX2 information element IDs
// ---------------------------------------------------------------------------

/// IAX2 Information Element types.
pub mod ie {
    pub const CALLED_NUMBER: u8 = 1;
    pub const CALLING_NUMBER: u8 = 2;
    pub const CALLING_ANI: u8 = 3;
    pub const CALLING_NAME: u8 = 4;
    pub const CALLED_CONTEXT: u8 = 5;
    pub const USERNAME: u8 = 6;
    pub const PASSWORD: u8 = 7;
    pub const CAPABILITY: u8 = 8;
    pub const FORMAT: u8 = 9;
    pub const LANGUAGE: u8 = 10;
    pub const VERSION: u8 = 11;
    pub const ADSICPE: u8 = 12;
    pub const DNID: u8 = 13;
    pub const AUTHMETHODS: u8 = 14;
    pub const CHALLENGE: u8 = 15;
    pub const MD5_RESULT: u8 = 16;
    pub const RSA_RESULT: u8 = 17;
    pub const APPARENT_ADDR: u8 = 18;
    pub const REFRESH: u8 = 19;
    pub const DPSTATUS: u8 = 20;
    pub const CALLNO: u8 = 21;
    pub const CAUSE: u8 = 22;
    pub const IAX_UNKNOWN: u8 = 23;
    pub const MSGCOUNT: u8 = 24;
    pub const AUTOANSWER: u8 = 25;
    pub const MUSICONHOLD: u8 = 26;
    pub const TRANSFERID: u8 = 27;
    pub const RDNIS: u8 = 28;
    pub const DATETIME: u8 = 31;
    pub const CALLINGPRES: u8 = 38;
    pub const CALLINGTON: u8 = 39;
    pub const CALLINGTNS: u8 = 40;
    pub const SAMPLINGRATE: u8 = 41;
    pub const CAUSECODE: u8 = 42;
    pub const ENCRYPTION: u8 = 43;
    pub const ENCKEY: u8 = 44;
    pub const CODEC_PREFS: u8 = 45;
    pub const RR_JITTER: u8 = 46;
    pub const RR_LOSS: u8 = 47;
    pub const RR_PKTS: u8 = 48;
    pub const RR_DELAY: u8 = 49;
    pub const RR_DROPPED: u8 = 50;
    pub const RR_OOO: u8 = 51;
    pub const VARIABLE: u8 = 52;
    pub const OSPTOKEN: u8 = 53;
    pub const CALLTOKEN: u8 = 54;
}

/// Authentication methods bitmask.
pub mod auth_method {
    pub const PLAINTEXT: u16 = 1 << 0;
    pub const MD5: u16 = 1 << 1;
    pub const RSA: u16 = 1 << 2;
}

/// Meta frame types.
pub const IAX_META_TRUNK: u8 = 1;
pub const IAX_META_VIDEO: u8 = 2;

// ---------------------------------------------------------------------------
// Frame structures
// ---------------------------------------------------------------------------

/// Parsed IAX2 full frame header (12 bytes on wire).
#[derive(Debug, Clone)]
pub struct Iax2FullHeader {
    /// Source call number (lower 15 bits). High bit is always set for full frames.
    pub src_call_number: u16,
    /// Destination call number (lower 15 bits). High bit = retransmission flag.
    pub dst_call_number: u16,
    /// Whether this is a retransmission.
    pub retransmit: bool,
    /// 32-bit timestamp in milliseconds.
    pub timestamp: u32,
    /// Outgoing sequence number.
    pub oseqno: u8,
    /// Next expected incoming sequence number.
    pub iseqno: u8,
    /// Frame type.
    pub frame_type: u8,
    /// Compressed subclass.
    pub subclass: u8,
}

impl Iax2FullHeader {
    /// Full frame header is 12 bytes.
    pub const SIZE: usize = 12;

    /// Parse a full frame header from a byte slice. The slice must be at least
    /// 12 bytes.
    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < Self::SIZE {
            return Err(AsteriskError::Parse(format!(
                "IAX2 full frame too short: {} bytes",
                data.len()
            )));
        }

        let scallno = u16::from_be_bytes([data[0], data[1]]);
        if scallno & IAX_FLAG_FULL == 0 {
            return Err(AsteriskError::Parse(
                "Not a full frame (high bit not set)".into(),
            ));
        }

        let dcallno = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let oseqno = data[8];
        let iseqno = data[9];
        let frame_type = data[10];
        let subclass = data[11];

        Ok(Self {
            src_call_number: scallno & !IAX_FLAG_FULL,
            dst_call_number: dcallno & !IAX_FLAG_RETRANS,
            retransmit: dcallno & IAX_FLAG_RETRANS != 0,
            timestamp,
            oseqno,
            iseqno,
            frame_type,
            subclass,
        })
    }

    /// Serialize to 12 bytes.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        let scallno = self.src_call_number | IAX_FLAG_FULL;
        let dcallno = self.dst_call_number
            | if self.retransmit {
                IAX_FLAG_RETRANS
            } else {
                0
            };
        buf[0..2].copy_from_slice(&scallno.to_be_bytes());
        buf[2..4].copy_from_slice(&dcallno.to_be_bytes());
        buf[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        buf[8] = self.oseqno;
        buf[9] = self.iseqno;
        buf[10] = self.frame_type;
        buf[11] = self.subclass;
        buf
    }
}

/// Parsed IAX2 mini frame header (4 bytes on wire).
///
/// Used for voice data when the full header is unnecessary.
/// Frame type is implicitly Voice, subclass is remembered from the last
/// full voice frame.
#[derive(Debug, Clone)]
pub struct Iax2MiniHeader {
    /// Source call number (lower 15 bits). High bit must be 0.
    pub call_number: u16,
    /// 16-bit timestamp (high 16 bits inherited from last full frame).
    pub timestamp: u16,
}

impl Iax2MiniHeader {
    pub const SIZE: usize = 4;

    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < Self::SIZE {
            return Err(AsteriskError::Parse(format!(
                "IAX2 mini frame too short: {} bytes",
                data.len()
            )));
        }

        let callno = u16::from_be_bytes([data[0], data[1]]);
        if callno & IAX_FLAG_FULL != 0 {
            return Err(AsteriskError::Parse(
                "Not a mini frame (high bit is set)".into(),
            ));
        }
        if callno == 0 {
            return Err(AsteriskError::Parse(
                "Not a mini frame (callno is zero -- meta frame)".into(),
            ));
        }

        let ts = u16::from_be_bytes([data[2], data[3]]);

        Ok(Self {
            call_number: callno,
            timestamp: ts,
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..2].copy_from_slice(&(self.call_number & !IAX_FLAG_FULL).to_be_bytes());
        buf[2..4].copy_from_slice(&self.timestamp.to_be_bytes());
        buf
    }
}

/// Parsed IAX2 meta frame header (4 bytes on wire).
///
/// Used for trunk mode and video multiplexing. Identified by first two
/// bytes being zero.
#[derive(Debug, Clone)]
pub struct Iax2MetaHeader {
    /// Always 0x0000.
    pub zeros: u16,
    /// Meta command (1 = trunk, 2 = video).
    pub meta_cmd: u8,
    /// Command data.
    pub cmd_data: u8,
}

impl Iax2MetaHeader {
    pub const SIZE: usize = 4;

    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < Self::SIZE {
            return Err(AsteriskError::Parse(format!(
                "IAX2 meta frame too short: {} bytes",
                data.len()
            )));
        }

        let zeros = u16::from_be_bytes([data[0], data[1]]);
        if zeros != 0 {
            return Err(AsteriskError::Parse(
                "Not a meta frame (first two bytes nonzero)".into(),
            ));
        }

        Ok(Self {
            zeros: 0,
            meta_cmd: data[2],
            cmd_data: data[3],
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        [0, 0, self.meta_cmd, self.cmd_data]
    }
}

/// Trunk meta frame sub-header: 32-bit timestamp for all contained entries.
#[derive(Debug, Clone)]
pub struct Iax2MetaTrunkHeader {
    pub timestamp: u32,
}

impl Iax2MetaTrunkHeader {
    pub const SIZE: usize = 4;

    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < Self::SIZE {
            return Err(AsteriskError::Parse("Trunk header too short".into()));
        }
        Ok(Self {
            timestamp: u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
        })
    }
}

/// A single entry inside a trunk meta frame (supermini format).
#[derive(Debug, Clone)]
pub struct Iax2TrunkEntry {
    /// Call number for this entry.
    pub call_number: u16,
    /// Length of voice data following.
    pub data_len: u16,
}

impl Iax2TrunkEntry {
    pub const SIZE: usize = 4;

    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < Self::SIZE {
            return Err(AsteriskError::Parse("Trunk entry too short".into()));
        }
        Ok(Self {
            call_number: u16::from_be_bytes([data[0], data[1]]),
            data_len: u16::from_be_bytes([data[2], data[3]]),
        })
    }
}

// ---------------------------------------------------------------------------
// Information Element parsing
// ---------------------------------------------------------------------------

/// A single parsed IAX2 information element.
#[derive(Debug, Clone)]
pub struct InformationElement {
    pub ie_type: u8,
    pub data: Bytes,
}

impl InformationElement {
    /// Parse a TLV-encoded IE from the given slice. Returns the IE and the
    /// number of bytes consumed.
    pub fn parse(data: &[u8]) -> Result<(Self, usize), AsteriskError> {
        if data.len() < 2 {
            return Err(AsteriskError::Parse("IE too short for type+len".into()));
        }
        let ie_type = data[0];
        let ie_len = data[1] as usize;
        let total = 2 + ie_len;
        if data.len() < total {
            return Err(AsteriskError::Parse(format!(
                "IE {} claims {} bytes but only {} available",
                ie_type,
                ie_len,
                data.len() - 2
            )));
        }
        Ok((
            Self {
                ie_type,
                data: Bytes::copy_from_slice(&data[2..total]),
            },
            total,
        ))
    }

    /// Get the data as a UTF-8 string (common for string IEs).
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }

    /// Get the data as a u16 (big-endian).
    pub fn as_u16(&self) -> Option<u16> {
        if self.data.len() >= 2 {
            Some(u16::from_be_bytes([self.data[0], self.data[1]]))
        } else {
            None
        }
    }

    /// Get the data as a u32 (big-endian).
    pub fn as_u32(&self) -> Option<u32> {
        if self.data.len() >= 4 {
            Some(u32::from_be_bytes([
                self.data[0],
                self.data[1],
                self.data[2],
                self.data[3],
            ]))
        } else {
            None
        }
    }
}

/// Parse all information elements from a byte slice (after the full frame
/// header).
pub fn parse_information_elements(data: &[u8]) -> Result<Vec<InformationElement>, AsteriskError> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (element, consumed) = InformationElement::parse(&data[offset..])?;
        result.push(element);
        offset += consumed;
    }
    Ok(result)
}

/// Serialize a list of information elements into a byte buffer.
///
/// IE data is limited to 255 bytes by the protocol (length field is u8).
/// Data longer than 255 bytes is truncated with a warning.
pub fn serialize_information_elements(ies: &[InformationElement]) -> BytesMut {
    let mut buf = BytesMut::new();
    for elem in ies {
        let len = elem.data.len().min(255);
        if elem.data.len() > 255 {
            tracing::warn!(
                ie_type = elem.ie_type,
                actual_len = elem.data.len(),
                "IAX2 IE data exceeds 255 bytes; truncating"
            );
        }
        buf.put_u8(elem.ie_type);
        buf.put_u8(len as u8);
        buf.put_slice(&elem.data[..len]);
    }
    buf
}

/// Builder helper: create a string IE.
pub fn ie_string(ie_type: u8, value: &str) -> InformationElement {
    InformationElement {
        ie_type,
        data: Bytes::copy_from_slice(value.as_bytes()),
    }
}

/// Builder helper: create a u16 IE.
pub fn ie_u16(ie_type: u8, value: u16) -> InformationElement {
    InformationElement {
        ie_type,
        data: Bytes::copy_from_slice(&value.to_be_bytes()),
    }
}

/// Builder helper: create a u32 IE.
pub fn ie_u32(ie_type: u8, value: u32) -> InformationElement {
    InformationElement {
        ie_type,
        data: Bytes::copy_from_slice(&value.to_be_bytes()),
    }
}

/// Builder helper: create a u8 IE.
pub fn ie_byte(ie_type: u8, value: u8) -> InformationElement {
    InformationElement {
        ie_type,
        data: Bytes::copy_from_slice(&[value]),
    }
}

/// Builder helper: create an empty IE (presence flag only).
pub fn ie_empty(ie_type: u8) -> InformationElement {
    InformationElement {
        ie_type,
        data: Bytes::new(),
    }
}

// ---------------------------------------------------------------------------
// Parsed IAX2 IEs collection
// ---------------------------------------------------------------------------

/// Parsed collection of information elements from an IAX2 frame (similar to
/// `struct iax_ies` in C).
#[derive(Debug, Clone, Default)]
pub struct Iax2Ies {
    pub called_number: Option<String>,
    pub calling_number: Option<String>,
    pub calling_ani: Option<String>,
    pub calling_name: Option<String>,
    pub called_context: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub capability: Option<u32>,
    pub format: Option<u32>,
    pub language: Option<String>,
    pub version: Option<u16>,
    pub authmethods: Option<u16>,
    pub challenge: Option<String>,
    pub md5_result: Option<String>,
    pub rsa_result: Option<String>,
    pub refresh: Option<u16>,
    pub cause: Option<String>,
    pub causecode: Option<u8>,
    pub calltoken: Option<Bytes>,
}

impl Iax2Ies {
    /// Parse from raw IE bytes.
    pub fn from_elements(elements: &[InformationElement]) -> Self {
        let mut ies = Self::default();
        for elem in elements {
            match elem.ie_type {
                ie::CALLED_NUMBER => ies.called_number = elem.as_str().map(|s| s.to_string()),
                ie::CALLING_NUMBER => ies.calling_number = elem.as_str().map(|s| s.to_string()),
                ie::CALLING_ANI => ies.calling_ani = elem.as_str().map(|s| s.to_string()),
                ie::CALLING_NAME => ies.calling_name = elem.as_str().map(|s| s.to_string()),
                ie::CALLED_CONTEXT => ies.called_context = elem.as_str().map(|s| s.to_string()),
                ie::USERNAME => ies.username = elem.as_str().map(|s| s.to_string()),
                ie::PASSWORD => ies.password = elem.as_str().map(|s| s.to_string()),
                ie::CAPABILITY => ies.capability = elem.as_u32(),
                ie::FORMAT => ies.format = elem.as_u32(),
                ie::LANGUAGE => ies.language = elem.as_str().map(|s| s.to_string()),
                ie::VERSION => ies.version = elem.as_u16(),
                ie::AUTHMETHODS => ies.authmethods = elem.as_u16(),
                ie::CHALLENGE => ies.challenge = elem.as_str().map(|s| s.to_string()),
                ie::MD5_RESULT => ies.md5_result = elem.as_str().map(|s| s.to_string()),
                ie::RSA_RESULT => ies.rsa_result = elem.as_str().map(|s| s.to_string()),
                ie::REFRESH => ies.refresh = elem.as_u16(),
                ie::CAUSE => ies.cause = elem.as_str().map(|s| s.to_string()),
                ie::CAUSECODE => {
                    if !elem.data.is_empty() {
                        ies.causecode = Some(elem.data[0]);
                    }
                }
                ie::CALLTOKEN => ies.calltoken = Some(elem.data.clone()),
                _ => {
                    // Unknown / unhandled IE -- skip.
                }
            }
        }
        ies
    }
}

// ---------------------------------------------------------------------------
// Top-level packet discrimination
// ---------------------------------------------------------------------------

/// The three types of IAX2 packets that can arrive on the wire.
#[derive(Debug)]
pub enum Iax2Packet {
    /// Reliable full frame.
    Full {
        header: Iax2FullHeader,
        ie_data: Bytes,
    },
    /// Unreliable mini voice frame.
    Mini {
        header: Iax2MiniHeader,
        voice_data: Bytes,
    },
    /// Meta frame (trunk or video).
    Meta {
        header: Iax2MetaHeader,
        payload: Bytes,
    },
}

/// Parse a raw UDP datagram into an `Iax2Packet`.
pub fn parse_iax2_packet(data: &[u8]) -> Result<Iax2Packet, AsteriskError> {
    if data.len() < 4 {
        return Err(AsteriskError::Parse("IAX2 packet too short".into()));
    }

    let first_two = u16::from_be_bytes([data[0], data[1]]);

    if first_two == 0 {
        // Meta frame.
        let header = Iax2MetaHeader::parse(data)?;
        let payload = if data.len() > Iax2MetaHeader::SIZE {
            Bytes::copy_from_slice(&data[Iax2MetaHeader::SIZE..])
        } else {
            Bytes::new()
        };
        Ok(Iax2Packet::Meta { header, payload })
    } else if first_two & IAX_FLAG_FULL != 0 {
        // Full frame.
        let header = Iax2FullHeader::parse(data)?;
        let ie_data = if data.len() > Iax2FullHeader::SIZE {
            Bytes::copy_from_slice(&data[Iax2FullHeader::SIZE..])
        } else {
            Bytes::new()
        };
        Ok(Iax2Packet::Full { header, ie_data })
    } else {
        // Mini frame.
        let header = Iax2MiniHeader::parse(data)?;
        let voice_data = if data.len() > Iax2MiniHeader::SIZE {
            Bytes::copy_from_slice(&data[Iax2MiniHeader::SIZE..])
        } else {
            Bytes::new()
        };
        Ok(Iax2Packet::Mini {
            header,
            voice_data,
        })
    }
}

// ---------------------------------------------------------------------------
// Full frame construction helpers
// ---------------------------------------------------------------------------

/// Build a full frame packet (header + IE payload).
pub fn build_full_frame(header: &Iax2FullHeader, ie_data: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(Iax2FullHeader::SIZE + ie_data.len());
    buf.put_slice(&header.to_bytes());
    buf.put_slice(ie_data);
    buf.freeze()
}

/// Build a mini frame packet (header + voice payload).
pub fn build_mini_frame(header: &Iax2MiniHeader, voice_data: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(Iax2MiniHeader::SIZE + voice_data.len());
    buf.put_slice(&header.to_bytes());
    buf.put_slice(voice_data);
    buf.freeze()
}

// ---------------------------------------------------------------------------
// MD5 authentication
// ---------------------------------------------------------------------------

/// Compute MD5 challenge response: MD5(challenge + password).
pub fn iax2_md5_response(challenge: &str, password: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(challenge.as_bytes());
    hasher.update(password.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

// ---------------------------------------------------------------------------
// Jitter buffer (simplified)
// ---------------------------------------------------------------------------

/// Simple jitter buffer for IAX2 voice frames.
///
/// Buffers incoming voice frames and re-orders them by timestamp to smooth
/// out network jitter.
#[derive(Debug)]
pub struct JitterBuffer {
    /// Buffered frames, keyed by timestamp.
    buffer: std::collections::BTreeMap<u32, Frame>,
    /// Target jitter buffer depth in milliseconds.
    target_ms: u32,
    /// Minimum observed timestamp.
    min_ts: Option<u32>,
    /// Last timestamp delivered.
    last_delivered: u32,
}

impl JitterBuffer {
    pub fn new(target_ms: u32) -> Self {
        Self {
            buffer: std::collections::BTreeMap::new(),
            target_ms,
            min_ts: None,
            last_delivered: 0,
        }
    }

    /// Insert a frame into the jitter buffer.
    pub fn put(&mut self, timestamp: u32, frame: Frame) {
        if self.min_ts.is_none() {
            self.min_ts = Some(timestamp);
        }
        self.buffer.insert(timestamp, frame);
    }

    /// Try to get the next frame that should be played out.
    pub fn get(&mut self) -> Option<Frame> {
        let min = *self.min_ts.as_ref()?;
        let threshold = min.wrapping_add(self.target_ms);

        // Only deliver if we have frames past the jitter threshold.
        let first_key = *self.buffer.keys().next()?;
        if first_key <= threshold || self.buffer.len() > 100 {
            let (ts, frame) = self.buffer.pop_first()?;
            self.last_delivered = ts;
            Some(frame)
        } else {
            None
        }
    }

    /// Number of frames currently buffered.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

// ---------------------------------------------------------------------------
// IAX2 call state
// ---------------------------------------------------------------------------

/// State of a single IAX2 call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iax2CallState {
    /// NEW sent or received.
    Initiated,
    /// ACCEPT received.
    Accepted,
    /// Call is up.
    Up,
    /// Call is being torn down.
    Hangup,
    /// Call is terminated.
    Down,
}

/// Per-call private data.
#[allow(dead_code)]
struct Iax2Private {
    /// Our call number.
    our_callno: u16,
    /// Their call number.
    their_callno: u16,
    /// Remote address.
    remote_addr: SocketAddr,
    /// Call state.
    state: Iax2CallState,
    /// Outgoing sequence number.
    oseqno: AtomicU32,
    /// Next expected incoming sequence.
    iseqno: AtomicU32,
    /// Timestamp base (ms since call start).
    ts_base: std::time::Instant,
    /// Frame channel for delivering frames to `read_frame`.
    frame_tx: mpsc::Sender<Frame>,
    /// Frame receiver.
    frame_rx: Mutex<mpsc::Receiver<Frame>>,
    /// Jitter buffer.
    jitter_buf: Mutex<JitterBuffer>,
    /// Last voice codec format from full frame.
    voice_format: AtomicU32,
}

impl fmt::Debug for Iax2Private {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Iax2Private")
            .field("our_callno", &self.our_callno)
            .field("their_callno", &self.their_callno)
            .field("remote_addr", &self.remote_addr)
            .field("state", &self.state)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Channel driver
// ---------------------------------------------------------------------------

/// IAX2 channel driver.
///
/// Port of `chan_iax2.c`. Implements the IAX2 protocol for Asterisk-to-Asterisk
/// connectivity over a single UDP socket.
pub struct Iax2Driver {
    /// UDP socket (shared for all calls).
    socket: Option<Arc<UdpSocket>>,
    /// Local listen address.
    local_addr: SocketAddr,
    /// Active channels keyed by channel unique ID.
    channels: RwLock<HashMap<String, Arc<Iax2Private>>>,
    /// Call number allocator.
    next_callno: AtomicU16,
}

impl fmt::Debug for Iax2Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Iax2Driver")
            .field("local_addr", &self.local_addr)
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl Iax2Driver {
    /// Create a new IAX2 driver with the specified listen address.
    pub fn new(local_addr: SocketAddr) -> Self {
        Self {
            socket: None,
            local_addr,
            channels: RwLock::new(HashMap::new()),
            next_callno: AtomicU16::new(1),
        }
    }

    /// Bind the UDP socket.
    pub async fn bind(&mut self) -> AsteriskResult<()> {
        let socket = UdpSocket::bind(self.local_addr).await?;
        self.local_addr = socket.local_addr()?;
        self.socket = Some(Arc::new(socket));
        info!(addr = %self.local_addr, "IAX2 socket bound");
        Ok(())
    }

    fn allocate_callno(&self) -> u16 {
        let n = self.next_callno.fetch_add(1, Ordering::Relaxed);
        (n % (IAX_MAX_CALLS - 1)) + 1 // 1..32767
    }

    fn get_private(&self, id: &str) -> Option<Arc<Iax2Private>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<Iax2Private>> {
        self.channels.write().remove(id)
    }

    fn get_socket(&self) -> AsteriskResult<Arc<UdpSocket>> {
        self.socket
            .clone()
            .ok_or_else(|| AsteriskError::Internal("IAX2 socket not bound".into()))
    }

    /// Get the current timestamp (ms since call start) for a call.
    fn current_ts(priv_data: &Iax2Private) -> u32 {
        priv_data.ts_base.elapsed().as_millis() as u32
    }

    /// Build and send a full frame.
    async fn send_full_frame(
        socket: &UdpSocket,
        priv_data: &Iax2Private,
        frame_type: u8,
        subclass: u8,
        ie_data: &[u8],
    ) -> AsteriskResult<()> {
        let oseq = priv_data.oseqno.fetch_add(1, Ordering::Relaxed) as u8;
        let iseq = priv_data.iseqno.load(Ordering::Relaxed) as u8;
        let header = Iax2FullHeader {
            src_call_number: priv_data.our_callno,
            dst_call_number: priv_data.their_callno,
            retransmit: false,
            timestamp: Self::current_ts(priv_data),
            oseqno: oseq,
            iseqno: iseq,
            frame_type,
            subclass,
        };
        let packet = build_full_frame(&header, ie_data);
        socket.send_to(&packet, priv_data.remote_addr).await?;
        Ok(())
    }
}

impl Default for Iax2Driver {
    fn default() -> Self {
        Self::new(SocketAddr::from(([0, 0, 0, 0], IAX2_DEFAULT_PORT)))
    }
}

#[async_trait]
impl ChannelDriver for Iax2Driver {
    fn name(&self) -> &str {
        "IAX2"
    }

    fn description(&self) -> &str {
        "Inter-Asterisk eXchange v2 Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        // dest format: "user@host[:port][/exten[@context]]"
        let (host_part, _exten_part) = match dest.split_once('/') {
            Some((h, e)) => (h, Some(e)),
            None => (dest, None),
        };

        let (_user, host) = match host_part.split_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, host_part),
        };

        let remote_addr: SocketAddr = if host.contains(':') {
            host.parse()
        } else {
            format!("{}:{}", host, IAX2_DEFAULT_PORT).parse()
        }
        .map_err(|e| AsteriskError::InvalidArgument(format!("Bad IAX2 address: {}", e)))?;

        let callno = self.allocate_callno();

        let (frame_tx, frame_rx) = mpsc::channel(256);

        let chan_name = format!("IAX2/{}", dest);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(Iax2Private {
            our_callno: callno,
            their_callno: 0,
            remote_addr,
            state: Iax2CallState::Initiated,
            oseqno: AtomicU32::new(0),
            iseqno: AtomicU32::new(0),
            ts_base: std::time::Instant::now(),
            frame_tx,
            frame_rx: Mutex::new(frame_rx),
            jitter_buf: Mutex::new(JitterBuffer::new(60)),
            voice_format: AtomicU32::new(0),
        });

        self.channels.write().insert(channel_id, priv_data);
        info!(callno, dest, "IAX2 channel requested");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let socket = self.get_socket()?;

        // Build NEW command with IEs.
        let mut ies = Vec::new();
        ies.push(ie_u16(ie::VERSION, IAX_PROTO_VERSION));

        // Parse extension from dest.
        if let Some((_host, exten)) = dest.split_once('/') {
            let (ext, ctx) = match exten.split_once('@') {
                Some((e, c)) => (e, Some(c)),
                None => (exten, None),
            };
            ies.push(ie_string(ie::CALLED_NUMBER, ext));
            if let Some(ctx) = ctx {
                ies.push(ie_string(ie::CALLED_CONTEXT, ctx));
            }
        }

        ies.push(ie_u32(ie::CAPABILITY, 0x04 | 0x08)); // ulaw | alaw
        ies.push(ie_u32(ie::FORMAT, 0x04)); // prefer ulaw

        let ie_bytes = serialize_information_elements(&ies);

        Self::send_full_frame(
            &socket,
            &priv_data,
            Iax2FrameType::Iax as u8,
            IaxCommand::New as u8,
            &ie_bytes,
        )
        .await?;

        channel.set_state(ChannelState::Dialing);
        info!(callno = priv_data.our_callno, dest, "IAX2 NEW sent");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(socket) = &self.socket {
            // Send control ANSWER frame.
            Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::Control as u8,
                4, // AST_CONTROL_ANSWER
                &[],
            )
            .await?;
        }

        channel.answer();
        info!(callno = priv_data.our_callno, "IAX2 call answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = match self.remove_private(channel.unique_id.as_str()) {
            Some(p) => p,
            None => return Ok(()),
        };

        if let Some(socket) = &self.socket {
            let ies = vec![ie_string(ie::CAUSE, "Normal Clearing")];
            let ie_bytes = serialize_information_elements(&ies);
            let _ = Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::Iax as u8,
                IaxCommand::Hangup as u8,
                &ie_bytes,
            )
            .await;
        }

        channel.set_state(ChannelState::Down);
        info!(callno = priv_data.our_callno, "IAX2 call hungup");
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let mut rx = priv_data.frame_rx.lock().await;
        match rx.recv().await {
            Some(frame) => Ok(frame),
            None => Ok(Frame::control(ControlFrame::Hangup)),
        }
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let socket = self.get_socket()?;

        match frame {
            Frame::Voice {
                data,
                ..
            } => {
                // Send as mini frame for efficiency.
                let ts = Self::current_ts(&priv_data);
                let mini = Iax2MiniHeader {
                    call_number: priv_data.our_callno,
                    timestamp: (ts & 0xFFFF) as u16,
                };
                let packet = build_mini_frame(&mini, data);
                socket.send_to(&packet, priv_data.remote_addr).await?;
            }
            _ => {
                // Other frame types are sent as full frames.
                debug!(
                    frame_type = ?frame.frame_type(),
                    "IAX2: non-voice write frame (not yet implemented)"
                );
            }
        }
        Ok(())
    }

    async fn send_digit_begin(&self, channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(socket) = &self.socket {
            Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::DtmfBegin as u8,
                digit as u8,
                &[],
            )
            .await?;
        }
        Ok(())
    }

    async fn send_digit_end(
        &self,
        channel: &mut Channel,
        digit: char,
        _duration: u32,
    ) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(socket) = &self.socket {
            Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::DtmfEnd as u8,
                digit as u8,
                &[],
            )
            .await?;
        }
        Ok(())
    }

    async fn send_text(&self, channel: &mut Channel, text: &str) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(socket) = &self.socket {
            Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::Text as u8,
                0,
                text.as_bytes(),
            )
            .await?;
        }
        Ok(())
    }

    async fn indicate(
        &self,
        channel: &mut Channel,
        condition: i32,
        _data: &[u8],
    ) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(socket) = &self.socket {
            // Control subclass values match ControlFrame repr values.
            Self::send_full_frame(
                socket,
                &priv_data,
                Iax2FrameType::Control as u8,
                condition as u8,
                &[],
            )
            .await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_header_roundtrip() {
        let header = Iax2FullHeader {
            src_call_number: 42,
            dst_call_number: 100,
            retransmit: false,
            timestamp: 1234567,
            oseqno: 5,
            iseqno: 3,
            frame_type: Iax2FrameType::Iax as u8,
            subclass: IaxCommand::New as u8,
        };

        let bytes = header.to_bytes();
        let parsed = Iax2FullHeader::parse(&bytes).unwrap();

        assert_eq!(parsed.src_call_number, 42);
        assert_eq!(parsed.dst_call_number, 100);
        assert!(!parsed.retransmit);
        assert_eq!(parsed.timestamp, 1234567);
        assert_eq!(parsed.oseqno, 5);
        assert_eq!(parsed.iseqno, 3);
        assert_eq!(parsed.frame_type, Iax2FrameType::Iax as u8);
        assert_eq!(parsed.subclass, IaxCommand::New as u8);
    }

    #[test]
    fn test_full_header_retransmit() {
        let header = Iax2FullHeader {
            src_call_number: 1,
            dst_call_number: 2,
            retransmit: true,
            timestamp: 0,
            oseqno: 0,
            iseqno: 0,
            frame_type: Iax2FrameType::Iax as u8,
            subclass: IaxCommand::Ack as u8,
        };
        let bytes = header.to_bytes();
        let parsed = Iax2FullHeader::parse(&bytes).unwrap();
        assert!(parsed.retransmit);
        assert_eq!(parsed.dst_call_number, 2);
    }

    #[test]
    fn test_mini_header_roundtrip() {
        let header = Iax2MiniHeader {
            call_number: 42,
            timestamp: 0x1234,
        };
        let bytes = header.to_bytes();
        let parsed = Iax2MiniHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.call_number, 42);
        assert_eq!(parsed.timestamp, 0x1234);
    }

    #[test]
    fn test_meta_header_parse() {
        let data = [0u8, 0, IAX_META_TRUNK, 0];
        let header = Iax2MetaHeader::parse(&data).unwrap();
        assert_eq!(header.meta_cmd, IAX_META_TRUNK);
    }

    #[test]
    fn test_information_element_roundtrip() {
        let ies = vec![
            ie_string(ie::CALLED_NUMBER, "100"),
            ie_string(ie::CALLING_NAME, "Alice"),
            ie_u16(ie::VERSION, IAX_PROTO_VERSION),
            ie_u32(ie::CAPABILITY, 0x04),
        ];

        let bytes = serialize_information_elements(&ies);
        let parsed = parse_information_elements(&bytes).unwrap();

        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].ie_type, ie::CALLED_NUMBER);
        assert_eq!(parsed[0].as_str(), Some("100"));
        assert_eq!(parsed[1].ie_type, ie::CALLING_NAME);
        assert_eq!(parsed[1].as_str(), Some("Alice"));
        assert_eq!(parsed[2].ie_type, ie::VERSION);
        assert_eq!(parsed[2].as_u16(), Some(2));
        assert_eq!(parsed[3].ie_type, ie::CAPABILITY);
        assert_eq!(parsed[3].as_u32(), Some(0x04));
    }

    #[test]
    fn test_iax2_ies_from_elements() {
        let ies = vec![
            ie_string(ie::CALLED_NUMBER, "100"),
            ie_string(ie::CALLED_CONTEXT, "default"),
            ie_string(ie::CALLING_NAME, "Bob"),
            ie_u16(ie::AUTHMETHODS, auth_method::MD5),
            ie_string(ie::CHALLENGE, "abc123"),
        ];

        let parsed = Iax2Ies::from_elements(&ies);
        assert_eq!(parsed.called_number.as_deref(), Some("100"));
        assert_eq!(parsed.called_context.as_deref(), Some("default"));
        assert_eq!(parsed.calling_name.as_deref(), Some("Bob"));
        assert_eq!(parsed.authmethods, Some(auth_method::MD5));
        assert_eq!(parsed.challenge.as_deref(), Some("abc123"));
    }

    #[test]
    fn test_packet_discrimination_full() {
        let header = Iax2FullHeader {
            src_call_number: 1,
            dst_call_number: 0,
            retransmit: false,
            timestamp: 0,
            oseqno: 0,
            iseqno: 0,
            frame_type: Iax2FrameType::Iax as u8,
            subclass: IaxCommand::New as u8,
        };
        let packet = build_full_frame(&header, &[]);
        let parsed = parse_iax2_packet(&packet).unwrap();
        assert!(matches!(parsed, Iax2Packet::Full { .. }));
    }

    #[test]
    fn test_packet_discrimination_mini() {
        let header = Iax2MiniHeader {
            call_number: 42,
            timestamp: 100,
        };
        let packet = build_mini_frame(&header, &[0u8; 160]);
        let parsed = parse_iax2_packet(&packet).unwrap();
        assert!(matches!(parsed, Iax2Packet::Mini { .. }));
    }

    #[test]
    fn test_packet_discrimination_meta() {
        let meta = Iax2MetaHeader {
            zeros: 0,
            meta_cmd: IAX_META_TRUNK,
            cmd_data: 0,
        };
        let mut pkt = BytesMut::new();
        pkt.put_slice(&meta.to_bytes());
        // Add trunk header
        pkt.put_u32(12345); // trunk timestamp
        let parsed = parse_iax2_packet(&pkt).unwrap();
        assert!(matches!(parsed, Iax2Packet::Meta { .. }));
    }

    #[test]
    fn test_md5_response() {
        let response = iax2_md5_response("challenge123", "secret");
        // Just verify it produces a 32-char hex string.
        assert_eq!(response.len(), 32);
        assert!(response.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_jitter_buffer() {
        let mut jb = JitterBuffer::new(60);
        assert!(jb.is_empty());

        jb.put(0, Frame::voice(0, 160, Bytes::from_static(&[0; 320])));
        jb.put(20, Frame::voice(0, 160, Bytes::from_static(&[0; 320])));
        jb.put(40, Frame::voice(0, 160, Bytes::from_static(&[0; 320])));

        // Frames should come out in order once threshold is met.
        assert_eq!(jb.len(), 3);
        let f = jb.get();
        assert!(f.is_some());
    }

    #[test]
    fn test_trunk_entry_parse() {
        let data = [0x00, 0x2A, 0x00, 0xA0]; // callno=42, len=160
        let entry = Iax2TrunkEntry::parse(&data).unwrap();
        assert_eq!(entry.call_number, 42);
        assert_eq!(entry.data_len, 160);
    }
}
