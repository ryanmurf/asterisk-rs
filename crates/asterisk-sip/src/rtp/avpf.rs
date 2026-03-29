//! AVPF / RTP Feedback (RFC 4585).
//!
//! Audio-Video Profile with Feedback. Implements RTCP feedback messages
//! including NACK (Generic Negative Acknowledgment), PLI (Picture Loss
//! Indication), FIR (Full Intra Request), and TMMBR/TMMBN (Temporary
//! Maximum Media Bitrate Request/Notification).
//!
//! Also provides an RTP retransmission buffer for NACK response.

use std::collections::VecDeque;

use bytes::{BufMut, Bytes, BytesMut};

use asterisk_types::AsteriskError;

// ---------------------------------------------------------------------------
// RTCP Feedback types
// ---------------------------------------------------------------------------

/// RTCP payload types for feedback messages.
pub const RTCP_PT_RTPFB: u8 = 205; // Transport layer feedback (NACK)
pub const RTCP_PT_PSFB: u8 = 206; // Payload-specific feedback (PLI, FIR, etc.)

/// Feedback message type (FMT field).
pub const FMT_NACK: u8 = 1;
pub const FMT_PLI: u8 = 1;
pub const FMT_SLI: u8 = 2;
pub const FMT_RPSI: u8 = 3;
pub const FMT_FIR: u8 = 4;
pub const FMT_TMMBR: u8 = 3;
pub const FMT_TMMBN: u8 = 4;
pub const FMT_REMB: u8 = 15; // Application-specific (REMB uses PSFB with FMT=15)

/// Types of RTCP feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedbackType {
    /// Generic NACK: request retransmission of specific packets.
    Nack,
    /// Picture Loss Indication: request a video keyframe.
    NackPli,
    /// Slice Loss Indication.
    NackSli,
    /// Reference Picture Selection Indication.
    NackRpsi,
    /// Full Intra Request: explicit keyframe request.
    CcmFir,
    /// Temporary Maximum Media Bitrate Request.
    CcmTmmbr,
    /// Temporary Maximum Media Bitrate Notification.
    CcmTmmbn,
}

impl FeedbackType {
    /// Parse from SDP `a=rtcp-fb:` attribute value.
    /// e.g. "nack", "nack pli", "ccm fir", "ccm tmmbr"
    pub fn from_sdp(value: &str) -> Option<Self> {
        let lower = value.to_lowercase();
        let parts: Vec<&str> = lower.split_whitespace().collect();
        match parts.as_slice() {
            ["nack"] => Some(Self::Nack),
            ["nack", "pli"] => Some(Self::NackPli),
            ["nack", "sli"] => Some(Self::NackSli),
            ["nack", "rpsi"] => Some(Self::NackRpsi),
            ["ccm", "fir"] => Some(Self::CcmFir),
            ["ccm", "tmmbr"] => Some(Self::CcmTmmbr),
            ["ccm", "tmmbn"] => Some(Self::CcmTmmbn),
            _ => None,
        }
    }

    /// Generate SDP `a=rtcp-fb:` attribute value.
    pub fn to_sdp(&self) -> &'static str {
        match self {
            Self::Nack => "nack",
            Self::NackPli => "nack pli",
            Self::NackSli => "nack sli",
            Self::NackRpsi => "nack rpsi",
            Self::CcmFir => "ccm fir",
            Self::CcmTmmbr => "ccm tmmbr",
            Self::CcmTmmbn => "ccm tmmbn",
        }
    }
}

/// A parsed RTCP feedback message.
#[derive(Debug, Clone)]
pub struct RtcpFeedback {
    /// Feedback message type (FMT field).
    pub fmt: u8,
    /// RTCP packet type (205 = RTPFB, 206 = PSFB).
    pub pt: u8,
    /// SSRC of the packet sender.
    pub ssrc_sender: u32,
    /// SSRC of the media source.
    pub ssrc_media: u32,
    /// Feedback Control Information (FCI), varies by type.
    pub fci: Vec<u8>,
}

