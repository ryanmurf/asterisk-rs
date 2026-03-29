//! RTP/RTCP session management.
//!
//! Provides RTP send/receive with proper header handling, payload type
//! mapping, and RFC 2833 DTMF support. Also includes RTCP SR/RR.
//!
//! Sub-modules:
//! - `jitter_buffer`: Fixed and adaptive jitter buffer implementations.
//! - `engine`: Pluggable RTP engine abstraction.
//! - `avpf`: AVPF / RTP Feedback (RFC 4585) -- NACK, PLI, FIR, TMMBR.
//! - `bundle`: BUNDLE (RFC 8843) -- multiple media on one transport.
//! - `ice_transport`: ICE-integrated RTP transport (RFC 8445).

pub mod jitter_buffer;
pub mod engine;
pub mod avpf;
pub mod bundle;
pub mod ice_transport;
pub mod mos;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{BufMut, Bytes, BytesMut};
use tokio::net::UdpSocket;

use asterisk_types::{AsteriskError, AsteriskResult, Frame};

/// Standard RTP header size.
const RTP_HEADER_SIZE: usize = 12;
/// Maximum RTP packet size.
const RTP_MAX_MTU: usize = 1500;
/// RTCP sender report type.
const RTCP_PT_SR: u8 = 200;
/// RTCP receiver report type.
const RTCP_PT_RR: u8 = 201;

/// RTP header.
#[derive(Debug, Clone)]
pub struct RtpHeader {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub csrc_count: u8,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    #[inline]
    pub fn parse(data: &[u8]) -> Result<Self, AsteriskError> {
        if data.len() < RTP_HEADER_SIZE {
            return Err(AsteriskError::Parse("RTP packet too short".into()));
        }
        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return Err(AsteriskError::Parse(format!("Invalid RTP version: {}", version)));
        }
        Ok(Self {
            version,
            padding: (data[0] & 0x20) != 0,
            extension: (data[0] & 0x10) != 0,
            csrc_count: data[0] & 0x0F,
            marker: (data[1] & 0x80) != 0,
            payload_type: data[1] & 0x7F,
            sequence: u16::from_be_bytes([data[2], data[3]]),
            timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ssrc: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        })
    }

    #[inline(always)]
    pub fn header_size(&self) -> usize {
        RTP_HEADER_SIZE + (self.csrc_count as usize) * 4
    }

    #[inline]
    pub fn to_bytes(&self) -> [u8; RTP_HEADER_SIZE] {
        let mut buf = [0u8; RTP_HEADER_SIZE];
        buf[0] = (self.version << 6)
            | if self.padding { 0x20 } else { 0 }
            | if self.extension { 0x10 } else { 0 }
            | (self.csrc_count & 0x0F);
        buf[1] = if self.marker { 0x80 } else { 0 } | (self.payload_type & 0x7F);
        buf[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        buf[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        buf[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        buf
    }
}

/// Build an RTP packet.
#[inline]
pub fn build_rtp_packet(header: &RtpHeader, payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(RTP_HEADER_SIZE + payload.len());
    buf.put_slice(&header.to_bytes());
    buf.put_slice(payload);
    buf.freeze()
}

/// Parse an RTP packet.
#[inline]
pub fn parse_rtp_header(data: &[u8]) -> Result<(RtpHeader, &[u8]), AsteriskError> {
    let header = RtpHeader::parse(data)?;
    let offset = header.header_size();
    if data.len() < offset {
        return Err(AsteriskError::Parse("RTP packet truncated".into()));
    }
    Ok((header, &data[offset..]))
}

/// RFC 2833 DTMF event payload.
#[derive(Debug, Clone)]
pub struct DtmfEvent {
    pub event: u8,
    pub end: bool,
    pub volume: u8,
    pub duration: u16,
}

impl DtmfEvent {
    /// Encode a DTMF event to bytes.
    pub fn to_bytes(&self) -> [u8; 4] {
        let mut buf = [0u8; 4];
        buf[0] = self.event;
        buf[1] = if self.end { 0x80 } else { 0 } | (self.volume & 0x3F);
        buf[2..4].copy_from_slice(&self.duration.to_be_bytes());
        buf
    }

    /// Parse a DTMF event from bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        Some(Self {
            event: data[0],
            end: (data[1] & 0x80) != 0,
            volume: data[1] & 0x3F,
            duration: u16::from_be_bytes([data[2], data[3]]),
        })
    }

    /// Convert DTMF event number to digit character.
    pub fn event_to_digit(event: u8) -> char {
        match event {
            0..=9 => (b'0' + event) as char,
            10 => '*',
            11 => '#',
            12 => 'A',
            13 => 'B',
            14 => 'C',
            15 => 'D',
            _ => '?',
        }
    }

    /// Convert digit character to DTMF event number.
    pub fn digit_to_event(digit: char) -> u8 {
        match digit {
            '0'..='9' => digit as u8 - b'0',
            '*' => 10,
            '#' => 11,
            'A' | 'a' => 12,
            'B' | 'b' => 13,
            'C' | 'c' => 14,
            'D' | 'd' => 15,
            _ => 0,
        }
    }
}

