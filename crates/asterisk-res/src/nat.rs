//! NAT traversal helpers.
//!
//! Provides utilities for NAT detection (via STUN), symmetric RTP address
//! learning, and SIP Via `rport` processing used throughout the Asterisk
//! channel drivers and SIP stack.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum NatError {
    #[error("STUN discovery failed: {0}")]
    StunFailed(String),
    #[error("NAT detection timeout")]
    Timeout,
    #[error("NAT error: {0}")]
    Other(String),
}

pub type NatResult<T> = Result<T, NatError>;

// ---------------------------------------------------------------------------
// NAT types
// ---------------------------------------------------------------------------

/// Detected NAT type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// No NAT detected; public address.
    NoNat,
    /// Full cone NAT.
    FullCone,
    /// Restricted cone NAT.
    RestrictedCone,
    /// Port-restricted cone NAT.
    PortRestricted,
    /// Symmetric NAT.
    Symmetric,
    /// Could not determine.
    Unknown,
}

impl NatType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoNat => "No NAT",
            Self::FullCone => "Full Cone",
            Self::RestrictedCone => "Restricted Cone",
            Self::PortRestricted => "Port Restricted",
            Self::Symmetric => "Symmetric",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for NatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// NAT configuration
// ---------------------------------------------------------------------------

/// NAT traversal configuration.
#[derive(Debug, Clone)]
pub struct NatConfig {
    /// STUN server address (host:port).
    pub stun_server: Option<String>,
    /// TURN server address (host:port).
    pub turn_server: Option<String>,
    /// TURN username.
    pub turn_username: Option<String>,
    /// TURN password.
    pub turn_password: Option<String>,
    /// Whether ICE support is enabled.
    pub ice_support: bool,
    /// Whether symmetric RTP (address learning) is enabled.
    pub symmetric_rtp: bool,
    /// Whether to force rport in SIP Via headers.
    pub force_rport: bool,
    /// External media address (for static NAT configurations).
    pub external_media_address: Option<IpAddr>,
    /// External signaling address.
    pub external_signaling_address: Option<IpAddr>,
    /// Local network subnets (traffic to these is not NATed).
    pub local_nets: Vec<String>,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            stun_server: None,
            turn_server: None,
            turn_username: None,
            turn_password: None,
            ice_support: false,
            symmetric_rtp: true,
            force_rport: true,
            external_media_address: None,
            external_signaling_address: None,
            local_nets: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Symmetric RTP address learner
// ---------------------------------------------------------------------------

/// Learns the remote RTP address from the first incoming packet.
///
/// Used when `symmetric_rtp` is enabled: the initial remote address from
/// signaling is replaced by the source address of the first received RTP
/// packet, working around NAT mismatches.
pub struct SymmetricRtpLearner {
    /// The signaling-provided remote address.
    signaled_addr: RwLock<SocketAddr>,
    /// The learned (actual) remote address.
    learned_addr: RwLock<Option<SocketAddr>>,
    /// Whether learning is complete.
    learned: AtomicBool,
}

impl SymmetricRtpLearner {
    /// Create a new learner with the signaled remote address.
    pub fn new(signaled_addr: SocketAddr) -> Self {
        Self {
            signaled_addr: RwLock::new(signaled_addr),
            learned_addr: RwLock::new(None),
            learned: AtomicBool::new(false),
        }
    }

    /// Process an incoming packet's source address.
    ///
    /// If the source differs from the signaled address, updates the learned
    /// address. Returns the effective remote address to use.
    pub fn process_incoming(&self, source: SocketAddr) -> SocketAddr {
        if self.learned.load(Ordering::Relaxed) {
            return self.learned_addr.read().unwrap_or(source);
        }

        let signaled = *self.signaled_addr.read();
        if source != signaled {
            info!(
                signaled = %signaled,
                learned = %source,
                "Symmetric RTP: learned new remote address"
            );
            *self.learned_addr.write() = Some(source);
            self.learned.store(true, Ordering::Relaxed);
            source
        } else {
            self.learned.store(true, Ordering::Relaxed);
            signaled
        }
    }

    /// Get the effective remote address (learned if available, otherwise signaled).
    pub fn remote_addr(&self) -> SocketAddr {
        if let Some(learned) = *self.learned_addr.read() {
            learned
        } else {
            *self.signaled_addr.read()
        }
    }

    /// Whether a different address has been learned.
    pub fn has_learned(&self) -> bool {
        self.learned_addr.read().is_some()
    }

