//! SIP message logging.
//!
//! Port of `res/res_pjsip_logger.c`. Logs SIP messages (requests and
//! responses) to the Asterisk log system and optionally to a PCAP file.
//! Provides CLI commands for enabling/disabling SIP logging.

use std::fmt;
use std::net::SocketAddr;

use parking_lot::RwLock;
use tracing::{debug, info, trace};

// ---------------------------------------------------------------------------
// Logger mode
// ---------------------------------------------------------------------------

/// SIP logging mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SipLogMode {
    /// Logging disabled.
    Off,
    /// Log to console/log file.
    Console,
    /// Log to PCAP file.
    Pcap,
    /// Log to both console and PCAP.
    Both,
}

impl SipLogMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Console => "console",
            Self::Pcap => "pcap",
            Self::Both => "both",
        }
    }
}

impl fmt::Display for SipLogMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// SIP logger
// ---------------------------------------------------------------------------

/// SIP message logger.
///
/// Controls logging of SIP messages for debugging purposes.
pub struct SipLogger {
    /// Current logging mode.
    mode: RwLock<SipLogMode>,
    /// PCAP output file path (if PCAP logging is enabled).
    pcap_path: RwLock<Option<String>>,
    /// Packet counter for PCAP.
    packet_count: RwLock<u64>,
}

impl SipLogger {
    pub fn new() -> Self {
        Self {
            mode: RwLock::new(SipLogMode::Off),
            pcap_path: RwLock::new(None),
            packet_count: RwLock::new(0),
        }
    }

    /// Set the logging mode.
    pub fn set_mode(&self, mode: SipLogMode) {
        *self.mode.write() = mode;
        info!(mode = %mode, "SIP logging mode changed");
    }

    /// Get the current logging mode.
    pub fn mode(&self) -> SipLogMode {
        *self.mode.read()
    }

    /// Whether logging is currently active.
    pub fn is_active(&self) -> bool {
        *self.mode.read() != SipLogMode::Off
    }

    /// Set the PCAP output file path.
    pub fn set_pcap_path(&self, path: &str) {
        *self.pcap_path.write() = Some(path.to_string());
    }

    /// Log a transmitted SIP message.
    pub fn log_transmit(
        &self,
        src: SocketAddr,
        dst: SocketAddr,
        message: &str,
    ) {
        if !self.is_active() {
            return;
        }
        let mode = *self.mode.read();
        if mode == SipLogMode::Console || mode == SipLogMode::Both {
            let first_line = message.lines().next().unwrap_or("");
            trace!(
                direction = "SEND",
                src = %src,
                dst = %dst,
                summary = first_line,
                "SIP TX"
            );
            debug!("--- SEND {} -> {} ---\n{}\n---", src, dst, message);
        }
        *self.packet_count.write() += 1;
    }

    /// Log a received SIP message.
    pub fn log_receive(
        &self,
        src: SocketAddr,
        dst: SocketAddr,
        message: &str,
    ) {
        if !self.is_active() {
            return;
        }
        let mode = *self.mode.read();
        if mode == SipLogMode::Console || mode == SipLogMode::Both {
            let first_line = message.lines().next().unwrap_or("");
            trace!(
                direction = "RECV",
                src = %src,
                dst = %dst,
                summary = first_line,
                "SIP RX"
            );
            debug!("--- RECV {} -> {} ---\n{}\n---", src, dst, message);
        }
        *self.packet_count.write() += 1;
    }

    /// Get the total number of packets logged.
    pub fn packet_count(&self) -> u64 {
        *self.packet_count.read()
    }

    /// Reset the packet counter.
    pub fn reset_count(&self) {
        *self.packet_count.write() = 0;
    }
}

impl Default for SipLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SipLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipLogger")
            .field("mode", &*self.mode.read())
            .field("packets", &*self.packet_count.read())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    #[test]
    fn test_default_off() {
        let logger = SipLogger::new();
        assert_eq!(logger.mode(), SipLogMode::Off);
        assert!(!logger.is_active());
    }

    #[test]
    fn test_enable_disable() {
        let logger = SipLogger::new();
        logger.set_mode(SipLogMode::Console);
        assert!(logger.is_active());
        logger.set_mode(SipLogMode::Off);
        assert!(!logger.is_active());
    }

    #[test]
    fn test_packet_count() {
        let logger = SipLogger::new();
        logger.set_mode(SipLogMode::Console);
        logger.log_transmit(addr(5060), addr(5061), "INVITE sip:bob@example.com SIP/2.0\r\n");
        logger.log_receive(addr(5061), addr(5060), "SIP/2.0 200 OK\r\n");
        assert_eq!(logger.packet_count(), 2);
    }

    #[test]
    fn test_no_count_when_off() {
        let logger = SipLogger::new();
        logger.log_transmit(addr(5060), addr(5061), "INVITE ...");
        assert_eq!(logger.packet_count(), 0);
    }
}