/// An RTP session managing send/receive of media over UDP.
#[derive(Debug)]
pub struct RtpSession {
    /// UDP socket for RTP.
    pub socket: Arc<UdpSocket>,
    /// Remote address.
    pub remote_addr: Option<SocketAddr>,
    /// Our SSRC.
    pub ssrc: u32,
    /// Outgoing sequence number.
    sequence: AtomicU16,
    /// Outgoing timestamp.
    timestamp: AtomicU32,
    /// Payload type for outgoing packets.
    pub payload_type: u8,
    /// DTMF payload type (RFC 2833).
    pub dtmf_payload_type: u8,
    /// Samples per packet (for timestamp advancement).
    pub samples_per_packet: u32,
    /// Statistics.
    pub stats: RtpStats,
}

/// RTP session statistics.
#[derive(Debug, Default)]
pub struct RtpStats {
    pub packets_sent: AtomicU32,
    pub packets_received: AtomicU32,
    pub octets_sent: AtomicU32,
    pub octets_received: AtomicU32,
}

impl RtpSession {
    /// Bind an RTP session to a local address.
    pub async fn bind(addr: SocketAddr) -> AsteriskResult<Self> {
        let socket = UdpSocket::bind(addr).await?;
        let ssrc = generate_ssrc();
        Ok(Self {
            socket: Arc::new(socket),
            remote_addr: None,
            ssrc,
            sequence: AtomicU16::new(0),
            timestamp: AtomicU32::new(0),
            payload_type: 0,
            dtmf_payload_type: 101,
            samples_per_packet: 160,
            stats: RtpStats::default(),
        })
    }

    /// Get the local address.
    pub fn local_addr(&self) -> AsteriskResult<SocketAddr> {
        self.socket.local_addr().map_err(AsteriskError::Io)
    }

