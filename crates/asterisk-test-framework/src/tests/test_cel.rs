//! Port of asterisk/tests/test_cel.c
//!
//! Tests the Channel Event Logging (CEL) framework. The C tests simulate
//! channel operations and verify that the correct CEL events are generated
//! in the correct order.
//!
//! Key scenarios:
//! - Channel events: CHANNEL_START, CHANNEL_END, ANSWER, HANGUP
//! - Bridge events: BRIDGE_ENTER, BRIDGE_EXIT
//! - Park events: PARK_START, PARK_END
//! - Transfer events: BLIND_TRANSFER, ATTENDED_TRANSFER
//! - Event ordering: verify correct chronological order
//! - LinkedID: proper linkedid propagation
//! - Single party: create channel, hangup
//! - Two party bridge: create two channels, bridge, hangup
//! - Blind transfer: A->B, B transfers to C
//! - Attended transfer: A->B, B->C, transfer

use asterisk_res::cel::{
    CelBackend, CelConfig, CelEngine, CelError, CelEvent, CelEventType, CustomCelBackend,
};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Mock CEL backend for collecting events in tests
// ---------------------------------------------------------------------------

/// Mock backend that collects CEL events for verification.
#[derive(Debug)]
#[allow(dead_code)]
struct MockCelBackend {
    events: parking_lot::Mutex<Vec<CelEvent>>,
}

#[allow(dead_code)]
impl MockCelBackend {
    fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.events.lock().len()
    }

    fn events(&self) -> Vec<CelEvent> {
        self.events.lock().clone()
    }

    fn last(&self) -> Option<CelEvent> {
        self.events.lock().last().cloned()
    }

    fn event_types(&self) -> Vec<CelEventType> {
        self.events.lock().iter().map(|e| e.event_type).collect()
    }

    fn clear(&self) {
        self.events.lock().clear();
    }
}

impl CelBackend for MockCelBackend {
    fn name(&self) -> &str {
        "CEL Test Logging"
    }

    fn write(&self, event: &CelEvent) -> Result<(), CelError> {
        self.events.lock().push(event.clone());
        Ok(())
    }
}

/// Helper: create a CEL engine configured to track all events.
fn setup_cel() -> (CelEngine, Arc<MockCelBackend>) {
    let config = CelConfig::track_all();
    let engine = CelEngine::new(config);
    let backend = Arc::new(MockCelBackend::new());
    engine.register_backend(backend.clone()).unwrap();
    (engine, backend)
}

// ---------------------------------------------------------------------------
// Event type tests
// ---------------------------------------------------------------------------

/// Port of CEL event type parsing and naming.
#[test]
fn cel_event_type_from_name() {
    assert_eq!(CelEventType::from_name("CHAN_START"), Some(CelEventType::ChannelStart));
    assert_eq!(CelEventType::from_name("CHANNEL_START"), Some(CelEventType::ChannelStart));
    assert_eq!(CelEventType::from_name("CHAN_END"), Some(CelEventType::ChannelEnd));
    assert_eq!(CelEventType::from_name("CHANNEL_END"), Some(CelEventType::ChannelEnd));
    assert_eq!(CelEventType::from_name("HANGUP"), Some(CelEventType::Hangup));
    assert_eq!(CelEventType::from_name("ANSWER"), Some(CelEventType::Answer));
    assert_eq!(CelEventType::from_name("BRIDGE_ENTER"), Some(CelEventType::BridgeEnter));
    assert_eq!(CelEventType::from_name("BRIDGE_EXIT"), Some(CelEventType::BridgeExit));
    assert_eq!(CelEventType::from_name("PARK_START"), Some(CelEventType::ParkStart));
    assert_eq!(CelEventType::from_name("PARK_END"), Some(CelEventType::ParkEnd));
    assert_eq!(CelEventType::from_name("BLINDTRANSFER"), Some(CelEventType::BlindTransfer));
    assert_eq!(CelEventType::from_name("ATTENDEDTRANSFER"), Some(CelEventType::AttendedTransfer));
    assert_eq!(CelEventType::from_name("NONEXISTENT"), None);
}