    /// Reset the learner (e.g., on re-INVITE).
    pub fn reset(&self, new_signaled: SocketAddr) {
        *self.signaled_addr.write() = new_signaled;
        *self.learned_addr.write() = None;
        self.learned.store(false, Ordering::Relaxed);
        debug!(signaled = %new_signaled, "Symmetric RTP learner reset");
    }
}

// ---------------------------------------------------------------------------
// Via rport processing
// ---------------------------------------------------------------------------

/// Parsed SIP Via header fields relevant to NAT processing.
#[derive(Debug, Clone)]
pub struct ViaInfo {
    /// The Via sent-by address.
    pub sent_by_host: String,
    /// The Via sent-by port.
    pub sent_by_port: u16,
    /// Whether the `rport` parameter was present in the request.
    pub rport_present: bool,
    /// The `rport` value (filled in by the server).
    pub rport_value: Option<u16>,
    /// The `received` parameter (real source IP).
    pub received: Option<String>,
}

/// Process rport for a SIP Via header.
///
/// When a request arrives, the server fills in the `received` parameter with
/// the actual source IP and the `rport` parameter with the actual source
/// port, allowing the UAC behind NAT to learn its public transport address.
pub fn process_rport(via: &mut ViaInfo, actual_source: SocketAddr) {
    let actual_ip = actual_source.ip().to_string();
    let actual_port = actual_source.port();

    // Always set `received` if the source IP differs from the Via sent-by.
    if via.sent_by_host != actual_ip {
        via.received = Some(actual_ip);
    }

    // Set `rport` if it was requested.
    if via.rport_present {
        via.rport_value = Some(actual_port);
    }
}

/// Determine the effective reply address from Via parameters.
///
/// If `received` is set, use it instead of the Via sent-by host. If `rport`
/// has a value, use it instead of the sent-by port.
pub fn via_reply_address(via: &ViaInfo) -> (String, u16) {
    let host = via
        .received
        .as_deref()
        .unwrap_or(&via.sent_by_host)
        .to_string();
    let port = via.rport_value.unwrap_or(via.sent_by_port);
    (host, port)
}

/// Quick check: is the given address an RFC 1918 private address?
pub fn is_private_address(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 10.0.0.0/8
            octets[0] == 10
            // 172.16.0.0/12
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            // 192.168.0.0/16
            || (octets[0] == 192 && octets[1] == 168)
            // 127.0.0.0/8
            || octets[0] == 127
        }
        IpAddr::V6(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_nat_type_display() {
        assert_eq!(NatType::Symmetric.as_str(), "Symmetric");
        assert_eq!(NatType::NoNat.as_str(), "No NAT");
    }

    #[test]
    fn test_symmetric_rtp_no_change() {
        let signaled: SocketAddr = "203.0.113.1:10000".parse().unwrap();
        let learner = SymmetricRtpLearner::new(signaled);

        let result = learner.process_incoming(signaled);
        assert_eq!(result, signaled);
        // Signaled matches, so no "different" learned address.
        assert!(!learner.has_learned());
    }

    #[test]
    fn test_symmetric_rtp_learns_new() {
        let signaled: SocketAddr = "203.0.113.1:10000".parse().unwrap();
        let actual: SocketAddr = "198.51.100.5:20000".parse().unwrap();
        let learner = SymmetricRtpLearner::new(signaled);

        let result = learner.process_incoming(actual);
        assert_eq!(result, actual);
        assert!(learner.has_learned());
        assert_eq!(learner.remote_addr(), actual);
    }

    #[test]
    fn test_symmetric_rtp_reset() {
        let signaled: SocketAddr = "203.0.113.1:10000".parse().unwrap();
        let actual: SocketAddr = "198.51.100.5:20000".parse().unwrap();
        let learner = SymmetricRtpLearner::new(signaled);

        learner.process_incoming(actual);
        assert!(learner.has_learned());

        let new_signaled: SocketAddr = "203.0.113.1:30000".parse().unwrap();
        learner.reset(new_signaled);
        assert!(!learner.has_learned());
        assert_eq!(learner.remote_addr(), new_signaled);
    }

    #[test]
    fn test_process_rport() {
        let mut via = ViaInfo {
            sent_by_host: "10.0.0.1".to_string(),
            sent_by_port: 5060,
            rport_present: true,
            rport_value: None,
            received: None,
        };

        let source: SocketAddr = "203.0.113.50:12345".parse().unwrap();
        process_rport(&mut via, source);

        assert_eq!(via.received, Some("203.0.113.50".to_string()));
        assert_eq!(via.rport_value, Some(12345));
    }

    #[test]
    fn test_via_reply_address() {
        let via = ViaInfo {
            sent_by_host: "10.0.0.1".to_string(),
            sent_by_port: 5060,
            rport_present: true,
            rport_value: Some(12345),
            received: Some("203.0.113.50".to_string()),
        };

        let (host, port) = via_reply_address(&via);
        assert_eq!(host, "203.0.113.50");
        assert_eq!(port, 12345);
    }

    #[test]
    fn test_is_private_address() {
        assert!(is_private_address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_address(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_private_address(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private_address(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        assert!(!is_private_address(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(!is_private_address(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
    }
}