    /// Set the remote address.
    pub fn set_remote_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = Some(addr);
    }

    /// Send an audio frame as RTP.
    pub async fn send_frame(&self, frame: &Frame) -> AsteriskResult<()> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Ok(()),
        };

        let remote = self
            .remote_addr
            .ok_or_else(|| AsteriskError::InvalidArgument("No remote address".into()))?;

        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let ts = self
            .timestamp
            .fetch_add(self.samples_per_packet, Ordering::Relaxed);

        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: self.payload_type,
            sequence: seq,
            timestamp: ts,
            ssrc: self.ssrc,
        };

        let packet = build_rtp_packet(&header, data);
        self.socket.send_to(&packet, remote).await?;

        self.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.stats
            .octets_sent
            .fetch_add(data.len() as u32, Ordering::Relaxed);

        Ok(())
    }

    /// Receive an RTP packet and convert to a Frame.
    pub async fn recv_frame(&self) -> AsteriskResult<Frame> {
        let mut buf = vec![0u8; RTP_MAX_MTU];
        let (len, _src) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);

        let (header, payload) = parse_rtp_header(&buf)?;

        self.stats.packets_received.fetch_add(1, Ordering::Relaxed);
        self.stats
            .octets_received
            .fetch_add(payload.len() as u32, Ordering::Relaxed);

        // Check for DTMF (RFC 2833)
        if header.payload_type == self.dtmf_payload_type {
            if let Some(event) = DtmfEvent::from_bytes(payload) {
                let digit = DtmfEvent::event_to_digit(event.event);
                if event.end {
                    return Ok(Frame::dtmf_end(digit, event.duration as u32 / 8));
                } else {
                    return Ok(Frame::dtmf_begin(digit));
                }
            }
        }

        let samples = match header.payload_type {
            0 | 8 => payload.len() as u32,
            _ => (payload.len() as u32) / 2,
        };

        Ok(Frame::voice(
            header.payload_type as u32,
            samples,
            Bytes::copy_from_slice(payload),
        ))
    }

    /// Send a DTMF digit via RFC 2833.
    pub async fn send_dtmf(
        &self,
        digit: char,
        duration_samples: u16,
    ) -> AsteriskResult<()> {
        let remote = self
            .remote_addr
            .ok_or_else(|| AsteriskError::InvalidArgument("No remote address".into()))?;

        let event_num = DtmfEvent::digit_to_event(digit);
        let start_seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let start_ts = self.timestamp.load(Ordering::Relaxed);

        // Send begin events (3 packets as per common practice)
        for i in 0..3 {
            let event = DtmfEvent {
                event: event_num,
                end: false,
                volume: 10,
                duration: 160 * (i + 1),
            };
            let header = RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                csrc_count: 0,
                marker: i == 0,
                payload_type: self.dtmf_payload_type,
                sequence: start_seq.wrapping_add(i),
                timestamp: start_ts,
                ssrc: self.ssrc,
            };
            let packet = build_rtp_packet(&header, &event.to_bytes());
            self.socket.send_to(&packet, remote).await?;
        }

        // Send end event (3 times for reliability)
        for i in 0..3 {
            let event = DtmfEvent {
                event: event_num,
                end: true,
                volume: 10,
                duration: duration_samples,
            };
            let header = RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                csrc_count: 0,
                marker: false,
                payload_type: self.dtmf_payload_type,
                sequence: start_seq.wrapping_add(3 + i),
                timestamp: start_ts,
                ssrc: self.ssrc,
            };
            let packet = build_rtp_packet(&header, &event.to_bytes());
            self.socket.send_to(&packet, remote).await?;
        }

        // Advance sequence past the DTMF events
        self.sequence.store(start_seq.wrapping_add(6), Ordering::Relaxed);

        Ok(())
    }
}

/// RTCP session for sender/receiver reports.
#[derive(Debug)]
pub struct RtcpSession {
    pub socket: Arc<UdpSocket>,
    pub remote_addr: Option<SocketAddr>,
    pub ssrc: u32,
}

impl RtcpSession {
    /// Bind an RTCP session (typically RTP port + 1).
    pub async fn bind(addr: SocketAddr) -> AsteriskResult<Self> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Self {
            socket: Arc::new(socket),
            remote_addr: None,
            ssrc: generate_ssrc(),
        })
    }

    pub fn set_remote_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = Some(addr);
    }

    /// Build and send a Sender Report (SR).
    pub async fn send_sr(
        &self,
        packet_count: u32,
        octet_count: u32,
        rtp_timestamp: u32,
    ) -> AsteriskResult<()> {
        let remote = match self.remote_addr {
            Some(addr) => addr,
            None => return Ok(()),
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        // NTP epoch offset (seconds between 1900-01-01 and 1970-01-01).
        // Use wrapping_add to handle the NTP era rollover (Feb 2036).
        let ntp_secs = (now.as_secs() as u32).wrapping_add(2_208_988_800u32);
        let ntp_frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;

        let mut buf = BytesMut::with_capacity(28);
        // RTCP header: V=2, P=0, RC=0, PT=200(SR), length=6 (words-1)
        buf.put_u8(0x80); // V=2, P=0, RC=0
        buf.put_u8(RTCP_PT_SR);
        buf.put_u16(6); // length in 32-bit words minus one
        buf.put_u32(self.ssrc);
        // NTP timestamp
        buf.put_u32(ntp_secs);
        buf.put_u32(ntp_frac as u32);
        // RTP timestamp
        buf.put_u32(rtp_timestamp);
        // Sender packet count
        buf.put_u32(packet_count);
        // Sender octet count
        buf.put_u32(octet_count);

        self.socket.send_to(&buf, remote).await?;
        Ok(())
    }

    /// Build and send a Receiver Report (RR).
    pub async fn send_rr(
        &self,
        remote_ssrc: u32,
        fraction_lost: u8,
        cumulative_lost: u32,
        highest_seq: u32,
        jitter: u32,
    ) -> AsteriskResult<()> {
        let remote = match self.remote_addr {
            Some(addr) => addr,
            None => return Ok(()),
        };

        let mut buf = BytesMut::with_capacity(32);
        // V=2, P=0, RC=1, PT=201(RR), length=7
        buf.put_u8(0x81); // V=2, P=0, RC=1
        buf.put_u8(RTCP_PT_RR);
        buf.put_u16(7);
        buf.put_u32(self.ssrc);
        // Report block
        buf.put_u32(remote_ssrc);
        // Fraction lost (8 bits) + cumulative lost (24 bits)
        buf.put_u8(fraction_lost);
        buf.put_u8(((cumulative_lost >> 16) & 0xFF) as u8);
        buf.put_u8(((cumulative_lost >> 8) & 0xFF) as u8);
        buf.put_u8((cumulative_lost & 0xFF) as u8);
        // Highest sequence number received
        buf.put_u32(highest_seq);
        // Interarrival jitter
        buf.put_u32(jitter);
        // Last SR (LSR) - 0 for now
        buf.put_u32(0);
        // Delay since last SR (DLSR) - 0 for now
        buf.put_u32(0);

        self.socket.send_to(&buf, remote).await?;
        Ok(())
    }
}

