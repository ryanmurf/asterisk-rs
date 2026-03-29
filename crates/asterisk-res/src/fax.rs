//! FAX framework.
//!
//! Port of `res/res_fax.c`. Provides a pluggable fax technology framework
//! that supports different fax engines (SpanDSP, T.38 gateway, etc.)
//! through a common session-based API.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum FaxError {
    #[error("fax session not in correct state: expected {expected:?}, got {actual:?}")]
    InvalidState {
        expected: FaxState,
        actual: FaxState,
    },
    #[error("fax technology error: {0}")]
    TechError(String),
    #[error("fax session error: {0}")]
    SessionError(String),
}

pub type FaxResult<T> = Result<T, FaxError>;

// ---------------------------------------------------------------------------
// Fax state machine
// ---------------------------------------------------------------------------

/// State of a fax session.
///
/// Mirrors `enum ast_fax_state` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaxState {
    /// Session created but not configured.
    Uninitialized,
    /// Session configured, ready to connect.
    Initialized,
    /// Connected to a channel.
    Open,
    /// Fax transmission/reception in progress.
    Active,
    /// Fax completed (successfully or with error).
    Complete,
    /// Session torn down.
    Inactive,
}

impl FaxState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Initialized => "initialized",
            Self::Open => "open",
            Self::Active => "active",
            Self::Complete => "complete",
            Self::Inactive => "inactive",
        }
    }

    /// Check whether transitioning from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: FaxState) -> bool {
        matches!(
            (self, next),
            (Self::Uninitialized, Self::Initialized)
                | (Self::Initialized, Self::Open)
                | (Self::Open, Self::Active)
                | (Self::Active, Self::Complete)
                | (Self::Complete, Self::Inactive)
                // Allow early teardown from any state.
                | (_, Self::Inactive)
        )
    }
}

impl fmt::Display for FaxState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Fax modems / capabilities
// ---------------------------------------------------------------------------

/// Fax modem capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaxModem {
    V17,
    V27,
    V29,
    V34,
}

// ---------------------------------------------------------------------------
// Fax technology trait
// ---------------------------------------------------------------------------

/// A pluggable fax technology (engine).
///
/// Mirrors `struct ast_fax_tech` from the C source.
pub trait FaxTechnology: Send + Sync + fmt::Debug {
    /// Technology name (e.g., "spandsp", "res_fax_digium").
    fn name(&self) -> &str;

    /// Create engine-specific data for a new fax session.
    fn create_session(&self, session: &mut FaxSession) -> FaxResult<()>;

    /// Write audio/T.38 data to the fax engine.
    fn write(&self, session: &mut FaxSession, data: &[u8]) -> FaxResult<()>;

    /// Read audio/T.38 data from the fax engine.
    fn read(&self, session: &mut FaxSession, buf: &mut Vec<u8>) -> FaxResult<usize>;

    /// Start the fax session (begin negotiation).
    fn start(&self, session: &mut FaxSession) -> FaxResult<()>;

