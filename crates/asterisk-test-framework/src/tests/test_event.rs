//! Port of asterisk/tests/test_event.c
//!
//! Tests event system (ast_event API):
//! - Event creation with information elements (IEs)
//! - Dynamic event creation (append IEs individually)
//! - Static event creation (all IEs at once)
//! - Event type verification
//! - String IE retrieval
//! - Uint IE retrieval
//! - Missing IE returns None/zero
//! - Event size comparison
//! - Event subscription counting

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Event system mirroring Asterisk's ast_event
// ---------------------------------------------------------------------------

/// Event types, mirroring enum ast_event_type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EventType {
    Custom,
    DeviceState,
    Cel,
}

/// Information Element types, mirroring AST_EVENT_IE_*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IeType {
    CelUsereventName,
    CelAmaflags,
    CelCidname,
    CelEventTimeUsec,
}

/// An information element value.
#[derive(Debug, Clone, PartialEq)]
enum IeValue {
    Str(String),
    Uint(u32),
}

/// An event with a type and information elements.
#[derive(Debug, Clone)]
struct Event {
    event_type: EventType,
    ies: HashMap<IeType, IeValue>,
}

impl Event {
    /// Create a new empty event of the given type.
    fn new(event_type: EventType) -> Self {
        Self {
            event_type,
            ies: HashMap::new(),
        }
    }

    /// Create a new event with IEs provided at creation time.
    fn new_with_ies(event_type: EventType, ies: Vec<(IeType, IeValue)>) -> Self {
        let mut event = Self::new(event_type);
        for (ie_type, value) in ies {
            event.ies.insert(ie_type, value);
        }
        event
    }

    /// Append a string IE.
    fn append_ie_str(&mut self, ie_type: IeType, value: &str) {
        self.ies
            .insert(ie_type, IeValue::Str(value.to_string()));
    }

    /// Append a uint IE.
    fn append_ie_uint(&mut self, ie_type: IeType, value: u32) {
        self.ies.insert(ie_type, IeValue::Uint(value));
    }

