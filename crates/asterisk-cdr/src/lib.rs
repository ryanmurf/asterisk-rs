//! asterisk-cdr: Call Detail Record engine.
//!
//! This crate provides the CDR (Call Detail Record) system that tracks
//! call information for billing, auditing, and analytics purposes.
//! It ports the core CDR engine from Asterisk C's main/cdr.c and
//! multiple backends for persisting CDR records.
//!
//! Architecture:
//! - CdrEngine: Tracks channel lifecycle events and produces CDR records
//! - CdrBackend (trait): Pluggable backends for storing CDR records
//! - CsvCdrBackend: Writes CDR records to CSV files (cdr_csv.c)
//! - CustomCdrBackend: Configurable template-based output (cdr_custom.c)
//! - ManagerCdrBackend: AMI event output (cdr_manager.c)
//! - Sqlite3CdrBackend: SQLite database output (cdr_sqlite3_custom.c)
//! - SyslogCdrBackend: Syslog output (cdr_syslog.c)
//! - Cdr: The actual CDR data record

pub mod engine;
pub mod csv_backend;
pub mod cdr_custom;
pub mod cdr_manager;
pub mod cdr_sqlite3;
pub mod cdr_syslog;
pub mod cdr_beanstalkd;
pub mod cdr_odbc;
pub mod cdr_pgsql;
pub mod cdr_radius;
pub mod cdr_tds;

pub use engine::{CdrEngine, CdrState};
pub use csv_backend::CsvCdrBackend;
pub use cdr_custom::CustomCdrBackend;
pub use cdr_manager::ManagerCdrBackend;
pub use cdr_sqlite3::Sqlite3CdrBackend;
pub use cdr_syslog::SyslogCdrBackend;
pub use cdr_beanstalkd::BeanstalkdCdrBackend;
pub use cdr_odbc::OdbcCdrBackend;
pub use cdr_pgsql::PgsqlCdrBackend;
pub use cdr_radius::RadiusCdrBackend;
pub use cdr_tds::TdsCdrBackend;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// A Call Detail Record - contains all information about a single call leg.
///
/// This is the Rust equivalent of `struct ast_cdr` from cdr.h.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cdr {
    /// Account code of the caller
    pub account_code: String,
    /// Source (caller) identification
    pub src: String,
    /// Destination extension
    pub dst: String,
    /// Destination context
    pub dst_context: String,
    /// Caller ID string
    pub caller_id: String,
    /// Calling channel name
    pub channel: String,
    /// Destination channel name (if applicable)
    pub dst_channel: String,
    /// Last application executed
    pub last_app: String,
    /// Arguments to the last application
    pub last_data: String,
    /// Time the call started (channel created)
    pub start: SystemTime,
    /// Time the call was answered (None if never answered)
    pub answer: Option<SystemTime>,
    /// Time the call ended
    pub end: SystemTime,
    /// Total duration in seconds (end - start)
    pub duration: i64,
    /// Billable seconds (end - answer, 0 if never answered)
    pub billsec: i64,
    /// Call disposition
    pub disposition: CdrDisposition,
    /// AMA (Automatic Message Accounting) flags
    pub ama_flags: AmaFlags,
    /// Account code for the peer (destination)
    pub peer_account: String,
    /// Unique call identifier
    pub unique_id: String,
    /// Linked ID (ties related CDRs together)
    pub linked_id: String,
    /// User-defined field
    pub user_field: String,
    /// CDR sequence number
    pub sequence: u64,
    /// User-defined variables attached to this CDR
    pub variables: HashMap<String, String>,
}

impl Cdr {
    /// Create a new CDR with the current time as start.
    pub fn new(channel: String, unique_id: String) -> Self {
        let now = SystemTime::now();
        Self {
            account_code: String::new(),
            src: String::new(),
            dst: String::new(),
            dst_context: String::new(),
            caller_id: String::new(),
            channel,
            dst_channel: String::new(),
            last_app: String::new(),
            last_data: String::new(),
            start: now,
            answer: None,
            end: now,
            duration: 0,
            billsec: 0,
            disposition: CdrDisposition::NoAnswer,
            ama_flags: AmaFlags::Default,
            peer_account: String::new(),
            unique_id,
            linked_id: String::new(),
            user_field: String::new(),
            sequence: 0,
            variables: HashMap::new(),
        }
    }

    /// Mark the CDR as answered at the current time.
    pub fn mark_answered(&mut self) {
        self.answer = Some(SystemTime::now());
        self.disposition = CdrDisposition::Answered;
    }

    /// Finalize the CDR - compute duration and billable seconds.
    pub fn finalize(&mut self) {
        self.end = SystemTime::now();

        // Calculate duration
        self.duration = self
            .end
            .duration_since(self.start)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Calculate billable seconds
        self.billsec = if let Some(answer) = self.answer {
            self.end
                .duration_since(answer)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        } else {
            0
        };
    }

    /// Set a CDR variable.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    /// Get a CDR variable.
    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.variables.get(name)
    }

    /// Format the CDR as a human-readable string.
    pub fn summary(&self) -> String {
        format!(
            "{} -> {} via {} ({}, {}s/{}s)",
            self.src,
            self.dst,
            self.channel,
            self.disposition.as_str(),
            self.duration,
            self.billsec,
        )
    }
}

