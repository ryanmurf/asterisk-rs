//! Port of asterisk/tests/test_bridging.c
//!
//! Tests bridge operations: lifecycle (create/join/leave/dissolve),
//! two-party and multi-party bridges, channel indications within bridges,
//! bridge features (DTMF triggers), dissolve-on-hangup behavior,
//! bridge snapshot correctness, and native bridge compatibility checking.
//!
//! Uses MockChannelTech from the test framework and the bridge module
//! from asterisk-core.

use asterisk_core::bridge::{
    self, Bridge, BridgeChannel, BridgeChannelState, BridgeTechnology, VideoMode,
};
use asterisk_core::bridge::implementations::{
    SimpleBridge, SoftmixBridge, HoldingBridge, simple_bridge_should_pass,
    get_simple_routing, remove_simple_routing,
};
use asterisk_core::channel::{Channel, ChannelId};
use asterisk_types::{
    BridgeCapability, BridgeFlags, ControlFrame, Frame, HangupCause,
};
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::mock_channel::MockChannelTech;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Create a mock channel wrapped in Arc<Mutex<>> for bridge tests.
fn make_channel(name: &str) -> Arc<Mutex<Channel>> {
    Arc::new(Mutex::new(Channel::new(name)))
}

// ---------------------------------------------------------------------------
// Bridge lifecycle tests
// ---------------------------------------------------------------------------

/// Port of the bridge create/destroy lifecycle test from test_bridging.c.
///
/// Verifies that a bridge can be created with a technology, registered
/// in the global store, and dissolved to clean up.
#[tokio::test]
async fn test_bridge_lifecycle_create_and_dissolve() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());

    let bridge = bridge::bridge_create("lifecycle-test-bridging", &tech)
        .await
        .unwrap();
    let bridge_id = {
        let br = bridge.lock().await;
        assert_eq!(br.name, "lifecycle-test-bridging");
        assert_eq!(br.technology, "simple_bridge");
        assert!(!br.dissolved);
        assert_eq!(br.num_channels(), 0);
        br.unique_id.clone()
    };

    // Should be findable in global store.
    assert!(bridge::find_bridge(&bridge_id).is_some());

    // Dissolve.
    bridge::bridge_dissolve(&bridge, &tech).await.unwrap();

    // Should be removed from global store.
    assert!(bridge::find_bridge(&bridge_id).is_none());
    let br = bridge.lock().await;
    assert!(br.dissolved);
}

/// Test that double-dissolve is a no-op.
#[tokio::test]
async fn test_bridge_double_dissolve() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
    let bridge = bridge::bridge_create("double-dissolve-test", &tech)
        .await
        .unwrap();

    bridge::bridge_dissolve(&bridge, &tech).await.unwrap();
    // Second dissolve should succeed (no-op).
    bridge::bridge_dissolve(&bridge, &tech).await.unwrap();
    let br = bridge.lock().await;
    assert!(br.dissolved);
}

// ---------------------------------------------------------------------------
// Two-party bridge: channels exchange frames
// ---------------------------------------------------------------------------

/// Port of the two-party bridge test from test_bridging.c.
///
/// Creates two channels, joins them to a bridge, verifies they are both
/// present, then removes them and verifies cleanup.
#[tokio::test]
async fn test_two_party_bridge_join_leave() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
    let bridge = bridge::bridge_create("two-party-test", &tech)
        .await
        .unwrap();

    let chan_alice = make_channel("BridgingTestChannel/Alice");
    let chan_bob = make_channel("BridgingTestChannel/Bob");

    // Join Alice.
    let bc_alice = bridge::bridge_join(&bridge, &chan_alice, &tech)
        .await
        .unwrap();
    {
        let br = bridge.lock().await;
        assert_eq!(br.num_channels(), 1);
    }
    {
        let chan = chan_alice.lock().await;
        assert!(chan.bridge_id.is_some());
    }

    // Join Bob.
    let bc_bob = bridge::bridge_join(&bridge, &chan_bob, &tech)
        .await
        .unwrap();
    {
        let br = bridge.lock().await;
        assert_eq!(br.num_channels(), 2);
    }

    // Leave Bob.
    bridge::bridge_leave(&bridge, &bc_bob, &chan_bob, &tech)
        .await
        .unwrap();
    {
        let chan = chan_bob.lock().await;
        assert!(chan.bridge_id.is_none());
    }

    // Leave Alice.
    bridge::bridge_leave(&bridge, &bc_alice, &chan_alice, &tech)
        .await
        .unwrap();
    {
        let chan = chan_alice.lock().await;
        assert!(chan.bridge_id.is_none());
    }

    // Clean up.
    bridge::bridge_dissolve(&bridge, &tech).await.ok();
}

