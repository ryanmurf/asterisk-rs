//! Multicast RTP.
//!
//! Port of `res/res_rtp_multicast.c`. Provides multicast RTP sending,
//! allowing audio to be streamed to a multicast group for paging and
//! intercom systems.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum MulticastRtpError {
    #[error("socket error: {0}")]
    Socket(#[from] std::io::Error),
    #[error("invalid multicast address: {0}")]
    InvalidAddress(String),
    #[error("multicast session not active")]
    NotActive,
}

pub type MulticastRtpResult<T> = Result<T, MulticastRtpError>;

// ---------------------------------------------------------------------------
// Multicast type
// ---------------------------------------------------------------------------

/// The type of multicast stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulticastType {
    /// Basic multicast: raw RTP packets without Linksys control header.
    Basic,
    /// Linksys multicast: prepends a Linksys control header to each packet.
    Linksys,
}

impl MulticastType {
    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "basic" => Some(Self::Basic),
            "linksys" => Some(Self::Linksys),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Linksys control header
// ---------------------------------------------------------------------------

/// Linksys multicast control packet (prepended to RTP).
#[derive(Debug, Clone)]
struct LinksysHeader {
    /// Unique stream identifier.
    unique_id: u32,
    /// Codec in use.
    codec: u8,
    /// Control flags.
    flags: u8,
}

impl LinksysHeader {
    fn new(unique_id: u32, codec: u8) -> Self {
        Self {
            unique_id,
            codec,
            flags: 0,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4);
        buf.extend_from_slice(&self.unique_id.to_be_bytes());
        // Reserved bytes + codec + flags (simplified).
        buf.push(self.codec);
        buf.push(self.flags);
        buf.extend_from_slice(&[0u8; 2]); // padding to 8 bytes
        buf
    }
}

// ---------------------------------------------------------------------------
// Multicast RTP session
// ---------------------------------------------------------------------------

/// A multicast RTP session.
///
/// Sends RTP packets to a multicast group address so that multiple
/// receivers on the network can listen simultaneously.
pub struct MulticastRtp {
    /// The UDP socket bound for sending.
    socket: Option<UdpSocket>,
    /// Target multicast group address.
    pub group_addr: Ipv4Addr,
    /// Target port.
    pub port: u16,
    /// IP TTL for multicast packets.
    pub ttl: u32,
    /// Multicast type (basic or linksys).
    pub multicast_type: MulticastType,
    /// RTP sequence number.
    seq: u16,
    /// RTP timestamp.
    timestamp: u32,
    /// RTP SSRC.
    pub ssrc: u32,
    /// Packets sent counter.
    pub packets_sent: u64,
    /// Bytes sent counter.
    pub bytes_sent: u64,
}

impl MulticastRtp {
    /// Create a new multicast RTP session.
    ///
    /// `group_addr` must be a valid multicast address (224.0.0.0/4).
    pub fn new(
        group_addr: Ipv4Addr,
        port: u16,
        multicast_type: MulticastType,
    ) -> MulticastRtpResult<Self> {
        if !group_addr.is_multicast() {
            return Err(MulticastRtpError::InvalidAddress(group_addr.to_string()));
        }

        Ok(Self {
            socket: None,
            group_addr,
            port,
            ttl: 32,
            multicast_type,
            seq: 0,
            timestamp: 0,
            ssrc: rand::random(),
            packets_sent: 0,
            bytes_sent: 0,
        })
    }

    /// Set the multicast TTL.
    pub fn set_ttl(&mut self, ttl: u32) {
        self.ttl = ttl;
    }

    /// Join the multicast group and bind the socket.
    pub fn join(&mut self) -> MulticastRtpResult<()> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_multicast_ttl_v4(self.ttl)?;
        // Join the multicast group on all interfaces.
        socket.join_multicast_v4(&self.group_addr, &Ipv4Addr::UNSPECIFIED)?;
        info!(
            group = %self.group_addr,
            port = self.port,
            ttl = self.ttl,
            "Joined multicast group"
        );
        self.socket = Some(socket);
        Ok(())
    }

    /// Leave the multicast group.
    pub fn leave(&mut self) -> MulticastRtpResult<()> {
        if let Some(ref socket) = self.socket {
            let _ = socket.leave_multicast_v4(&self.group_addr, &Ipv4Addr::UNSPECIFIED);
            debug!(group = %self.group_addr, "Left multicast group");
        }
        self.socket = None;
        Ok(())
    }