/// Generate a random SSRC.
fn generate_ssrc() -> u32 {
    use rand::Rng;
    rand::thread_rng().gen()
}

// ---------------------------------------------------------------------------
// Comfort Noise (RFC 3389)
// ---------------------------------------------------------------------------

/// RFC 3389 Comfort Noise Generator.
///
/// During silence periods in a call, CNG frames are sent to indicate
/// that the connection is still alive and to provide a background noise
/// level hint to the receiver.
///
/// The CNG payload consists of a single noise level byte (0-127 dBov)
/// optionally followed by spectral parameters.
///
/// This implementation generates actual comfort noise audio samples
/// with spectral shaping to approximate typical background noise.
#[derive(Debug, Clone)]
pub struct ComfortNoise {
    /// Noise level in -dBov (0 = loudest, 127 = silence).
    /// Typical values: 40-60 for office noise, 70-80 for quiet room.
    pub level: i8,
    /// Whether CNG generation is currently active (we are in a silence period).
    pub active: bool,
    /// Payload type for CNG (RFC 3389 specifies PT 13 for 8kHz, or dynamic).
    pub payload_type: u8,
    /// LFSR state for white noise generation.
    noise_state: u32,
    /// One-pole low-pass filter state for spectral shaping.
    filter_state: f32,
    /// Filter coefficient for spectral shaping (controls spectral tilt).
    filter_coeff: f32,
}

impl ComfortNoise {
    /// Create a new CNG generator with the given noise level.
    ///
    /// `level` is in -dBov: 0 = maximum noise, 127 = digital silence.
    pub fn new(level: i8) -> Self {
        Self {
            level,
            active: false,
            payload_type: 13, // Static PT for 8kHz CNG
            noise_state: 0xACE1_u32,
            filter_state: 0.0,
            filter_coeff: 0.7, // Low-pass for pink-ish noise
        }
    }

    /// Generate a CNG frame for transmission during a silence period.
    ///
    /// Returns an `ast_frame` compatible CNG frame with the noise level
    /// as payload.
    pub fn generate_frame(&self) -> asterisk_types::Frame {
        asterisk_types::Frame::Cng {
            level: self.level as i32,
        }
    }