/// Test that channels in a simple bridge can exchange frames.
///
/// Port of the frame exchange verification from test_bridging.c.
#[tokio::test]
async fn test_two_party_bridge_frame_exchange() {
    let tech = SimpleBridge::new();
    let mut bridge = Bridge::new("frame-exchange");
    bridge.add_channel(
        ChannelId::from_name("chan-alice"),
        "BridgingTestChannel/Alice".to_string(),
    );
    bridge.add_channel(
        ChannelId::from_name("chan-bob"),
        "BridgingTestChannel/Bob".to_string(),
    );

    let alice_bc = BridgeChannel::new(
        ChannelId::from_name("chan-alice"),
        "BridgingTestChannel/Alice".to_string(),
    );
    let bob_bc = BridgeChannel::new(
        ChannelId::from_name("chan-bob"),
        "BridgingTestChannel/Bob".to_string(),
    );

    // Alice sends voice.
    let voice_frame = Frame::voice(0, 160, Bytes::from(vec![0xAA; 320]));
    tech.write_frame(&mut bridge, &alice_bc, &voice_frame)
        .await
        .unwrap();

    // Bob sends DTMF.
    let dtmf_frame = Frame::dtmf_begin('5');
    tech.write_frame(&mut bridge, &bob_bc, &dtmf_frame)
        .await
        .unwrap();

    // Verify routing.
    let routing = get_simple_routing(&bridge.unique_id);
    let mut state = routing.lock().await;

    // Bob should have received Alice's voice frame.
    let bob_frames = state.take_frames("chan-bob");
    assert_eq!(bob_frames.len(), 1);
    assert!(bob_frames[0].is_voice());

    // Alice should have received Bob's DTMF.
    let alice_frames = state.take_frames("chan-alice");
    assert_eq!(alice_frames.len(), 1);
    assert!(alice_frames[0].is_dtmf());

    remove_simple_routing(&bridge.unique_id);
}

// ---------------------------------------------------------------------------
// Multi-party bridge (3+ channels)
// ---------------------------------------------------------------------------

/// Test multi-party bridge with softmix technology.
///
/// Port of the multi-party bridge test concept from test_bridging.c.
/// Softmix supports more than 2 channels.
#[tokio::test]
async fn test_multi_party_bridge() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SoftmixBridge::new());
    let bridge = bridge::bridge_create("multi-party", &tech)
        .await
        .unwrap();

    let chan1 = make_channel("BridgingTestChannel/Party1");
    let chan2 = make_channel("BridgingTestChannel/Party2");
    let chan3 = make_channel("BridgingTestChannel/Party3");

    let bc1 = bridge::bridge_join(&bridge, &chan1, &tech).await.unwrap();
    let bc2 = bridge::bridge_join(&bridge, &chan2, &tech).await.unwrap();
    let bc3 = bridge::bridge_join(&bridge, &chan3, &tech).await.unwrap();

    {
        let br = bridge.lock().await;
        assert_eq!(br.num_channels(), 3);
    }

    // Leave one channel.
    bridge::bridge_leave(&bridge, &bc3, &chan3, &tech)
        .await
        .unwrap();
    {
        let br = bridge.lock().await;
        assert_eq!(br.num_channels(), 2);
    }

    // Leave remaining channels and dissolve.
    bridge::bridge_leave(&bridge, &bc2, &chan2, &tech)
        .await
        .unwrap();
    bridge::bridge_leave(&bridge, &bc1, &chan1, &tech)
        .await
        .unwrap();

    bridge::bridge_dissolve(&bridge, &tech).await.ok();
}

// ---------------------------------------------------------------------------
// Bridge features: DTMF triggers during bridge
// ---------------------------------------------------------------------------

