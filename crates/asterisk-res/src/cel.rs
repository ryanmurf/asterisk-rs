//! Channel Event Logging (CEL).
//!
//! Port of `main/cel.c`, `cel/cel_custom.c`, and `cel/cel_manager.c`.
//! Provides a framework for logging significant channel events (start, end,
//! answer, bridge, transfer, etc.) to pluggable backends (custom CSV, AMI
//! manager events, databases, etc.).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum CelError {
    #[error("CEL backend not found: {0}")]
    BackendNotFound(String),
    #[error("CEL backend already registered: {0}")]
    BackendAlreadyRegistered(String),
    #[error("CEL error: {0}")]
    Other(String),
}

pub type CelResult<T> = Result<T, CelError>;

// ---------------------------------------------------------------------------
// CEL event types (from include/asterisk/cel.h)
// ---------------------------------------------------------------------------

/// Channel Event Logging event types.
///
/// Mirrors the `enum ast_cel_event_type` from the C header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum CelEventType {
    /// Channel created.
    ChannelStart = 1,
    /// Channel destroyed.
    ChannelEnd = 2,
    /// Channel hung up.
    Hangup = 3,
    /// Channel answered.
    Answer = 4,
    /// Application started.
    AppStart = 5,
    /// Application ended.
    AppEnd = 6,
    /// Channel entered a bridge.
    BridgeEnter = 7,
    /// Channel exited a bridge.
    BridgeExit = 8,
    /// Call parked.
    ParkStart = 9,
    /// Parked call retrieved.
    ParkEnd = 10,
    /// Blind transfer.
    BlindTransfer = 11,
    /// Attended transfer.
    AttendedTransfer = 12,
    /// User-defined event.
    UserDefined = 13,
    /// Last channel with this linked ID ended.
    LinkedIdEnd = 14,
    /// Call pickup.
    Pickup = 15,
    /// Call forwarded.
    Forward = 16,
    /// Local channel optimization completed.
    LocalOptimize = 17,
    /// Local channel optimization began.
    LocalOptimizeBegin = 18,
    /// Media stream began (e.g., MOH).
    StreamBegin = 19,
    /// Media stream ended.
    StreamEnd = 20,
    /// DTMF digit.
    Dtmf = 21,
}

impl CelEventType {
    /// Parse from the event name string (as used in cel.conf).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_uppercase().as_str() {
            "CHAN_START" | "CHANNEL_START" => Some(Self::ChannelStart),
            "CHAN_END" | "CHANNEL_END" => Some(Self::ChannelEnd),
            "HANGUP" => Some(Self::Hangup),
            "ANSWER" => Some(Self::Answer),
            "APP_START" => Some(Self::AppStart),
            "APP_END" => Some(Self::AppEnd),
            "BRIDGE_ENTER" => Some(Self::BridgeEnter),
            "BRIDGE_EXIT" => Some(Self::BridgeExit),
            "PARK_START" => Some(Self::ParkStart),
            "PARK_END" => Some(Self::ParkEnd),
            "BLINDTRANSFER" => Some(Self::BlindTransfer),
            "ATTENDEDTRANSFER" => Some(Self::AttendedTransfer),
            "USER_DEFINED" => Some(Self::UserDefined),
            "LINKEDID_END" => Some(Self::LinkedIdEnd),
            "PICKUP" => Some(Self::Pickup),
            "FORWARD" => Some(Self::Forward),
            "LOCAL_OPTIMIZE" => Some(Self::LocalOptimize),
            "LOCAL_OPTIMIZE_BEGIN" => Some(Self::LocalOptimizeBegin),
            "STREAM_BEGIN" => Some(Self::StreamBegin),
            "STREAM_END" => Some(Self::StreamEnd),
            "DTMF" => Some(Self::Dtmf),
            _ => None,
        }
    }

    /// Return the event name as a string.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ChannelStart => "CHAN_START",
            Self::ChannelEnd => "CHAN_END",
            Self::Hangup => "HANGUP",
            Self::Answer => "ANSWER",
            Self::AppStart => "APP_START",
            Self::AppEnd => "APP_END",
            Self::BridgeEnter => "BRIDGE_ENTER",
            Self::BridgeExit => "BRIDGE_EXIT",
            Self::ParkStart => "PARK_START",
            Self::ParkEnd => "PARK_END",
            Self::BlindTransfer => "BLINDTRANSFER",
            Self::AttendedTransfer => "ATTENDEDTRANSFER",
            Self::UserDefined => "USER_DEFINED",
            Self::LinkedIdEnd => "LINKEDID_END",
            Self::Pickup => "PICKUP",
            Self::Forward => "FORWARD",
            Self::LocalOptimize => "LOCAL_OPTIMIZE",
            Self::LocalOptimizeBegin => "LOCAL_OPTIMIZE_BEGIN",
            Self::StreamBegin => "STREAM_BEGIN",
            Self::StreamEnd => "STREAM_END",
            Self::Dtmf => "DTMF",
        }
    }

    /// All defined event types.
    pub fn all() -> &'static [CelEventType] {
        &[
            Self::ChannelStart,
            Self::ChannelEnd,
            Self::Hangup,
            Self::Answer,
            Self::AppStart,
            Self::AppEnd,
            Self::BridgeEnter,
            Self::BridgeExit,
            Self::ParkStart,
            Self::ParkEnd,
            Self::BlindTransfer,
            Self::AttendedTransfer,
            Self::UserDefined,
            Self::LinkedIdEnd,
            Self::Pickup,
            Self::Forward,
            Self::LocalOptimize,
            Self::LocalOptimizeBegin,
            Self::StreamBegin,
            Self::StreamEnd,
            Self::Dtmf,
        ]
    }
}

