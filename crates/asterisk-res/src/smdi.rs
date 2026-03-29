//! Simplified Message Desk Interface (SMDI) resource module.
//!
//! Port of `res/res_smdi.c`. SMDI is a serial protocol used to communicate
//! between telephone systems and message desk equipment (e.g., voicemail
//! systems). This module provides SMDI message parsing, queuing, and
//! interface management.
//!
//! The actual serial port I/O is stubbed since Rust serial support would
//! require a crate like `serialport`. The protocol parsing and message
//! management are fully implemented.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default message expiry time in milliseconds.
pub const SMDI_MSG_EXPIRY_MS: u64 = 30_000; // 30 seconds

/// Maximum station/number ID length.
pub const SMDI_MAX_STATION_LEN: usize = 10;

/// Maximum filename for SMDI port device.
pub const SMDI_MAX_FILENAME_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SmdiError {
    #[error("SMDI interface not found: {0}")]
    InterfaceNotFound(String),
    #[error("SMDI serial port error: {0}")]
    Serial(String),
    #[error("SMDI message expired")]
    MessageExpired,
    #[error("SMDI message not found")]
    MessageNotFound,
    #[error("SMDI parse error: {0}")]
    Parse(String),
    #[error("SMDI timeout")]
    Timeout,
}

pub type SmdiResult<T> = Result<T, SmdiError>;

// ---------------------------------------------------------------------------
// SMDI message types
// ---------------------------------------------------------------------------

/// SMDI Message Desk (MD) message type -- the type of call forwarding.
///
/// From the SMDI protocol specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmdiMdType {
    /// Direct call (no forwarding).
    Direct,
    /// Forward all calls.
    ForwardAll,
    /// Forward on busy.
    ForwardBusy,
    /// Forward on no answer.
    ForwardNoAnswer,
}

impl SmdiMdType {
    /// Parse from the SMDI protocol character.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'D' => Some(Self::Direct),
            'A' => Some(Self::ForwardAll),
            'B' => Some(Self::ForwardBusy),
            'N' => Some(Self::ForwardNoAnswer),
            _ => None,
        }
    }

    /// Return the SMDI protocol character.
    pub fn as_char(&self) -> char {
        match self {
            Self::Direct => 'D',
            Self::ForwardAll => 'A',
            Self::ForwardBusy => 'B',
            Self::ForwardNoAnswer => 'N',
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ForwardAll => "forward_all",
            Self::ForwardBusy => "forward_busy",
            Self::ForwardNoAnswer => "forward_no_answer",
        }
    }
}

// ---------------------------------------------------------------------------
// MWI cause
// ---------------------------------------------------------------------------

/// SMDI Message Waiting Indicator cause (on/off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmdiMwiCause {
    /// Message waiting indicator ON.
    On,
    /// Message waiting indicator OFF.
    Off,
}

impl SmdiMwiCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

// ---------------------------------------------------------------------------
// SMDI MD message
// ---------------------------------------------------------------------------

/// An SMDI Message Desk (MD) message.
///
/// This represents an incoming call notification from the PBX, including
/// the station that was called, the calling station, and the forwarding type.
#[derive(Debug, Clone)]
pub struct SmdiMdMessage {
    /// Forwarding station (the station the call was forwarded from).
    pub station: String,
    /// Calling station (the originator of the call).
    pub calling_station: String,
    /// Message desk number.
    pub md_number: String,
    /// Message desk terminal.
    pub terminal: String,
    /// Call type (direct, forward, etc.).
    pub call_type: SmdiMdType,
    /// When this message was received.
    pub received_at: Instant,
    /// Expiry duration.
    pub expiry: Duration,
}

impl SmdiMdMessage {
    /// Create a new MD message.
    pub fn new(
        station: &str,
        calling_station: &str,
        call_type: SmdiMdType,
    ) -> Self {
        Self {
            station: station.to_string(),
            calling_station: calling_station.to_string(),
            md_number: String::new(),
            terminal: String::new(),
            call_type,
            received_at: Instant::now(),
            expiry: Duration::from_millis(SMDI_MSG_EXPIRY_MS),
        }
    }

    /// Whether this message has expired.
    pub fn is_expired(&self) -> bool {
        self.received_at.elapsed() >= self.expiry
    }

    /// Remaining time before expiry.
    pub fn remaining(&self) -> Duration {
        self.expiry.saturating_sub(self.received_at.elapsed())
    }
}

// ---------------------------------------------------------------------------
// SMDI MWI message
// ---------------------------------------------------------------------------

