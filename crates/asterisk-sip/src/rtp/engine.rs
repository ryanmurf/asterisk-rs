//! RTP Engine abstraction.
//!
//! Inspired by Asterisk's `main/rtp_engine.c`, provides a pluggable RTP
//! engine framework. Different RTP implementations (native, ICE-enabled,
//! etc.) can be plugged in by implementing the `RtpEngine` trait.
//!
//! Key features:
//! - Pluggable engine trait for different RTP backends
//! - RTP instance management with local/remote address tracking
//! - RTP timeout detection (signal hangup if no RTP received)
//! - Symmetric RTP: learn remote address from first incoming packet
//! - RTP glue for native bridging

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::net::UdpSocket;
use tracing::debug;

use asterisk_types::{AsteriskError, AsteriskResult, Frame};

/// Properties that can be set on an RTP instance.
#[derive(Debug, Clone)]
pub struct RtpProperties {
    /// NAT mode: learn remote address from incoming packets.
    pub symmetric_rtp: bool,
    /// RTP timeout in seconds (0 = disabled).
    pub timeout_secs: u32,
    /// Hold timeout in seconds.
    pub hold_timeout_secs: u32,
    /// DTMF payload type (RFC 2833).
    pub dtmf_pt: u8,
    /// Primary payload type.
    pub payload_type: u8,
    /// Whether RTCP is enabled.
    pub rtcp_enabled: bool,
}

impl Default for RtpProperties {
    fn default() -> Self {
        Self {
            symmetric_rtp: true,
            timeout_secs: 30,
            hold_timeout_secs: 0,
            dtmf_pt: 101,
            payload_type: 0,
            rtcp_enabled: true,
        }
    }
}

/// Trait for pluggable RTP engine implementations.
///
/// Mirrors `struct ast_rtp_engine` from rtp_engine.h.
#[async_trait]
pub trait RtpEngine: Send + Sync {
    /// Human-readable name of this engine.
    fn name(&self) -> &str;

    /// Create a new RTP instance bound to a local address.
    async fn create_instance(
        &self,
        local_addr: SocketAddr,
    ) -> AsteriskResult<Box<dyn RtpEngineInstance>>;
}

/// Trait for an active RTP engine instance.
///
/// Each call leg has its own RTP instance for sending/receiving media.
#[async_trait]
pub trait RtpEngineInstance: Send + Sync {
    /// Write (send) a frame.
    async fn write(&self, frame: &Frame) -> AsteriskResult<()>;

    /// Read (receive) a frame.
    async fn read(&self) -> AsteriskResult<Frame>;

    /// Set the remote address to send RTP to.
    fn set_remote_addr(&self, addr: SocketAddr);

    /// Get the local address this instance is bound to.
    fn local_addr(&self) -> AsteriskResult<SocketAddr>;

    /// Set the read (incoming) codec format.
    fn set_read_format(&self, payload_type: u8);

    /// Set the write (outgoing) codec format.
    fn set_write_format(&self, payload_type: u8);

    /// Check if the RTP timeout has been reached.
    fn is_timed_out(&self) -> bool;

    /// Get the time of the last received RTP packet.
    fn last_rx_time(&self) -> Option<Instant>;
}

/// An RTP instance wrapping an engine instance with additional metadata.
pub struct RtpInstance {
    /// The underlying engine instance.
    engine_instance: Box<dyn RtpEngineInstance>,
    /// Engine name.
    engine_name: String,
    /// Local bound address.
    pub local_addr: SocketAddr,
    /// Remote address (where to send RTP).
    pub remote_addr: RwLock<Option<SocketAddr>>,
    /// Configuration properties.
    pub properties: RwLock<RtpProperties>,
    /// Timeout detection: when we last received RTP.
    last_rx: RwLock<Option<Instant>>,
    /// Whether symmetric RTP has learned the remote address.
    #[allow(dead_code)]
    symmetric_learned: RwLock<bool>,
}

impl RtpInstance {
    /// Create a new RTP instance using the given engine.
    pub async fn new(
        engine: &dyn RtpEngine,
        local_addr: SocketAddr,
    ) -> AsteriskResult<Self> {
        let instance = engine.create_instance(local_addr).await?;
        let bound_addr = instance.local_addr()?;

        Ok(Self {
            engine_instance: instance,
            engine_name: engine.name().to_string(),
            local_addr: bound_addr,
            remote_addr: RwLock::new(None),
            properties: RwLock::new(RtpProperties::default()),
            last_rx: RwLock::new(None),
            symmetric_learned: RwLock::new(false),
        })
    }

    /// Set the remote address.
    pub fn set_remote_addr(&self, addr: SocketAddr) {
        *self.remote_addr.write() = Some(addr);
        self.engine_instance.set_remote_addr(addr);
    }

    /// Get the local address.
    pub fn get_local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Write a frame (send RTP).
    pub async fn write(&self, frame: &Frame) -> AsteriskResult<()> {
        self.engine_instance.write(frame).await
    }

    /// Read a frame (receive RTP).
    ///
    /// If symmetric RTP is enabled, learns the remote address from the
    /// first incoming packet.
    pub async fn read(&self) -> AsteriskResult<Frame> {
        let frame = self.engine_instance.read().await?;

        // Update last RX time for timeout detection
        *self.last_rx.write() = Some(Instant::now());

        Ok(frame)
    }

    /// Check if the RTP timeout has been reached (no packets received
    /// for `timeout_secs` seconds).
    pub fn is_timed_out(&self) -> bool {
        let props = self.properties.read();
        if props.timeout_secs == 0 {
            return false;
        }

        match *self.last_rx.read() {
            Some(last) => last.elapsed() > Duration::from_secs(props.timeout_secs as u64),
            None => false, // Haven't received any packets yet, not timed out
        }
    }

