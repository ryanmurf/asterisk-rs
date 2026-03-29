//! CDR Engine - tracks channel lifecycle and produces CDR records.
//!
//! Port of the CDR state machine from main/cdr.c in Asterisk C.
//! The engine monitors channel events (creation, answer, bridge, hangup)
//! and produces finalized CDR records that are dispatched to backends.

use crate::{Cdr, CdrBackend, CdrDisposition};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// The state of a CDR as it progresses through the call lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdrState {
    /// CDR has been created (channel exists but hasn't done anything yet)
    Single,
    /// Channel is in a dial operation
    Dial,
    /// Channel is in a bridge
    Bridge,
    /// CDR has been finalized (call ended)
    Finalized,
}

impl CdrState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Dial => "Dial",
            Self::Bridge => "Bridge",
            Self::Finalized => "Finalized",
        }
    }
}

impl std::fmt::Display for CdrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An active CDR being tracked by the engine.
#[derive(Debug)]
struct ActiveCdr {
    /// The CDR data
    cdr: Cdr,
    /// Current state
    state: CdrState,
}

/// CDR Engine configuration.
#[derive(Debug, Clone)]
pub struct CdrConfig {
    /// Whether CDR logging is enabled
    pub enabled: bool,
    /// Whether to log unanswered calls
    pub log_unanswered: bool,
    /// Whether to log congested calls
    pub log_congestion: bool,
    /// Whether to end CDR before h extension processing
    pub end_before_h_exten: bool,
    /// Use initiated seconds for billsec rounding
    pub initiated_seconds: bool,
    /// Whether CDR is enabled on channels by default
    pub channel_default_enabled: bool,
}

impl Default for CdrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_unanswered: false,
            log_congestion: false,
            end_before_h_exten: true,
            initiated_seconds: false,
            channel_default_enabled: true,
        }
    }
}

/// The CDR engine that manages the lifecycle of CDR records.
///
/// It tracks active CDRs keyed by channel unique ID and dispatches
/// finalized CDRs to registered backends.
pub struct CdrEngine {
    /// Configuration
    config: RwLock<CdrConfig>,
    /// Active (in-progress) CDRs keyed by channel unique ID
    active_cdrs: RwLock<HashMap<String, ActiveCdr>>,
    /// Registered CDR backends
    backends: RwLock<Vec<Arc<dyn CdrBackend>>>,
    /// Sequence counter for CDR sequence numbers
    sequence: std::sync::atomic::AtomicU64,
    /// Total CDRs processed
    total_processed: std::sync::atomic::AtomicU64,
}