    /// Generate actual comfort noise audio samples at the configured level.
    ///
    /// - `num_samples`: number of PCM samples to generate
    ///
    /// Returns i16 PCM samples shaped to approximate background noise.
    pub fn generate_audio(&mut self, num_samples: usize) -> Vec<i16> {
        if self.level >= 127 {
            // Digital silence
            return vec![0i16; num_samples];
        }

        // Convert -dBov level to linear amplitude.
        // Level 0 = 0 dBov (loudest), 127 = -127 dBov (silence).
        // Typical comfortable CNG is around level 40-60 (-40 to -60 dBov).
        let amplitude = 32768.0 * 10.0f32.powf(-(self.level as f32) / 20.0);
        // Clamp to reasonable range
        let amplitude = amplitude.min(4000.0);

        let mut output = Vec::with_capacity(num_samples);
        for _ in 0..num_samples {
            // Generate white noise using LFSR
            self.noise_state ^= self.noise_state << 13;
            self.noise_state ^= self.noise_state >> 17;
            self.noise_state ^= self.noise_state << 5;
            let white = (self.noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0;

            // Apply spectral shaping (one-pole low-pass for pink-ish noise)
            // This makes the noise sound more natural (real background noise
            // has more energy at lower frequencies).
            self.filter_state = self.filter_coeff * self.filter_state
                + (1.0 - self.filter_coeff) * white;

            let sample = (self.filter_state * amplitude)
                .round()
                .clamp(-32768.0, 32767.0) as i16;
            output.push(sample);
        }

        output
    }

    /// Set the noise level from a received CNG frame.
    pub fn set_level_from_received(&mut self, level: i8) {
        self.level = level;
    }

    /// Build a raw CNG RTP payload (RFC 3389 Section 3).
    ///
    /// The payload is: noise_level (1 byte) + optional spectral params.
    pub fn build_payload(&self) -> Vec<u8> {
        // The noise level byte: 0 = loudest CNG, 127 = digital silence.
        // RFC 3389 uses unsigned; we store as i8 for Asterisk compat.
        vec![self.level as u8]
    }

    /// Parse an incoming CNG RTP payload.
    ///
    /// Returns the noise level from the received CNG frame.
    pub fn parse_payload(data: &[u8]) -> Option<i8> {
        if data.is_empty() {
            return None;
        }
        Some(data[0] as i8)
    }

    /// Check if a received frame is CNG and should suppress playout.
    pub fn is_cng_frame(payload_type: u8) -> bool {
        payload_type == 13 // Static CNG payload type
    }

    /// Enter silence period (start generating CNG).
    pub fn start_silence(&mut self) {
        self.active = true;
    }

    /// Exit silence period (resume normal audio).
    pub fn stop_silence(&mut self) {
        self.active = false;
    }
}

impl Default for ComfortNoise {
    fn default() -> Self {
        Self::new(60) // Moderate background noise
    }
}

// ---------------------------------------------------------------------------
// RTCP-MUX (RFC 5761)
// ---------------------------------------------------------------------------

/// RTCP payload types used for MUX detection (200-213 per IANA).
const RTCP_PT_RANGE_START: u8 = 200;
const RTCP_PT_RANGE_END: u8 = 213;
/// RTCP SDES type.
const RTCP_PT_SDES: u8 = 202;
/// RTCP BYE type.
const RTCP_PT_BYE: u8 = 203;
/// RTCP APP type.
const RTCP_PT_APP: u8 = 204;

/// Result of demuxing a packet on a muxed socket.
#[derive(Debug)]
pub enum MuxedPacket {
    /// An RTP packet (header + payload).
    Rtp(RtpHeader, Bytes),
    /// An RTCP packet (raw bytes).
    Rtcp(Bytes),
}

/// Detect whether a received packet is RTP or RTCP (RFC 5761).
///
/// The distinguishing rule:
/// - Second byte (after V/P/X/CC): payload type field
/// - RTCP: PT in 200..=213
/// - RTP: PT in 0..=127 (7-bit field, high bit is marker)
///
/// For the second byte of the packet:
/// - RTCP: byte[1] is the RTCP PT directly (200-213)
/// - RTP: byte[1] has marker bit (bit 7) + PT (bits 0-6)
pub fn is_rtcp_packet(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let pt = data[1];
    // RTCP packets have PT in [200, 213] range.
    // RTP packets have byte[1] = marker_bit | (pt & 0x7F), so the
    // full byte value is 0-127 or 128-255.
    // RTCP PTs 200-204 fall in the range where RTP PTs would be
    // 200-204 with marker=0, or 72-76 with marker=1. Since PT 72-76
    // are unassigned, this demux is safe.
    pt >= RTCP_PT_RANGE_START && pt <= RTCP_PT_RANGE_END
}

/// A muxed RTP/RTCP session (RFC 5761).
///
/// Multiplexes RTCP on the same port as RTP. A single UDP socket is used
/// for both RTP and RTCP traffic.
#[derive(Debug)]
pub struct MuxedRtpSession {
    /// Underlying RTP session (shares its socket for RTCP).
    pub rtp: RtpSession,
    /// Whether muxing has been negotiated (both sides offered `a=rtcp-mux`).
    pub mux_enabled: bool,
}

impl MuxedRtpSession {
    /// Create a muxed session wrapping an existing RTP session.
    pub fn new(rtp: RtpSession, mux_enabled: bool) -> Self {
        Self { rtp, mux_enabled }
    }