impl RtcpFeedback {
    /// Parse an RTCP feedback message from raw bytes.
    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < 12 {
            return Err(AsteriskError::Parse(
                "RTCP feedback packet too short".into(),
            ));
        }

        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return Err(AsteriskError::Parse(format!(
                "Invalid RTCP version: {}",
                version
            )));
        }

        let fmt = data[0] & 0x1F;
        let pt = data[1];
        let length_words = u16::from_be_bytes([data[2], data[3]]) as usize;
        let ssrc_sender = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc_media = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let fci_len = (length_words + 1) * 4 - 12;
        let fci = if data.len() > 12 && fci_len > 0 {
            data[12..std::cmp::min(data.len(), 12 + fci_len)].to_vec()
        } else {
            Vec::new()
        };

        Ok(Self {
            fmt,
            pt,
            ssrc_sender,
            ssrc_media,
            fci,
        })
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Bytes {
        let length_words = (12 + self.fci.len()) / 4 - 1;
        let mut buf = BytesMut::with_capacity(12 + self.fci.len());

        buf.put_u8(0x80 | (self.fmt & 0x1F)); // V=2, P=0, FMT
        buf.put_u8(self.pt);
        buf.put_u16(length_words as u16);
        buf.put_u32(self.ssrc_sender);
        buf.put_u32(self.ssrc_media);
        buf.put_slice(&self.fci);

        buf.freeze()
    }
}

// ---------------------------------------------------------------------------
// NACK packet (RFC 4585 Section 6.2.1)
// ---------------------------------------------------------------------------

/// A Generic NACK packet.
///
/// `pid` is the Packet ID of the first lost packet.
/// `blp` is a bitmask of the following 16 sequence numbers (1 = lost).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NackPacket {
    /// Packet ID of the first lost packet.
    pub pid: u16,
    /// Bitmask of following lost packets (bit 0 = pid+1, bit 15 = pid+16).
    pub blp: u16,
}

impl NackPacket {
    /// Create a NACK for a single lost packet.
    pub fn single(seq: u16) -> Self {
        Self { pid: seq, blp: 0 }
    }

    /// Build a NACK packet from a list of lost sequence numbers.
    ///
    /// Groups consecutive losses into PID + BLP format.
    pub fn from_lost_sequences(mut lost: Vec<u16>) -> Vec<Self> {
        if lost.is_empty() {
            return Vec::new();
        }

        lost.sort();
        lost.dedup();

        let mut nacks = Vec::new();
        let mut i = 0;

        while i < lost.len() {
            let pid = lost[i];
            let mut blp: u16 = 0;
            let mut j = i + 1;

            while j < lost.len() {
                let offset = lost[j].wrapping_sub(pid);
                if offset >= 1 && offset <= 16 {
                    blp |= 1 << (offset - 1);
                    j += 1;
                } else {
                    break;
                }
            }

            nacks.push(NackPacket { pid, blp });
            i = j;
        }

        nacks
    }

    /// Expand this NACK back into individual lost sequence numbers.
    pub fn to_lost_sequences(&self) -> Vec<u16> {
        let mut lost = vec![self.pid];
        for bit in 0..16u16 {
            if (self.blp >> bit) & 1 == 1 {
                lost.push(self.pid.wrapping_add(bit + 1));
            }
        }
        lost
    }

    /// Encode to 4 bytes.
    pub fn to_bytes(&self) -> [u8; 4] {
        let mut buf = [0u8; 4];
        buf[0..2].copy_from_slice(&self.pid.to_be_bytes());
        buf[2..4].copy_from_slice(&self.blp.to_be_bytes());
        buf
    }

    /// Parse from 4 bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        Some(Self {
            pid: u16::from_be_bytes([data[0], data[1]]),
            blp: u16::from_be_bytes([data[2], data[3]]),
        })
    }
}