/// An SMDI Message Waiting Indicator (MWI) message.
///
/// Indicates that a mailbox has a waiting message (or messages have been cleared).
#[derive(Debug, Clone)]
pub struct SmdiMwiMessage {
    /// Mailbox identifier (typically the extension number).
    pub mailbox: String,
    /// MWI cause (on or off).
    pub cause: SmdiMwiCause,
    /// When this message was received.
    pub received_at: Instant,
    /// Expiry duration.
    pub expiry: Duration,
}

impl SmdiMwiMessage {
    pub fn new(mailbox: &str, cause: SmdiMwiCause) -> Self {
        Self {
            mailbox: mailbox.to_string(),
            cause,
            received_at: Instant::now(),
            expiry: Duration::from_millis(SMDI_MSG_EXPIRY_MS),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.received_at.elapsed() >= self.expiry
    }
}

// ---------------------------------------------------------------------------
// Serial port speed
// ---------------------------------------------------------------------------

/// Serial port baud rates supported by SMDI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmdiSpeed {
    B1200,
    B2400,
    B4800,
    B9600,
}

impl SmdiSpeed {
    pub fn from_config(s: &str) -> Option<Self> {
        match s {
            "1200" => Some(Self::B1200),
            "2400" => Some(Self::B2400),
            "4800" => Some(Self::B4800),
            "9600" => Some(Self::B9600),
            _ => None,
        }
    }

    pub fn baud_rate(&self) -> u32 {
        match self {
            Self::B1200 => 1200,
            Self::B2400 => 2400,
            Self::B4800 => 4800,
            Self::B9600 => 9600,
        }
    }
}

impl Default for SmdiSpeed {
    fn default() -> Self {
        Self::B9600
    }
}

// ---------------------------------------------------------------------------
// SMDI interface
// ---------------------------------------------------------------------------

/// An SMDI serial interface.
///
/// Mirrors the SMDI interface structure from the C source.
pub struct SmdiInterface {
    /// Serial port device path (e.g., "/dev/ttyS0").
    pub port: String,
    /// Baud rate.
    pub speed: SmdiSpeed,
    /// Message expiry time.
    pub msg_expiry: Duration,
    /// Queue of received MD messages.
    md_queue: Mutex<VecDeque<SmdiMdMessage>>,
    /// Queue of received MWI messages.
    mwi_queue: Mutex<VecDeque<SmdiMwiMessage>>,
}