    /// Send an RTP packet with the given payload type and audio data.
    pub fn send_rtp(
        &mut self,
        payload_type: u8,
        payload: &[u8],
        samples: u32,
    ) -> MulticastRtpResult<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or(MulticastRtpError::NotActive)?;

        let dest = SocketAddrV4::new(self.group_addr, self.port);

        // Build the packet based on type.
        let packet = match self.multicast_type {
            MulticastType::Linksys => {
                let header = LinksysHeader::new(self.ssrc, payload_type);
                let rtp = self.build_rtp_header(payload_type, payload);
                let mut pkt = header.to_bytes();
                pkt.extend_from_slice(&rtp);
                pkt
            }
            MulticastType::Basic => self.build_rtp_header(payload_type, payload),
        };

        socket.send_to(&packet, dest)?;

        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(samples);
        self.packets_sent += 1;
        self.bytes_sent += packet.len() as u64;

        Ok(())
    }

    /// Build an RTP header + payload.
    fn build_rtp_header(&self, payload_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + payload.len());
        // V=2, P=0, X=0, CC=0  ->  0x80
        buf.push(0x80);
        // M=0, PT
        buf.push(payload_type & 0x7F);
        // Sequence number (big-endian).
        buf.extend_from_slice(&self.seq.to_be_bytes());
        // Timestamp (big-endian).
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        // SSRC (big-endian).
        buf.extend_from_slice(&self.ssrc.to_be_bytes());
        // Payload.
        buf.extend_from_slice(payload);
        buf
    }

    /// Whether the session is active (socket bound and joined).
    pub fn is_active(&self) -> bool {
        self.socket.is_some()
    }
}

impl Drop for MulticastRtp {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

impl fmt::Debug for MulticastRtp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MulticastRtp")
            .field("group", &self.group_addr)
            .field("port", &self.port)
            .field("type", &self.multicast_type)
            .field("active", &self.is_active())
            .field("packets_sent", &self.packets_sent)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_address() {
        let result = MulticastRtp::new(
            Ipv4Addr::new(10, 0, 0, 1),
            5004,
            MulticastType::Basic,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_multicast_address() {
        let mcast = MulticastRtp::new(
            Ipv4Addr::new(239, 0, 0, 1),
            5004,
            MulticastType::Basic,
        )
        .unwrap();
        assert_eq!(mcast.group_addr, Ipv4Addr::new(239, 0, 0, 1));
        assert_eq!(mcast.port, 5004);
        assert!(!mcast.is_active());
    }

    #[test]
    fn test_multicast_type_parse() {
        assert_eq!(
            MulticastType::from_str_name("basic"),
            Some(MulticastType::Basic)
        );
        assert_eq!(
            MulticastType::from_str_name("Linksys"),
            Some(MulticastType::Linksys)
        );
        assert_eq!(MulticastType::from_str_name("unknown"), None);
    }

    #[test]
    fn test_rtp_header_build() {
        let mcast = MulticastRtp::new(
            Ipv4Addr::new(239, 0, 0, 1),
            5004,
            MulticastType::Basic,
        )
        .unwrap();

        let payload = [0u8; 160];
        let rtp = mcast.build_rtp_header(0, &payload);
        // 12-byte header + 160-byte payload.
        assert_eq!(rtp.len(), 172);
        // Version bits should be 0x80.
        assert_eq!(rtp[0], 0x80);
        // PT should be 0.
        assert_eq!(rtp[1], 0);
    }

    #[test]
    fn test_send_without_join_fails() {
        let mut mcast = MulticastRtp::new(
            Ipv4Addr::new(239, 0, 0, 1),
            5004,
            MulticastType::Basic,
        )
        .unwrap();

        let result = mcast.send_rtp(0, &[0u8; 160], 160);
        assert!(result.is_err());
    }

    #[test]
    fn test_linksys_header() {
        let hdr = LinksysHeader::new(12345, 0);
        let bytes = hdr.to_bytes();
        assert_eq!(bytes.len(), 8);
        // First 4 bytes are the unique ID in big-endian.
        assert_eq!(&bytes[0..4], &12345u32.to_be_bytes());
    }
}