/// Build a complete RTCP NACK feedback message.
pub fn build_nack(
    sender_ssrc: u32,
    media_ssrc: u32,
    lost_sequences: &[u16],
) -> Bytes {
    let nacks = NackPacket::from_lost_sequences(lost_sequences.to_vec());
    let fci_len = nacks.len() * 4;
    let total_len = 12 + fci_len;
    let length_words = total_len / 4 - 1;

    let mut buf = BytesMut::with_capacity(total_len);
    buf.put_u8(0x80 | FMT_NACK); // V=2, P=0, FMT=1
    buf.put_u8(RTCP_PT_RTPFB);
    buf.put_u16(length_words as u16);
    buf.put_u32(sender_ssrc);
    buf.put_u32(media_ssrc);

    for nack in &nacks {
        buf.put_slice(&nack.to_bytes());
    }

    buf.freeze()
}

/// Build an RTCP PLI (Picture Loss Indication) message.
pub fn build_pli(sender_ssrc: u32, media_ssrc: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(12);
    buf.put_u8(0x80 | FMT_PLI);
    buf.put_u8(RTCP_PT_PSFB);
    buf.put_u16(2); // length = 2 (12 bytes / 4 - 1)
    buf.put_u32(sender_ssrc);
    buf.put_u32(media_ssrc);
    buf.freeze()
}

/// Build an RTCP FIR (Full Intra Request) message.
pub fn build_fir(sender_ssrc: u32, media_ssrc: u32, seq_nr: u8) -> Bytes {
    let mut buf = BytesMut::with_capacity(20);
    buf.put_u8(0x80 | FMT_FIR);
    buf.put_u8(RTCP_PT_PSFB);
    buf.put_u16(4); // length = 4 (20 bytes / 4 - 1)
    buf.put_u32(sender_ssrc);
    buf.put_u32(0); // Media source SSRC (0 for FIR)
    // FCI
    buf.put_u32(media_ssrc);
    buf.put_u8(seq_nr);
    buf.put_u8(0); // reserved
    buf.put_u16(0); // reserved
    buf.freeze()
}

// ---------------------------------------------------------------------------
// RTP Retransmission Buffer
// ---------------------------------------------------------------------------

/// Buffer of recently sent RTP packets for NACK retransmission.
#[derive(Debug)]
pub struct RetransmissionBuffer {
    /// Stored packets: (sequence_number, packet_data).
    packets: VecDeque<(u16, Bytes)>,
    /// Maximum number of packets to buffer.
    capacity: usize,
}

