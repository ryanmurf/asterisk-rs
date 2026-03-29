//! SIP transport layer.
//!
//! Provides UDP, TCP, TLS, and WebSocket transports for sending/receiving
//! SIP messages. Supports both IPv4 and IPv6 binding and dual-stack operation.

pub mod tls;
pub mod websocket;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::parser::SipMessage;

/// Trait for SIP transports.
#[async_trait]
pub trait SipTransport: Send + Sync + std::fmt::Debug {
    /// Send a SIP message to the given address.
    async fn send(&self, msg: &SipMessage, addr: SocketAddr) -> Result<(), TransportError>;

    /// Get the local address this transport is bound to.
    fn local_addr(&self) -> Result<SocketAddr, TransportError>;

    /// Get the transport protocol name.
    fn protocol(&self) -> &str;
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Connection error: {0}")]
    Connection(String),
}

// ---------------------------------------------------------------------------
// IPv6 utilities
// ---------------------------------------------------------------------------

/// Check if an address string represents an IPv6 address.
pub fn is_ipv6(addr: &str) -> bool {
    // Strip brackets if present (e.g. "[::1]")
    let addr = addr.trim_start_matches('[').trim_end_matches(']');
    addr.parse::<Ipv6Addr>().is_ok()
}

/// Format an IP address for use in SIP headers (brackets for IPv6).
pub fn format_host_for_sip(addr: &IpAddr) -> String {
    match addr {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{}]", v6),
    }
}

/// Format a SocketAddr for use in SIP headers (brackets for IPv6).
pub fn format_addr_for_sip(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => format!("{}:{}", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("[{}]:{}", v6.ip(), v6.port()),
    }
}

/// Build a Via header value with correct formatting for the address family.
pub fn build_via_value(protocol: &str, addr: &SocketAddr, branch: &str) -> String {
    format!(
        "SIP/2.0/{} {};branch={}",
        protocol,
        format_addr_for_sip(addr),
        branch
    )
}

/// Determine the address type string for SDP (`IP4` or `IP6`).
pub fn sdp_addr_type(addr: &IpAddr) -> &'static str {
    match addr {
        IpAddr::V4(_) => "IP4",
        IpAddr::V6(_) => "IP6",
    }
}

/// Try to resolve an address as IPv6 first, then fall back to IPv4.
///
/// Returns the preferred SocketAddr for dual-stack operation.
pub fn prefer_ipv6(addrs: &[SocketAddr]) -> Option<SocketAddr> {
    // Prefer IPv6
    addrs
        .iter()
        .find(|a| a.is_ipv6())
        .or_else(|| addrs.first())
        .copied()
}

// ---------------------------------------------------------------------------
// UDP transport
// ---------------------------------------------------------------------------

/// UDP-based SIP transport.
#[derive(Debug)]
pub struct UdpTransport {
    socket: Arc<UdpSocket>,
}

impl UdpTransport {
    /// Bind a UDP transport to the given address (IPv4 or IPv6).
    pub async fn bind(addr: SocketAddr) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr).await?;
        let local = socket.local_addr()?;
        let family = if local.is_ipv6() { "IPv6" } else { "IPv4" };
        info!(addr = %local, family, "UDP SIP transport bound");
        Ok(Self {
            socket: Arc::new(socket),
        })
    }

    /// Bind a UDP transport to an IPv6 address.
    pub async fn bind_ipv6(port: u16) -> Result<Self, TransportError> {
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
        Self::bind(addr).await
    }

    /// Bind a UDP transport to an IPv4 address.
    pub async fn bind_ipv4(port: u16) -> Result<Self, TransportError> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        Self::bind(addr).await
    }

    /// Receive a SIP message. Returns the parsed message and source address.
    pub async fn recv(&self) -> Result<(SipMessage, SocketAddr), TransportError> {
        let mut buf = vec![0u8; 65535];
        let (len, src) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        let msg = SipMessage::parse(&buf).map_err(|e| TransportError::Parse(e.0))?;
        debug!(src = %src, "Received SIP message via UDP");
        Ok((msg, src))
    }

    /// Get a reference to the underlying socket for use with select.
    pub fn socket(&self) -> &UdpSocket {
        &self.socket
    }
}

#[async_trait]
impl SipTransport for UdpTransport {
    async fn send(&self, msg: &SipMessage, addr: SocketAddr) -> Result<(), TransportError> {
        let data = msg.to_string();
        self.socket.send_to(data.as_bytes(), addr).await?;
        debug!(dest = %addr, "Sent SIP message via UDP");
        Ok(())
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.socket.local_addr()?)
    }

    fn protocol(&self) -> &str {
        "UDP"
    }
}

// ---------------------------------------------------------------------------
// TCP transport with connection reuse
// ---------------------------------------------------------------------------

/// An identifier for a TCP connection, used for connection reuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId {
    /// Remote address of the connection.
    pub remote_addr: SocketAddr,
    /// Via alias for RFC 5923 connection reuse (optional).
    pub alias: Option<String>,
}

