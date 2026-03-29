//! RTP (bare) channel driver.
//!
//! Port of `channels/chan_rtp.c`. Channel backed by raw UDP RTP socket.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use parking_lot::RwLock;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

const RTP_HEADER_SIZE: usize = 12;
const RTP_MAX_PAYLOAD: usize = 1400;

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

    pub fn header_size(&self) -> usize {
        RTP_HEADER_SIZE + (self.csrc_count as usize) * 4
    }

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

pub fn build_rtp_packet(header: &RtpHeader, payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(RTP_HEADER_SIZE + payload.len());
    buf.put_slice(&header.to_bytes());
    buf.put_slice(payload);
    buf.freeze()
}

pub fn parse_rtp_packet(data: &[u8]) -> Result<(RtpHeader, &[u8]), AsteriskError> {
    let header = RtpHeader::parse(data)?;
    let offset = header.header_size();
    if data.len() < offset {
        return Err(AsteriskError::Parse("RTP packet truncated".into()));
    }
    Ok((header, &data[offset..]))
}

struct RtpPrivate {
    socket: Arc<UdpSocket>,
    remote_addr: Option<SocketAddr>,
    sequence: AtomicU16,
    timestamp: AtomicU32,
    ssrc: u32,
    payload_type: u8,
    samples_per_packet: u32,
}

impl fmt::Debug for RtpPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtpPrivate")
            .field("remote_addr", &self.remote_addr)
            .field("ssrc", &self.ssrc)
            .field("payload_type", &self.payload_type)
            .finish()
    }
}

pub struct RtpChannelDriver {
    channels: RwLock<HashMap<String, Arc<Mutex<RtpPrivate>>>>,
}

impl fmt::Debug for RtpChannelDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtpChannelDriver")
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl RtpChannelDriver {
    pub fn new() -> Self {
        Self { channels: RwLock::new(HashMap::new()) }
    }

    fn get_private(&self, id: &str) -> Option<Arc<Mutex<RtpPrivate>>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<Mutex<RtpPrivate>>> {
        self.channels.write().remove(id)
    }

    fn generate_ssrc() -> u32 {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            ^ 0xDEAD_BEEF
    }
}

impl Default for RtpChannelDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for RtpChannelDriver {
    fn name(&self) -> &str {
        "UnicastRTP"
    }

    fn description(&self) -> &str {
        "Unicast RTP Media Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let (addr_str, _options) = match dest.split_once('/') {
            Some((a, o)) => (a, Some(o)),
            None => (dest, None),
        };

        let remote_addr: SocketAddr = addr_str.parse().map_err(|e| {
            AsteriskError::InvalidArgument(format!("Invalid address '{}': {}", addr_str, e))
        })?;

        let bind_addr = if remote_addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let socket = UdpSocket::bind(bind_addr).await?;
        let local_addr = socket.local_addr()?;
        let ssrc = Self::generate_ssrc();

        let chan_name = format!("UnicastRTP/{}", addr_str);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(Mutex::new(RtpPrivate {
            socket: Arc::new(socket),
            remote_addr: Some(remote_addr),
            sequence: AtomicU16::new(0),
            timestamp: AtomicU32::new(0),
            ssrc,
            payload_type: 0,
            samples_per_packet: 160,
        }));
        self.channels.write().insert(channel_id, priv_data);
        info!(remote = %remote_addr, local = %local_addr, ssrc, "RTP channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        channel.answer();
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        self.remove_private(channel.unique_id.as_str());
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "RTP channel hungup");
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_arc = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;
        let priv_data = priv_arc.lock().await;
        let mut buf = vec![0u8; RTP_HEADER_SIZE + RTP_MAX_PAYLOAD];
        let (len, _) = priv_data.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        let (header, payload) = parse_rtp_packet(&buf)?;
        let samples = match header.payload_type {
            0 | 8 => payload.len() as u32,
            _ => (payload.len() as u32) / 2,
        };
        Ok(Frame::voice(header.payload_type as u32, samples, Bytes::copy_from_slice(payload)))
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let data = match frame {
            Frame::Voice { data, .. } => data.clone(),
            _ => return Ok(()),
        };

        let priv_arc = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;
        let priv_data = priv_arc.lock().await;
        let remote_addr = priv_data.remote_addr
            .ok_or_else(|| AsteriskError::InvalidArgument("No remote RTP address".into()))?;

        let seq = priv_data.sequence.fetch_add(1, Ordering::Relaxed);
        let ts = priv_data.timestamp.fetch_add(priv_data.samples_per_packet, Ordering::Relaxed);

        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: priv_data.payload_type,
            sequence: seq,
            timestamp: ts,
            ssrc: priv_data.ssrc,
        };

        let packet = build_rtp_packet(&header, &data);
        priv_data.socket.send_to(&packet, remote_addr).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtp_header_roundtrip() {
        let header = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: true, payload_type: 0, sequence: 1234,
            timestamp: 56789, ssrc: 0xDEADBEEF,
        };
        let bytes = header.to_bytes();
        let parsed = RtpHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.version, 2);
        assert!(parsed.marker);
        assert_eq!(parsed.payload_type, 0);
        assert_eq!(parsed.sequence, 1234);
        assert_eq!(parsed.timestamp, 56789);
        assert_eq!(parsed.ssrc, 0xDEADBEEF);
    }

    #[test]
    fn test_build_and_parse_rtp_packet() {
        let header = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: false, payload_type: 8, sequence: 42,
            timestamp: 320, ssrc: 12345,
        };
        let payload = vec![0x80; 160];
        let packet = build_rtp_packet(&header, &payload);
        let (h, p) = parse_rtp_packet(&packet).unwrap();
        assert_eq!(h.payload_type, 8);
        assert_eq!(h.sequence, 42);
        assert_eq!(p.len(), 160);
    }
}
