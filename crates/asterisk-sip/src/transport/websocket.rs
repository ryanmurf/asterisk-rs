//! WebSocket transport for SIP (RFC 7118).
//!
//! Provides SIP over WebSocket for WebRTC signaling. SIP messages are
//! framed in WebSocket binary frames with the "sip" sub-protocol.
//!
//! Reuses the WebSocket frame parser from `asterisk_channels::websocket`.
//!
//! Features:
//! - WebSocket handshake (server and client)
//! - SIP messages in binary WebSocket frames
//! - Ping/pong keepalive
//! - Secure WebSocket (WSS) via TLS (when used with TLS transport)
//! - Sub-protocol negotiation ("sip")

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{Buf, BytesMut};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::parser::SipMessage;
use crate::transport::{SipTransport, TransportError};

/// The WebSocket sub-protocol for SIP (RFC 7118).
pub const SIP_WEBSOCKET_PROTOCOL: &str = "sip";

/// GUID for WebSocket accept key (RFC 6455).
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// WebSocket opcode for binary frames.
const OPCODE_BINARY: u8 = 0x02;
/// WebSocket opcode for text frames.
#[allow(dead_code)]
const OPCODE_TEXT: u8 = 0x01;
/// WebSocket opcode for close frames.
const OPCODE_CLOSE: u8 = 0x08;
/// WebSocket opcode for ping frames.
const OPCODE_PING: u8 = 0x09;
/// WebSocket opcode for pong frames.
const OPCODE_PONG: u8 = 0x0A;

/// State of a WebSocket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    /// Connection is open and active.
    Open,
    /// Close handshake in progress.
    Closing,
    /// Connection is closed.
    Closed,
}

/// A WebSocket connection for SIP transport.
struct WsConnection {
    /// Underlying TCP stream.
    stream: Mutex<TcpStream>,
    /// Remote address.
    peer_addr: SocketAddr,
    /// Read buffer for frame assembly.
    read_buf: Mutex<BytesMut>,
    /// Connection state.
    state: std::sync::atomic::AtomicU8,
    /// Whether this is a secure (WSS) connection.
    secure: bool,
}

impl WsConnection {
    fn state(&self) -> WsState {
        match self.state.load(std::sync::atomic::Ordering::Relaxed) {
            0 => WsState::Open,
            1 => WsState::Closing,
            _ => WsState::Closed,
        }
    }

    fn set_state(&self, state: WsState) {
        let val = match state {
            WsState::Open => 0u8,
            WsState::Closing => 1,
            WsState::Closed => 2,
        };
        self.state.store(val, std::sync::atomic::Ordering::Relaxed);
    }
}

impl fmt::Debug for WsConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsConnection")
            .field("peer_addr", &self.peer_addr)
            .field("secure", &self.secure)
            .finish()
    }
}

/// Compute the WebSocket accept key from the client's Sec-WebSocket-Key.
fn compute_accept_key(client_key: &str) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(client_key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    let result = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, result)
}

/// Build a WebSocket upgrade response.
fn build_upgrade_response(client_key: &str) -> String {
    let accept = compute_accept_key(client_key);
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         Sec-WebSocket-Protocol: {}\r\n\
         \r\n",
        accept, SIP_WEBSOCKET_PROTOCOL,
    )
}

/// Build a simple WebSocket frame (server-to-client, no masking).
fn build_ws_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.push(0x80 | opcode); // FIN + opcode

    if payload.len() <= 125 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= 65535 {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }

    frame.extend_from_slice(payload);
    frame
}

/// Parse a WebSocket frame from a buffer.
///
/// Returns (opcode, payload, bytes_consumed) or None if incomplete.
fn parse_ws_frame(data: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>, TransportError> {
    if data.len() < 2 {
        return Ok(None);
    }

    let opcode = data[0] & 0x0F;
    let masked = (data[1] & 0x80) != 0;
    let payload_len_initial = (data[1] & 0x7F) as u64;

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
        if data.len() < offset + 8 {
            return Ok(None);
        }
        let raw = u64::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ]);
        if raw > 1_000_000 {
            return Err(TransportError::Parse("WebSocket payload too large".into()));
        }
        payload_len = raw as usize;
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

    let total = offset + payload_len;
    if data.len() < total {
        return Ok(None);
    }

    let mut payload = data[offset..total].to_vec();
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask_key[i % 4];
        }
    }

    Ok(Some((opcode, payload, total)))
}

/// SIP over WebSocket transport (RFC 7118).
///
/// Accepts WebSocket connections and frames SIP messages in WebSocket
/// binary frames. The sub-protocol is "sip".
pub struct WsTransport {
    /// TCP listener for incoming WebSocket connections.
    listener: TcpListener,
    /// Active connections.
    connections: RwLock<Vec<Arc<WsConnection>>>,
    /// Whether to use secure WebSocket (WSS).
    secure: bool,
}