/// Metadata for a tracked connection.
#[derive(Debug)]
struct TrackedConnection {
    stream: Arc<Mutex<TcpStream>>,
    /// Registration binding associated with this connection (RFC 5626).
    registration_id: Option<String>,
    /// Via alias for connection identification (RFC 5923).
    alias: Option<String>,
}

/// TCP-based SIP transport.
///
/// Manages TCP connections for SIP message exchange. Supports connection
/// reuse (persistent connections) per RFC 3261, Via alias per RFC 5923,
/// and connection-oriented registration per RFC 5626.
#[derive(Debug)]
pub struct TcpTransport {
    listener: TcpListener,
    /// Active connections keyed by remote address.
    connections: Mutex<HashMap<SocketAddr, TrackedConnection>>,
}

impl TcpTransport {
    /// Bind a TCP transport listener (IPv4 or IPv6).
    pub async fn bind(addr: SocketAddr) -> Result<Self, TransportError> {
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let family = if local.is_ipv6() { "IPv6" } else { "IPv4" };
        info!(addr = %local, family, "TCP SIP transport bound");
        Ok(Self {
            listener,
            connections: Mutex::new(HashMap::new()),
        })
    }

    /// Bind a TCP transport to an IPv6 address.
    pub async fn bind_ipv6(port: u16) -> Result<Self, TransportError> {
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
        Self::bind(addr).await
    }

    /// Accept an incoming connection and read a SIP message.
    pub async fn accept(
        &self,
    ) -> Result<(SipMessage, SocketAddr, Arc<Mutex<TcpStream>>), TransportError> {
        let (stream, addr) = self.listener.accept().await?;
        let stream = Arc::new(Mutex::new(stream));

        let msg = Self::read_from_stream(&stream).await?;

        // Store the connection for reuse.
        self.connections.lock().await.insert(
            addr,
            TrackedConnection {
                stream: Arc::clone(&stream),
                registration_id: None,
                alias: None,
            },
        );

        Ok((msg, addr, stream))
    }

    /// Send raw bytes over a TCP connection (e.g. CRLF keep-alive).
    pub async fn send_raw(
        &self,
        addr: SocketAddr,
        data: &[u8],
    ) -> Result<(), TransportError> {
        let stream = self.get_connection(addr).await?;
        let mut s = stream.lock().await;
        s.write_all(data).await?;
        Ok(())
    }

    /// Send a CRLF keep-alive (`\r\n\r\n`) on a TCP connection.
    ///
    /// Per RFC 5626, this keeps NAT bindings alive for TCP/TLS connections.
    pub async fn send_crlf_keepalive(&self, addr: SocketAddr) -> Result<(), TransportError> {
        self.send_raw(addr, b"\r\n\r\n").await
    }

    /// Bind a connection to a registration (for RFC 5626 flow tracking).
    pub async fn bind_registration(
        &self,
        addr: SocketAddr,
        registration_id: String,
    ) {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.get_mut(&addr) {
            conn.registration_id = Some(registration_id);
        }
    }

    /// Set a Via alias for a connection (RFC 5923).
    pub async fn set_alias(&self, addr: SocketAddr, alias: String) {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.get_mut(&addr) {
            conn.alias = Some(alias);
        }
    }

    /// Find a connection by its Via alias.
    pub async fn find_by_alias(
        &self,
        alias: &str,
    ) -> Option<(SocketAddr, Arc<Mutex<TcpStream>>)> {
        let conns = self.connections.lock().await;
        for (addr, conn) in conns.iter() {
            if conn.alias.as_deref() == Some(alias) {
                return Some((*addr, Arc::clone(&conn.stream)));
            }
        }
        None
    }

    /// Find a connection by registration ID (RFC 5626).
    pub async fn find_by_registration(
        &self,
        registration_id: &str,
    ) -> Option<(SocketAddr, Arc<Mutex<TcpStream>>)> {
        let conns = self.connections.lock().await;
        for (addr, conn) in conns.iter() {
            if conn.registration_id.as_deref() == Some(registration_id) {
                return Some((*addr, Arc::clone(&conn.stream)));
            }
        }
        None
    }

    /// Remove a dead connection.
    pub async fn remove_connection(&self, addr: &SocketAddr) {
        self.connections.lock().await.remove(addr);
    }

    /// Check if a connection to the given address exists.
    pub async fn has_connection(&self, addr: &SocketAddr) -> bool {
        self.connections.lock().await.contains_key(addr)
    }

    /// Read a SIP message from a TCP stream.
    ///
    /// Uses Content-Length header to determine body size. Reads headers
    /// line-by-line until the blank line, then reads the body.
    async fn read_from_stream(
        stream: &Arc<Mutex<TcpStream>>,
    ) -> Result<SipMessage, TransportError> {
        let mut stream = stream.lock().await;
        let mut buf = Vec::with_capacity(4096);
        let mut temp = [0u8; 4096];

        // Read until we find the header/body separator
        loop {
            let n = stream.read(&mut temp).await?;
            if n == 0 {
                return Err(TransportError::Connection("Connection closed".into()));
            }
            buf.extend_from_slice(&temp[..n]);

            // Check for header/body separator
            if let Some(sep_pos) = find_header_end(&buf) {
                // Parse headers to get Content-Length
                let header_text = std::str::from_utf8(&buf[..sep_pos])
                    .map_err(|e| TransportError::Parse(e.to_string()))?;

                let content_length = extract_content_length(header_text);
                let body_start = sep_pos + 4; // Skip \r\n\r\n
                let total_needed = body_start + content_length;

                // Read remaining body if needed
                while buf.len() < total_needed {
                    let n = stream.read(&mut temp).await?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&temp[..n]);
                }

                buf.truncate(total_needed);
                break;
            }

            if buf.len() > 65535 {
                return Err(TransportError::Parse("SIP message too large".into()));
            }
        }

