//! Calendar integration framework.
//!
//! Port of `res/res_calendar.c`, `res_calendar_caldav.c`, and
//! `res_calendar_icalendar.c`. Provides a pluggable calendar framework
//! with event fetching, busy detection, and dialplan functions
//! CALENDAR_BUSY(), CALENDAR_EVENT(), CALENDAR_QUERY().

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum CalendarError {
    #[error("calendar not found: {0}")]
    CalendarNotFound(String),
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("calendar error: {0}")]
    FetchError(String),
    #[error("calendar error: {0}")]
    Other(String),
}

pub type CalendarResult<T> = Result<T, CalendarError>;

// ---------------------------------------------------------------------------
// Busy state
// ---------------------------------------------------------------------------

/// Calendar busy state for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyState {
    Free = 0,
    Tentative = 1,
    Busy = 2,
}

impl fmt::Display for BusyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Free => write!(f, "0"),
            Self::Tentative => write!(f, "1"),
            Self::Busy => write!(f, "2"),
        }
    }
}

// ---------------------------------------------------------------------------
// Calendar event
// ---------------------------------------------------------------------------

/// A calendar event (mirrors `ast_calendar_event`).
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// Unique ID of the event.
    pub uid: String,
    /// Event summary/subject.
    pub summary: String,
    /// Event description.
    pub description: String,
    /// Organizer name or email.
    pub organizer: String,
    /// Location.
    pub location: String,
    /// Categories (comma-separated).
    pub categories: String,
    /// Priority (1-9, 0 = undefined).
    pub priority: i32,
    /// Start time (UNIX timestamp).
    pub start: u64,
    /// End time (UNIX timestamp).
    pub end: u64,
    /// Busy state during this event.
    pub busy_state: BusyState,
    /// Associated calendar name.
    pub calendar_name: String,
}

impl CalendarEvent {
    pub fn new(uid: &str) -> Self {
        Self {
            uid: uid.to_string(),
            summary: String::new(),
            description: String::new(),
            organizer: String::new(),
            location: String::new(),
            categories: String::new(),
            priority: 0,
            start: 0,
            end: 0,
            busy_state: BusyState::Busy,
            calendar_name: String::new(),
        }
    }

    /// Check if this event is currently active.
    pub fn is_active_at(&self, timestamp: u64) -> bool {
        timestamp >= self.start && timestamp < self.end
    }

    /// Check if this event is currently active (using system time).
    pub fn is_active_now(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.is_active_at(now)
    }