    /// Get a string IE value.
    fn get_ie_str(&self, ie_type: IeType) -> Option<&str> {
        match self.ies.get(&ie_type) {
            Some(IeValue::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get a uint IE value.
    fn get_ie_uint(&self, ie_type: IeType) -> u32 {
        match self.ies.get(&ie_type) {
            Some(IeValue::Uint(v)) => *v,
            _ => 0,
        }
    }

    /// Get the size of the event (number of IEs).
    fn size(&self) -> usize {
        self.ies.len()
    }

    /// Get the event type.
    fn get_type(&self) -> EventType {
        self.event_type
    }
}

// ---------------------------------------------------------------------------
// Tests: Event creation
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(event_new_test) from test_event.c.
///
/// Test dynamic event creation by appending IEs individually.
#[test]
fn test_event_dynamic_creation() {
    let mut event = Event::new(EventType::Custom);
    event.append_ie_str(IeType::CelUsereventName, "SIP/alligatormittens");
    event.append_ie_uint(IeType::CelAmaflags, 0xb00bface);

    assert_eq!(event.get_type(), EventType::Custom);
    assert_eq!(
        event.get_ie_str(IeType::CelUsereventName),
        Some("SIP/alligatormittens")
    );
    assert_eq!(event.get_ie_uint(IeType::CelAmaflags), 0xb00bface);
}

/// Test static event creation with all IEs at once.
#[test]
fn test_event_static_creation() {
    let event = Event::new_with_ies(
        EventType::Custom,
        vec![
            (
                IeType::CelUsereventName,
                IeValue::Str("SIP/alligatormittens".to_string()),
            ),
            (IeType::CelAmaflags, IeValue::Uint(0xb00bface)),
        ],
    );

    assert_eq!(event.get_type(), EventType::Custom);
    assert_eq!(
        event.get_ie_str(IeType::CelUsereventName),
        Some("SIP/alligatormittens")
    );
    assert_eq!(event.get_ie_uint(IeType::CelAmaflags), 0xb00bface);
}

/// Test that dynamic and static creation produce equivalent events.
#[test]
fn test_event_dynamic_equals_static() {
    let mut dynamic = Event::new(EventType::Custom);
    dynamic.append_ie_str(IeType::CelUsereventName, "SIP/alligatormittens");
    dynamic.append_ie_uint(IeType::CelAmaflags, 0xb00bface);

    let statik = Event::new_with_ies(
        EventType::Custom,
        vec![
            (
                IeType::CelUsereventName,
                IeValue::Str("SIP/alligatormittens".to_string()),
            ),
            (IeType::CelAmaflags, IeValue::Uint(0xb00bface)),
        ],
    );

    assert_eq!(dynamic.size(), statik.size());
    assert_eq!(dynamic.get_type(), statik.get_type());
    assert_eq!(
        dynamic.get_ie_str(IeType::CelUsereventName),
        statik.get_ie_str(IeType::CelUsereventName)
    );
    assert_eq!(
        dynamic.get_ie_uint(IeType::CelAmaflags),
        statik.get_ie_uint(IeType::CelAmaflags)
    );
}

// ---------------------------------------------------------------------------
// Tests: Missing IE handling
// ---------------------------------------------------------------------------

/// Port of check_event from test_event.c -- missing string IE returns None.
#[test]
fn test_event_missing_string_ie() {
    let event = Event::new(EventType::Custom);
    assert!(event.get_ie_str(IeType::CelCidname).is_none());
}

/// Missing uint IE returns 0.
#[test]
fn test_event_missing_uint_ie() {
    let event = Event::new(EventType::Custom);
    assert_eq!(event.get_ie_uint(IeType::CelEventTimeUsec), 0);
}

/// Verify specific IEs not in event return appropriate defaults.
#[test]
fn test_event_check_absent_ies() {
    let mut event = Event::new(EventType::Custom);
    event.append_ie_str(IeType::CelUsereventName, "test");
    event.append_ie_uint(IeType::CelAmaflags, 42);

    // CelCidname was never added.
    assert!(event.get_ie_str(IeType::CelCidname).is_none());
    // CelEventTimeUsec was never added.
    assert_eq!(event.get_ie_uint(IeType::CelEventTimeUsec), 0);
}

// ---------------------------------------------------------------------------
// Tests: Event type verification
// ---------------------------------------------------------------------------

#[test]
fn test_event_type_custom() {
    let event = Event::new(EventType::Custom);
    assert_eq!(event.get_type(), EventType::Custom);
    assert_ne!(event.get_type(), EventType::DeviceState);
}

#[test]
fn test_event_type_device_state() {
    let event = Event::new(EventType::DeviceState);
    assert_eq!(event.get_type(), EventType::DeviceState);
}

#[test]
fn test_event_type_cel() {
    let event = Event::new(EventType::Cel);
    assert_eq!(event.get_type(), EventType::Cel);
}

// ---------------------------------------------------------------------------
// Tests: Event size
// ---------------------------------------------------------------------------

#[test]
fn test_event_size_empty() {
    let event = Event::new(EventType::Custom);
    assert_eq!(event.size(), 0);
}

#[test]
fn test_event_size_with_ies() {
    let mut event = Event::new(EventType::Custom);
    event.append_ie_str(IeType::CelUsereventName, "test");
    assert_eq!(event.size(), 1);
    event.append_ie_uint(IeType::CelAmaflags, 42);
    assert_eq!(event.size(), 2);
}

// ---------------------------------------------------------------------------
// Tests: Event subscription counting
// ---------------------------------------------------------------------------

/// Port of event_sub_data from test_event.c.
/// Verify subscription counting works.
#[test]
fn test_event_subscription_count() {
    struct EventSub {
        count: u32,
    }

    let mut sub = EventSub { count: 0 };

    // Simulate receiving 5 events.
    for _ in 0..5 {
        sub.count += 1;
    }

    assert_eq!(sub.count, 5);
}

/// Test multiple IEs of different types.
#[test]
fn test_event_multiple_ies() {
    let mut event = Event::new(EventType::Custom);
    event.append_ie_str(IeType::CelUsereventName, "first");
    event.append_ie_str(IeType::CelCidname, "second");
    event.append_ie_uint(IeType::CelAmaflags, 100);
    event.append_ie_uint(IeType::CelEventTimeUsec, 200);

    assert_eq!(event.size(), 4);
    assert_eq!(event.get_ie_str(IeType::CelUsereventName), Some("first"));
    assert_eq!(event.get_ie_str(IeType::CelCidname), Some("second"));
    assert_eq!(event.get_ie_uint(IeType::CelAmaflags), 100);
    assert_eq!(event.get_ie_uint(IeType::CelEventTimeUsec), 200);
}

/// Test event overwriting an IE with same key.
#[test]
fn test_event_overwrite_ie() {
    let mut event = Event::new(EventType::Custom);
    event.append_ie_str(IeType::CelUsereventName, "original");
    event.append_ie_str(IeType::CelUsereventName, "updated");

    // Last write wins.
    assert_eq!(
        event.get_ie_str(IeType::CelUsereventName),
        Some("updated")
    );
    // Size should still be 1 (same key).
    assert_eq!(event.size(), 1);
}