impl fmt::Display for CelEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// CEL event
// ---------------------------------------------------------------------------

/// A Channel Event Logging event record.
///
/// This is the core data structure written to CEL backends.
#[derive(Debug, Clone)]
pub struct CelEvent {
    /// Event type.
    pub event_type: CelEventType,
    /// Timestamp (seconds since UNIX epoch).
    pub timestamp: u64,
    /// Timestamp microseconds component.
    pub timestamp_usec: u32,
    /// Channel name.
    pub channel_name: String,
    /// Channel unique ID.
    pub unique_id: String,
    /// Linked ID (groups related channels).
    pub linked_id: String,
    /// Caller ID number.
    pub caller_id_num: String,
    /// Caller ID name.
    pub caller_id_name: String,
    /// Caller ID ANI.
    pub caller_id_ani: String,
    /// Caller ID RDNIS.
    pub caller_id_rdnis: String,
    /// Caller ID DNID.
    pub caller_id_dnid: String,
    /// Dialplan context.
    pub context: String,
    /// Dialplan extension.
    pub extension: String,
    /// Dialplan priority.
    pub priority: i32,
    /// Application name (for APP_START/APP_END).
    pub application: String,
    /// Application data.
    pub application_data: String,
    /// Account code.
    pub account_code: String,
    /// Peer account.
    pub peer_account: String,
    /// Bridge ID (for BRIDGE_ENTER/BRIDGE_EXIT).
    pub bridge_id: String,
    /// Extra data (JSON string with event-specific details).
    pub extra: String,
    /// User-defined event name (for USER_DEFINED type).
    pub user_defined_name: String,
    /// Peer channel name (for transfers).
    pub peer: String,
}

impl CelEvent {
    /// Create a new CEL event with the current timestamp.
    pub fn new(event_type: CelEventType, channel_name: &str, unique_id: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        Self {
            event_type,
            timestamp: now.as_secs(),
            timestamp_usec: now.subsec_micros(),
            channel_name: channel_name.to_string(),
            unique_id: unique_id.to_string(),
            linked_id: String::new(),
            caller_id_num: String::new(),
            caller_id_name: String::new(),
            caller_id_ani: String::new(),
            caller_id_rdnis: String::new(),
            caller_id_dnid: String::new(),
            context: String::new(),
            extension: String::new(),
            priority: 1,
            application: String::new(),
            application_data: String::new(),
            account_code: String::new(),
            peer_account: String::new(),
            bridge_id: String::new(),
            extra: String::new(),
            user_defined_name: String::new(),
            peer: String::new(),
        }
    }