impl CdrEngine {
    /// Create a new CDR engine with default configuration.
    pub fn new() -> Self {
        Self {
            config: RwLock::new(CdrConfig::default()),
            active_cdrs: RwLock::new(HashMap::new()),
            backends: RwLock::new(Vec::new()),
            sequence: std::sync::atomic::AtomicU64::new(1),
            total_processed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Create a new CDR engine with the given configuration.
    pub fn with_config(config: CdrConfig) -> Self {
        Self {
            config: RwLock::new(config),
            active_cdrs: RwLock::new(HashMap::new()),
            backends: RwLock::new(Vec::new()),
            sequence: std::sync::atomic::AtomicU64::new(1),
            total_processed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Update the CDR engine configuration.
    pub fn set_config(&self, config: CdrConfig) {
        *self.config.write() = config;
    }

    /// Register a CDR backend.
    pub fn register_backend(&self, backend: Arc<dyn CdrBackend>) {
        let name = backend.name().to_string();
        self.backends.write().push(backend);
        info!("CDR engine: registered backend '{}'", name);
    }

    /// Handle a channel creation event.
    pub fn channel_created(
        &self,
        unique_id: &str,
        channel_name: &str,
        caller_id: &str,
        src: &str,
        context: &str,
    ) {
        if !self.config.read().enabled {
            return;
        }

        let seq = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut cdr = Cdr::new(channel_name.to_string(), unique_id.to_string());
        cdr.caller_id = caller_id.to_string();
        cdr.src = src.to_string();
        cdr.dst_context = context.to_string();
        cdr.linked_id = unique_id.to_string();
        cdr.sequence = seq;

        debug!(
            "CDR engine: channel created '{}' (uid: {})",
            channel_name, unique_id
        );

        self.active_cdrs.write().insert(
            unique_id.to_string(),
            ActiveCdr {
                cdr,
                state: CdrState::Single,
            },
        );
    }

    /// Handle a dial begin event.
    pub fn dial_begin(
        &self,
        caller_unique_id: &str,
        callee_channel: &str,
        dst: &str,
    ) {
        if !self.config.read().enabled {
            return;
        }

        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(caller_unique_id) {
            entry.state = CdrState::Dial;
            entry.cdr.dst = dst.to_string();
            entry.cdr.dst_channel = callee_channel.to_string();
            entry.cdr.last_app = "Dial".to_string();
            debug!(
                "CDR engine: dial begin from '{}' to '{}'",
                entry.cdr.channel, callee_channel
            );
        }
    }

    /// Handle a channel answer event.
    pub fn channel_answered(&self, unique_id: &str) {
        if !self.config.read().enabled {
            return;
        }

        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(unique_id) {
            entry.cdr.mark_answered();
            debug!("CDR engine: channel answered '{}'", entry.cdr.channel);
        }
    }

    /// Handle a bridge enter event.
    pub fn bridge_enter(
        &self,
        unique_id: &str,
        bridge_id: &str,
        peer_channel: &str,
    ) {
        if !self.config.read().enabled {
            return;
        }

        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(unique_id) {
            entry.state = CdrState::Bridge;
            if entry.cdr.dst_channel.is_empty() {
                entry.cdr.dst_channel = peer_channel.to_string();
            }
            debug!(
                "CDR engine: bridge enter '{}' in bridge '{}'",
                entry.cdr.channel, bridge_id
            );
        }
    }

    /// Handle a bridge leave event.
    pub fn bridge_leave(&self, unique_id: &str, bridge_id: &str) {
        if !self.config.read().enabled {
            return;
        }

        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(unique_id) {
            // Transition back from Bridge state
            debug!(
                "CDR engine: bridge leave '{}' from bridge '{}'",
                entry.cdr.channel, bridge_id
            );
        }
    }

    /// Handle a channel hangup event. This finalizes the CDR.
    pub fn channel_hangup(
        &self,
        unique_id: &str,
        cause: u32,
        last_app: &str,
        last_data: &str,
    ) {
        if !self.config.read().enabled {
            return;
        }

        let entry = self.active_cdrs.write().remove(unique_id);
        if let Some(mut entry) = entry {
            // Update last app/data
            if !last_app.is_empty() {
                entry.cdr.last_app = last_app.to_string();
            }
            if !last_data.is_empty() {
                entry.cdr.last_data = last_data.to_string();
            }

            // Set disposition based on hangup cause if not already answered
            if entry.cdr.disposition != CdrDisposition::Answered {
                entry.cdr.disposition = match cause {
                    17 => CdrDisposition::Busy,
                    34 | 42 => CdrDisposition::Congestion,
                    _ => CdrDisposition::NoAnswer,
                };
            }

            // Finalize the CDR
            entry.cdr.finalize();
            entry.state = CdrState::Finalized;

            info!(
                "CDR engine: finalized CDR for '{}': {}",
                entry.cdr.channel,
                entry.cdr.summary()
            );

            // Dispatch to backends
            self.dispatch_cdr(&entry.cdr);
        }
    }

    /// Update the last application and data for a channel.
    pub fn update_app(&self, unique_id: &str, app: &str, data: &str) {
        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(unique_id) {
            entry.cdr.last_app = app.to_string();
            entry.cdr.last_data = data.to_string();
        }
    }

    /// Set a CDR variable for a channel.
    pub fn set_variable(&self, unique_id: &str, name: &str, value: &str) {
        let mut active = self.active_cdrs.write();
        if let Some(entry) = active.get_mut(unique_id) {
            entry.cdr.set_variable(name, value);
        }
    }

    /// Get a CDR variable for a channel.
    pub fn get_variable(&self, unique_id: &str, name: &str) -> Option<String> {
        let active = self.active_cdrs.read();
        active
            .get(unique_id)
            .and_then(|entry| entry.cdr.get_variable(name).cloned())
    }

    /// Dispatch a finalized CDR to all registered backends.
    fn dispatch_cdr(&self, cdr: &Cdr) {
        let config = self.config.read();

        // Check if we should log this CDR
        if !config.log_unanswered && cdr.disposition == CdrDisposition::NoAnswer {
            debug!("CDR engine: skipping unanswered CDR for '{}'", cdr.channel);
            return;
        }
        if !config.log_congestion && cdr.disposition == CdrDisposition::Congestion {
            debug!("CDR engine: skipping congested CDR for '{}'", cdr.channel);
            return;
        }
        drop(config);

        let backends = self.backends.read();
        for backend in backends.iter() {
            if let Err(e) = backend.log(cdr) {
                warn!(
                    "CDR engine: backend '{}' failed to log CDR: {}",
                    backend.name(),
                    e
                );
            }
        }

        self.total_processed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the number of active (in-progress) CDRs.
    pub fn active_count(&self) -> usize {
        self.active_cdrs.read().len()
    }

    /// Get the total number of CDRs processed.
    pub fn total_processed(&self) -> u64 {
        self.total_processed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the number of registered backends.
    pub fn backend_count(&self) -> usize {
        self.backends.read().len()
    }

    /// List registered backend names.
    pub fn backend_names(&self) -> Vec<String> {
        self.backends
            .read()
            .iter()
            .map(|b| b.name().to_string())
            .collect()
    }

    /// Get a snapshot of all active CDRs for status reporting.
    pub fn active_cdr_summary(&self) -> Vec<(String, String, CdrState)> {
        self.active_cdrs
            .read()
            .iter()
            .map(|(uid, entry)| {
                (uid.clone(), entry.cdr.channel.clone(), entry.state)
            })
            .collect()
    }
}

impl Default for CdrEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = CdrEngine::new();
        assert_eq!(engine.active_count(), 0);
        assert_eq!(engine.total_processed(), 0);
    }

    #[test]
    fn test_channel_lifecycle() {
        let engine = CdrEngine::new();

        // Create channel
        engine.channel_created("uid-1", "SIP/alice-001", "Alice <5551234>", "5551234", "default");
        assert_eq!(engine.active_count(), 1);

        // Dial
        engine.dial_begin("uid-1", "SIP/bob-001", "100");

        // Answer
        engine.channel_answered("uid-1");

        // Bridge
        engine.bridge_enter("uid-1", "bridge-1", "SIP/bob-001");
        engine.bridge_leave("uid-1", "bridge-1");

        // Hangup
        engine.channel_hangup("uid-1", 16, "Dial", "SIP/bob,30");
        assert_eq!(engine.active_count(), 0);
    }

    #[test]
    fn test_cdr_state_transitions() {
        let engine = CdrEngine::new();

        engine.channel_created("uid-1", "SIP/test-001", "", "", "default");

        // Check initial state
        {
            let active = engine.active_cdrs.read();
            let entry = active.get("uid-1").unwrap();
            assert_eq!(entry.state, CdrState::Single);
        }

        // Dial transitions to Dial state
        engine.dial_begin("uid-1", "SIP/dest-001", "100");
        {
            let active = engine.active_cdrs.read();
            let entry = active.get("uid-1").unwrap();
            assert_eq!(entry.state, CdrState::Dial);
        }

        // Bridge transitions to Bridge state
        engine.bridge_enter("uid-1", "bridge-1", "SIP/dest-001");
        {
            let active = engine.active_cdrs.read();
            let entry = active.get("uid-1").unwrap();
            assert_eq!(entry.state, CdrState::Bridge);
        }
    }

    #[test]
    fn test_cdr_variables() {
        let engine = CdrEngine::new();
        engine.channel_created("uid-1", "test", "", "", "default");

        engine.set_variable("uid-1", "CUSTOM_FIELD", "custom_value");
        assert_eq!(
            engine.get_variable("uid-1", "CUSTOM_FIELD"),
            Some("custom_value".to_string())
        );
    }

    #[test]
    fn test_disabled_engine() {
        let config = CdrConfig {
            enabled: false,
            ..Default::default()
        };
        let engine = CdrEngine::with_config(config);
        engine.channel_created("uid-1", "test", "", "", "default");
        // Should not track when disabled
        assert_eq!(engine.active_count(), 0);
    }
}