impl RetransmissionBuffer {
    /// Create a new retransmission buffer.
    pub fn new(capacity: usize) -> Self {
        Self {
            packets: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Store a sent packet.
    pub fn store(&mut self, seq: u16, packet: Bytes) {
        if self.packets.len() >= self.capacity {
            self.packets.pop_front();
        }
        self.packets.push_back((seq, packet));
    }

    /// Retrieve a packet by sequence number for retransmission.
    pub fn get(&self, seq: u16) -> Option<&Bytes> {
        self.packets
            .iter()
            .find(|(s, _)| *s == seq)
            .map(|(_, data)| data)
    }

    /// Retrieve multiple packets for NACK response.
    pub fn get_for_nack(&self, nack: &NackPacket) -> Vec<(u16, Bytes)> {
        let mut result = Vec::new();
        for seq in nack.to_lost_sequences() {
            if let Some(data) = self.get(seq) {
                result.push((seq, data.clone()));
            }
        }
        result
    }

    /// Number of packets in the buffer.
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.packets.clear();
    }
}

impl Default for RetransmissionBuffer {
    fn default() -> Self {
        Self::new(256) // Buffer last 256 packets
    }
}

// ---------------------------------------------------------------------------
// SDP negotiation helpers
// ---------------------------------------------------------------------------

/// Parse `a=rtcp-fb:` attributes from SDP.
///
/// Returns a list of (payload_type_or_wildcard, feedback_type) pairs.
pub fn parse_rtcp_fb_attributes(
    attributes: &[(String, Option<String>)],
) -> Vec<(Option<u8>, FeedbackType)> {
    let mut result = Vec::new();

    for (name, value) in attributes {
        if name != "rtcp-fb" {
            continue;
        }
        if let Some(val) = value {
            let parts: Vec<&str> = val.splitn(2, ' ').collect();
            if parts.len() < 2 {
                continue;
            }

            let pt = if parts[0] == "*" {
                None
            } else {
                parts[0].parse::<u8>().ok()
            };

            if let Some(fb_type) = FeedbackType::from_sdp(parts[1]) {
                result.push((pt, fb_type));
            }
        }
    }

    result
}

/// Generate SDP `a=rtcp-fb:` attribute lines.
pub fn generate_rtcp_fb_attributes(
    payload_type: u8,
    feedback_types: &[FeedbackType],
) -> Vec<(String, Option<String>)> {
    feedback_types
        .iter()
        .map(|fb| {
            (
                "rtcp-fb".to_string(),
                Some(format!("{} {}", payload_type, fb.to_sdp())),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nack_single() {
        let nack = NackPacket::single(1000);
        assert_eq!(nack.pid, 1000);
        assert_eq!(nack.blp, 0);

        let lost = nack.to_lost_sequences();
        assert_eq!(lost, vec![1000]);
    }

    #[test]
    fn test_nack_from_lost_sequences() {
        // Consecutive losses: 100, 101, 102, 105
        let nacks = NackPacket::from_lost_sequences(vec![100, 101, 102, 105]);
        assert_eq!(nacks.len(), 1);
        assert_eq!(nacks[0].pid, 100);
        // blp: bit 0 (101-100-1=0) | bit 1 (102-100-1=1) | bit 4 (105-100-1=4)
        assert_eq!(nacks[0].blp, 0b0000_0000_0001_0011);
        // But 105-100=5, so bit index is 5-1=4
        assert!(nacks[0].blp & (1 << 0) != 0); // 101
        assert!(nacks[0].blp & (1 << 1) != 0); // 102
        assert!(nacks[0].blp & (1 << 4) != 0); // 105
    }

    #[test]
    fn test_nack_roundtrip() {
        let nack = NackPacket { pid: 500, blp: 0x0005 };
        let bytes = nack.to_bytes();
        let parsed = NackPacket::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.pid, 500);
        assert_eq!(parsed.blp, 0x0005);

        let lost = parsed.to_lost_sequences();
        assert!(lost.contains(&500));
        assert!(lost.contains(&501)); // bit 0
        assert!(lost.contains(&503)); // bit 2
    }

    #[test]
    fn test_nack_multiple_groups() {
        // Losses at 100 and 200 (too far apart for single NACK).
        let nacks = NackPacket::from_lost_sequences(vec![100, 200]);
        assert_eq!(nacks.len(), 2);
        assert_eq!(nacks[0].pid, 100);
        assert_eq!(nacks[0].blp, 0);
        assert_eq!(nacks[1].pid, 200);
        assert_eq!(nacks[1].blp, 0);
    }

    #[test]
    fn test_build_nack() {
        let data = build_nack(0x11111111, 0x22222222, &[100, 101]);
        assert!(data.len() >= 16); // 12 header + 4 NACK FCI
        assert_eq!(data[1], RTCP_PT_RTPFB);
        assert_eq!(data[0] & 0x1F, FMT_NACK);
    }

    #[test]
    fn test_build_pli() {
        let data = build_pli(0x11111111, 0x22222222);
        assert_eq!(data.len(), 12);
        assert_eq!(data[1], RTCP_PT_PSFB);
        assert_eq!(data[0] & 0x1F, FMT_PLI);
    }

    #[test]
    fn test_build_fir() {
        let data = build_fir(0x11111111, 0x22222222, 1);
        assert_eq!(data.len(), 20);
        assert_eq!(data[1], RTCP_PT_PSFB);
        assert_eq!(data[0] & 0x1F, FMT_FIR);
    }

    #[test]
    fn test_rtcp_feedback_parse() {
        let data = build_pli(0xAAAAAAAA, 0xBBBBBBBB);
        let fb = RtcpFeedback::parse(&data).unwrap();
        assert_eq!(fb.fmt, FMT_PLI);
        assert_eq!(fb.pt, RTCP_PT_PSFB);
        assert_eq!(fb.ssrc_sender, 0xAAAAAAAA);
        assert_eq!(fb.ssrc_media, 0xBBBBBBBB);
    }

    #[test]
    fn test_retransmission_buffer() {
        let mut buf = RetransmissionBuffer::new(4);
        assert!(buf.is_empty());

        buf.store(100, Bytes::from_static(b"pkt100"));
        buf.store(101, Bytes::from_static(b"pkt101"));
        buf.store(102, Bytes::from_static(b"pkt102"));
        assert_eq!(buf.len(), 3);

        assert_eq!(buf.get(100), Some(&Bytes::from_static(b"pkt100")));
        assert_eq!(buf.get(101), Some(&Bytes::from_static(b"pkt101")));
        assert!(buf.get(99).is_none());
    }

    #[test]
    fn test_retransmission_buffer_overflow() {
        let mut buf = RetransmissionBuffer::new(2);
        buf.store(1, Bytes::from_static(b"a"));
        buf.store(2, Bytes::from_static(b"b"));
        buf.store(3, Bytes::from_static(b"c"));

        // Oldest (seq=1) should have been evicted.
        assert!(buf.get(1).is_none());
        assert!(buf.get(2).is_some());
        assert!(buf.get(3).is_some());
    }

    #[test]
    fn test_retransmission_buffer_get_for_nack() {
        let mut buf = RetransmissionBuffer::new(10);
        buf.store(100, Bytes::from_static(b"a"));
        buf.store(101, Bytes::from_static(b"b"));
        buf.store(102, Bytes::from_static(b"c"));

        let nack = NackPacket { pid: 100, blp: 0x0001 }; // 100 and 101
        let packets = buf.get_for_nack(&nack);
        assert_eq!(packets.len(), 2);
    }

    #[test]
    fn test_feedback_type_sdp_roundtrip() {
        let types = [
            FeedbackType::Nack,
            FeedbackType::NackPli,
            FeedbackType::CcmFir,
            FeedbackType::CcmTmmbr,
        ];
        for fb in &types {
            let sdp = fb.to_sdp();
            let parsed = FeedbackType::from_sdp(sdp).unwrap();
            assert_eq!(*fb, parsed);
        }
    }

    #[test]
    fn test_parse_rtcp_fb_attributes() {
        let attrs = vec![
            ("rtcp-fb".to_string(), Some("* nack".to_string())),
            ("rtcp-fb".to_string(), Some("96 nack pli".to_string())),
            ("rtcp-fb".to_string(), Some("96 ccm fir".to_string())),
            ("other".to_string(), Some("ignored".to_string())),
        ];

        let fbs = parse_rtcp_fb_attributes(&attrs);
        assert_eq!(fbs.len(), 3);
        assert_eq!(fbs[0], (None, FeedbackType::Nack));
        assert_eq!(fbs[1], (Some(96), FeedbackType::NackPli));
        assert_eq!(fbs[2], (Some(96), FeedbackType::CcmFir));
    }

    #[test]
    fn test_generate_rtcp_fb_attributes() {
        let attrs = generate_rtcp_fb_attributes(
            96,
            &[FeedbackType::Nack, FeedbackType::NackPli],
        );
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].1, Some("96 nack".to_string()));
        assert_eq!(attrs[1].1, Some("96 nack pli".to_string()));
    }
}