    /// Bind a new muxed session.
    pub async fn bind(addr: SocketAddr, mux_enabled: bool) -> AsteriskResult<Self> {
        let rtp = RtpSession::bind(addr).await?;
        Ok(Self::new(rtp, mux_enabled))
    }

    /// Receive and demux a packet.
    ///
    /// Returns `MuxedPacket::Rtp` for RTP data or `MuxedPacket::Rtcp` for RTCP.
    pub async fn recv_muxed(&self) -> AsteriskResult<MuxedPacket> {
        let mut buf = vec![0u8; RTP_MAX_MTU];
        let (len, _src) = self.rtp.socket.recv_from(&mut buf).await?;
        buf.truncate(len);

        if self.mux_enabled && is_rtcp_packet(&buf) {
            Ok(MuxedPacket::Rtcp(Bytes::from(buf)))
        } else {
            let (header, payload) = parse_rtp_header(&buf)?;
            Ok(MuxedPacket::Rtp(header, Bytes::copy_from_slice(payload)))
        }
    }

    /// Send an RTCP packet on the muxed socket.
    pub async fn send_rtcp_muxed(&self, rtcp_data: &[u8]) -> AsteriskResult<()> {
        let remote = self
            .rtp
            .remote_addr
            .ok_or_else(|| AsteriskError::InvalidArgument("No remote address".into()))?;
        self.rtp.socket.send_to(rtcp_data, remote).await?;
        Ok(())
    }

    /// Send an RTP frame (delegates to underlying RTP session).
    pub async fn send_frame(&self, frame: &Frame) -> AsteriskResult<()> {
        self.rtp.send_frame(frame).await
    }

    /// Get the local address.
    pub fn local_addr(&self) -> AsteriskResult<SocketAddr> {
        self.rtp.local_addr()
    }

    /// Set the remote address.
    pub fn set_remote_addr(&mut self, addr: SocketAddr) {
        self.rtp.set_remote_addr(addr);
    }
}

/// Check if an SDP media description offers `rtcp-mux`.
pub fn sdp_offers_rtcp_mux(attributes: &[(String, Option<String>)]) -> bool {
    attributes.iter().any(|(name, _)| name == "rtcp-mux")
}

/// Check if both local and remote SDP offer `rtcp-mux` (negotiation).
pub fn rtcp_mux_negotiated(
    local_attrs: &[(String, Option<String>)],
    remote_attrs: &[(String, Option<String>)],
) -> bool {
    sdp_offers_rtcp_mux(local_attrs) && sdp_offers_rtcp_mux(remote_attrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtp_header_roundtrip() {
        let h = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: true, payload_type: 0, sequence: 100,
            timestamp: 1600, ssrc: 12345,
        };
        let bytes = h.to_bytes();
        let parsed = RtpHeader::parse(&bytes).unwrap();
        assert!(parsed.marker);
        assert_eq!(parsed.sequence, 100);
        assert_eq!(parsed.timestamp, 1600);
        assert_eq!(parsed.ssrc, 12345);
    }

    #[test]
    fn test_dtmf_event_roundtrip() {
        let event = DtmfEvent {
            event: 5,
            end: true,
            volume: 10,
            duration: 1600,
        };
        let bytes = event.to_bytes();
        let parsed = DtmfEvent::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.event, 5);
        assert!(parsed.end);
        assert_eq!(parsed.volume, 10);
        assert_eq!(parsed.duration, 1600);
    }

    #[test]
    fn test_dtmf_digit_conversion() {
        assert_eq!(DtmfEvent::event_to_digit(0), '0');
        assert_eq!(DtmfEvent::event_to_digit(9), '9');
        assert_eq!(DtmfEvent::event_to_digit(10), '*');
        assert_eq!(DtmfEvent::event_to_digit(11), '#');
        assert_eq!(DtmfEvent::digit_to_event('5'), 5);
        assert_eq!(DtmfEvent::digit_to_event('*'), 10);
    }

    #[test]
    fn test_comfort_noise_default() {
        let cng = ComfortNoise::default();
        assert_eq!(cng.level, 60);
        assert!(!cng.active);
        assert_eq!(cng.payload_type, 13);
    }

