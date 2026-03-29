//! SIP keepalive OPTIONS pings.
//!
//! Port of `res/res_pjsip_keepalive.c` (conceptual - the actual keepalive
//! logic in Asterisk is spread across the transport layer). Sends periodic
//! SIP OPTIONS requests to registered contacts to detect reachability.

use std::time::{Duration, Instant};

use tracing::debug;

// ---------------------------------------------------------------------------
// Keepalive configuration
// ---------------------------------------------------------------------------

/// Default keepalive interval in seconds.
pub const DEFAULT_KEEPALIVE_INTERVAL: u64 = 90;

/// Keepalive configuration for a SIP endpoint/contact.
#[derive(Debug, Clone)]
pub struct KeepaliveConfig {
    /// Interval between keepalive probes.
    pub interval: Duration,
    /// Number of missed responses before declaring unreachable.
    pub max_failures: u32,
    /// Whether to use OPTIONS (true) or CRLF keepalive (false).
    pub use_options: bool,
}

impl KeepaliveConfig {
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval: Duration::from_secs(interval_secs),
            max_failures: 3,
            use_options: true,
        }
    }
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self::new(DEFAULT_KEEPALIVE_INTERVAL)
    }
}

// ---------------------------------------------------------------------------
// Keepalive state
// ---------------------------------------------------------------------------

/// Tracks the keepalive state for a single contact.
#[derive(Debug, Clone)]
pub struct KeepaliveState {
    /// Contact URI being monitored.
    pub contact_uri: String,
    /// Configuration.
    pub config: KeepaliveConfig,
    /// Time of last successful response.
    pub last_success: Option<Instant>,
    /// Time of last probe sent.
    pub last_sent: Option<Instant>,
    /// Consecutive failures.
    pub failures: u32,
    /// Whether the contact is considered reachable.
    pub reachable: bool,
}

impl KeepaliveState {
    pub fn new(contact_uri: &str, config: KeepaliveConfig) -> Self {
        Self {
            contact_uri: contact_uri.to_string(),
            config,
            last_success: None,
            last_sent: None,
            failures: 0,
            reachable: true,
        }
    }

    /// Check if a probe should be sent now.
    pub fn should_probe(&self) -> bool {
        match self.last_sent {
            None => true,
            Some(sent) => sent.elapsed() >= self.config.interval,
        }
    }

    /// Record that a probe was sent.
    pub fn probe_sent(&mut self) {
        self.last_sent = Some(Instant::now());
        debug!(contact = %self.contact_uri, "Keepalive probe sent");
    }

    /// Record a successful response.
    pub fn response_received(&mut self) {
        self.last_success = Some(Instant::now());
        let was_unreachable = !self.reachable;
        self.failures = 0;
        self.reachable = true;
        if was_unreachable {
            debug!(contact = %self.contact_uri, "Contact became reachable");
        }
    }

    /// Record a failed probe (timeout or error response).
    pub fn probe_failed(&mut self) {
        self.failures += 1;
        if self.failures >= self.config.max_failures {
            if self.reachable {
                debug!(
                    contact = %self.contact_uri,
                    failures = self.failures,
                    "Contact became unreachable"
                );
            }
            self.reachable = false;
        }
    }

    /// Time since last successful response.
    pub fn time_since_success(&self) -> Option<Duration> {
        self.last_success.map(|t| t.elapsed())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KeepaliveConfig::default();
        assert_eq!(config.interval, Duration::from_secs(90));
        assert_eq!(config.max_failures, 3);
        assert!(config.use_options);
    }

    #[test]
    fn test_initial_state() {
        let state = KeepaliveState::new("sip:1001@10.0.0.1:5060", KeepaliveConfig::default());
        assert!(state.reachable);
        assert_eq!(state.failures, 0);
        assert!(state.should_probe());
    }

    #[test]
    fn test_probe_lifecycle() {
        let mut state = KeepaliveState::new("sip:1001@10.0.0.1:5060", KeepaliveConfig::default());
        state.probe_sent();
        assert!(state.last_sent.is_some());

        state.response_received();
        assert!(state.reachable);
        assert_eq!(state.failures, 0);
    }

    #[test]
    fn test_failure_detection() {
        let config = KeepaliveConfig {
            interval: Duration::from_secs(30),
            max_failures: 2,
            use_options: true,
        };
        let mut state = KeepaliveState::new("sip:1001@10.0.0.1:5060", config);

        state.probe_failed();
        assert!(state.reachable); // 1 failure, threshold is 2

        state.probe_failed();
        assert!(!state.reachable); // 2 failures, now unreachable

        state.response_received();
        assert!(state.reachable); // recovered
        assert_eq!(state.failures, 0);
    }
}
