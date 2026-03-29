//! Security event logging.
//!
//! Port of `res/res_security_log.c`. Provides structured logging for
//! security-relevant events such as authentication failures, ACL
//! violations, and invalid transport usage.

use std::collections::VecDeque;
use std::fmt;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Security event types
// ---------------------------------------------------------------------------

/// Types of security events.
///
/// Mirrors `enum ast_security_event_type` from the C header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityEvent {
    /// Authentication attempt failed.
    FailedAuth,
    /// Authentication succeeded.
    SuccessfulAuth,
    /// Access denied by ACL.
    AclViolation,
    /// Invalid password supplied.
    InvalidPassword,
    /// Challenge-response exchange.
    ChallengeResponse,
    /// Request arrived on an invalid transport.
    InvalidTransport,
    /// Request was denied by rate limiting.
    RequestFlood,
    /// Session limit exceeded.
    SessionLimit,
    /// Memory limit exceeded.
    MemoryLimit,
    /// Load average exceeded.
    LoadAverage,
    /// Unexpected address.
    UnexpectedAddress,
    /// Failed to parse request.
    RequestBadFormat,
    /// Authentication method not allowed.
    AuthMethodNotAllowed,
}

impl SecurityEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailedAuth => "FailedACL",
            Self::SuccessfulAuth => "SuccessfulAuth",
            Self::AclViolation => "ACLViolation",
            Self::InvalidPassword => "InvalidPassword",
            Self::ChallengeResponse => "ChallengeResponseFailed",
            Self::InvalidTransport => "InvalidTransport",
            Self::RequestFlood => "RequestFlood",
            Self::SessionLimit => "SessionLimit",
            Self::MemoryLimit => "MemoryLimit",
            Self::LoadAverage => "LoadAverage",
            Self::UnexpectedAddress => "UnexpectedAddress",
            Self::RequestBadFormat => "RequestBadFormat",
            Self::AuthMethodNotAllowed => "AuthMethodNotAllowed",
        }
    }

    /// Severity level for this event type.
    pub fn severity(&self) -> SecuritySeverity {
        match self {
            Self::SuccessfulAuth => SecuritySeverity::Informational,
            Self::ChallengeResponse => SecuritySeverity::Informational,
            Self::FailedAuth | Self::InvalidPassword => SecuritySeverity::Error,
            Self::AclViolation | Self::InvalidTransport => SecuritySeverity::Error,
            Self::RequestFlood | Self::SessionLimit => SecuritySeverity::Warning,
            Self::MemoryLimit | Self::LoadAverage => SecuritySeverity::Warning,
            Self::UnexpectedAddress => SecuritySeverity::Warning,
            Self::RequestBadFormat => SecuritySeverity::Warning,
            Self::AuthMethodNotAllowed => SecuritySeverity::Error,
        }
    }
}

impl fmt::Display for SecurityEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// Severity level for security events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecuritySeverity {
    Informational,
    Warning,
    Error,
}

impl SecuritySeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Informational => "Informational",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }
}

// ---------------------------------------------------------------------------
// Security event data
// ---------------------------------------------------------------------------

/// Detailed data for a security event.
#[derive(Debug, Clone)]
pub struct SecurityEventData {
    /// Event type.
    pub event: SecurityEvent,
    /// Timestamp (seconds since UNIX epoch).
    pub timestamp: u64,
    /// Account ID (e.g., SIP username).
    pub account_id: String,
    /// Session ID (e.g., SIP Call-ID).
    pub session_id: String,
    /// Local address (our side).
    pub local_addr: Option<SocketAddr>,
    /// Remote address (their side).
    pub remote_addr: Option<SocketAddr>,
    /// Service name (e.g., "SIP", "AMI", "ARI").
    pub service: String,
    /// Additional module-specific context.
    pub module: String,
    /// Session transport (TCP, UDP, TLS, WS).
    pub transport: String,
    /// Human-readable message.
    pub message: String,
}

