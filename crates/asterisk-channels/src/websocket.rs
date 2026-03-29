//! WebSocket channel driver.
//!
//! Port of `channels/chan_websocket.c` and `res/res_http_websocket.c`.
//! Provides WebSocket-based channels for WebRTC and other web-based
//! telephony applications. Includes full WebSocket frame parsing and
//! construction per RFC 6455.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

// ---------------------------------------------------------------------------
// WebSocket protocol constants (RFC 6455)
// ---------------------------------------------------------------------------

/// GUID used for the WebSocket handshake accept key computation (RFC 6455 sec 4.2.2).
pub const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Maximum WebSocket frame header size (2 + 8 + 4 = 14 bytes).
pub const MAX_WS_HDR_SIZE: usize = 14;

/// Minimum WebSocket frame header size.
pub const MIN_WS_HDR_SIZE: usize = 2;

/// Default maximum payload size.
pub const MAX_PAYLOAD_SIZE: usize = 65536;

// ---------------------------------------------------------------------------
// WebSocket opcodes
// ---------------------------------------------------------------------------

/// WebSocket frame opcodes (RFC 6455 sec 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WebSocketOpcode {
    /// Continuation frame.
    Continuation = 0x0,
    /// Text frame (UTF-8 encoded).
    Text = 0x1,
    /// Binary frame.
    Binary = 0x2,
    // 0x3-0x7 reserved for further non-control frames.
    /// Connection close.
    Close = 0x8,
    /// Ping.
    Ping = 0x9,
    /// Pong.
    Pong = 0xA,
}