/// Verify all event types round-trip through name -> parse.
#[test]
fn cel_event_type_roundtrip() {
    for evt in CelEventType::all() {
        let name = evt.name();
        let parsed = CelEventType::from_name(name);
        assert!(
            parsed.is_some(),
            "Failed to parse event type name: {}",
            name
        );
        assert_eq!(*evt, parsed.unwrap());
    }
}

/// Verify event type Display implementation.
#[test]
fn cel_event_type_display() {
    assert_eq!(format!("{}", CelEventType::ChannelStart), "CHAN_START");
    assert_eq!(format!("{}", CelEventType::Hangup), "HANGUP");
    assert_eq!(format!("{}", CelEventType::BridgeEnter), "BRIDGE_ENTER");
}

// ---------------------------------------------------------------------------
// Event creation tests
// ---------------------------------------------------------------------------

/// Port of basic CelEvent creation.
#[test]
fn cel_event_creation() {
    let event = CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice")
        .with_dialplan("default", "100", 1)
        .with_linked_id("uid-alice");

    assert_eq!(event.event_type, CelEventType::ChannelStart);
    assert_eq!(event.channel_name, "CELTestChannel/Alice");
    assert_eq!(event.unique_id, "uid-alice");
    assert_eq!(event.caller_id_num, "100");
    assert_eq!(event.caller_id_name, "Alice");
    assert_eq!(event.context, "default");
    assert_eq!(event.extension, "100");
    assert_eq!(event.priority, 1);
    assert_eq!(event.linked_id, "uid-alice");
    assert!(event.timestamp > 0);
}

/// Verify event builder for application info.
#[test]
fn cel_event_with_application() {
    let event = CelEvent::new(CelEventType::AppStart, "CELTestChannel/Alice", "uid-alice")
        .with_application("Dial", "SIP/bob,30");

    assert_eq!(event.application, "Dial");
    assert_eq!(event.application_data, "SIP/bob,30");
}

/// Verify event builder for extra data.
#[test]
fn cel_event_with_extra() {
    let event = CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "dialstatus": ""}"#);

    assert!(event.extra.contains("hangupcause"));
    assert!(event.extra.contains("16"));
}

// ---------------------------------------------------------------------------
// Channel creation/destruction tests
// ---------------------------------------------------------------------------

/// Port of test_cel_channel_creation: verify CHANNEL_START and CHANNEL_END events.
#[test]
fn cel_channel_creation() {
    let (engine, backend) = setup_cel();

    // Create channel -> CHANNEL_START
    let start_event = CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice");
    engine.report(&start_event);

    // Hangup -> HANGUP + CHANNEL_END
    let hangup_event = CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#);
    engine.report(&hangup_event);

    let end_event = CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice");
    engine.report(&end_event);

    let types = backend.event_types();
    assert_eq!(types.len(), 3);
    assert_eq!(types[0], CelEventType::ChannelStart);
    assert_eq!(types[1], CelEventType::Hangup);
    assert_eq!(types[2], CelEventType::ChannelEnd);
}

/// Port of test_cel_unanswered_inbound_call: unanswered call.
#[test]
fn cel_unanswered_inbound_call() {
    let (engine, backend) = setup_cel();

    // CHANNEL_START
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));

    // Executes Wait app (no specific CEL event for app unless tracked)

    // HANGUP
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));

    // CHANNEL_END
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert_eq!(types[0], CelEventType::ChannelStart);
    assert!(types.contains(&CelEventType::Hangup));
    assert!(types.contains(&CelEventType::ChannelEnd));
}