impl SecurityEventData {
    /// Create a new security event with the current timestamp.
    pub fn new(event: SecurityEvent, service: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            event,
            timestamp: now.as_secs(),
            account_id: String::new(),
            session_id: String::new(),
            local_addr: None,
            remote_addr: None,
            service: service.to_string(),
            module: String::new(),
            transport: String::new(),
            message: String::new(),
        }
    }

    pub fn with_account(mut self, account_id: &str) -> Self {
        self.account_id = account_id.to_string();
        self
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = session_id.to_string();
        self
    }

    pub fn with_addresses(mut self, local: SocketAddr, remote: SocketAddr) -> Self {
        self.local_addr = Some(local);
        self.remote_addr = Some(remote);
        self
    }

    pub fn with_transport(mut self, transport: &str) -> Self {
        self.transport = transport.to_string();
        self
    }

    pub fn with_module(mut self, module: &str) -> Self {
        self.module = module.to_string();
        self
    }

    pub fn with_message(mut self, msg: &str) -> Self {
        self.message = msg.to_string();
        self
    }

    /// Format this event as a log line.
    pub fn to_log_line(&self) -> String {
        let mut parts = vec![
            format!("SecurityEvent=\"{}\"", self.event.as_str()),
            format!("Severity=\"{}\"", self.event.severity().as_str()),
            format!("Service=\"{}\"", self.service),
        ];
        if !self.account_id.is_empty() {
            parts.push(format!("AccountID=\"{}\"", self.account_id));
        }
        if !self.session_id.is_empty() {
            parts.push(format!("SessionID=\"{}\"", self.session_id));
        }
        if let Some(local) = self.local_addr {
            parts.push(format!("LocalAddress=\"{}\"", local));
        }
        if let Some(remote) = self.remote_addr {
            parts.push(format!("RemoteAddress=\"{}\"", remote));
        }
        if !self.transport.is_empty() {
            parts.push(format!("Transport=\"{}\"", self.transport));
        }
        if !self.message.is_empty() {
            parts.push(format!("Message=\"{}\"", self.message));
        }
        parts.join(",")
    }
}

// ---------------------------------------------------------------------------
// Security logger
// ---------------------------------------------------------------------------

/// Security event logger.
///
/// Logs security events through the tracing infrastructure and maintains
/// an in-memory ring buffer of recent events for inspection.
pub struct SecurityLogger {
    /// Whether logging is enabled.
    pub enabled: bool,
    /// Recent events ring buffer.
    recent_events: RwLock<VecDeque<SecurityEventData>>,
    /// Maximum ring buffer size.
    pub max_recent: usize,
    /// Total events logged.
    total_logged: RwLock<u64>,
}

impl SecurityLogger {
    pub fn new() -> Self {
        Self {
            enabled: true,
            recent_events: RwLock::new(VecDeque::new()),
            max_recent: 1000,
            total_logged: RwLock::new(0),
        }
    }

    /// Log a security event.
    pub fn log_security_event(&self, event: SecurityEventData) {
        if !self.enabled {
            return;
        }

        let log_line = event.to_log_line();
        match event.event.severity() {
            SecuritySeverity::Error => {
                warn!(security = true, "{}", log_line);
            }
            SecuritySeverity::Warning => {
                warn!(security = true, "{}", log_line);
            }
            SecuritySeverity::Informational => {
                info!(security = true, "{}", log_line);
            }
        }

        let mut recent = self.recent_events.write();
        if recent.len() >= self.max_recent {
            recent.pop_front();
        }
        recent.push_back(event);

        *self.total_logged.write() += 1;
    }

    /// Get recent events.
    pub fn recent_events(&self) -> Vec<SecurityEventData> {
        self.recent_events.read().iter().cloned().collect()
    }

    /// Get recent events filtered by type.
    pub fn recent_by_type(&self, event_type: SecurityEvent) -> Vec<SecurityEventData> {
        self.recent_events
            .read()
            .iter()
            .filter(|e| e.event == event_type)
            .cloned()
            .collect()
    }

    /// Total number of events logged.
    pub fn total_logged(&self) -> u64 {
        *self.total_logged.read()
    }