    /// Get a field value by name (for CALENDAR_EVENT() function).
    pub fn get_field(&self, field: &str) -> Option<String> {
        match field.to_lowercase().as_str() {
            "summary" => Some(self.summary.clone()),
            "description" => Some(self.description.clone()),
            "organizer" => Some(self.organizer.clone()),
            "location" => Some(self.location.clone()),
            "categories" => Some(self.categories.clone()),
            "priority" => Some(self.priority.to_string()),
            "calendar" => Some(self.calendar_name.clone()),
            "uid" => Some(self.uid.clone()),
            "start" => Some(self.start.to_string()),
            "end" => Some(self.end.to_string()),
            "busystate" => Some(self.busy_state.to_string()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Calendar provider trait
// ---------------------------------------------------------------------------

/// Trait for calendar technology providers (CalDAV, iCalendar, Exchange, etc.).
pub trait CalendarProvider: Send + Sync + fmt::Debug {
    /// Provider name (e.g., "caldav", "icalendar", "ews").
    fn name(&self) -> &str;

    /// Fetch events for the given time range.
    fn fetch_events(
        &self,
        config: &CalendarConfig,
        start: u64,
        end: u64,
    ) -> CalendarResult<Vec<CalendarEvent>>;
}

// ---------------------------------------------------------------------------
// Calendar configuration
// ---------------------------------------------------------------------------

/// Configuration for a single calendar (from `calendar.conf`).
#[derive(Debug, Clone)]
pub struct CalendarConfig {
    /// Calendar name (section name in calendar.conf).
    pub name: String,
    /// Provider type (e.g., "caldav", "icalendar", "ews").
    pub provider_type: String,
    /// URL for the calendar source.
    pub url: String,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication.
    pub password: String,
    /// Refresh interval in seconds.
    pub refresh: u64,
    /// How far in the future to fetch events (seconds).
    pub timeframe: u64,
    /// Whether to set device state based on busy status.
    pub autoreminder: i32,
    /// Channel to dial for event reminders.
    pub channel: String,
    /// Context to dial for event reminders.
    pub context: String,
    /// Extension to dial for event reminders.
    pub extension: String,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            provider_type: String::new(),
            url: String::new(),
            username: String::new(),
            password: String::new(),
            refresh: 3600,
            timeframe: 3600,
            autoreminder: 0,
            channel: String::new(),
            context: String::new(),
            extension: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Calendar instance
// ---------------------------------------------------------------------------

/// A loaded calendar with cached events.
#[derive(Debug)]
pub struct Calendar {
    pub config: CalendarConfig,
    events: RwLock<Vec<CalendarEvent>>,
    last_refresh: RwLock<u64>,
}

impl Calendar {
    pub fn new(config: CalendarConfig) -> Self {
        Self {
            config,
            events: RwLock::new(Vec::new()),
            last_refresh: RwLock::new(0),
        }
    }

    /// Update the cached events.
    pub fn set_events(&self, events: Vec<CalendarEvent>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        *self.events.write() = events;
        *self.last_refresh.write() = now;
    }

    /// Check if the calendar is busy right now (CALENDAR_BUSY function).
    pub fn is_busy(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.events
            .read()
            .iter()
            .any(|e| e.is_active_at(now) && e.busy_state == BusyState::Busy)
    }

    /// Check busy state at a specific time.
    pub fn is_busy_at(&self, timestamp: u64) -> bool {
        self.events
            .read()
            .iter()
            .any(|e| e.is_active_at(timestamp) && e.busy_state == BusyState::Busy)
    }

    /// Get events in a time range.
    pub fn query_events(&self, start: u64, end: u64) -> Vec<CalendarEvent> {
        self.events
            .read()
            .iter()
            .filter(|e| e.start < end && e.end > start)
            .cloned()
            .collect()
    }

    /// Get all cached events.
    pub fn events(&self) -> Vec<CalendarEvent> {
        self.events.read().clone()
    }
}

// ---------------------------------------------------------------------------
// Calendar manager
// ---------------------------------------------------------------------------

/// Manager for all configured calendars.
#[derive(Debug)]
pub struct CalendarManager {
    calendars: RwLock<HashMap<String, Calendar>>,
    providers: RwLock<HashMap<String, Box<dyn CalendarProvider>>>,
}

impl CalendarManager {
    pub fn new() -> Self {
        Self {
            calendars: RwLock::new(HashMap::new()),
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a calendar provider.
    pub fn register_provider(&self, provider: Box<dyn CalendarProvider>) {
        let name = provider.name().to_string();
        info!(provider = %name, "Registered calendar provider");
        self.providers.write().insert(name, provider);
    }

    /// Add a calendar configuration and create the calendar.
    pub fn add_calendar(&self, config: CalendarConfig) {
        let name = config.name.clone();
        self.calendars
            .write()
            .insert(name, Calendar::new(config));
    }

    /// Check if a named calendar is busy (CALENDAR_BUSY function).
    pub fn calendar_busy(&self, name: &str) -> CalendarResult<bool> {
        self.calendars
            .read()
            .get(name)
            .map(|cal| cal.is_busy())
            .ok_or_else(|| CalendarError::CalendarNotFound(name.to_string()))
    }

    /// Get an event field from the currently active event on a calendar.
    pub fn calendar_event_field(
        &self,
        calendar_name: &str,
        field: &str,
    ) -> CalendarResult<Option<String>> {
        let calendars = self.calendars.read();
        let cal = calendars
            .get(calendar_name)
            .ok_or_else(|| CalendarError::CalendarNotFound(calendar_name.to_string()))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let events = cal.events.read();
        for event in events.iter() {
            if event.is_active_at(now) {
                return Ok(event.get_field(field));
            }
        }
        Ok(None)
    }

    /// List all calendar names.
    pub fn calendar_names(&self) -> Vec<String> {
        self.calendars.read().keys().cloned().collect()
    }
}

impl Default for CalendarManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calendar_event_fields() {
        let mut event = CalendarEvent::new("ev-001");
        event.summary = "Meeting".to_string();
        event.location = "Room 42".to_string();
        event.busy_state = BusyState::Busy;

        assert_eq!(event.get_field("summary"), Some("Meeting".to_string()));
        assert_eq!(event.get_field("location"), Some("Room 42".to_string()));
        assert_eq!(event.get_field("busystate"), Some("2".to_string()));
        assert_eq!(event.get_field("nonexistent"), None);
    }

    #[test]
    fn test_event_active() {
        let mut event = CalendarEvent::new("ev-001");
        event.start = 100;
        event.end = 200;

        assert!(event.is_active_at(150));
        assert!(!event.is_active_at(50));
        assert!(!event.is_active_at(200));
    }

    #[test]
    fn test_calendar_busy() {
        let cal = Calendar::new(CalendarConfig {
            name: "test".to_string(),
            ..Default::default()
        });

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut event = CalendarEvent::new("ev-001");
        event.start = now - 100;
        event.end = now + 100;
        event.busy_state = BusyState::Busy;

        cal.set_events(vec![event]);
        assert!(cal.is_busy());
    }

    #[test]
    fn test_calendar_not_busy() {
        let cal = Calendar::new(CalendarConfig {
            name: "test".to_string(),
            ..Default::default()
        });
        assert!(!cal.is_busy());
    }

    #[test]
    fn test_calendar_manager() {
        let manager = CalendarManager::new();
        manager.add_calendar(CalendarConfig {
            name: "office".to_string(),
            provider_type: "caldav".to_string(),
            ..Default::default()
        });

        assert!(manager.calendar_busy("office").is_ok());
        assert!(manager.calendar_busy("missing").is_err());
    }

    #[test]
    fn test_busy_state() {
        assert_eq!(BusyState::Free.to_string(), "0");
        assert_eq!(BusyState::Tentative.to_string(), "1");
        assert_eq!(BusyState::Busy.to_string(), "2");
    }
}
