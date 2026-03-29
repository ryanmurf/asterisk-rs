//! SIP History-Info header support.
//!
//! Port of `res/res_pjsip_history.c`. Provides a SIP message history
//! facility for debugging/logging. Captures transmitted and received
//! SIP messages with timestamps and addressing for CLI inspection
//! and PCAP-style export.

use std::collections::VecDeque;
use std::fmt;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use tracing::debug;

// ---------------------------------------------------------------------------
// History entry
// ---------------------------------------------------------------------------

/// A single SIP message history entry.
///
/// Mirrors `struct pjsip_history_entry` from the C source.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Packet number (sequential).
    pub number: u64,
    /// Whether we transmitted this packet (true) or received it (false).
    pub transmitted: bool,
    /// Timestamp in microseconds since epoch.
    pub timestamp_us: u64,
    /// Source address.
    pub src: SocketAddr,
    /// Destination address.
    pub dst: SocketAddr,
    /// SIP method or response code summary line.
    pub summary: String,
    /// Full SIP message text.
    pub message: String,
}

impl HistoryEntry {
    /// Format as a one-line summary for CLI display.
    pub fn summary_line(&self) -> String {
        let direction = if self.transmitted { ">>>" } else { "<<<" };
        format!(
            "{:>6} {} {} {} {} {}",
            self.number,
            direction,
            self.src,
            if self.transmitted { "->" } else { "<-" },
            self.dst,
            self.summary,
        )
    }
}

impl fmt::Display for HistoryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary_line())
    }
}

// ---------------------------------------------------------------------------
// History store
// ---------------------------------------------------------------------------

/// Default maximum number of entries to keep.
pub const DEFAULT_HISTORY_SIZE: usize = 256;

/// SIP message history store.
///
/// Keeps a bounded ring buffer of recent SIP messages for debugging.
pub struct SipHistory {
    /// Whether history capture is enabled.
    enabled: RwLock<bool>,
    /// Packet counter.
    counter: RwLock<u64>,
    /// History entries (bounded deque).
    entries: RwLock<VecDeque<HistoryEntry>>,
    /// Maximum number of entries.
    max_entries: usize,
}

impl SipHistory {
    pub fn new() -> Self {
        Self {
            enabled: RwLock::new(false),
            counter: RwLock::new(0),
            entries: RwLock::new(VecDeque::with_capacity(DEFAULT_HISTORY_SIZE)),
            max_entries: DEFAULT_HISTORY_SIZE,
        }
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            enabled: RwLock::new(false),
            counter: RwLock::new(0),
            entries: RwLock::new(VecDeque::with_capacity(max_entries)),
            max_entries,
        }
    }

    /// Enable or disable history capture.
    pub fn set_enabled(&self, enabled: bool) {
        *self.enabled.write() = enabled;
        debug!(enabled = enabled, "SIP history capture");
    }

    /// Whether history capture is enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Record a SIP message.
    pub fn record(
        &self,
        transmitted: bool,
        src: SocketAddr,
        dst: SocketAddr,
        summary: &str,
        message: &str,
    ) {
        if !self.is_enabled() {
            return;
        }

        let mut counter = self.counter.write();
        *counter += 1;
        let number = *counter;

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let entry = HistoryEntry {
            number,
            transmitted,
            timestamp_us: ts,
            src,
            dst,
            summary: summary.to_string(),
            message: message.to_string(),
        };

        let mut entries = self.entries.write();
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Get all history entries.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        self.entries.read().iter().cloned().collect()
    }

    /// Clear the history.
    pub fn clear(&self) {
        self.entries.write().clear();
        *self.counter.write() = 0;
        debug!("SIP history cleared");
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

impl Default for SipHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SipHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipHistory")
            .field("enabled", &*self.enabled.read())
            .field("entries", &self.entries.read().len())
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
    fn test_disabled_by_default() {
        let hist = SipHistory::new();
        assert!(!hist.is_enabled());
        hist.record(true, addr(5060), addr(5061), "INVITE", "...");
        assert!(hist.is_empty());
    }

    #[test]
    fn test_record_and_retrieve() {
        let hist = SipHistory::new();
        hist.set_enabled(true);
        hist.record(true, addr(5060), addr(5061), "INVITE sip:bob@example.com", "full msg");
        hist.record(false, addr(5061), addr(5060), "200 OK", "full response");

        assert_eq!(hist.len(), 2);
        let entries = hist.entries();
        assert!(entries[0].transmitted);
        assert!(!entries[1].transmitted);
        assert_eq!(entries[0].number, 1);
        assert_eq!(entries[1].number, 2);
    }

    #[test]
    fn test_capacity_limit() {
        let hist = SipHistory::with_capacity(2);
        hist.set_enabled(true);
        hist.record(true, addr(5060), addr(5061), "INVITE", "1");
        hist.record(true, addr(5060), addr(5061), "ACK", "2");
        hist.record(true, addr(5060), addr(5061), "BYE", "3");

        assert_eq!(hist.len(), 2);
        let entries = hist.entries();
        assert_eq!(entries[0].summary, "ACK");
        assert_eq!(entries[1].summary, "BYE");
    }

    #[test]
    fn test_clear() {
        let hist = SipHistory::new();
        hist.set_enabled(true);
        hist.record(true, addr(5060), addr(5061), "INVITE", "msg");
        hist.clear();
        assert!(hist.is_empty());
    }
}