/// CDR disposition - the final state of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CdrDisposition {
    /// Call was not answered
    NoAnswer,
    /// Called party was busy
    Busy,
    /// Call was answered
    Answered,
    /// Network congestion prevented the call
    Congestion,
    /// Call failed for other reasons
    Failed,
}

impl CdrDisposition {
    /// String representation for CDR output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoAnswer => "NO ANSWER",
            Self::Busy => "BUSY",
            Self::Answered => "ANSWERED",
            Self::Congestion => "CONGESTION",
            Self::Failed => "FAILED",
        }
    }

    /// Parse from string.
    pub fn from_str_name(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "ANSWERED" => Self::Answered,
            "BUSY" => Self::Busy,
            "NO ANSWER" | "NOANSWER" => Self::NoAnswer,
            "CONGESTION" => Self::Congestion,
            "FAILED" => Self::Failed,
            _ => Self::NoAnswer,
        }
    }

    /// Numeric value for CSV output.
    pub fn as_int(&self) -> i32 {
        match self {
            Self::NoAnswer => 0,
            Self::Busy => 1,
            Self::Answered => 2,
            Self::Congestion => 3,
            Self::Failed => 4,
        }
    }
}

impl std::fmt::Display for CdrDisposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// AMA (Automatic Message Accounting) flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AmaFlags {
    /// Omit this CDR from billing
    Omit,
    /// This CDR is for billing purposes
    Billing,
    /// This CDR is for documentation only
    Documentation,
    /// Default AMA flag
    #[default]
    Default,
}

impl AmaFlags {
    /// String representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Omit => "OMIT",
            Self::Billing => "BILLING",
            Self::Documentation => "DOCUMENTATION",
            Self::Default => "DEFAULT",
        }
    }

    /// Parse from string.
    pub fn from_str_name(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "OMIT" | "1" => Self::Omit,
            "BILLING" | "BILL" | "2" => Self::Billing,
            "DOCUMENTATION" | "DOC" | "3" => Self::Documentation,
            _ => Self::Default,
        }
    }

    /// Numeric value.
    pub fn as_int(&self) -> i32 {
        match self {
            Self::Omit => 1,
            Self::Billing => 2,
            Self::Documentation => 3,
            Self::Default => 0,
        }
    }
}

impl std::fmt::Display for AmaFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Trait for CDR backend implementations.
///
/// Each backend is responsible for persisting CDR records to some
/// storage medium (files, databases, etc.).
pub trait CdrBackend: Send + Sync {
    /// Name of this backend (e.g., "csv", "mysql", "pgsql").
    fn name(&self) -> &str;

    /// Log a finalized CDR record.
    fn log(&self, cdr: &Cdr) -> Result<(), CdrError>;
}

/// CDR-specific errors.
#[derive(Debug)]
pub enum CdrError {
    /// I/O error writing CDR
    Io(std::io::Error),
    /// Backend-specific error
    Backend(String),
    /// CDR is not in a state to be logged
    InvalidState(String),
}

impl std::fmt::Display for CdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "CDR I/O error: {}", e),
            Self::Backend(msg) => write!(f, "CDR backend error: {}", msg),
            Self::InvalidState(msg) => write!(f, "CDR invalid state: {}", msg),
        }
    }
}

impl std::error::Error for CdrError {}

impl From<std::io::Error> for CdrError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdr_creation() {
        let cdr = Cdr::new("SIP/alice-001".to_string(), "unique-123".to_string());
        assert_eq!(cdr.channel, "SIP/alice-001");
        assert_eq!(cdr.unique_id, "unique-123");
        assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
        assert_eq!(cdr.billsec, 0);
    }

    #[test]
    fn test_cdr_answered() {
        let mut cdr = Cdr::new("SIP/alice-001".to_string(), "unique-123".to_string());
        cdr.mark_answered();
        assert_eq!(cdr.disposition, CdrDisposition::Answered);
        assert!(cdr.answer.is_some());
    }

    #[test]
    fn test_cdr_finalize() {
        let mut cdr = Cdr::new("SIP/alice-001".to_string(), "unique-123".to_string());
        cdr.finalize();
        assert!(cdr.duration >= 0);
    }

    #[test]
    fn test_disposition_parsing() {
        assert_eq!(CdrDisposition::from_str_name("ANSWERED"), CdrDisposition::Answered);
        assert_eq!(CdrDisposition::from_str_name("BUSY"), CdrDisposition::Busy);
        assert_eq!(CdrDisposition::from_str_name("NO ANSWER"), CdrDisposition::NoAnswer);
    }

    #[test]
    fn test_ama_flags() {
        assert_eq!(AmaFlags::from_str_name("BILLING"), AmaFlags::Billing);
        assert_eq!(AmaFlags::from_str_name("2"), AmaFlags::Billing);
        assert_eq!(AmaFlags::from_str_name("unknown"), AmaFlags::Default);
    }

    #[test]
    fn test_cdr_variables() {
        let mut cdr = Cdr::new("test".to_string(), "uid".to_string());
        cdr.set_variable("custom_field", "custom_value");
        assert_eq!(
            cdr.get_variable("custom_field"),
            Some(&"custom_value".to_string())
        );
    }
}