    /// Cancel a running session.
    fn cancel(&self, session: &mut FaxSession) -> FaxResult<()> {
        session.transition(FaxState::Inactive)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session ID generator
// ---------------------------------------------------------------------------

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Fax session
// ---------------------------------------------------------------------------

/// A fax session.
///
/// Mirrors `struct ast_fax_session` from the C source.
pub struct FaxSession {
    /// Unique session ID.
    pub id: u64,
    /// Technology name.
    pub tech_name: String,
    /// Current state.
    pub state: FaxState,
    /// Local station ID (TSI/CSI).
    pub local_station_id: String,
    /// Remote station ID (received during negotiation).
    pub remote_station_id: String,
    /// Pages transferred so far.
    pub pages: u32,
    /// Current bitrate in bits per second.
    pub bitrate: u32,
    /// Whether this is a sending (true) or receiving (false) session.
    pub is_sending: bool,
    /// T.38 mode enabled.
    pub t38_mode: bool,
    /// Fax filename for send/receive.
    pub filename: String,
    /// Header string for transmitted faxes.
    pub header_info: String,
    /// ECM (Error Correction Mode) enabled.
    pub ecm_enabled: bool,
    /// Supported modems.
    pub modems: Vec<FaxModem>,
    /// Engine-specific opaque data.
    pub tech_data: Option<Box<dyn std::any::Any + Send + Sync>>,
    /// Creation timestamp.
    pub created: SystemTime,
}

impl FaxSession {
    /// Create a new fax session.
    pub fn new(tech_name: &str) -> Self {
        Self {
            id: next_session_id(),
            tech_name: tech_name.to_string(),
            state: FaxState::Uninitialized,
            local_station_id: String::new(),
            remote_station_id: String::new(),
            pages: 0,
            bitrate: 14400,
            is_sending: false,
            t38_mode: false,
            filename: String::new(),
            header_info: String::new(),
            ecm_enabled: true,
            modems: vec![FaxModem::V17, FaxModem::V27, FaxModem::V29],
            tech_data: None,
            created: SystemTime::now(),
        }
    }

    /// Attempt a state transition. Returns an error if the transition is invalid.
    pub fn transition(&mut self, next: FaxState) -> FaxResult<()> {
        if !self.state.can_transition_to(next) {
            return Err(FaxError::InvalidState {
                expected: next,
                actual: self.state,
            });
        }
        debug!(
            session = self.id,
            from = %self.state,
            to = %next,
            "Fax state transition"
        );
        self.state = next;
        Ok(())
    }

    /// Generate a result summary for a completed session.
    pub fn result(&self) -> FaxResult_ {
        FaxResult_ {
            success: self.state == FaxState::Complete,
            error: String::new(),
            pages: self.pages,
            bitrate: self.bitrate,
            resolution: "204x196".to_string(),
            remote_station_id: self.remote_station_id.clone(),
            local_station_id: self.local_station_id.clone(),
            filename: self.filename.clone(),
            is_sending: self.is_sending,
        }
    }

    /// Builder: set local station ID.
    pub fn with_local_station_id(mut self, id: &str) -> Self {
        self.local_station_id = id.to_string();
        self
    }

    /// Builder: set filename.
    pub fn with_filename(mut self, filename: &str) -> Self {
        self.filename = filename.to_string();
        self
    }

    /// Builder: set sending mode.
    pub fn as_sender(mut self) -> Self {
        self.is_sending = true;
        self
    }

    /// Builder: enable T.38 mode.
    pub fn with_t38(mut self) -> Self {
        self.t38_mode = true;
        self
    }
}

impl fmt::Debug for FaxSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaxSession")
            .field("id", &self.id)
            .field("tech", &self.tech_name)
            .field("state", &self.state)
            .field("pages", &self.pages)
            .field("bitrate", &self.bitrate)
            .field("sending", &self.is_sending)
            .field("t38", &self.t38_mode)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Fax result (named with trailing underscore to avoid conflict with Result)
// ---------------------------------------------------------------------------

/// Summary of a completed fax session.
#[derive(Debug, Clone)]
pub struct FaxResult_ {
    /// Whether the fax completed successfully.
    pub success: bool,
    /// Error description (empty on success).
    pub error: String,
    /// Number of pages transferred.
    pub pages: u32,
    /// Final negotiated bitrate.
    pub bitrate: u32,
    /// Resolution string (e.g., "204x196").
    pub resolution: String,
    /// Remote station identification.
    pub remote_station_id: String,
    /// Local station identification.
    pub local_station_id: String,
    /// Filename used.
    pub filename: String,
    /// Direction.
    pub is_sending: bool,
}

impl fmt::Display for FaxResult_ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Fax {} {} pages={} rate={} remote={}",
            if self.is_sending { "TX" } else { "RX" },
            if self.success { "OK" } else { "FAILED" },
            self.pages,
            self.bitrate,
            self.remote_station_id,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fax_state_transitions() {
        assert!(FaxState::Uninitialized.can_transition_to(FaxState::Initialized));
        assert!(FaxState::Initialized.can_transition_to(FaxState::Open));
        assert!(FaxState::Open.can_transition_to(FaxState::Active));
        assert!(FaxState::Active.can_transition_to(FaxState::Complete));
        assert!(FaxState::Complete.can_transition_to(FaxState::Inactive));
        // Early teardown allowed from any state.
        assert!(FaxState::Active.can_transition_to(FaxState::Inactive));
        // Invalid transitions.
        assert!(!FaxState::Uninitialized.can_transition_to(FaxState::Active));
        assert!(!FaxState::Complete.can_transition_to(FaxState::Open));
    }

    #[test]
    fn test_session_creation() {
        let session = FaxSession::new("spandsp")
            .with_local_station_id("5551234")
            .with_filename("/tmp/fax.tif")
            .as_sender();

        assert_eq!(session.state, FaxState::Uninitialized);
        assert_eq!(session.local_station_id, "5551234");
        assert_eq!(session.filename, "/tmp/fax.tif");
        assert!(session.is_sending);
        assert!(session.id > 0);
    }

    #[test]
    fn test_session_transition() {
        let mut session = FaxSession::new("test");
        session.transition(FaxState::Initialized).unwrap();
        session.transition(FaxState::Open).unwrap();
        session.transition(FaxState::Active).unwrap();
        session.transition(FaxState::Complete).unwrap();
        assert_eq!(session.state, FaxState::Complete);
    }

    #[test]
    fn test_session_invalid_transition() {
        let mut session = FaxSession::new("test");
        let result = session.transition(FaxState::Active);
        assert!(result.is_err());
    }

    #[test]
    fn test_session_result() {
        let mut session = FaxSession::new("test");
        session.pages = 3;
        session.bitrate = 9600;
        session.remote_station_id = "5559876".to_string();
        session.state = FaxState::Complete;

        let result = session.result();
        assert!(result.success);
        assert_eq!(result.pages, 3);
        assert_eq!(result.bitrate, 9600);
        assert_eq!(result.remote_station_id, "5559876");
    }

    #[test]
    fn test_unique_session_ids() {
        let s1 = FaxSession::new("test");
        let s2 = FaxSession::new("test");
        assert_ne!(s1.id, s2.id);
    }
}