        SipMessage::parse(&buf).map_err(|e| TransportError::Parse(e.0))
    }

    /// Get or create a connection to the given address.
    async fn get_connection(
        &self,
        addr: SocketAddr,
    ) -> Result<Arc<Mutex<TcpStream>>, TransportError> {
        // Check for existing connection
        {
            let conns = self.connections.lock().await;
            if let Some(conn) = conns.get(&addr) {
                return Ok(Arc::clone(&conn.stream));
            }
        }

        // Create new connection
        let stream = TcpStream::connect(addr).await?;
        let stream = Arc::new(Mutex::new(stream));
        self.connections.lock().await.insert(
            addr,
            TrackedConnection {
                stream: Arc::clone(&stream),
                registration_id: None,
                alias: None,
            },
        );

        Ok(stream)
    }
}

#[async_trait]
impl SipTransport for TcpTransport {
    async fn send(&self, msg: &SipMessage, addr: SocketAddr) -> Result<(), TransportError> {
        let stream = self.get_connection(addr).await?;
        let data = msg.to_string();
        let mut s = stream.lock().await;
        s.write_all(data.as_bytes()).await?;
        debug!(dest = %addr, "Sent SIP message via TCP");
        Ok(())
    }

    fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.listener.local_addr()?)
    }

    fn protocol(&self) -> &str {
        "TCP"
    }
}

/// Find the end of headers (\r\n\r\n) in a buffer.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Extract Content-Length from raw header text.
fn extract_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") || lower.starts_with("l:") {
            if let Some(val) = line.split_once(':') {
                if let Ok(len) = val.1.trim().parse::<usize>() {
                    return len;
                }
            }
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ipv6() {
        assert!(is_ipv6("::1"));
        assert!(is_ipv6("[::1]"));
        assert!(is_ipv6("2001:db8::1"));
        assert!(is_ipv6("[2001:db8::1]"));
        assert!(is_ipv6("::"));
        assert!(!is_ipv6("10.0.0.1"));
        assert!(!is_ipv6("example.com"));
    }

    #[test]
    fn test_format_host_for_sip() {
        let v4: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(format_host_for_sip(&v4), "10.0.0.1");

        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(format_host_for_sip(&v6), "[2001:db8::1]");
    }

    #[test]
    fn test_format_addr_for_sip() {
        let v4: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        assert_eq!(format_addr_for_sip(&v4), "10.0.0.1:5060");

        let v6: SocketAddr = "[2001:db8::1]:5060".parse().unwrap();
        assert_eq!(format_addr_for_sip(&v6), "[2001:db8::1]:5060");
    }

    #[test]
    fn test_build_via_value_ipv4() {
        let addr: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let via = build_via_value("UDP", &addr, "z9hG4bK123");
        assert_eq!(via, "SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bK123");
    }

    #[test]
    fn test_build_via_value_ipv6() {
        let addr: SocketAddr = "[2001:db8::1]:5060".parse().unwrap();
        let via = build_via_value("UDP", &addr, "z9hG4bK456");
        assert_eq!(
            via,
            "SIP/2.0/UDP [2001:db8::1]:5060;branch=z9hG4bK456"
        );
    }

    #[test]
    fn test_sdp_addr_type() {
        let v4: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(sdp_addr_type(&v4), "IP4");

        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(sdp_addr_type(&v6), "IP6");
    }

    #[test]
    fn test_prefer_ipv6() {
        let addrs: Vec<SocketAddr> = vec![
            "10.0.0.1:5060".parse().unwrap(),
            "[2001:db8::1]:5060".parse().unwrap(),
        ];
        let preferred = prefer_ipv6(&addrs).unwrap();
        assert!(preferred.is_ipv6());

        let v4_only: Vec<SocketAddr> = vec!["10.0.0.1:5060".parse().unwrap()];
        let preferred = prefer_ipv6(&v4_only).unwrap();
        assert!(preferred.is_ipv4());

        let empty: Vec<SocketAddr> = vec![];
        assert!(prefer_ipv6(&empty).is_none());
    }

    #[test]
    fn test_connection_id() {
        let id1 = ConnectionId {
            remote_addr: "10.0.0.1:5060".parse().unwrap(),
            alias: Some("abc123".to_string()),
        };
        let id2 = ConnectionId {
            remote_addr: "10.0.0.1:5060".parse().unwrap(),
            alias: Some("abc123".to_string()),
        };
        assert_eq!(id1, id2);
    }
}