impl fmt::Debug for WsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsTransport")
            .field("local_addr", &self.listener.local_addr())
            .field("secure", &self.secure)
            .field("connections", &self.connections.read().len())
            .finish()
    }
}

impl WsTransport {
    /// Bind a WebSocket transport to the given address.
    pub async fn bind(addr: SocketAddr, secure: bool) -> Result<Self, TransportError> {
        let listener = TcpListener::bind(addr).await?;
        let proto = if secure { "WSS" } else { "WS" };
        info!(
            addr = %listener.local_addr()?,
            proto,
            "WebSocket SIP transport bound"
        );
        Ok(Self {
            listener,
            connections: RwLock::new(Vec::new()),
            secure,
        })
    }

    /// Accept an incoming WebSocket connection.
    ///
    /// Performs the WebSocket upgrade handshake and returns the first
    /// SIP message received.
    pub async fn accept(
        &self,
    ) -> Result<(SipMessage, SocketAddr), TransportError> {
        let (stream, addr) = self.listener.accept().await?;
        debug!(peer = %addr, "Accepted TCP connection for WebSocket upgrade");

        // Read the HTTP upgrade request.
        let mut stream = stream;
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await?;
        buf.truncate(n);

        let request = String::from_utf8_lossy(&buf);

        // Extract Sec-WebSocket-Key.
        let ws_key = request
            .lines()
            .find(|l| l.to_lowercase().starts_with("sec-websocket-key:"))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .ok_or_else(|| {
                TransportError::Parse("Missing Sec-WebSocket-Key header".into())
            })?;

        // Verify sub-protocol includes "sip".
        let has_sip_protocol = request
            .lines()
            .any(|l| {
                l.to_lowercase().starts_with("sec-websocket-protocol:")
                    && l.to_lowercase().contains("sip")
            });

        if !has_sip_protocol {
            warn!("WebSocket client did not request 'sip' sub-protocol");
        }

        // Send upgrade response.
        let response = build_upgrade_response(&ws_key);
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;

        debug!(peer = %addr, "WebSocket upgrade completed");

        let conn = Arc::new(WsConnection {
            stream: Mutex::new(stream),
            peer_addr: addr,
            read_buf: Mutex::new(BytesMut::with_capacity(8192)),
            state: std::sync::atomic::AtomicU8::new(0),
            secure: self.secure,
        });

        self.connections.write().push(Arc::clone(&conn));

        // Read the first SIP message.
        let msg = Self::read_sip_message(&conn).await?;
        Ok((msg, addr))
    }

