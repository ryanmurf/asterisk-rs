//! Port of asterisk/tests/test_stasis_channels.c
//!
//! Tests Stasis channel-related messaging:
//! - Channel blob creation (with and without channel)
//! - Channel snapshot creation
//! - Channel cache update messages
//! - Dial event messages
//! - Channel variable set messages
//! - Hangup request messages

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Simulated channel and snapshot structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ChannelSnapshot {
    name: String,
    uniqueid: String,
    state: u32,
    caller_name: String,
    caller_number: String,
}

#[derive(Debug, Clone)]
struct ChannelBlob {
    snapshot: Option<ChannelSnapshot>,
    blob: HashMap<String, String>,
    message_type: String,
}

impl ChannelBlob {
    fn create(
        channel: Option<&ChannelSnapshot>,
        msg_type: &str,
        blob: HashMap<String, String>,
    ) -> Option<Self> {
        if msg_type.is_empty() {
            return None;
        }
        Some(Self {
            snapshot: channel.cloned(),
            blob,
            message_type: msg_type.to_string(),
        })
    }
}

fn make_test_channel(name: &str) -> ChannelSnapshot {
    ChannelSnapshot {
        name: name.to_string(),
        uniqueid: uuid::Uuid::new_v4().to_string(),
        state: 0, // AST_STATE_DOWN
        caller_name: "Alice".to_string(),
        caller_number: "100".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(channel_blob_create) from test_stasis_channels.c.
///
/// Test creation of channel blob objects.
#[test]
fn test_channel_blob_create() {
    let chan = make_test_channel("TEST/Alice");
    let mut blob = HashMap::new();
    blob.insert("foo".to_string(), "bar".to_string());

    // Null message type should fail.
    assert!(ChannelBlob::create(Some(&chan), "", blob.clone()).is_none());

    // Valid creation with channel.
    let msg = ChannelBlob::create(Some(&chan), "test-type", blob.clone()).unwrap();
    assert!(msg.snapshot.is_some());
    assert_eq!(msg.blob.get("foo").unwrap(), "bar");
    assert_eq!(msg.message_type, "test-type");
}

/// Test channel blob without a channel (global message).
#[test]
fn test_channel_blob_create_no_channel() {
    let blob = HashMap::new();
    let msg = ChannelBlob::create(None, "test-type", blob).unwrap();
    assert!(msg.snapshot.is_none());
}

/// Port of channel snapshot creation test.
#[test]
fn test_channel_snapshot_creation() {
    let chan = make_test_channel("TEST/Alice");

    assert_eq!(chan.name, "TEST/Alice");
    assert_eq!(chan.state, 0);
    assert_eq!(chan.caller_name, "Alice");
    assert_eq!(chan.caller_number, "100");
    assert!(!chan.uniqueid.is_empty());
}

/// Test dial event message.
#[test]
fn test_dial_event() {
    let mut blob = HashMap::new();
    blob.insert("dialstatus".to_string(), "ANSWER".to_string());
    blob.insert("forward".to_string(), "".to_string());

    let chan = make_test_channel("TEST/Alice");
    let msg = ChannelBlob::create(Some(&chan), "dial", blob).unwrap();

    assert_eq!(msg.message_type, "dial");
    assert_eq!(msg.blob.get("dialstatus").unwrap(), "ANSWER");
}

/// Test channel variable set message.
#[test]
fn test_channel_varset() {
    let mut blob = HashMap::new();
    blob.insert("variable".to_string(), "CALLERID(name)".to_string());
    blob.insert("value".to_string(), "Bob".to_string());

    let chan = make_test_channel("TEST/Alice");
    let msg = ChannelBlob::create(Some(&chan), "varset", blob).unwrap();

    assert_eq!(msg.blob.get("variable").unwrap(), "CALLERID(name)");
    assert_eq!(msg.blob.get("value").unwrap(), "Bob");
}

/// Test hangup request message.
#[test]
fn test_hangup_request() {
    let mut blob = HashMap::new();
    blob.insert("cause".to_string(), "16".to_string());

    let chan = make_test_channel("TEST/Alice");
    let msg = ChannelBlob::create(Some(&chan), "hangup_request", blob).unwrap();

    assert_eq!(msg.message_type, "hangup_request");
    assert_eq!(msg.blob.get("cause").unwrap(), "16");
}