    /// Builder: set the linked ID.
    pub fn with_linked_id(mut self, id: &str) -> Self {
        self.linked_id = id.to_string();
        self
    }

    /// Builder: set caller ID.
    pub fn with_caller_id(mut self, num: &str, name: &str) -> Self {
        self.caller_id_num = num.to_string();
        self.caller_id_name = name.to_string();
        self
    }

    /// Builder: set dialplan location.
    pub fn with_dialplan(mut self, context: &str, extension: &str, priority: i32) -> Self {
        self.context = context.to_string();
        self.extension = extension.to_string();
        self.priority = priority;
        self
    }

    /// Builder: set application info.
    pub fn with_application(mut self, app: &str, data: &str) -> Self {
        self.application = app.to_string();
        self.application_data = data.to_string();
        self
    }

    /// Builder: set extra data.
    pub fn with_extra(mut self, extra: &str) -> Self {
        self.extra = extra.to_string();
        self
    }

    /// Format as a CSV line (for cel_custom backend).
    pub fn to_csv(&self, separator: char) -> String {
        format!(
            "\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"{}\"",
            self.event_type.name(),
            separator,
            self.timestamp,
            separator,
            self.channel_name,
            separator,
            self.unique_id,
            separator,
            self.linked_id,
            separator,
            self.caller_id_num,
            separator,
            self.caller_id_name,
            separator,
            self.priority,
            separator,
            self.context,
            separator,
            self.extension,
            separator,
            self.application,
        )
    }
}

// ---------------------------------------------------------------------------
// CEL backend trait
// ---------------------------------------------------------------------------

/// Trait for CEL event backends (custom CSV, AMI, database, etc.).
///
/// Mirrors the `cel_backend` structure from the C source.
pub trait CelBackend: Send + Sync + fmt::Debug {
    /// Backend name.
    fn name(&self) -> &str;

    /// Write a CEL event to this backend.
    fn write(&self, event: &CelEvent) -> CelResult<()>;
}

// ---------------------------------------------------------------------------
// Custom CSV backend
// ---------------------------------------------------------------------------

/// A CEL backend that writes events to a CSV-formatted log.
///
/// Port of `cel/cel_custom.c`.
#[derive(Debug)]
pub struct CustomCelBackend {
    /// Backend name.
    name: String,
    /// Field separator character.
    pub separator: char,
    /// Collected events (in a real implementation, this would write to a file).
    events: RwLock<Vec<String>>,
}

impl CustomCelBackend {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            separator: ',',
            events: RwLock::new(Vec::new()),
        }
    }

    /// Get all logged events as CSV lines.
    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }

    /// Clear all logged events.
    pub fn clear(&self) {
        self.events.write().clear();
    }
}

impl CelBackend for CustomCelBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let csv_line = event.to_csv(self.separator);
        debug!(backend = %self.name, event = %event.event_type, "CEL custom write");
        self.events.write().push(csv_line);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Manager (AMI) backend
// ---------------------------------------------------------------------------

/// A CEL backend that formats events as AMI manager events.
///
/// Port of `cel/cel_manager.c`.
#[derive(Debug)]
pub struct ManagerCelBackend {
    name: String,
    /// Collected AMI event strings.
    events: RwLock<Vec<String>>,
}

impl ManagerCelBackend {
    pub fn new() -> Self {
        Self {
            name: "manager".to_string(),
            events: RwLock::new(Vec::new()),
        }
    }