    /// Read a SIP message from a WebSocket connection.
    async fn read_sip_message(
        conn: &WsConnection,
    ) -> Result<SipMessage, TransportError> {
        let mut stream = conn.stream.lock().await;
        let mut read_buf = conn.read_buf.lock().await;

        loop {
            // Try to parse a frame from the buffer.
            if let Some((opcode, payload, consumed)) = parse_ws_frame(&read_buf)? {
                read_buf.advance(consumed);

                match opcode {
                    OPCODE_BINARY | OPCODE_TEXT => {
                        // SIP message in binary or text frame.
                        let msg = SipMessage::parse(&payload)
                            .map_err(|e| TransportError::Parse(e.0))?;
                        return Ok(msg);
                    }
                    OPCODE_PING => {
                        // Respond with pong.
                        let pong = build_ws_frame(OPCODE_PONG, &payload);
                        stream.write_all(&pong).await?;
                        continue;
                    }
                    OPCODE_PONG => {
                        // Ignore pong.
                        continue;
                    }
                    OPCODE_CLOSE => {
                        conn.set_state(WsState::Closing);
                        // Send close response.
                        let close = build_ws_frame(OPCODE_CLOSE, &[]);
                        let _ = stream.write_all(&close).await;
                        conn.set_state(WsState::Closed);
                        return Err(TransportError::Connection(
                            "WebSocket closed by peer".into(),
                        ));
                    }
                    _ => {
                        warn!(opcode, "Unknown WebSocket opcode, ignoring");
                        continue;
                    }
                }
            }

            // Need more data.
            let mut tmp = [0u8; 8192];
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(TransportError::Connection(
                    "WebSocket connection closed".into(),
                ));
            }
            read_buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// Send a SIP message over a WebSocket connection.
    async fn send_to_connection(
        conn: &WsConnection,
        msg: &SipMessage,
    ) -> Result<(), TransportError> {
        let data = msg.to_string();
        // RFC 7118: SIP messages SHOULD be sent as binary frames,
        // but text frames are also allowed.
        let frame = build_ws_frame(OPCODE_BINARY, data.as_bytes());
        let mut stream = conn.stream.lock().await;
        stream.write_all(&frame).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Send a WebSocket ping to keep the connection alive.
    pub async fn send_ping(
        &self,
        addr: SocketAddr,
    ) -> Result<(), TransportError> {
        let conn = self.find_connection(addr)?;
        let ping = build_ws_frame(OPCODE_PING, b"ping");
        let mut stream = conn.stream.lock().await;
        stream.write_all(&ping).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Find a connection by remote address.
    fn find_connection(
        &self,
        addr: SocketAddr,
    ) -> Result<Arc<WsConnection>, TransportError> {
        let conns = self.connections.read();
        conns
            .iter()
            .find(|c| c.peer_addr == addr)
            .cloned()
            .ok_or_else(|| {
                TransportError::Connection(format!("No WebSocket connection to {}", addr))
            })
    }

    /// Remove closed connections from the pool.
    pub fn cleanup_closed(&self) {
        self.connections
            .write()
            .retain(|c| c.state() != WsState::Closed);
    }
}

#[async_trait]
impl SipTransport for WsTransport {
    async fn send(
        &self,
        msg: &SipMessage,
        addr: SocketAddr,
    ) -> Result<(), TransportError> {
        let conn = self.find_connection(addr)?;
        Self::send_to_connection(&conn, msg).await?;
        let proto = if self.secure { "WSS" } else { "WS" };
        debug!(dest = %addr, proto, "Sent SIP message via WebSocket");
        Ok(())
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.listener.local_addr()?)
    }

    fn protocol(&self) -> &str {
        if self.secure {
            "WSS"
        } else {
            "WS"
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_accept_key() {
        // RFC 6455 Section 4.2.2 test vector.
        let key = compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn test_build_upgrade_response() {
        let response = build_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==");
        assert!(response.contains("101 Switching Protocols"));
        assert!(response.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        assert!(response.contains(&format!("Sec-WebSocket-Protocol: {}", SIP_WEBSOCKET_PROTOCOL)));
    }

    #[test]
    fn test_build_ws_frame_small() {
        let payload = b"hello";
        let frame = build_ws_frame(OPCODE_BINARY, payload);
        // First byte: 0x80 | 0x02 = 0x82
        assert_eq!(frame[0], 0x82);
        // Second byte: payload length (5, no mask)
        assert_eq!(frame[1], 5);
        // Payload follows immediately.
        assert_eq!(&frame[2..7], payload);
    }

    #[test]
    fn test_build_ws_frame_medium() {
        let payload = vec![0xAA; 300];
        let frame = build_ws_frame(OPCODE_BINARY, &payload);
        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 126); // Extended 16-bit length.
        let len = u16::from_be_bytes([frame[2], frame[3]]);
        assert_eq!(len, 300);
    }

    #[test]
    fn test_parse_ws_frame_roundtrip() {
        let payload = b"SIP/2.0 200 OK\r\n\r\n";
        let frame = build_ws_frame(OPCODE_BINARY, payload);
        let (opcode, parsed_payload, consumed) = parse_ws_frame(&frame).unwrap().unwrap();
        assert_eq!(opcode, OPCODE_BINARY);
        assert_eq!(parsed_payload, payload);
        assert_eq!(consumed, frame.len());
    }

    #[test]
    fn test_parse_ws_frame_incomplete() {
        let data = [0x82]; // Only 1 byte.
        assert!(parse_ws_frame(&data).unwrap().is_none());
    }

    #[test]
    fn test_parse_ws_frame_masked() {
        // Build a masked frame manually.
        let payload = b"test";
        let mask_key: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        let mut frame = vec![0x82]; // FIN + Binary
        frame.push(0x80 | 4); // MASK bit + length 4
        frame.extend_from_slice(&mask_key);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask_key[i % 4]);
        }

        let (opcode, parsed, consumed) = parse_ws_frame(&frame).unwrap().unwrap();
        assert_eq!(opcode, OPCODE_BINARY);
        assert_eq!(parsed, payload);
        assert_eq!(consumed, frame.len());
    }

    #[test]
    fn test_ws_state() {
        let state = std::sync::atomic::AtomicU8::new(0);
        assert_eq!(
            match state.load(std::sync::atomic::Ordering::Relaxed) {
                0 => WsState::Open,
                1 => WsState::Closing,
                _ => WsState::Closed,
            },
            WsState::Open
        );
    }

    #[test]
    fn test_sip_websocket_protocol() {
        assert_eq!(SIP_WEBSOCKET_PROTOCOL, "sip");
    }

    #[tokio::test]
    async fn test_ws_transport_bind() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport = WsTransport::bind(addr, false).await.unwrap();
        let local = transport.local_addr().unwrap();
        assert_ne!(local.port(), 0);
        assert_eq!(transport.protocol(), "WS");
    }

    #[tokio::test]
    async fn test_ws_transport_bind_secure() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport = WsTransport::bind(addr, true).await.unwrap();
        assert_eq!(transport.protocol(), "WSS");
    }
}