/// Port of test_cel_unanswered_outbound_call: outbound never answered.
#[test]
fn cel_unanswered_outbound_call() {
    let (engine, backend) = setup_cel();

    // CHANNEL_START
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("", ""));

    // HANGUP
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));

    // CHANNEL_END
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert!(types.contains(&CelEventType::ChannelStart));
    assert!(types.contains(&CelEventType::ChannelEnd));
}

// ---------------------------------------------------------------------------
// Single party tests
// ---------------------------------------------------------------------------

/// Port of test_cel_single_party: answered single-channel call.
#[test]
fn cel_single_party() {
    let (engine, backend) = setup_cel();

    // CHANNEL_START
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));

    // ANSWER
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice"));

    // HANGUP
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));

    // CHANNEL_END
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert_eq!(types[0], CelEventType::ChannelStart);
    assert_eq!(types[1], CelEventType::Answer);
    assert_eq!(types[2], CelEventType::Hangup);
    assert_eq!(types[3], CelEventType::ChannelEnd);
}

// ---------------------------------------------------------------------------
// Bridge tests
// ---------------------------------------------------------------------------

/// Port of test_cel_single_bridge: single party entering/leaving a bridge.
#[test]
fn cel_single_bridge() {
    let (engine, backend) = setup_cel();

    // CHANNEL_START
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));

    // ANSWER
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice"));

    // BRIDGE_ENTER
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // BRIDGE_EXIT
    engine.report(&CelEvent::new(CelEventType::BridgeExit, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // HANGUP + CHANNEL_END
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert!(types.contains(&CelEventType::BridgeEnter));
    assert!(types.contains(&CelEventType::BridgeExit));

    // BRIDGE_ENTER must come before BRIDGE_EXIT
    let enter_idx = types.iter().position(|t| *t == CelEventType::BridgeEnter).unwrap();
    let exit_idx = types.iter().position(|t| *t == CelEventType::BridgeExit).unwrap();
    assert!(enter_idx < exit_idx);
}

/// Port of test_cel_single_twoparty_bridge: two parties in a bridge.
#[test]
fn cel_two_party_bridge() {
    let (engine, backend) = setup_cel();

    // Alice: CHANNEL_START, ANSWER
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice"));

    // Bob: CHANNEL_START, ANSWER
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Bob", "uid-bob")
        .with_caller_id("200", "Bob"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Bob", "uid-bob"));

    // Both enter bridge
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Bob", "uid-bob")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // Both exit bridge
    engine.report(&CelEvent::new(CelEventType::BridgeExit, "CELTestChannel/Bob", "uid-bob")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));
    engine.report(&CelEvent::new(CelEventType::BridgeExit, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // Hangup both
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Bob", "uid-bob")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Bob", "uid-bob"));

    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"hangupcause": 16, "hangupsource": "", "dialstatus": ""}"#));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let events = backend.events();

    // Verify we have the right number of events
    assert_eq!(events.len(), 12);

    // Verify bridge events exist for both channels
    let bridge_enters: Vec<_> = events.iter()
        .filter(|e| e.event_type == CelEventType::BridgeEnter)
        .collect();
    assert_eq!(bridge_enters.len(), 2);

    let bridge_exits: Vec<_> = events.iter()
        .filter(|e| e.event_type == CelEventType::BridgeExit)
        .collect();
    assert_eq!(bridge_exits.len(), 2);
}

// ---------------------------------------------------------------------------
// Park tests
// ---------------------------------------------------------------------------