impl WebSocketOpcode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xA => Some(Self::Pong),
            _ => None,
        }
    }

    /// Whether this is a control frame (close, ping, pong).
    pub fn is_control(&self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

// ---------------------------------------------------------------------------
// WebSocket frame
// ---------------------------------------------------------------------------

/// A parsed WebSocket frame.
#[derive(Debug, Clone)]
pub struct WebSocketFrame {
    /// Whether this is the final fragment of a message.
    pub fin: bool,
    /// Frame opcode.
    pub opcode: WebSocketOpcode,
    /// Whether the payload is masked.
    pub masked: bool,
    /// Masking key (4 bytes, only valid if `masked` is true).
    pub mask_key: [u8; 4],
    /// Payload data (already unmasked if it was masked).
    pub payload: Bytes,
}

impl WebSocketFrame {
    /// Parse a WebSocket frame from a byte buffer.
    ///
    /// Returns the parsed frame and the number of bytes consumed, or `None`
    /// if the buffer does not contain a complete frame.
    pub fn parse(data: &[u8]) -> Result<Option<(Self, usize)>, AsteriskError> {
        if data.len() < MIN_WS_HDR_SIZE {
            return Ok(None); // Need more data.
        }

        let byte0 = data[0];
        let byte1 = data[1];

        let fin = (byte0 & 0x80) != 0;
        let rsv = (byte0 >> 4) & 0x07;
        if rsv != 0 {
            return Err(AsteriskError::Parse(format!(
                "WebSocket RSV bits set: {}",
                rsv
            )));
        }

        let opcode_val = byte0 & 0x0F;
        let opcode = WebSocketOpcode::from_u8(opcode_val).ok_or_else(|| {
            AsteriskError::Parse(format!("Unknown WebSocket opcode: 0x{:x}", opcode_val))
        })?;

        let masked = (byte1 & 0x80) != 0;
        let payload_len_initial = (byte1 & 0x7F) as u64;

        let mut offset = 2usize;
        let payload_len: usize;

        if payload_len_initial <= 125 {
            payload_len = payload_len_initial as usize;
        } else if payload_len_initial == 126 {
            if data.len() < offset + 2 {
                return Ok(None);
            }
            payload_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
        } else {
            // payload_len_initial == 127
            if data.len() < offset + 8 {
                return Ok(None);
            }
            let raw_len = u64::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            // Reject payload lengths that exceed our maximum to prevent DoS.
            // Also guard against truncation on 32-bit platforms.
            if raw_len > MAX_PAYLOAD_SIZE as u64 {
                return Err(AsteriskError::Parse(format!(
                    "WebSocket payload too large: {} bytes (max {})",
                    raw_len, MAX_PAYLOAD_SIZE
                )));
            }
            payload_len = raw_len as usize;
            offset += 8;
        }

        let mut mask_key = [0u8; 4];
        if masked {
            if data.len() < offset + 4 {
                return Ok(None);
            }
            mask_key.copy_from_slice(&data[offset..offset + 4]);
            offset += 4;
        }

        let total_len = offset + payload_len;
        if data.len() < total_len {
            return Ok(None); // Need more data.
        }

        let mut payload = data[offset..total_len].to_vec();

        // Unmask if necessary.
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask_key[i % 4];
            }
        }

        // RFC 6455 sec 5.5: control frames (close, ping, pong) MUST have
        // payload <= 125 bytes.
        if opcode.is_control() && payload.len() > 125 {
            return Err(AsteriskError::Parse(format!(
                "WebSocket control frame payload too large: {} bytes (max 125)",
                payload.len()
            )));
        }

        // RFC 6455 sec 7.1.5: close frames with a body must have >= 2 bytes
        // for the status code, and the status code must be valid.
        if opcode == WebSocketOpcode::Close && !payload.is_empty() {
            if payload.len() < 2 {
                return Err(AsteriskError::Parse(
                    "WebSocket close frame has body but < 2 bytes for status code".into(),
                ));
            }
            let code = u16::from_be_bytes([payload[0], payload[1]]);
            // RFC 6455 sec 7.4.1: codes 0-999 are not used.
            // Codes 1000-2999 are reserved for the protocol.
            // Codes 3000-3999 are for libraries/frameworks.
            // Codes 4000-4999 are for private use.
            // Specific codes like 1005, 1006, 1015 MUST NOT appear on wire.
            if code < 1000
                || code == close_code::NO_STATUS
                || code == close_code::ABNORMAL
                || (code >= 1016 && code < 3000)
            {
                return Err(AsteriskError::Parse(format!(
                    "WebSocket close frame has invalid status code: {}",
                    code
                )));
            }
        }

        Ok(Some((
            Self {
                fin,
                opcode,
                masked,
                mask_key,
                payload: Bytes::from(payload),
            },
            total_len,
        )))
    }

    /// Serialize a WebSocket frame to bytes (server -> client, no masking).
    pub fn to_bytes(&self) -> BytesMut {
        self.to_bytes_with_mask(false, &[0; 4])
    }

    /// Serialize a WebSocket frame with optional masking (client -> server).
    pub fn to_bytes_with_mask(&self, mask: bool, mask_key: &[u8; 4]) -> BytesMut {
        let payload_len = self.payload.len();
        let mut buf = BytesMut::with_capacity(MAX_WS_HDR_SIZE + payload_len);

        let byte0 = (if self.fin { 0x80 } else { 0x00 }) | (self.opcode as u8);
        buf.put_u8(byte0);

        let mask_bit: u8 = if mask { 0x80 } else { 0x00 };

        if payload_len <= 125 {
            buf.put_u8(mask_bit | (payload_len as u8));
        } else if payload_len <= 65535 {
            buf.put_u8(mask_bit | 126);
            buf.put_u16(payload_len as u16);
        } else {
            buf.put_u8(mask_bit | 127);
            buf.put_u64(payload_len as u64);
        }

        if mask {
            buf.put_slice(mask_key);
            let mut masked_payload = self.payload.to_vec();
            for (i, byte) in masked_payload.iter_mut().enumerate() {
                *byte ^= mask_key[i % 4];
            }
            buf.put_slice(&masked_payload);
        } else {
            buf.put_slice(&self.payload);
        }

        buf
    }

    /// Create a text frame.
    pub fn text(data: &str) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Text,
            masked: false,
            mask_key: [0; 4],
            payload: Bytes::copy_from_slice(data.as_bytes()),
        }
    }

    /// Create a binary frame.
    pub fn binary(data: Bytes) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Binary,
            masked: false,
            mask_key: [0; 4],
            payload: data,
        }
    }

    /// Create a close frame.
    pub fn close(status_code: u16, reason: &str) -> Self {
        let mut payload = BytesMut::with_capacity(2 + reason.len());
        payload.put_u16(status_code);
        payload.put_slice(reason.as_bytes());
        Self {
            fin: true,
            opcode: WebSocketOpcode::Close,
            masked: false,
            mask_key: [0; 4],
            payload: payload.freeze(),
        }
    }

    /// Create a ping frame.
    pub fn ping(data: &[u8]) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Ping,
            masked: false,
            mask_key: [0; 4],
            payload: Bytes::copy_from_slice(data),
        }
    }

    /// Create a pong frame.
    pub fn pong(data: &[u8]) -> Self {
        Self {
            fin: true,
            opcode: WebSocketOpcode::Pong,
            masked: false,
            mask_key: [0; 4],
            payload: Bytes::copy_from_slice(data),
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket handshake
// ---------------------------------------------------------------------------

/// Compute the WebSocket accept key from the client's Sec-WebSocket-Key.
pub fn compute_accept_key(client_key: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(client_key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    let result = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &result)
}

/// Build a WebSocket upgrade response for a server handshake.
pub fn build_upgrade_response(client_key: &str, protocol: Option<&str>) -> String {
    let accept = compute_accept_key(client_key);
    let mut response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n",
        accept
    );
    if let Some(proto) = protocol {
        response.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", proto));
    }
    response.push_str("\r\n");
    response
}

// ---------------------------------------------------------------------------
// WebSocket server (simplified)
// ---------------------------------------------------------------------------

/// WebSocket server that accepts connections on an HTTP path.
#[derive(Debug)]
pub struct WebSocketServer {
    /// Path to accept WebSocket connections on.
    pub path: String,
    /// Registered protocol handlers.
    pub protocols: Vec<String>,
}

impl WebSocketServer {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            protocols: Vec::new(),
        }
    }

    /// Register a sub-protocol.
    pub fn add_protocol(&mut self, protocol: &str) {
        self.protocols.push(protocol.to_string());
    }
}