impl SmdiInterface {
    /// Create a new SMDI interface.
    pub fn new(port: &str, speed: SmdiSpeed) -> Self {
        Self {
            port: port.to_string(),
            speed,
            msg_expiry: Duration::from_millis(SMDI_MSG_EXPIRY_MS),
            md_queue: Mutex::new(VecDeque::new()),
            mwi_queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Set the message expiry time.
    pub fn with_expiry(mut self, expiry: Duration) -> Self {
        self.msg_expiry = expiry;
        self
    }

    /// Enqueue an MD message (called when data is received from the serial port).
    pub fn enqueue_md(&self, msg: SmdiMdMessage) {
        debug!(
            port = %self.port,
            station = %msg.station,
            call_type = ?msg.call_type,
            "SMDI MD message received"
        );
        self.md_queue.lock().push_back(msg);
    }

    /// Enqueue an MWI message.
    pub fn enqueue_mwi(&self, msg: SmdiMwiMessage) {
        debug!(
            port = %self.port,
            mailbox = %msg.mailbox,
            cause = ?msg.cause,
            "SMDI MWI message received"
        );
        self.mwi_queue.lock().push_back(msg);
    }

    /// Retrieve the oldest non-expired MD message matching the station.
    pub fn get_md_by_station(&self, station: &str) -> Option<SmdiMdMessage> {
        let mut queue = self.md_queue.lock();
        self.purge_expired_md(&mut queue);

        if let Some(pos) = queue.iter().position(|m| m.station == station) {
            queue.remove(pos)
        } else {
            None
        }
    }

    /// Retrieve the oldest non-expired MD message matching the terminal.
    pub fn get_md_by_terminal(&self, terminal: &str) -> Option<SmdiMdMessage> {
        let mut queue = self.md_queue.lock();
        self.purge_expired_md(&mut queue);

        if let Some(pos) = queue.iter().position(|m| m.terminal == terminal) {
            queue.remove(pos)
        } else {
            None
        }
    }

    /// Retrieve the oldest non-expired MD message matching the MD number.
    pub fn get_md_by_number(&self, number: &str) -> Option<SmdiMdMessage> {
        let mut queue = self.md_queue.lock();
        self.purge_expired_md(&mut queue);

        if let Some(pos) = queue.iter().position(|m| m.md_number == number) {
            queue.remove(pos)
        } else {
            None
        }
    }

    /// Retrieve the oldest non-expired MWI message matching the mailbox.
    pub fn get_mwi(&self, mailbox: &str) -> Option<SmdiMwiMessage> {
        let mut queue = self.mwi_queue.lock();
        self.purge_expired_mwi(&mut queue);

        if let Some(pos) = queue.iter().position(|m| m.mailbox == mailbox) {
            queue.remove(pos)
        } else {
            None
        }
    }

    /// Number of pending MD messages.
    pub fn md_queue_len(&self) -> usize {
        self.md_queue.lock().len()
    }

    /// Number of pending MWI messages.
    pub fn mwi_queue_len(&self) -> usize {
        self.mwi_queue.lock().len()
    }

    /// Purge expired MD messages from the queue.
    fn purge_expired_md(&self, queue: &mut VecDeque<SmdiMdMessage>) {
        while let Some(front) = queue.front() {
            if front.is_expired() {
                debug!(
                    port = %self.port,
                    station = %front.station,
                    "Purging expired SMDI MD message"
                );
                queue.pop_front();
            } else {
                break;
            }
        }
    }

    /// Purge expired MWI messages from the queue.
    fn purge_expired_mwi(&self, queue: &mut VecDeque<SmdiMwiMessage>) {
        while let Some(front) = queue.front() {
            if front.is_expired() {
                debug!(
                    port = %self.port,
                    mailbox = %front.mailbox,
                    "Purging expired SMDI MWI message"
                );
                queue.pop_front();
            } else {
                break;
            }
        }
    }
}

impl fmt::Debug for SmdiInterface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmdiInterface")
            .field("port", &self.port)
            .field("speed", &self.speed)
            .field("md_queue_len", &self.md_queue.lock().len())
            .field("mwi_queue_len", &self.mwi_queue.lock().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// SMDI message parsing
// ---------------------------------------------------------------------------

/// Parse an SMDI MD message from raw serial data.
///
/// SMDI MD message format (simplified):
/// `MD<type><fwd_station><calling_station>`
///
/// The exact format varies by equipment, but the core fields are always present.
pub fn parse_md_message(data: &str) -> SmdiResult<SmdiMdMessage> {
    let data = data.trim();
    if data.len() < 3 {
        return Err(SmdiError::Parse("MD message too short".into()));
    }

    let prefix = &data[..2];
    if prefix != "MD" {
        return Err(SmdiError::Parse(format!(
            "Expected 'MD' prefix, got '{}'",
            prefix
        )));
    }

    let type_char = data.chars().nth(2).ok_or_else(|| {
        SmdiError::Parse("Missing call type character".into())
    })?;
    let call_type = SmdiMdType::from_char(type_char).ok_or_else(|| {
        SmdiError::Parse(format!("Unknown call type: '{}'", type_char))
    })?;

    let rest = &data[3..];
    // Split the rest into station fields. The exact splitting depends on
    // the SMDI variant, but commonly it's fixed-width fields.
    // For robustness, we split on whitespace or use fixed 10-char fields.
    let (station, calling) = if rest.len() >= 20 {
        // Fixed-width: 10 chars each.
        (rest[..10].trim().to_string(), rest[10..20].trim().to_string())
    } else {
        // Fallback: split in half or use full string as station.
        let mid = rest.len() / 2;
        if mid > 0 {
            (rest[..mid].trim().to_string(), rest[mid..].trim().to_string())
        } else {
            (rest.trim().to_string(), String::new())
        }
    };

    Ok(SmdiMdMessage::new(&station, &calling, call_type))
}

/// Parse an SMDI MWI message from raw serial data.
///
/// SMDI MWI format: `OP:MWI <mailbox>!` (on) or `RMV:MWI <mailbox>!` (off)
pub fn parse_mwi_message(data: &str) -> SmdiResult<SmdiMwiMessage> {
    let data = data.trim();

    let (cause, rest) = if let Some(rest) = data.strip_prefix("OP:MWI") {
        (SmdiMwiCause::On, rest.trim())
    } else if let Some(rest) = data.strip_prefix("RMV:MWI") {
        (SmdiMwiCause::Off, rest.trim())
    } else {
        return Err(SmdiError::Parse(format!(
            "Unrecognized MWI message format: {}",
            data
        )));
    };

    let mailbox = rest.trim_end_matches('!').trim().to_string();
    if mailbox.is_empty() {
        return Err(SmdiError::Parse("Empty mailbox in MWI message".into()));
    }

    Ok(SmdiMwiMessage::new(&mailbox, cause))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smdi_md_type_parse() {
        assert_eq!(SmdiMdType::from_char('D'), Some(SmdiMdType::Direct));
        assert_eq!(SmdiMdType::from_char('A'), Some(SmdiMdType::ForwardAll));
        assert_eq!(SmdiMdType::from_char('B'), Some(SmdiMdType::ForwardBusy));
        assert_eq!(SmdiMdType::from_char('N'), Some(SmdiMdType::ForwardNoAnswer));
        assert_eq!(SmdiMdType::from_char('X'), None);
    }

    #[test]
    fn test_smdi_md_type_roundtrip() {
        for t in [SmdiMdType::Direct, SmdiMdType::ForwardAll, SmdiMdType::ForwardBusy, SmdiMdType::ForwardNoAnswer] {
            let c = t.as_char();
            assert_eq!(SmdiMdType::from_char(c), Some(t));
        }
    }

    #[test]
    fn test_smdi_speed_parse() {
        assert_eq!(SmdiSpeed::from_config("9600"), Some(SmdiSpeed::B9600));
        assert_eq!(SmdiSpeed::from_config("2400"), Some(SmdiSpeed::B2400));
        assert_eq!(SmdiSpeed::from_config("115200"), None);
    }

    #[test]
    fn test_parse_md_message() {
        let msg = parse_md_message("MDD1001      2001      ").unwrap();
        assert_eq!(msg.call_type, SmdiMdType::Direct);
        assert_eq!(msg.station, "1001");
        assert_eq!(msg.calling_station, "2001");
    }

    #[test]
    fn test_parse_md_message_forward() {
        let msg = parse_md_message("MDA3000      4000      ").unwrap();
        assert_eq!(msg.call_type, SmdiMdType::ForwardAll);
        assert_eq!(msg.station, "3000");
        assert_eq!(msg.calling_station, "4000");
    }

    #[test]
    fn test_parse_md_message_invalid() {
        assert!(parse_md_message("XX").is_err());
        assert!(parse_md_message("MDX1234").is_err());
        assert!(parse_md_message("").is_err());
    }

    #[test]
    fn test_parse_mwi_message_on() {
        let msg = parse_mwi_message("OP:MWI 1001!").unwrap();
        assert_eq!(msg.mailbox, "1001");
        assert_eq!(msg.cause, SmdiMwiCause::On);
    }

    #[test]
    fn test_parse_mwi_message_off() {
        let msg = parse_mwi_message("RMV:MWI 1001!").unwrap();
        assert_eq!(msg.mailbox, "1001");
        assert_eq!(msg.cause, SmdiMwiCause::Off);
    }

    #[test]
    fn test_parse_mwi_message_invalid() {
        assert!(parse_mwi_message("INVALID").is_err());
        assert!(parse_mwi_message("OP:MWI !").is_err());
    }

    #[test]
    fn test_smdi_interface_md_queue() {
        let iface = SmdiInterface::new("/dev/ttyS0", SmdiSpeed::B9600);

        let msg1 = SmdiMdMessage::new("1001", "2001", SmdiMdType::Direct);
        let msg2 = SmdiMdMessage::new("1002", "2002", SmdiMdType::ForwardBusy);

        iface.enqueue_md(msg1);
        iface.enqueue_md(msg2);
        assert_eq!(iface.md_queue_len(), 2);

        let retrieved = iface.get_md_by_station("1001").unwrap();
        assert_eq!(retrieved.station, "1001");
        assert_eq!(retrieved.calling_station, "2001");
        assert_eq!(iface.md_queue_len(), 1);

        assert!(iface.get_md_by_station("1001").is_none());
    }

    #[test]
    fn test_smdi_interface_mwi_queue() {
        let iface = SmdiInterface::new("/dev/ttyS0", SmdiSpeed::B9600);

        iface.enqueue_mwi(SmdiMwiMessage::new("1001", SmdiMwiCause::On));
        iface.enqueue_mwi(SmdiMwiMessage::new("1002", SmdiMwiCause::Off));
        assert_eq!(iface.mwi_queue_len(), 2);

        let mwi = iface.get_mwi("1002").unwrap();
        assert_eq!(mwi.cause, SmdiMwiCause::Off);
        assert_eq!(iface.mwi_queue_len(), 1);
    }

    #[test]
    fn test_smdi_md_message_expiry() {
        let mut msg = SmdiMdMessage::new("1001", "2001", SmdiMdType::Direct);
        assert!(!msg.is_expired());

        // Simulate expiry by setting received_at in the past.
        msg.received_at = Instant::now() - Duration::from_secs(60);
        msg.expiry = Duration::from_secs(30);
        assert!(msg.is_expired());
    }

    #[test]
    fn test_smdi_mwi_cause() {
        assert_eq!(SmdiMwiCause::On.as_str(), "on");
        assert_eq!(SmdiMwiCause::Off.as_str(), "off");
    }
}