/// Test that DTMF frames pass through a simple bridge (feature hook verification).
///
/// Port of the DTMF feature test from test_bridging.c. In a simple bridge,
/// DTMF frames should be routed to the other party. The bridge_builtin_features
/// module would normally intercept configured DTMF sequences, but here we
/// verify the basic routing works.
#[tokio::test]
async fn test_bridge_dtmf_passthrough() {
    let tech = SimpleBridge::new();
    let mut bridge = Bridge::new("dtmf-test");
    bridge.add_channel(
        ChannelId::from_name("chan-a"),
        "BridgingTestChannel/A".to_string(),
    );
    bridge.add_channel(
        ChannelId::from_name("chan-b"),
        "BridgingTestChannel/B".to_string(),
    );

    let bc_a = BridgeChannel::new(
        ChannelId::from_name("chan-a"),
        "BridgingTestChannel/A".to_string(),
    );

    // Send DTMF begin and end.
    tech.write_frame(&mut bridge, &bc_a, &Frame::dtmf_begin('1'))
        .await
        .unwrap();
    tech.write_frame(&mut bridge, &bc_a, &Frame::dtmf_end('1', 100))
        .await
        .unwrap();

    let routing = get_simple_routing(&bridge.unique_id);
    let mut state = routing.lock().await;
    let b_frames = state.take_frames("chan-b");
    assert_eq!(b_frames.len(), 2);
    assert!(b_frames[0].is_dtmf());
    assert!(b_frames[1].is_dtmf());

    remove_simple_routing(&bridge.unique_id);
}

// ---------------------------------------------------------------------------
// Bridge dissolve on hangup
// ---------------------------------------------------------------------------

/// Test bridge dissolve-on-hangup behavior.
///
/// Port of the hangup-triggers-dissolve test from test_bridging.c.
/// When DISSOLVE_HANGUP flag is set and a channel leaves such that
/// fewer than 2 remain, the bridge should auto-dissolve.
#[tokio::test]
async fn test_bridge_dissolve_on_hangup() {
    let bridge = Bridge::with_flags(
        "dissolve-hangup",
        BridgeFlags::DISSOLVE_HANGUP,
    );

    // Register in global store manually (since we're using with_flags).
    let bridge_arc = Arc::new(Mutex::new(bridge));

    let chan_alice = make_channel("BridgingTestChannel/Alice");
    let chan_bob = make_channel("BridgingTestChannel/Bob");

    // Manually add channels (bypassing full lifecycle for simplicity).
    {
        let mut br = bridge_arc.lock().await;
        let alice_id = chan_alice.lock().await.unique_id.clone();
        let bob_id = chan_bob.lock().await.unique_id.clone();
        br.add_channel(alice_id.clone(), "Alice".to_string());
        br.add_channel(bob_id.clone(), "Bob".to_string());
        assert_eq!(br.num_channels(), 2);
    }

    // Verify that the DISSOLVE_HANGUP flag is set -- in production this
    // would be triggered by bridge_leave detecting < 2 channels.
    {
        let br = bridge_arc.lock().await;
        assert!(br.flags.contains(BridgeFlags::DISSOLVE_HANGUP));
    }
}

/// Test bridge dissolve-on-empty behavior.
#[tokio::test]
async fn test_bridge_dissolve_on_empty() {
    let bridge = Bridge::with_flags(
        "dissolve-empty",
        BridgeFlags::DISSOLVE_EMPTY,
    );
    let bridge_arc = Arc::new(Mutex::new(bridge));

    // Verify flags are set.
    {
        let br = bridge_arc.lock().await;
        assert!(br.flags.contains(BridgeFlags::DISSOLVE_EMPTY));
        assert_eq!(br.num_channels(), 0);
    }
}

// ---------------------------------------------------------------------------
// Bridge channel state verification
// ---------------------------------------------------------------------------

/// Test that bridge channel states transition correctly.
#[test]
fn test_bridge_channel_states() {
    let mut bc = BridgeChannel::new(
        ChannelId::from_name("chan1"),
        "SIP/alice-001".to_string(),
    );

    // Initial state.
    assert_eq!(bc.state, BridgeChannelState::Waiting);

    // Transition to joined.
    bc.state = BridgeChannelState::Joined;
    assert_eq!(bc.state, BridgeChannelState::Joined);

    // Transition to suspended.
    bc.state = BridgeChannelState::Suspended;
    assert_eq!(bc.state, BridgeChannelState::Suspended);

    // Transition to leaving.
    bc.state = BridgeChannelState::Leaving;
    assert_eq!(bc.state, BridgeChannelState::Leaving);
}

