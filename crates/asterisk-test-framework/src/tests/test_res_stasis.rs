//! Port of asterisk/tests/test_res_stasis.c
//!
//! Tests the Stasis application framework:
//! - Application registration and unregistration
//! - Sending events to non-existent apps
//! - Event dispatch to registered apps
//! - Application replacement
//! - Channel control via Stasis
//! - Bridge management (create, add/remove channels, destroy)

use asterisk_res::stasis_app::{
    StasisAppCallback, StasisAppManager, StasisBridgeType, StasisCommand,
};
use serde_json::{json, Value as JsonValue};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// App invocation -- non-existent
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(app_invoke_dne) from test_res_stasis.c.
///
/// Test that sending an event to a non-existent app fails.
#[test]
fn test_app_invoke_nonexistent() {
    let mgr = StasisAppManager::new();
    let result = mgr.send_event("i-am-not-an-app", &json!(null));
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// App registration
// ---------------------------------------------------------------------------

/// Test registering a Stasis application.
#[test]
fn test_app_register() {
    let mgr = StasisAppManager::new();

    let handler: StasisAppCallback = Arc::new(|_name, _event| {});
    let result = mgr.register_app("test-app", handler);
    assert!(result.is_ok());
    assert!(mgr.is_registered("test-app"));
}

/// Test registering an app twice fails.
#[test]
fn test_app_register_duplicate() {
    let mgr = StasisAppManager::new();

    let handler: StasisAppCallback = Arc::new(|_name, _event| {});
    mgr.register_app("test-app", handler.clone()).unwrap();

    let result = mgr.register_app("test-app", handler);
    assert!(result.is_err());
}

/// Test unregistering an app.
#[test]
fn test_app_unregister() {
    let mgr = StasisAppManager::new();

    let handler: StasisAppCallback = Arc::new(|_name, _event| {});
    mgr.register_app("test-app", handler).unwrap();

    let result = mgr.unregister_app("test-app");
    assert!(result.is_ok());
    assert!(!mgr.is_registered("test-app"));
}

/// Test unregistering a non-existent app fails.
#[test]
fn test_app_unregister_nonexistent() {
    let mgr = StasisAppManager::new();
    let result = mgr.unregister_app("nope");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// App invocation -- single handler
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(app_invoke_one) from test_res_stasis.c.
///
/// Test that sending an event to a registered app invokes its handler.
#[test]
fn test_app_invoke_one() {
    let mgr = StasisAppManager::new();
    let invocations = Arc::new(AtomicU32::new(0));
    let inv = Arc::clone(&invocations);

    let handler: StasisAppCallback = Arc::new(move |_name, _event| {
        inv.fetch_add(1, Ordering::SeqCst);
    });

    mgr.register_app("test-handler", handler).unwrap();

    let message = json!({"test-message": null});
    let result = mgr.send_event("test-handler", &message);
    assert!(result.is_ok());
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

/// Test multiple event dispatches.
#[test]
fn test_app_invoke_multiple() {
    let mgr = StasisAppManager::new();
    let invocations = Arc::new(AtomicU32::new(0));
    let inv = Arc::clone(&invocations);

    let handler: StasisAppCallback = Arc::new(move |_name, _event| {
        inv.fetch_add(1, Ordering::SeqCst);
    });

    mgr.register_app("test-handler", handler).unwrap();

    for i in 0..5 {
        let msg = json!({"event": i});
        mgr.send_event("test-handler", &msg).unwrap();
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 5);
}

// ---------------------------------------------------------------------------
// App name listing
// ---------------------------------------------------------------------------

/// Test listing registered app names.
#[test]
fn test_app_name_listing() {
    let mgr = StasisAppManager::new();
    let handler: StasisAppCallback = Arc::new(|_, _| {});

    mgr.register_app("app-alpha", handler.clone()).unwrap();
    mgr.register_app("app-beta", handler.clone()).unwrap();
    mgr.register_app("app-gamma", handler).unwrap();

    let names = mgr.app_names();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"app-alpha".to_string()));
    assert!(names.contains(&"app-beta".to_string()));
    assert!(names.contains(&"app-gamma".to_string()));
}

// ---------------------------------------------------------------------------
// Channel control
// ---------------------------------------------------------------------------

/// Test creating channel control handles.
#[test]
fn test_channel_control_creation() {
    let mgr = StasisAppManager::new();
    let handler: StasisAppCallback = Arc::new(|_, _| {});
    mgr.register_app("test-app", handler).unwrap();

    let control = mgr.create_control("channel-1", "test-app");
    assert!(control.is_ok());
    let control = control.unwrap();
    assert_eq!(control.channel_id, "channel-1");
    assert_eq!(control.app_name, "test-app");
}

/// Test channel control for non-existent app fails.
#[test]
fn test_channel_control_no_app() {
    let mgr = StasisAppManager::new();
    let result = mgr.create_control("channel-1", "nonexistent");
    assert!(result.is_err());
}

/// Test sending commands to a controlled channel.
#[test]
fn test_channel_control_commands() {
    let mgr = StasisAppManager::new();
    let handler: StasisAppCallback = Arc::new(|_, _| {});
    mgr.register_app("test-app", handler).unwrap();

    let control = mgr.create_control("channel-1", "test-app").unwrap();

    // Queue some commands.
    control.send_command(StasisCommand::Answer);
    control.send_command(StasisCommand::MohStart {
        moh_class: "default".to_string(),
    });
    control.send_command(StasisCommand::Hangup { cause: 16 });

    assert_eq!(control.pending_count(), 3);

    // Drain commands.
    let commands = control.drain_commands();
    assert_eq!(commands.len(), 3);
    assert_eq!(control.pending_count(), 0);
}

/// Test active channel count.
#[test]
fn test_active_channel_count() {
    let mgr = StasisAppManager::new();
    let handler: StasisAppCallback = Arc::new(|_, _| {});
    mgr.register_app("test-app", handler).unwrap();

    assert_eq!(mgr.active_channel_count(), 0);

    mgr.create_control("ch-1", "test-app").unwrap();
    mgr.create_control("ch-2", "test-app").unwrap();
    assert_eq!(mgr.active_channel_count(), 2);

    mgr.remove_control("ch-1").unwrap();
    assert_eq!(mgr.active_channel_count(), 1);
}

// ---------------------------------------------------------------------------
// Bridge management
// ---------------------------------------------------------------------------

/// Test creating a bridge.
#[test]
fn test_bridge_creation() {
    let mgr = StasisAppManager::new();
    let bridge = mgr.create_bridge(StasisBridgeType::Mixing, "test-bridge");

    assert_eq!(bridge.name, "test-bridge");
    assert_eq!(bridge.bridge_type, StasisBridgeType::Mixing);
    assert_eq!(bridge.channel_count(), 0);
    assert!(!bridge.id.is_empty());
}

/// Test adding and removing channels from a bridge.
#[test]
fn test_bridge_channel_management() {
    let mgr = StasisAppManager::new();
    let bridge = mgr.create_bridge(StasisBridgeType::Mixing, "test-bridge");
    let bridge_id = bridge.id.clone();

    mgr.add_channel_to_bridge(&bridge_id, "ch-1").unwrap();
    mgr.add_channel_to_bridge(&bridge_id, "ch-2").unwrap();

    let br = mgr.get_bridge(&bridge_id).unwrap();
    assert_eq!(br.channel_count(), 2);

    // Adding same channel again should not duplicate.
    mgr.add_channel_to_bridge(&bridge_id, "ch-1").unwrap();
    let br = mgr.get_bridge(&bridge_id).unwrap();
    assert_eq!(br.channel_count(), 2);

    // Remove a channel.
    mgr.remove_channel_from_bridge(&bridge_id, "ch-1").unwrap();
    let br = mgr.get_bridge(&bridge_id).unwrap();
    assert_eq!(br.channel_count(), 1);
}

/// Test destroying a bridge.
#[test]
fn test_bridge_destroy() {
    let mgr = StasisAppManager::new();
    let bridge = mgr.create_bridge(StasisBridgeType::Holding, "hold-bridge");
    let bridge_id = bridge.id.clone();

    let result = mgr.destroy_bridge(&bridge_id);
    assert!(result.is_ok());

    // Should no longer be gettable.
    assert!(mgr.get_bridge(&bridge_id).is_err());
}

/// Test bridge types.
#[test]
fn test_bridge_types() {
    assert_eq!(StasisBridgeType::Mixing.as_str(), "mixing");
    assert_eq!(StasisBridgeType::Holding.as_str(), "holding");
}

/// Test bridge ID listing.
#[test]
fn test_bridge_id_listing() {
    let mgr = StasisAppManager::new();
    let b1 = mgr.create_bridge(StasisBridgeType::Mixing, "b1");
    let b2 = mgr.create_bridge(StasisBridgeType::Holding, "b2");

    let ids = mgr.bridge_ids();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&b1.id));
    assert!(ids.contains(&b2.id));
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

/// Test subscribing an app to resources.
#[test]
fn test_app_subscription() {
    let mgr = StasisAppManager::new();
    let handler: StasisAppCallback = Arc::new(|_, _| {});
    mgr.register_app("test-app", handler).unwrap();

    // Subscribe to a resource.
    let result = mgr.subscribe("test-app", "channel:ch-123");
    assert!(result.is_ok());

    // Subscribe to non-existent app fails.
    let result = mgr.subscribe("nope", "channel:ch-123");
    assert!(result.is_err());

    // Unsubscribe.
    let result = mgr.unsubscribe("test-app", "channel:ch-123");
    assert!(result.is_ok());
}