/// Port of park events: PARK_START and PARK_END.
#[test]
fn cel_park_events() {
    let (engine, backend) = setup_cel();

    // Channel start
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice"));

    // Park
    engine.report(&CelEvent::new(CelEventType::ParkStart, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"parker_dial_string": "SIP/bob"}"#));

    // Unpark
    engine.report(&CelEvent::new(CelEventType::ParkEnd, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"reason": "timeout"}"#));

    // Hangup
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice"));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert!(types.contains(&CelEventType::ParkStart));
    assert!(types.contains(&CelEventType::ParkEnd));

    let park_start_idx = types.iter().position(|t| *t == CelEventType::ParkStart).unwrap();
    let park_end_idx = types.iter().position(|t| *t == CelEventType::ParkEnd).unwrap();
    assert!(park_start_idx < park_end_idx);
}

// ---------------------------------------------------------------------------
// Transfer tests
// ---------------------------------------------------------------------------

/// Port of blind transfer: A calls B, B blind transfers to C.
#[test]
fn cel_blind_transfer() {
    let (engine, backend) = setup_cel();

    // Alice starts
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice"));

    // Bob starts
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Bob", "uid-bob")
        .with_caller_id("200", "Bob"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Bob", "uid-bob"));

    // Bridge
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Bob", "uid-bob")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // Bob performs blind transfer to extension 300
    let transfer_extra = serde_json::json!({
        "extension": "300",
        "context": "default",
        "bridge_id": "bridge-1",
        "transferee_channel_name": "N/A",
        "transferee_channel_uniqueid": "N/A"
    });
    engine.report(&CelEvent::new(CelEventType::BlindTransfer, "CELTestChannel/Bob", "uid-bob")
        .with_extra(&transfer_extra.to_string()));

    // Cleanup
    engine.report(&CelEvent::new(CelEventType::BridgeExit, "CELTestChannel/Alice", "uid-alice"));
    engine.report(&CelEvent::new(CelEventType::BridgeExit, "CELTestChannel/Bob", "uid-bob"));

    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Bob", "uid-bob"));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Bob", "uid-bob"));
    engine.report(&CelEvent::new(CelEventType::Hangup, "CELTestChannel/Alice", "uid-alice"));
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "CELTestChannel/Alice", "uid-alice"));

    let types = backend.event_types();
    assert!(types.contains(&CelEventType::BlindTransfer));

    // Verify blind transfer event has correct data
    let transfer_events: Vec<_> = backend.events().into_iter()
        .filter(|e| e.event_type == CelEventType::BlindTransfer)
        .collect();
    assert_eq!(transfer_events.len(), 1);
    assert!(transfer_events[0].extra.contains("300"));
    assert!(transfer_events[0].extra.contains("default"));
}