    #[test]
    fn test_comfort_noise_payload() {
        let cng = ComfortNoise::new(50);
        let payload = cng.build_payload();
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0], 50);

        let level = ComfortNoise::parse_payload(&payload).unwrap();
        assert_eq!(level, 50);
    }

    #[test]
    fn test_comfort_noise_frame() {
        let cng = ComfortNoise::new(40);
        let frame = cng.generate_frame();
        match frame {
            Frame::Cng { level } => assert_eq!(level, 40),
            _ => panic!("Expected CNG frame"),
        }
    }

    #[test]
    fn test_comfort_noise_silence_lifecycle() {
        let mut cng = ComfortNoise::new(60);
        assert!(!cng.active);

        cng.start_silence();
        assert!(cng.active);

        cng.stop_silence();
        assert!(!cng.active);
    }

    #[test]
    fn test_cng_detection() {
        assert!(ComfortNoise::is_cng_frame(13));
        assert!(!ComfortNoise::is_cng_frame(0));
        assert!(!ComfortNoise::is_cng_frame(101));
    }

    #[test]
    fn test_cng_audio_generation() {
        let mut cng = ComfortNoise::new(50);
        let audio = cng.generate_audio(160);
        assert_eq!(audio.len(), 160);
        // Should not be all zeros (it's noise)
        let has_nonzero = audio.iter().any(|&s| s != 0);
        assert!(has_nonzero, "CNG audio should not be all zeros");
    }

    #[test]
    fn test_cng_audio_silence_level() {
        let mut cng = ComfortNoise::new(127);
        let audio = cng.generate_audio(160);
        // Level 127 = digital silence
        for &s in &audio {
            assert_eq!(s, 0, "Level 127 should produce silence");
        }
    }

    #[test]
    fn test_cng_audio_level_scaling() {
        // Louder level should produce higher amplitude noise
        let mut cng_loud = ComfortNoise::new(30);
        let loud_audio = cng_loud.generate_audio(8000);
        let loud_energy: f64 = loud_audio.iter().map(|&s| (s as f64) * (s as f64)).sum();

        let mut cng_quiet = ComfortNoise::new(80);
        let quiet_audio = cng_quiet.generate_audio(8000);
        let quiet_energy: f64 = quiet_audio.iter().map(|&s| (s as f64) * (s as f64)).sum();

        assert!(
            loud_energy > quiet_energy,
            "Louder CNG level should produce more energy: loud={}, quiet={}",
            loud_energy,
            quiet_energy
        );
    }

    #[test]
    fn test_cng_set_level() {
        let mut cng = ComfortNoise::new(60);
        cng.set_level_from_received(40);
        assert_eq!(cng.level, 40);
    }

    // -----------------------------------------------------------------------
    // RTCP-MUX tests (RFC 5761)
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_rtcp_packet_sr() {
        // Build a minimal RTCP SR packet.
        let mut data = vec![0u8; 28];
        data[0] = 0x80; // V=2, P=0, RC=0
        data[1] = 200;  // PT = SR
        data[2] = 0;
        data[3] = 6;    // length
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_is_rtcp_packet_rr() {
        let mut data = vec![0u8; 32];
        data[0] = 0x81;
        data[1] = 201; // PT = RR
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_is_rtcp_packet_bye() {
        let mut data = vec![0u8; 8];
        data[0] = 0x81;
        data[1] = 203; // PT = BYE
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_is_rtp_packet() {
        // Build a minimal RTP packet.
        let h = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 0, // PCMU
            sequence: 1,
            timestamp: 160,
            ssrc: 999,
        };
        let data = h.to_bytes();
        assert!(!is_rtcp_packet(&data));
    }

    #[test]
    fn test_is_rtp_packet_with_marker() {
        let h = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: true,
            payload_type: 96, // dynamic
            sequence: 1,
            timestamp: 160,
            ssrc: 999,
        };
        let data = h.to_bytes();
        // marker=true means byte[1] = 0x80 | 96 = 224. Not in RTCP range.
        assert!(!is_rtcp_packet(&data));
    }

    #[test]
    fn test_is_rtcp_packet_too_short() {
        assert!(!is_rtcp_packet(&[]));
        assert!(!is_rtcp_packet(&[0x80]));
    }

    #[test]
    fn test_sdp_offers_rtcp_mux() {
        let attrs_with = vec![
            ("rtpmap".to_string(), Some("0 PCMU/8000".to_string())),
            ("rtcp-mux".to_string(), None),
        ];
        assert!(sdp_offers_rtcp_mux(&attrs_with));

        let attrs_without = vec![
            ("rtpmap".to_string(), Some("0 PCMU/8000".to_string())),
        ];
        assert!(!sdp_offers_rtcp_mux(&attrs_without));
    }

    #[test]
    fn test_rtcp_mux_negotiated() {
        let local = vec![("rtcp-mux".to_string(), None)];
        let remote = vec![("rtcp-mux".to_string(), None)];
        assert!(rtcp_mux_negotiated(&local, &remote));

        let remote_no = vec![("rtpmap".to_string(), Some("0 PCMU/8000".to_string()))];
        assert!(!rtcp_mux_negotiated(&local, &remote_no));
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL RTCP-MUX TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_rtcp_mux_rtp_pt0_pcmu_is_rtp() {
        // RTP PT 0 (PCMU) must be classified as RTP, not RTCP
        let h = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: false, payload_type: 0, // PCMU
            sequence: 1, timestamp: 160, ssrc: 999,
        };
        let data = h.to_bytes();
        assert!(!is_rtcp_packet(&data), "PT 0 (PCMU) should be classified as RTP");
    }

    #[test]
    fn test_rtcp_mux_rtcp_sr_pt200_is_rtcp() {
        // RTCP PT 200 (SR) must be classified as RTCP
        let mut data = vec![0u8; 28];
        data[0] = 0x80;
        data[1] = 200; // SR
        assert!(is_rtcp_packet(&data), "PT 200 (SR) should be classified as RTCP");
    }

    #[test]
    fn test_rtcp_mux_ambiguous_pt72_76_classified_correctly() {
        // PT 72-76 are ambiguous: for RTP they'd be PT 72-76 with marker=1
        // (byte[1] = 0x80 | PT = 200-204). But we classify based on byte[1] value.
        // With marker bit set (0x80), PT 72 => byte[1] = 0x80|72 = 200.
        // This looks like RTCP SR! This is the known ambiguity.
        // RFC 5761 recommends not using PT 72-76 for RTP to avoid this.
        let h = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: true, payload_type: 72, // byte[1] = 0x80|72 = 200
            sequence: 1, timestamp: 160, ssrc: 999,
        };
        let data = h.to_bytes();
        // byte[1] = 200, so it will be classified as RTCP (known behavior)
        assert!(is_rtcp_packet(&data), "PT 72 with marker is in RTCP range (known ambiguity)");
    }

    #[test]
    fn test_rtcp_mux_compound_rtcp_is_rtcp() {
        // A compound RTCP (SR + SDES) should be classified as RTCP
        // (we only check first packet's PT)
        let mut data = vec![0u8; 40];
        data[0] = 0x80;
        data[1] = 200; // SR
        data[2] = 0;
        data[3] = 6;   // length in words - 1
        // The second RTCP packet (SDES) would follow at offset 28
        // But our classifier only checks the first PT
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_rtcp_mux_rtp_dynamic_pt96_is_rtp() {
        let h = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: false, payload_type: 96, // Dynamic
            sequence: 1, timestamp: 160, ssrc: 999,
        };
        let data = h.to_bytes();
        assert!(!is_rtcp_packet(&data), "PT 96 should be classified as RTP");
    }

    #[test]
    fn test_rtcp_mux_rtp_dynamic_pt127_is_rtp() {
        let h = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: false, payload_type: 127, // Max static PT
            sequence: 1, timestamp: 160, ssrc: 999,
        };
        let data = h.to_bytes();
        assert!(!is_rtcp_packet(&data), "PT 127 should be classified as RTP");
    }

    #[test]
    fn test_rtcp_pt_sdes_202() {
        let mut data = vec![0u8; 12];
        data[0] = 0x81;
        data[1] = 202; // SDES
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_rtcp_pt_app_204() {
        let mut data = vec![0u8; 12];
        data[0] = 0x80;
        data[1] = 204; // APP
        assert!(is_rtcp_packet(&data));
    }

    #[test]
    fn test_rtcp_pt_above_range_is_not_rtcp() {
        let mut data = vec![0u8; 12];
        data[0] = 0x80;
        data[1] = 214; // Above RTCP range
        assert!(!is_rtcp_packet(&data));
    }
}