    /// Clear the recent events buffer.
    pub fn clear(&self) {
        self.recent_events.write().clear();
    }
}

impl Default for SecurityLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SecurityLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecurityLogger")
            .field("enabled", &self.enabled)
            .field("recent", &self.recent_events.read().len())
            .field("total", &*self.total_logged.read())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

/// Log a security event (standalone function matching C API).
pub fn log_security_event(logger: &SecurityLogger, event: SecurityEventData) {
    logger.log_security_event(event);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_event_types() {
        assert_eq!(SecurityEvent::FailedAuth.as_str(), "FailedACL");
        assert_eq!(SecurityEvent::SuccessfulAuth.as_str(), "SuccessfulAuth");
    }

    #[test]
    fn test_severity() {
        assert_eq!(
            SecurityEvent::FailedAuth.severity(),
            SecuritySeverity::Error
        );
        assert_eq!(
            SecurityEvent::SuccessfulAuth.severity(),
            SecuritySeverity::Informational
        );
    }

    #[test]
    fn test_event_data_builder() {
        let evt = SecurityEventData::new(SecurityEvent::FailedAuth, "SIP")
            .with_account("alice")
            .with_session("call-123")
            .with_transport("UDP")
            .with_message("bad password");

        assert_eq!(evt.event, SecurityEvent::FailedAuth);
        assert_eq!(evt.account_id, "alice");
        assert_eq!(evt.service, "SIP");
        assert_eq!(evt.transport, "UDP");
    }

    #[test]
    fn test_to_log_line() {
        let evt = SecurityEventData::new(SecurityEvent::AclViolation, "SIP")
            .with_account("bob")
            .with_addresses(
                "10.0.0.1:5060".parse().unwrap(),
                "203.0.113.1:12345".parse().unwrap(),
            );

        let line = evt.to_log_line();
        assert!(line.contains("SecurityEvent=\"ACLViolation\""));
        assert!(line.contains("AccountID=\"bob\""));
        assert!(line.contains("LocalAddress=\"10.0.0.1:5060\""));
        assert!(line.contains("RemoteAddress=\"203.0.113.1:12345\""));
    }

    #[test]
    fn test_logger() {
        let logger = SecurityLogger::new();
        let evt = SecurityEventData::new(SecurityEvent::SuccessfulAuth, "SIP")
            .with_account("alice");
        logger.log_security_event(evt);

        assert_eq!(logger.total_logged(), 1);
        assert_eq!(logger.recent_events().len(), 1);
    }

    #[test]
    fn test_logger_ring_buffer() {
        let mut logger = SecurityLogger::new();
        logger.max_recent = 3;

        for i in 0..5 {
            let evt = SecurityEventData::new(SecurityEvent::FailedAuth, "SIP")
                .with_account(&format!("user{}", i));
            logger.log_security_event(evt);
        }

        assert_eq!(logger.total_logged(), 5);
        assert_eq!(logger.recent_events().len(), 3);
        // Should have the 3 most recent.
        assert_eq!(logger.recent_events()[0].account_id, "user2");
    }

    #[test]
    fn test_logger_disabled() {
        let mut logger = SecurityLogger::new();
        logger.enabled = false;
        let evt = SecurityEventData::new(SecurityEvent::FailedAuth, "SIP");
        logger.log_security_event(evt);
        assert_eq!(logger.total_logged(), 0);
    }

    #[test]
    fn test_filter_by_type() {
        let logger = SecurityLogger::new();
        logger.log_security_event(
            SecurityEventData::new(SecurityEvent::FailedAuth, "SIP"),
        );
        logger.log_security_event(
            SecurityEventData::new(SecurityEvent::SuccessfulAuth, "SIP"),
        );
        logger.log_security_event(
            SecurityEventData::new(SecurityEvent::FailedAuth, "SIP"),
        );

        let failed = logger.recent_by_type(SecurityEvent::FailedAuth);
        assert_eq!(failed.len(), 2);
    }
}