/// Port of attended transfer: A calls B, B calls C, then transfer.
#[test]
fn cel_attended_transfer() {
    let (engine, backend) = setup_cel();

    // Alice starts
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice"));

    // Bob starts
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Bob", "uid-bob")
        .with_caller_id("200", "Bob"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Bob", "uid-bob"));

    // Bridge 1: Alice <-> Bob
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Alice", "uid-alice")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Bob", "uid-bob")
        .with_extra(r#"{"bridge_id": "bridge-1", "bridge_technology": "simple_bridge"}"#));

    // Charlie starts (Bob calls Charlie for consultation)
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Charlie", "uid-charlie")
        .with_caller_id("300", "Charlie"));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Charlie", "uid-charlie"));

    // Bridge 2: Bob <-> Charlie
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Bob", "uid-bob2")
        .with_extra(r#"{"bridge_id": "bridge-2", "bridge_technology": "simple_bridge"}"#));
    engine.report(&CelEvent::new(CelEventType::BridgeEnter, "CELTestChannel/Charlie", "uid-charlie")
        .with_extra(r#"{"bridge_id": "bridge-2", "bridge_technology": "simple_bridge"}"#));

    // Attended transfer event
    let transfer_extra = serde_json::json!({
        "bridge1_id": "bridge-1",
        "channel2_name": "CELTestChannel/Bob",
        "channel2_uniqueid": "uid-bob",
        "bridge2_id": "bridge-2",
        "transferee_channel_name": "CELTestChannel/Alice",
        "transferee_channel_uniqueid": "uid-alice",
        "transfer_target_channel_name": "CELTestChannel/Charlie",
        "transfer_target_channel_uniqueid": "uid-charlie"
    });
    engine.report(&CelEvent::new(CelEventType::AttendedTransfer, "CELTestChannel/Bob", "uid-bob")
        .with_extra(&transfer_extra.to_string()));

    let types = backend.event_types();
    assert!(types.contains(&CelEventType::AttendedTransfer));

    let transfer_events: Vec<_> = backend.events().into_iter()
        .filter(|e| e.event_type == CelEventType::AttendedTransfer)
        .collect();
    assert_eq!(transfer_events.len(), 1);
    assert!(transfer_events[0].extra.contains("bridge-1"));
    assert!(transfer_events[0].extra.contains("bridge-2"));
}

// ---------------------------------------------------------------------------
// Event ordering tests
// ---------------------------------------------------------------------------

/// Verify correct chronological ordering of events.
#[test]
fn cel_event_ordering() {
    let (engine, backend) = setup_cel();

    let events_in_order = vec![
        CelEventType::ChannelStart,
        CelEventType::Answer,
        CelEventType::BridgeEnter,
        CelEventType::BridgeExit,
        CelEventType::Hangup,
        CelEventType::ChannelEnd,
    ];

    for evt_type in &events_in_order {
        engine.report(&CelEvent::new(*evt_type, "CELTestChannel/Alice", "uid-alice"));
    }

    let recorded_types = backend.event_types();
    assert_eq!(recorded_types, events_in_order);
}

// ---------------------------------------------------------------------------
// LinkedID tests
// ---------------------------------------------------------------------------

/// Verify linkedid propagation through events.
#[test]
fn cel_linked_id_propagation() {
    let (engine, backend) = setup_cel();

    let linked_id = "uid-alice"; // In Asterisk, the first channel's uniqueid becomes the linkedid

    // Alice's events all carry the same linked_id
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Alice", "uid-alice")
        .with_linked_id(linked_id));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice")
        .with_linked_id(linked_id));

    // Bob's events should also carry Alice's linked_id when they're related
    engine.report(&CelEvent::new(CelEventType::ChannelStart, "CELTestChannel/Bob", "uid-bob")
        .with_linked_id(linked_id));
    engine.report(&CelEvent::new(CelEventType::Answer, "CELTestChannel/Bob", "uid-bob")
        .with_linked_id(linked_id));

    let events = backend.events();
    for event in &events {
        assert_eq!(event.linked_id, linked_id,
            "Event {} for {} has wrong linked_id: expected {}, got {}",
            event.event_type.name(), event.channel_name, linked_id, event.linked_id);
    }
}

// ---------------------------------------------------------------------------
// Engine configuration tests
// ---------------------------------------------------------------------------

/// Verify CEL engine tracks only configured event types.
#[test]
fn cel_config_event_tracking() {
    // Only track ChannelStart and ChannelEnd
    let config = CelConfig {
        enabled: true,
        tracked_events: vec![CelEventType::ChannelStart, CelEventType::ChannelEnd],
        tracked_apps: Vec::new(),
        date_format: "%Y-%m-%d %H:%M:%S".to_string(),
    };

    let engine = CelEngine::new(config);
    let backend = Arc::new(MockCelBackend::new());
    engine.register_backend(backend.clone()).unwrap();

    engine.report(&CelEvent::new(CelEventType::ChannelStart, "test", "uid-1"));
    engine.report(&CelEvent::new(CelEventType::Answer, "test", "uid-1"));  // Not tracked
    engine.report(&CelEvent::new(CelEventType::Hangup, "test", "uid-1")); // Not tracked
    engine.report(&CelEvent::new(CelEventType::ChannelEnd, "test", "uid-1"));

    // Only START and END should be recorded
    let types = backend.event_types();
    assert_eq!(types.len(), 2);
    assert_eq!(types[0], CelEventType::ChannelStart);
    assert_eq!(types[1], CelEventType::ChannelEnd);
}

/// Verify CEL engine disabled state.
#[test]
fn cel_disabled() {
    let config = CelConfig {
        enabled: false,
        ..Default::default()
    };

    let engine = CelEngine::new(config);
    let backend = Arc::new(MockCelBackend::new());
    engine.register_backend(backend.clone()).unwrap();

    engine.report(&CelEvent::new(CelEventType::ChannelStart, "test", "uid-1"));

    assert_eq!(backend.count(), 0);
}

/// Verify backend registration and unregistration.
#[test]
fn cel_backend_registration() {
    let engine = CelEngine::new(CelConfig::track_all());

    let backend = Arc::new(MockCelBackend::new());
    assert!(engine.register_backend(backend.clone()).is_ok());

    // Duplicate registration should fail
    let backend2 = Arc::new(MockCelBackend::new());
    assert!(engine.register_backend(backend2).is_err());

    // Unregister
    assert!(engine.unregister_backend("CEL Test Logging").is_ok());

    // Unregister again should fail
    assert!(engine.unregister_backend("CEL Test Logging").is_err());
}

/// Verify events_processed counter.
#[test]
fn cel_events_processed() {
    let (engine, _backend) = setup_cel();
    assert_eq!(engine.events_processed(), 0);

    engine.report(&CelEvent::new(CelEventType::ChannelStart, "test", "uid-1"));
    assert_eq!(engine.events_processed(), 1);

    engine.report(&CelEvent::new(CelEventType::Answer, "test", "uid-1"));
    assert_eq!(engine.events_processed(), 2);
}

/// Verify CelConfig::parse_events.
#[test]
fn cel_parse_events() {
    let events = CelConfig::parse_events("CHAN_START,ANSWER,HANGUP");
    assert_eq!(events.len(), 3);
    assert!(events.contains(&CelEventType::ChannelStart));
    assert!(events.contains(&CelEventType::Answer));
    assert!(events.contains(&CelEventType::Hangup));

    // "ALL" gives everything
    let all = CelConfig::parse_events("ALL");
    assert_eq!(all.len(), CelEventType::all().len());

    // Unknown events are skipped
    let with_unknown = CelConfig::parse_events("CHAN_START,BOGUS,ANSWER");
    assert_eq!(with_unknown.len(), 2);
}

/// Verify CelConfig::is_tracked.
#[test]
fn cel_is_tracked() {
    let config = CelConfig {
        enabled: true,
        tracked_events: vec![CelEventType::ChannelStart, CelEventType::Answer],
        ..Default::default()
    };

    assert!(config.is_tracked(CelEventType::ChannelStart));
    assert!(config.is_tracked(CelEventType::Answer));
    assert!(!config.is_tracked(CelEventType::Hangup));
    assert!(!config.is_tracked(CelEventType::BridgeEnter));
}

/// Verify CSV output format.
#[test]
fn cel_event_csv_format() {
    let event = CelEvent::new(CelEventType::Answer, "CELTestChannel/Alice", "uid-alice")
        .with_caller_id("100", "Alice")
        .with_dialplan("default", "100", 1);

    let csv = event.to_csv(',');
    assert!(csv.contains("ANSWER"));
    assert!(csv.contains("CELTestChannel/Alice"));
    assert!(csv.contains("uid-alice"));
    assert!(csv.contains("100"));
    assert!(csv.contains("Alice"));
}

/// Verify CustomCelBackend basic operation.
#[test]
fn cel_custom_backend() {
    let backend = CustomCelBackend::new("test-custom");
    assert_eq!(backend.name(), "test-custom");
    assert_eq!(backend.logged_events().len(), 0);

    let event = CelEvent::new(CelEventType::ChannelStart, "test", "uid-1");
    backend.write(&event).unwrap();
    assert_eq!(backend.logged_events().len(), 1);

    backend.clear();
    assert_eq!(backend.logged_events().len(), 0);
}