    /// Get all formatted AMI events.
    pub fn ami_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for ManagerCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for ManagerCelBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let ami_event = format!(
            "Event: CEL\r\n\
             EventName: {}\r\n\
             AccountCode: {}\r\n\
             CallerIDnum: {}\r\n\
             CallerIDname: {}\r\n\
             CallerIDani: {}\r\n\
             CallerIDrdnis: {}\r\n\
             CallerIDdnid: {}\r\n\
             Exten: {}\r\n\
             Context: {}\r\n\
             Channel: {}\r\n\
             Application: {}\r\n\
             AppData: {}\r\n\
             EventTime: {}.{:06}\r\n\
             UniqueID: {}\r\n\
             LinkedID: {}\r\n\
             Peer: {}\r\n\
             Extra: {}\r\n\
             \r\n",
            event.event_type.name(),
            event.account_code,
            event.caller_id_num,
            event.caller_id_name,
            event.caller_id_ani,
            event.caller_id_rdnis,
            event.caller_id_dnid,
            event.extension,
            event.context,
            event.channel_name,
            event.application,
            event.application_data,
            event.timestamp,
            event.timestamp_usec,
            event.unique_id,
            event.linked_id,
            event.peer,
            event.extra,
        );

        debug!(event = %event.event_type, "CEL manager write");
        self.events.write().push(ami_event);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CEL event tracking configuration
// ---------------------------------------------------------------------------

/// Configuration for the CEL engine, specifying which events to track.
#[derive(Debug, Clone)]
pub struct CelConfig {
    /// Whether CEL is enabled.
    pub enabled: bool,
    /// Set of event types to track.
    pub tracked_events: Vec<CelEventType>,
    /// Applications to track (for APP_START/APP_END).
    pub tracked_apps: Vec<String>,
    /// Date format string.
    pub date_format: String,
}

impl Default for CelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tracked_events: Vec::new(),
            tracked_apps: Vec::new(),
            date_format: "%Y-%m-%d %H:%M:%S".to_string(),
        }
    }
}

impl CelConfig {
    /// Create a config that tracks all events.
    pub fn track_all() -> Self {
        Self {
            enabled: true,
            tracked_events: CelEventType::all().to_vec(),
            tracked_apps: Vec::new(),
            date_format: "%Y-%m-%d %H:%M:%S".to_string(),
        }
    }

    /// Check whether a particular event type is being tracked.
    pub fn is_tracked(&self, event_type: CelEventType) -> bool {
        self.enabled && self.tracked_events.contains(&event_type)
    }

    /// Parse the `events` config value (comma-separated event names).
    pub fn parse_events(events_str: &str) -> Vec<CelEventType> {
        let mut events = Vec::new();
        for name in events_str.split(',') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("ALL") {
                return CelEventType::all().to_vec();
            }
            if let Some(evt) = CelEventType::from_name(name) {
                events.push(evt);
            } else if !name.is_empty() {
                warn!(event = name, "Unknown CEL event type in configuration");
            }
        }
        events
    }
}

// ---------------------------------------------------------------------------
// CEL engine
// ---------------------------------------------------------------------------

/// The CEL engine that distributes events to registered backends.
pub struct CelEngine {
    /// Configuration.
    pub config: RwLock<CelConfig>,
    /// Registered backends.
    backends: RwLock<HashMap<String, Arc<dyn CelBackend>>>,
    /// Total events processed.
    events_processed: RwLock<u64>,
}

impl CelEngine {
    /// Create a new CEL engine.
    pub fn new(config: CelConfig) -> Self {
        Self {
            config: RwLock::new(config),
            backends: RwLock::new(HashMap::new()),
            events_processed: RwLock::new(0),
        }
    }

    /// Register a CEL backend.
    pub fn register_backend(&self, backend: Arc<dyn CelBackend>) -> CelResult<()> {
        let name = backend.name().to_string();
        let mut backends = self.backends.write();
        if backends.contains_key(&name) {
            return Err(CelError::BackendAlreadyRegistered(name));
        }
        info!(backend = %name, "Registered CEL backend");
        backends.insert(name, backend);
        Ok(())
    }

    /// Unregister a CEL backend.
    pub fn unregister_backend(&self, name: &str) -> CelResult<()> {
        self.backends.write().remove(name).ok_or_else(|| {
            CelError::BackendNotFound(name.to_string())
        })?;
        info!(backend = name, "Unregistered CEL backend");
        Ok(())
    }