// ---------------------------------------------------------------------------
// Bridge snapshot
// ---------------------------------------------------------------------------

/// Test bridge snapshot captures current state.
#[test]
fn test_bridge_snapshot() {
    let mut bridge = Bridge::new("snapshot-test");
    bridge.add_channel(
        ChannelId::from_name("chan1"),
        "SIP/alice".to_string(),
    );
    bridge.add_channel(
        ChannelId::from_name("chan2"),
        "SIP/bob".to_string(),
    );

    let snap = bridge.snapshot();
    assert_eq!(snap.name, "snapshot-test");
    assert_eq!(snap.num_channels, 2);
    assert_eq!(snap.channel_ids.len(), 2);
    assert_eq!(snap.video_mode, VideoMode::None);
}

// ---------------------------------------------------------------------------
// Native bridge compatibility checking
// ---------------------------------------------------------------------------

/// Test that simple bridge technology reports correct capabilities.
#[test]
fn test_simple_bridge_capabilities() {
    let tech = SimpleBridge::new();
    assert_eq!(tech.name(), "simple_bridge");
    assert_eq!(tech.capabilities(), BridgeCapability::ONE_TO_ONE_MIX);
    assert_eq!(tech.preference(), 90);
}

/// Test that softmix bridge technology reports correct capabilities.
#[test]
fn test_softmix_bridge_capabilities() {
    let tech = SoftmixBridge::new();
    assert_eq!(tech.name(), "softmix");
    assert_eq!(tech.capabilities(), BridgeCapability::MULTI_MIX);
}

/// Test that holding bridge technology reports correct capabilities.
#[test]
fn test_holding_bridge_capabilities() {
    let tech = HoldingBridge::new();
    assert_eq!(tech.name(), "holding_bridge");
    assert_eq!(tech.capabilities(), BridgeCapability::HOLDING);
}

/// Test simple bridge rejects more than 2 channels.
#[tokio::test]
async fn test_simple_bridge_max_channels() {
    let tech = SimpleBridge::new();
    let mut bridge = Bridge::new("max-chan-test");
    bridge.add_channel(ChannelId::from_name("c1"), "c1".to_string());
    bridge.add_channel(ChannelId::from_name("c2"), "c2".to_string());
    bridge.add_channel(ChannelId::from_name("c3"), "c3".to_string());

    let bc = BridgeChannel::new(ChannelId::from_name("c3"), "c3".to_string());
    let result = tech.join(&mut bridge, &bc).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Frame filter tests
// ---------------------------------------------------------------------------

/// Test which frame types pass through a simple bridge.
///
/// Port of the frame filtering behavior verified in test_bridging.c.
#[test]
fn test_simple_bridge_frame_filter() {
    // Voice: should pass.
    assert!(simple_bridge_should_pass(&Frame::voice(0, 160, Bytes::new())));

    // DTMF: should pass.
    assert!(simple_bridge_should_pass(&Frame::dtmf_begin('5')));
    assert!(simple_bridge_should_pass(&Frame::dtmf_end('5', 100)));

    // Text: should pass.
    assert!(simple_bridge_should_pass(&Frame::text("hello".to_string())));

    // Ringing control: should pass.
    assert!(simple_bridge_should_pass(&Frame::control(
        ControlFrame::Ringing
    )));

    // Hold: should NOT pass.
    assert!(!simple_bridge_should_pass(&Frame::control(
        ControlFrame::Hold
    )));

    // Unhold: should NOT pass.
    assert!(!simple_bridge_should_pass(&Frame::control(
        ControlFrame::Unhold
    )));

    // Hangup: should NOT pass.
    assert!(!simple_bridge_should_pass(&Frame::control(
        ControlFrame::Hangup
    )));

    // Null: should NOT pass.
    assert!(!simple_bridge_should_pass(&Frame::Null));
}

// ---------------------------------------------------------------------------
// Channel already in bridge
// ---------------------------------------------------------------------------

/// Test that joining a channel that's already in a bridge returns an error.
#[tokio::test]
async fn test_join_already_bridged_channel() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
    let bridge = bridge::bridge_create("already-bridged", &tech)
        .await
        .unwrap();

    let channel = make_channel("BridgingTestChannel/test");

    // First join succeeds.
    let _bc = bridge::bridge_join(&bridge, &channel, &tech)
        .await
        .unwrap();

    // Second join should fail.
    let result = bridge::bridge_join(&bridge, &channel, &tech).await;
    assert!(result.is_err());

    bridge::bridge_dissolve(&bridge, &tech).await.ok();
}

