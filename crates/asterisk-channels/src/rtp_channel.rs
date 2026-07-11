//! RTP (bare) channel driver.
//!
//! Port of `channels/chan_rtp.c`. Channel backed by raw UDP RTP socket.
//!
//! The `UnicastRTP` technology is the media leg behind ARI's
//! `POST /channels/externalMedia`: the channel's `write_frame` forks audio as
//! RTP to a fixed external `host:port`, and `read_frame` injects RTP received
//! back from that endpoint into the channel. Destination syntax for
//! `request()` is `<host:port>[/<format>]`, e.g. `192.0.2.1:12345/ulaw`.

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

/// Channel variable exposing the local RTP bind address (like chan_rtp.c).
pub const UNICASTRTP_LOCAL_ADDRESS: &str = "UNICASTRTP_LOCAL_ADDRESS";
/// Channel variable exposing the local RTP bind port (like chan_rtp.c).
pub const UNICASTRTP_LOCAL_PORT: &str = "UNICASTRTP_LOCAL_PORT";

/// Map an Asterisk format name to its static RTP payload type and the number
/// of samples carried per 20ms packet. Only the narrowband G.711 variants are
/// supported until the codec layer grows transcoding.
fn format_to_payload(format: &str) -> Option<(u8, u32)> {
    match format {
        "ulaw" | "mulaw" | "pcmu" => Some((0, 160)),
        "alaw" | "pcma" => Some((8, 160)),
        _ => None,
    }
}

/// Formats accepted by [`format_to_payload`], for error messages and
/// ARI-side validation.
pub fn supported_formats() -> &'static [&'static str] {
    &["ulaw", "mulaw", "pcmu", "alaw", "pcma"]
}