    /// Report an event to all registered backends.
    pub fn report(&self, event: &CelEvent) {
        let config = self.config.read();
        if !config.is_tracked(event.event_type) {
            return;
        }

        // For APP_START/APP_END, check if the app is tracked.
        if matches!(event.event_type, CelEventType::AppStart | CelEventType::AppEnd) {
            if !config.tracked_apps.is_empty() {
                let app_lower = event.application.to_lowercase();
                if !config
                    .tracked_apps
                    .iter()
                    .any(|a| a.to_lowercase() == app_lower)
                {
                    return;
                }
            }
        }
        drop(config);

        let backends = self.backends.read();
        for backend in backends.values() {
            if let Err(e) = backend.write(event) {
                warn!(
                    backend = %backend.name(),
                    error = %e,
                    "Failed to write CEL event"
                );
            }
        }

        *self.events_processed.write() += 1;
    }

    /// Get the total number of events processed.
    pub fn events_processed(&self) -> u64 {
        *self.events_processed.read()
    }

    /// List registered backend names.
    pub fn backend_names(&self) -> Vec<String> {
        self.backends.read().keys().cloned().collect()
    }

    /// Update the configuration.
    pub fn set_config(&self, config: CelConfig) {
        *self.config.write() = config;
    }
}

impl fmt::Debug for CelEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CelEngine")
            .field("enabled", &self.config.read().enabled)
            .field("backends", &self.backends.read().len())
            .field("events_processed", &*self.events_processed.read())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cel_event_type_from_name() {
        assert_eq!(CelEventType::from_name("CHAN_START"), Some(CelEventType::ChannelStart));
        assert_eq!(CelEventType::from_name("HANGUP"), Some(CelEventType::Hangup));
        assert_eq!(CelEventType::from_name("BRIDGE_ENTER"), Some(CelEventType::BridgeEnter));
        assert_eq!(CelEventType::from_name("BLINDTRANSFER"), Some(CelEventType::BlindTransfer));
        assert_eq!(CelEventType::from_name("DTMF"), Some(CelEventType::Dtmf));
        assert_eq!(CelEventType::from_name("NONEXISTENT"), None);
    }

    #[test]
    fn test_cel_event_type_name_roundtrip() {
        for evt in CelEventType::all() {
            let name = evt.name();
            let parsed = CelEventType::from_name(name).unwrap();
            assert_eq!(*evt, parsed);
        }
    }

    #[test]
    fn test_cel_event_creation() {
        let event = CelEvent::new(CelEventType::ChannelStart, "SIP/alice-001", "12345.1")
            .with_caller_id("1001", "Alice")
            .with_dialplan("default", "100", 1)
            .with_linked_id("12345.1");

        assert_eq!(event.event_type, CelEventType::ChannelStart);
        assert_eq!(event.channel_name, "SIP/alice-001");
        assert_eq!(event.caller_id_num, "1001");
        assert_eq!(event.context, "default");
        assert!(event.timestamp > 0);
    }

    #[test]
    fn test_cel_event_csv() {
        let event = CelEvent::new(CelEventType::Answer, "SIP/bob-002", "12345.2")
            .with_caller_id("1002", "Bob");

        let csv = event.to_csv(',');
        assert!(csv.contains("ANSWER"));
        assert!(csv.contains("SIP/bob-002"));
        assert!(csv.contains("12345.2"));
    }

    #[test]
    fn test_cel_config_parse_events() {
        let events = CelConfig::parse_events("CHAN_START,CHAN_END,ANSWER,HANGUP");
        assert_eq!(events.len(), 4);
        assert!(events.contains(&CelEventType::ChannelStart));
        assert!(events.contains(&CelEventType::Hangup));
    }

    #[test]
    fn test_cel_config_parse_all() {
        let events = CelConfig::parse_events("ALL");
        assert_eq!(events.len(), CelEventType::all().len());
    }

    #[test]
    fn test_cel_config_tracking() {
        let config = CelConfig {
            enabled: true,
            tracked_events: vec![CelEventType::ChannelStart, CelEventType::Answer],
            ..Default::default()
        };
        assert!(config.is_tracked(CelEventType::ChannelStart));
        assert!(config.is_tracked(CelEventType::Answer));
        assert!(!config.is_tracked(CelEventType::Hangup));
    }

    #[test]
    fn test_cel_config_disabled() {
        let config = CelConfig::default();
        assert!(!config.enabled);
        assert!(!config.is_tracked(CelEventType::ChannelStart));
    }

    #[test]
    fn test_custom_cel_backend() {
        let backend = CustomCelBackend::new("test_custom");
        let event = CelEvent::new(CelEventType::Answer, "SIP/alice-001", "u1");
        backend.write(&event).unwrap();

        let logged = backend.logged_events();
        assert_eq!(logged.len(), 1);
        assert!(logged[0].contains("ANSWER"));
    }

    #[test]
    fn test_manager_cel_backend() {
        let backend = ManagerCelBackend::new();
        let event = CelEvent::new(CelEventType::BridgeEnter, "SIP/bob-002", "u2")
            .with_caller_id("1002", "Bob");
        backend.write(&event).unwrap();

        let ami_events = backend.ami_events();
        assert_eq!(ami_events.len(), 1);
        assert!(ami_events[0].contains("Event: CEL"));
        assert!(ami_events[0].contains("EventName: BRIDGE_ENTER"));
        assert!(ami_events[0].contains("CallerIDnum: 1002"));
    }

    #[test]
    fn test_cel_engine_basic() {
        let config = CelConfig::track_all();
        let engine = CelEngine::new(config);

        let custom = Arc::new(CustomCelBackend::new("custom"));
        let manager = Arc::new(ManagerCelBackend::new());

        engine.register_backend(custom.clone()).unwrap();
        engine.register_backend(manager.clone()).unwrap();

        assert_eq!(engine.backend_names().len(), 2);

        let event = CelEvent::new(CelEventType::ChannelStart, "SIP/alice-001", "u1");
        engine.report(&event);

        assert_eq!(engine.events_processed(), 1);
        assert_eq!(custom.logged_events().len(), 1);
        assert_eq!(manager.ami_events().len(), 1);
    }

    #[test]
    fn test_cel_engine_filtering() {
        let config = CelConfig {
            enabled: true,
            tracked_events: vec![CelEventType::Answer],
            ..Default::default()
        };
        let engine = CelEngine::new(config);

        let custom = Arc::new(CustomCelBackend::new("custom"));
        engine.register_backend(custom.clone()).unwrap();

        // This should be filtered out (not in tracked events).
        engine.report(&CelEvent::new(CelEventType::ChannelStart, "c", "u"));
        assert_eq!(engine.events_processed(), 0);

        // This should be logged.
        engine.report(&CelEvent::new(CelEventType::Answer, "c", "u"));
        assert_eq!(engine.events_processed(), 1);
        assert_eq!(custom.logged_events().len(), 1);
    }

    #[test]
    fn test_cel_engine_app_filtering() {
        let config = CelConfig {
            enabled: true,
            tracked_events: vec![CelEventType::AppStart],
            tracked_apps: vec!["Dial".to_string(), "Queue".to_string()],
            ..Default::default()
        };
        let engine = CelEngine::new(config);

        let custom = Arc::new(CustomCelBackend::new("custom"));
        engine.register_backend(custom.clone()).unwrap();

        // "Playback" not in tracked apps.
        let evt1 = CelEvent::new(CelEventType::AppStart, "c", "u")
            .with_application("Playback", "hello");
        engine.report(&evt1);
        assert_eq!(engine.events_processed(), 0);

        // "Dial" is tracked.
        let evt2 = CelEvent::new(CelEventType::AppStart, "c", "u")
            .with_application("Dial", "SIP/bob");
        engine.report(&evt2);
        assert_eq!(engine.events_processed(), 1);
    }

    #[test]
    fn test_cel_engine_duplicate_backend() {
        let engine = CelEngine::new(CelConfig::default());
        let b1 = Arc::new(CustomCelBackend::new("same_name"));
        let b2 = Arc::new(CustomCelBackend::new("same_name"));

        engine.register_backend(b1).unwrap();
        let result = engine.register_backend(b2);
        assert!(matches!(result, Err(CelError::BackendAlreadyRegistered(_))));
    }
}