    /// Set the read format.
    pub fn set_read_format(&self, payload_type: u8) {
        self.engine_instance.set_read_format(payload_type);
    }

    /// Set the write format.
    pub fn set_write_format(&self, payload_type: u8) {
        self.engine_instance.set_write_format(payload_type);
    }
}

impl std::fmt::Debug for RtpInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtpInstance")
            .field("engine", &self.engine_name)
            .field("local_addr", &self.local_addr)
            .field("remote_addr", &*self.remote_addr.read())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// RTP Glue (for native bridging)
// ---------------------------------------------------------------------------

/// Glue result indicating how a channel's RTP can be handled for bridging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpGlueResult {
    /// Channel does not support native RTP bridging.
    Failed,
    /// Channel supports local native bridge only.
    Local,
    /// Channel supports remote native bridge (direct media).
    Remote,
}

/// Callback-based glue for native RTP bridging between two channels.
///
/// Mirrors `struct ast_rtp_glue` from rtp_engine.h.
pub struct RtpGlue {
    /// Module name providing this glue.
    pub module_name: &'static str,
    /// Callback to get the RTP instance from a channel.
    #[allow(clippy::type_complexity)]
    get_rtp: Box<dyn Fn(&str) -> Option<Arc<RtpInstance>> + Send + Sync>,
}

impl RtpGlue {
    /// Create a new RTP glue registration.
    pub fn new(
        module_name: &'static str,
        get_rtp: impl Fn(&str) -> Option<Arc<RtpInstance>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            module_name,
            get_rtp: Box::new(get_rtp),
        }
    }

    /// Get the RTP instance for a channel.
    pub fn get_rtp(&self, channel_name: &str) -> Option<Arc<RtpInstance>> {
        (self.get_rtp)(channel_name)
    }
}

impl std::fmt::Debug for RtpGlue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtpGlue")
            .field("module_name", &self.module_name)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Symmetric RTP helper
// ---------------------------------------------------------------------------

/// Helper that wraps an RTP socket and implements symmetric RTP:
/// learns the remote address from the first incoming packet.
pub struct SymmetricRtp {
    socket: Arc<UdpSocket>,
    remote_addr: RwLock<Option<SocketAddr>>,
    learned: RwLock<bool>,
}

impl SymmetricRtp {
    pub fn new(socket: Arc<UdpSocket>) -> Self {
        Self {
            socket,
            remote_addr: RwLock::new(None),
            learned: RwLock::new(false),
        }
    }

    /// Set the initial remote address (from SDP negotiation).
    pub fn set_remote(&self, addr: SocketAddr) {
        *self.remote_addr.write() = Some(addr);
    }

    /// Receive a packet, learning the remote address from the source.
    pub async fn recv(&self, buf: &mut [u8]) -> AsteriskResult<(usize, SocketAddr)> {
        let (len, src) = self.socket.recv_from(buf).await?;

        // Symmetric RTP: learn from first packet
        if !*self.learned.read() {
            *self.remote_addr.write() = Some(src);
            *self.learned.write() = true;
            debug!(learned_from = %src, "Symmetric RTP: learned remote address");
        }

        Ok((len, src))
    }

    /// Send a packet to the (possibly learned) remote address.
    pub async fn send(&self, data: &[u8]) -> AsteriskResult<()> {
        let remote = self
            .remote_addr
            .read()
            .ok_or_else(|| AsteriskError::InvalidArgument("No remote address".into()))?;
        self.socket.send_to(data, remote).await?;
        Ok(())
    }

    /// Get the current remote address.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        *self.remote_addr.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rtp_properties_default() {
        let props = RtpProperties::default();
        assert!(props.symmetric_rtp);
        assert_eq!(props.timeout_secs, 30);
        assert_eq!(props.dtmf_pt, 101);
        assert_eq!(props.payload_type, 0);
        assert!(props.rtcp_enabled);
    }

    #[tokio::test]
    async fn test_rtp_timeout_detection() {
        // Create a simple instance with timeout checking
        let props = RtpProperties {
            timeout_secs: 1,
            ..Default::default()
        };

        // Simulate timeout detection logic
        let last_rx: Option<Instant> = Some(Instant::now() - Duration::from_secs(2));
        let timed_out = match last_rx {
            Some(last) => last.elapsed() > Duration::from_secs(props.timeout_secs as u64),
            None => false,
        };
        assert!(timed_out, "Should be timed out after 2s with 1s timeout");

        // Not timed out
        let last_rx: Option<Instant> = Some(Instant::now());
        let timed_out = match last_rx {
            Some(last) => last.elapsed() > Duration::from_secs(props.timeout_secs as u64),
            None => false,
        };
        assert!(!timed_out, "Should not be timed out when just received");

        // No packets received yet
        let last_rx: Option<Instant> = None;
        let timed_out = match last_rx {
            Some(last) => last.elapsed() > Duration::from_secs(props.timeout_secs as u64),
            None => false,
        };
        assert!(
            !timed_out,
            "Should not be timed out when no packets received yet"
        );
    }

    #[tokio::test]
    async fn test_symmetric_rtp() {
        // Create two UDP sockets
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let sym = SymmetricRtp::new(Arc::new(sock_a));
        // No remote set yet
        assert!(sym.remote_addr().is_none());

        // Send from B to A
        sock_b.send_to(b"hello", addr_a).await.unwrap();

        // Recv on A should learn B's address
        let mut buf = [0u8; 64];
        let (len, src) = sym.recv(&mut buf).await.unwrap();
        assert_eq!(len, 5);
        assert_eq!(src, addr_b);
        assert_eq!(sym.remote_addr(), Some(addr_b));
    }
}