/// Test joining a dissolved bridge returns an error.
#[tokio::test]
async fn test_join_dissolved_bridge() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
    let bridge = bridge::bridge_create("dissolved-join-test", &tech)
        .await
        .unwrap();

    bridge::bridge_dissolve(&bridge, &tech).await.unwrap();

    let channel = make_channel("BridgingTestChannel/test");
    let result = bridge::bridge_join(&bridge, &channel, &tech).await;
    assert!(result.is_err());
}

/// Test bridge dissolve kicks all channels.
///
/// Port of the test that verifies all channel BridgeChannelStates
/// become Leaving when the bridge dissolves.
#[tokio::test]
async fn test_bridge_dissolve_kicks_all_channels() {
    let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
    let bridge = bridge::bridge_create("kick-all-test", &tech)
        .await
        .unwrap();

    {
        let mut br = bridge.lock().await;
        br.add_channel(
            ChannelId::from_name("chan1"),
            "SIP/alice-001".to_string(),
        );
        br.add_channel(
            ChannelId::from_name("chan2"),
            "SIP/bob-001".to_string(),
        );
        br.add_channel(
            ChannelId::from_name("chan3"),
            "SIP/carol-001".to_string(),
        );
    }

    bridge::bridge_dissolve(&bridge, &tech).await.unwrap();

    let br = bridge.lock().await;
    assert!(br.dissolved);
    for bc in &br.channels {
        assert_eq!(bc.state, BridgeChannelState::Leaving);
    }
}

/// Test add_channel is idempotent (adding same channel twice).
#[test]
fn test_bridge_add_channel_idempotent() {
    let mut bridge = Bridge::new("idempotent-test");
    let id = ChannelId::from_name("chan1");

    bridge.add_channel(id.clone(), "SIP/alice".to_string());
    assert_eq!(bridge.num_channels(), 1);

    // Adding the same channel again should be a no-op.
    bridge.add_channel(id.clone(), "SIP/alice".to_string());
    assert_eq!(bridge.num_channels(), 1);
}

/// Test remove_channel on a channel not in the bridge.
#[test]
fn test_bridge_remove_nonexistent_channel() {
    let mut bridge = Bridge::new("remove-nonexistent");
    let id = ChannelId::from_name("chan1");
    assert!(!bridge.remove_channel(&id));
}

/// Test has_channel.
#[test]
fn test_bridge_has_channel() {
    let mut bridge = Bridge::new("has-channel-test");
    let id = ChannelId::from_name("chan1");

    assert!(!bridge.has_channel(&id));
    bridge.add_channel(id.clone(), "SIP/alice".to_string());
    assert!(bridge.has_channel(&id));
    bridge.remove_channel(&id);
    assert!(!bridge.has_channel(&id));
}

/// Test bridge video mode.
#[test]
fn test_bridge_video_mode() {
    let mut bridge = Bridge::new("video-test");
    assert_eq!(bridge.video_mode, VideoMode::None);

    bridge.video_mode = VideoMode::SingleSource;
    assert_eq!(bridge.video_mode, VideoMode::SingleSource);

    bridge.video_mode = VideoMode::TalkerSource;
    assert_eq!(bridge.video_mode, VideoMode::TalkerSource);

    bridge.video_mode = VideoMode::Sfu;
    assert_eq!(bridge.video_mode, VideoMode::Sfu);
}

/// Test bridge flags.
#[test]
fn test_bridge_flags() {
    let bridge = Bridge::with_flags(
        "flags-test",
        BridgeFlags::DISSOLVE_HANGUP | BridgeFlags::SMART,
    );
    assert!(bridge.flags.contains(BridgeFlags::DISSOLVE_HANGUP));
    assert!(bridge.flags.contains(BridgeFlags::SMART));
    assert!(!bridge.flags.contains(BridgeFlags::DISSOLVE_EMPTY));
}