// ---------------------------------------------------------------------------
// WebSocket close codes (RFC 6455 sec 7.4.1)
// ---------------------------------------------------------------------------

pub mod close_code {
    pub const NORMAL: u16 = 1000;
    pub const GOING_AWAY: u16 = 1001;
    pub const PROTOCOL_ERROR: u16 = 1002;
    pub const UNSUPPORTED_DATA: u16 = 1003;
    pub const NO_STATUS: u16 = 1005;
    pub const ABNORMAL: u16 = 1006;
    pub const INVALID_PAYLOAD: u16 = 1007;
    pub const POLICY_VIOLATION: u16 = 1008;
    pub const MESSAGE_TOO_BIG: u16 = 1009;
    pub const MANDATORY_EXTENSION: u16 = 1010;
    pub const INTERNAL_ERROR: u16 = 1011;
}

// ---------------------------------------------------------------------------
// Per-channel private data
// ---------------------------------------------------------------------------

struct WebSocketPrivate {
    stream: Mutex<TcpStream>,
    read_buf: Mutex<BytesMut>,
    closing: AtomicBool,
    close_sent: AtomicBool,
    session_id: String,
}

impl fmt::Debug for WebSocketPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketPrivate")
            .field("session_id", &self.session_id)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Channel driver
// ---------------------------------------------------------------------------

/// WebSocket channel driver.
///
/// Port of `chan_websocket.c` + `res_http_websocket.c`. Provides channels
/// backed by WebSocket connections for WebRTC and SIP-over-WebSocket
/// (RFC 7118) use cases.
pub struct WebSocketDriver {
    channels: RwLock<HashMap<String, Arc<WebSocketPrivate>>>,
}