/// Global counter for unique channel-name suffixes (like chan_pjsip's
/// counter); two concurrent channels to the same destination must not
/// collide on a name because driver private data is keyed by name.
static CHANNEL_COUNTER: AtomicU32 = AtomicU32::new(1);

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

    /// Private data is keyed by channel NAME (not unique_id): the global
    /// channel store may re-assign a channel's unique_id when registering it
    /// (`register_existing_channel`), which would orphan an id-keyed entry
    /// and leak its RTP socket -- the exact failure shape of finding F23.
    fn get_private(&self, name: &str) -> Option<Arc<Mutex<RtpPrivate>>> {
        self.channels.read().get(name).cloned()
    }

    fn remove_private(&self, name: &str) -> Option<Arc<Mutex<RtpPrivate>>> {
        self.channels.write().remove(name)
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

    /// Request a UnicastRTP channel.
    ///
    /// `dest` format: `<host:port>[/<format>]` where `<format>` is an
    /// Asterisk format name (default `ulaw`). Binds a fresh UDP socket (and
    /// thus a fresh SSRC), points it at the external address, and exposes
    /// the local bind address via the `UNICASTRTP_LOCAL_ADDRESS` /
    /// `UNICASTRTP_LOCAL_PORT` channel variables like chan_rtp.c.
    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let (addr_str, options) = match dest.split_once('/') {
            Some((a, o)) => (a, Some(o)),
            None => (dest, None),
        };

        let remote_addr: SocketAddr = addr_str.parse().map_err(|e| {
            AsteriskError::InvalidArgument(format!("Invalid address '{}': {}", addr_str, e))
        })?;

        let format = match options {
            Some(f) if !f.is_empty() => f,
            _ => "ulaw",
        };
        let (payload_type, samples_per_packet) =
            format_to_payload(format).ok_or_else(|| {
                AsteriskError::InvalidArgument(format!(
                    "Unsupported format '{}' for UnicastRTP (supported: {})",
                    format,
                    supported_formats().join(", ")
                ))
            })?;

        let bind_addr = if remote_addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let socket = UdpSocket::bind(bind_addr).await?;
        let local_addr = socket.local_addr()?;
        let ssrc = Self::generate_ssrc();

        let counter = CHANNEL_COUNTER.fetch_add(1, Ordering::Relaxed);
        let chan_name = format!("UnicastRTP/{}-{:08x}", addr_str, counter);
        let mut channel = Channel::new(chan_name);
        channel.read_format = format.to_string();
        channel.write_format = format.to_string();
        channel.variables.insert(
            UNICASTRTP_LOCAL_ADDRESS.to_string(),
            local_addr.ip().to_string(),
        );
        channel.variables.insert(
            UNICASTRTP_LOCAL_PORT.to_string(),
            local_addr.port().to_string(),
        );

        let priv_data = Arc::new(Mutex::new(RtpPrivate {
            socket: Arc::new(socket),
            remote_addr: Some(remote_addr),
            sequence: AtomicU16::new(0),
            timestamp: AtomicU32::new(0),
            ssrc,
            payload_type,
            samples_per_packet,
        }));
        self.channels.write().insert(channel.name.clone(), priv_data);
        info!(
            channel = %channel.name,
            remote = %remote_addr,
            local = %local_addr,
            ssrc,
            format,
            "RTP channel created"
        );
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

    /// Hang up: drops the private entry, which releases the bound RTP socket
    /// (no orphaned driver entries -- see finding F23 for the leak shape this
    /// must avoid).
    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        self.remove_private(&channel.name);
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "RTP channel hungup");
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_arc = self
            .get_private(&channel.name)
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
            .get_private(&channel.name)
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

    #[test]
    fn test_format_to_payload_mapping() {
        assert_eq!(format_to_payload("ulaw"), Some((0, 160)));
        assert_eq!(format_to_payload("alaw"), Some((8, 160)));
        assert_eq!(format_to_payload("pcmu"), Some((0, 160)));
        assert_eq!(format_to_payload("g729"), None);
        assert_eq!(format_to_payload(""), None);
    }

    #[tokio::test]
    async fn test_request_rejects_bad_dest_and_format() {
        let driver = RtpChannelDriver::new();
        assert!(driver.request("not-an-address", None).await.is_err());
        assert!(driver.request("127.0.0.1", None).await.is_err()); // no port
        assert!(driver.request("127.0.0.1:4000/g729", None).await.is_err());
    }

    #[tokio::test]
    async fn test_request_unique_names_and_local_addr_vars() {
        let driver = RtpChannelDriver::new();
        let a = driver.request("127.0.0.1:4000/ulaw", None).await.unwrap();
        let b = driver.request("127.0.0.1:4000/ulaw", None).await.unwrap();
        assert_ne!(a.name, b.name, "concurrent channels to the same dest must not collide");
        assert!(a.name.starts_with("UnicastRTP/127.0.0.1:4000-"));
        assert!(a.variables.contains_key(UNICASTRTP_LOCAL_ADDRESS));
        let port: u16 = a
            .variables
            .get(UNICASTRTP_LOCAL_PORT)
            .expect("local port var")
            .parse()
            .expect("port must be numeric");
        assert_ne!(port, 0);
    }

    /// Regression guard against the F23 leak shape: after `hangup()`, the
    /// driver entry (and its socket) must be gone, so further media ops fail
    /// rather than silently using a leaked socket.
    #[tokio::test]
    async fn test_hangup_releases_driver_entry() {
        let driver = RtpChannelDriver::new();
        let mut chan = driver.request("127.0.0.1:4000/ulaw", None).await.unwrap();
        assert!(driver.get_private(&chan.name).is_some());

        driver.hangup(&mut chan).await.unwrap();
        assert!(driver.get_private(&chan.name).is_none());
        let frame = Frame::voice(0, 160, Bytes::from_static(&[0x7F; 160]));
        assert!(driver.write_frame(&mut chan, &frame).await.is_err());
    }

    /// The media plane must survive the global store re-assigning unique_id
    /// (as `register_existing_channel` does): privates are keyed by name.
    #[tokio::test]
    async fn test_media_survives_uniqueid_reassignment() {
        let driver = RtpChannelDriver::new();
        let mut chan = driver.request("127.0.0.1:4001/alaw", None).await.unwrap();
        chan.unique_id = asterisk_core::channel::ChannelId::from_name("rewritten-id");

        // UDP send_to needs no listener; this only exercises the by-name lookup.
        let frame = Frame::voice(8, 160, Bytes::from_static(&[0x55; 160]));
        let res = driver.write_frame(&mut chan, &frame).await;
        assert!(res.is_ok(), "write_frame must find the private by name: {:?}", res);
    }
}