impl fmt::Debug for WebSocketDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketDriver")
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl WebSocketDriver {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }

    fn get_private(&self, id: &str) -> Option<Arc<WebSocketPrivate>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<WebSocketPrivate>> {
        self.channels.write().remove(id)
    }

    /// Send a WebSocket frame over the stream.
    async fn send_ws_frame(
        stream: &mut TcpStream,
        frame: &WebSocketFrame,
    ) -> AsteriskResult<()> {
        let data = frame.to_bytes();
        stream.write_all(&data).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Read a complete WebSocket frame from the stream + buffer.
    async fn recv_ws_frame(
        stream: &mut TcpStream,
        buf: &mut BytesMut,
    ) -> AsteriskResult<WebSocketFrame> {
        loop {
            // Try parsing what we have.
            if let Some((frame, consumed)) = WebSocketFrame::parse(buf)? {
                buf.advance(consumed);
                return Ok(frame);
            }

            // Need more data.
            let mut tmp = [0u8; 4096];
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(AsteriskError::Hangup("WebSocket connection closed".into()));
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    }
}

impl Default for WebSocketDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for WebSocketDriver {
    fn name(&self) -> &str {
        "WebSocket"
    }

    fn description(&self) -> &str {
        "WebSocket Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        // dest format: "addr:port"
        let stream = TcpStream::connect(dest).await.map_err(|e| {
            AsteriskError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("Failed to connect to WebSocket server '{}': {}", dest, e),
            ))
        })?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let chan_name = format!("WebSocket/{}", dest);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(WebSocketPrivate {
            stream: Mutex::new(stream),
            read_buf: Mutex::new(BytesMut::with_capacity(8192)),
            closing: AtomicBool::new(false),
            close_sent: AtomicBool::new(false),
            session_id,
        });

        self.channels.write().insert(channel_id, priv_data);
        info!(dest, "WebSocket channel created");
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
        if let Some(priv_data) = self.remove_private(channel.unique_id.as_str()) {
            if !priv_data.close_sent.load(Ordering::Relaxed) {
                priv_data.close_sent.store(true, Ordering::Relaxed);
                let close_frame = WebSocketFrame::close(close_code::NORMAL, "Hangup");
                let mut stream = priv_data.stream.lock().await;
                let _ = Self::send_ws_frame(&mut stream, &close_frame).await;
                let _ = stream.shutdown().await;
            }
            info!(session = %priv_data.session_id, "WebSocket channel hungup");
        }
        channel.set_state(ChannelState::Down);
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if priv_data.closing.load(Ordering::Relaxed) {
            return Ok(Frame::control(ControlFrame::Hangup));
        }

        let mut stream = priv_data.stream.lock().await;
        let mut buf = priv_data.read_buf.lock().await;

        loop {
            let ws_frame = Self::recv_ws_frame(&mut stream, &mut buf).await?;

            match ws_frame.opcode {
                WebSocketOpcode::Binary => {
                    // Binary frames carry audio data.
                    let samples = (ws_frame.payload.len() / 2) as u32;
                    return Ok(Frame::voice(0, samples, ws_frame.payload));
                }
                WebSocketOpcode::Text => {
                    // Text frames carry signaling / JSON.
                    let text = String::from_utf8_lossy(&ws_frame.payload).to_string();
                    return Ok(Frame::text(text));
                }
                WebSocketOpcode::Close => {
                    priv_data.closing.store(true, Ordering::Relaxed);
                    // Send close response if we haven't already.
                    if !priv_data.close_sent.load(Ordering::Relaxed) {
                        priv_data.close_sent.store(true, Ordering::Relaxed);
                        let close = WebSocketFrame::close(close_code::NORMAL, "");
                        let _ = Self::send_ws_frame(&mut stream, &close).await;
                    }
                    return Ok(Frame::control(ControlFrame::Hangup));
                }
                WebSocketOpcode::Ping => {
                    // Respond with pong (same payload).
                    let pong = WebSocketFrame::pong(&ws_frame.payload);
                    let _ = Self::send_ws_frame(&mut stream, &pong).await;
                    // Continue reading.
                }
                WebSocketOpcode::Pong => {
                    // Pong received -- ignore and continue.
                }
                WebSocketOpcode::Continuation => {
                    // Multi-frame messages not currently supported -- skip.
                    warn!("WebSocket continuation frame received (not supported)");
                }
            }
        }
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if priv_data.closing.load(Ordering::Relaxed) {
            return Err(AsteriskError::Hangup("WebSocket closing".into()));
        }

        let mut stream = priv_data.stream.lock().await;

        match frame {
            Frame::Voice { data, .. } => {
                let ws_frame = WebSocketFrame::binary(data.clone());
                Self::send_ws_frame(&mut stream, &ws_frame).await?;
            }
            Frame::Text { text } => {
                let ws_frame = WebSocketFrame::text(text);
                Self::send_ws_frame(&mut stream, &ws_frame).await?;
            }
            _ => {
                debug!(
                    frame_type = ?frame.frame_type(),
                    "WebSocket: ignoring unsupported frame type"
                );
            }
        }
        Ok(())
    }

    async fn send_text(&self, channel: &mut Channel, text: &str) -> AsteriskResult<()> {
        let frame = Frame::text(text.to_string());
        self.write_frame(channel, &frame).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_frame_text_roundtrip() {
        let frame = WebSocketFrame::text("Hello, world!");
        let bytes = frame.to_bytes();
        let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert!(parsed.fin);
        assert_eq!(parsed.opcode, WebSocketOpcode::Text);
        assert!(!parsed.masked);
        assert_eq!(parsed.payload, Bytes::from("Hello, world!"));
    }

    #[test]
    fn test_ws_frame_binary_roundtrip() {
        let data = Bytes::from(vec![0u8; 1024]);
        let frame = WebSocketFrame::binary(data.clone());
        let bytes = frame.to_bytes();
        let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.opcode, WebSocketOpcode::Binary);
        assert_eq!(parsed.payload.len(), 1024);
    }

    #[test]
    fn test_ws_frame_close_roundtrip() {
        let frame = WebSocketFrame::close(close_code::NORMAL, "bye");
        let bytes = frame.to_bytes();
        let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(parsed.opcode, WebSocketOpcode::Close);
        // First two bytes are the status code.
        let status = u16::from_be_bytes([parsed.payload[0], parsed.payload[1]]);
        assert_eq!(status, 1000);
        let reason = std::str::from_utf8(&parsed.payload[2..]).unwrap();
        assert_eq!(reason, "bye");
    }

    #[test]
    fn test_ws_frame_ping_pong() {
        let ping = WebSocketFrame::ping(b"hello");
        let bytes = ping.to_bytes();
        let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(parsed.opcode, WebSocketOpcode::Ping);
        assert_eq!(&parsed.payload[..], b"hello");

        let pong = WebSocketFrame::pong(&parsed.payload);
        let bytes = pong.to_bytes();
        let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(parsed.opcode, WebSocketOpcode::Pong);
    }

    #[test]
    fn test_ws_frame_masked() {
        let frame = WebSocketFrame::text("test");
        let mask_key = [0x37, 0xFA, 0x21, 0x3D];
        let bytes = frame.to_bytes_with_mask(true, &mask_key);

        let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        // After parsing, the payload should be unmasked automatically.
        assert_eq!(parsed.payload, Bytes::from("test"));
    }

    #[test]
    fn test_ws_frame_large_payload() {
        // Payload > 125 bytes (uses 16-bit length).
        let data = Bytes::from(vec![0xAB; 300]);
        let frame = WebSocketFrame::binary(data.clone());
        let bytes = frame.to_bytes();
        let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.payload.len(), 300);
    }

    #[test]
    fn test_ws_frame_64bit_length_within_limit() {
        // Payload > 65535 bytes (uses 64-bit length) but within MAX_PAYLOAD_SIZE.
        let size = MAX_PAYLOAD_SIZE; // Exactly at the limit.
        let data = Bytes::from(vec![0xCD; size]);
        let frame = WebSocketFrame::binary(data.clone());
        let bytes = frame.to_bytes();
        let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.payload.len(), size);
    }

    #[test]
    fn test_ws_frame_payload_exceeds_max_rejected() {
        // Craft a frame header claiming a payload larger than MAX_PAYLOAD_SIZE.
        let mut buf = vec![0u8; 14]; // max header size
        buf[0] = 0x82; // FIN + Binary
        buf[1] = 127; // 64-bit length
        let huge_len = (MAX_PAYLOAD_SIZE as u64) + 1;
        buf[2..10].copy_from_slice(&huge_len.to_be_bytes());
        // Don't need actual payload data -- the parser should reject based on length.
        // Add enough fake data so the length check doesn't return Ok(None).
        buf.extend_from_slice(&vec![0u8; 1024]);
        let result = WebSocketFrame::parse(&buf);
        assert!(result.is_err(), "Should reject payload exceeding MAX_PAYLOAD_SIZE");
    }

    #[test]
    fn test_ws_frame_incomplete() {
        // Only 1 byte -- not enough for even the header.
        let result = WebSocketFrame::parse(&[0x81]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_compute_accept_key() {
        // Test vector from RFC 6455 sec 4.2.2.
        let key = compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn test_build_upgrade_response() {
        let response = build_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==", Some("sip"));
        assert!(response.contains("101 Switching Protocols"));
        assert!(response.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        assert!(response.contains("Sec-WebSocket-Protocol: sip"));
    }

    #[test]
    fn test_opcode_is_control() {
        assert!(WebSocketOpcode::Close.is_control());
        assert!(WebSocketOpcode::Ping.is_control());
        assert!(WebSocketOpcode::Pong.is_control());
        assert!(!WebSocketOpcode::Text.is_control());
        assert!(!WebSocketOpcode::Binary.is_control());
    }
}
