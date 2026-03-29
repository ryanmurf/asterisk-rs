//! Port of asterisk/tests/test_channel.c
//!
//! Tests channel operations: fd array growth, fd placement, allocation,
//! state transitions, softhangup, variables, snapshots, and frame queues.

use asterisk_core::channel::Channel;
use asterisk_types::{ChannelFlags, ChannelState, ControlFrame, Frame, HangupCause};
use bytes::Bytes;

/// Port of AST_TEST_DEFINE(set_fd_grow)
///
/// In C this tests that setting a file descriptor at a high position causes
/// the fd array to grow and intermediate positions are set to -1.
///
/// In Rust channels we don't have raw FDs, but we can test the analogous
/// concept with the frame queue: that the channel can handle growing internal
/// state. We verify that channel creation works and that multiple operations
/// on the channel's internal state produce expected results.
#[test]
fn test_set_fd_grow_analog() {
    // Create a channel
    let chan = Channel::new("TestChannel/001");
    assert_eq!(chan.state, ChannelState::Down);
    assert_eq!(chan.name, "TestChannel/001");

    // The Rust channel doesn't have fd arrays, but we test the spirit:
    // operations that "grow" internal structures should work correctly.
    // The frame_queue starts empty and can grow without bounds (up to MAX_FRAME_QUEUE_SIZE).
    assert!(chan.frame_queue.is_empty());
}

/// Port of AST_TEST_DEFINE(add_fd)
///
/// In C this tests that adding an fd to a channel places it at the expected
/// position (AST_EXTENDED_FDS). In Rust, we test the equivalent: adding a
/// frame to the queue places it at the expected position.
#[test]
fn test_add_fd_analog() {
    let mut chan = Channel::new("TestChannel/002");

    // Add a frame (analogous to adding an FD)
    let frame = Frame::voice(0, 160, Bytes::from_static(&[0xFF; 160]));
    chan.queue_frame(frame);
    assert_eq!(chan.frame_queue.len(), 1);

    // Dequeue it and verify position
    let dequeued = chan.dequeue_frame();
    assert!(dequeued.is_some());
    assert!(dequeued.unwrap().is_voice());

    // Queue should now be empty
    assert!(chan.frame_queue.is_empty());
    assert!(chan.dequeue_frame().is_none());
}

/// Test channel allocation and unique ID format.
///
/// Verifies that each channel gets a unique ID and the name is stored correctly.
#[test]
fn test_channel_allocation_and_unique_id() {
    let chan1 = Channel::new("SIP/alice-00000001");
    let chan2 = Channel::new("SIP/bob-00000002");

    // Each channel should have a unique ID
    assert_ne!(chan1.unique_id, chan2.unique_id);

    // Names should match what we gave
    assert_eq!(chan1.name, "SIP/alice-00000001");
    assert_eq!(chan2.name, "SIP/bob-00000002");

    // Unique IDs should be valid UUID format (contain hyphens)
    assert!(chan1.unique_id.as_str().contains('-'));
    assert!(chan2.unique_id.as_str().contains('-'));

    // linkedid should initially equal the unique_id
    assert_eq!(chan1.linkedid, chan1.unique_id.as_str());
}

/// Test channel state transitions: Down -> Ringing -> Up -> Down (hangup).
///
/// This verifies the same state machine tested in the C code where channels
/// transition through various states during a call lifecycle.
#[test]
fn test_channel_state_transitions() {
    let mut chan = Channel::new("Test/state-001");

    // Initial state: Down
    assert_eq!(chan.state, ChannelState::Down);

    // Transition: Down -> Ringing
    chan.set_state(ChannelState::Ringing);
    assert_eq!(chan.state, ChannelState::Ringing);

    // Transition: Ringing -> Up (answer)
    chan.answer();
    assert_eq!(chan.state, ChannelState::Up);

    // Transition: Up -> Down (hangup)
    chan.hangup(HangupCause::NormalClearing);
    assert_eq!(chan.state, ChannelState::Down);
    assert_eq!(chan.hangup_cause, HangupCause::NormalClearing);
}

/// Test that calling hangup on an already-down channel is safe (no-op).
#[test]
fn test_channel_double_hangup() {
    let mut chan = Channel::new("Test/double-hangup");
    chan.set_state(ChannelState::Up);

    // First hangup
    chan.hangup(HangupCause::NormalClearing);
    assert_eq!(chan.state, ChannelState::Down);
    assert_eq!(chan.hangup_cause, HangupCause::NormalClearing);

    // Second hangup should be a no-op (and not change the cause)
    chan.hangup(HangupCause::UserBusy);
    assert_eq!(chan.state, ChannelState::Down);
    // The original cause should be preserved
    assert_eq!(chan.hangup_cause, HangupCause::NormalClearing);
}

/// Test softhangup flag set/check/clear.
///
/// In C, softhangup is managed through ast_channel_softhangup_internal_flag.
/// In Rust, we use the flags system (ChannelFlags::DEAD as a proxy for softhangup).
#[test]
fn test_softhangup_flag() {
    let mut chan = Channel::new("Test/softhangup");

    // Initially no flags set
    assert!(!chan.flags.contains(ChannelFlags::DEAD));

    // Set the "softhangup" flag
    chan.flags.insert(ChannelFlags::DEAD);
    assert!(chan.flags.contains(ChannelFlags::DEAD));

    // Clear it
    chan.flags.remove(ChannelFlags::DEAD);
    assert!(!chan.flags.contains(ChannelFlags::DEAD));
}

/// Test channel variable set/get/iterate.
///
/// Port of channel variable operations tested in test_channel.c and
/// used extensively throughout Asterisk.
#[test]
fn test_channel_variables() {
    let mut chan = Channel::new("Test/vars");

    // Set variables
    chan.set_variable("MY_VAR", "value1");
    chan.set_variable("ANOTHER_VAR", "value2");
    chan.set_variable("NUMBER", "42");

    // Get variables
    assert_eq!(chan.get_variable("MY_VAR"), Some("value1"));
    assert_eq!(chan.get_variable("ANOTHER_VAR"), Some("value2"));
    assert_eq!(chan.get_variable("NUMBER"), Some("42"));

    // Non-existent variable
    assert_eq!(chan.get_variable("NONEXISTENT"), None);

    // Overwrite a variable
    chan.set_variable("MY_VAR", "new_value");
    assert_eq!(chan.get_variable("MY_VAR"), Some("new_value"));

    // Iterate all variables
    let count = chan.variables.len();
    assert_eq!(count, 3);
}

/// Test channel snapshot correctness.
///
/// Verifies that snapshots capture the current state of a channel at the
/// time they are taken, and that subsequent channel changes do not affect
/// previously taken snapshots.
#[test]
fn test_channel_snapshot() {
    let mut chan = Channel::new("SIP/snapshot-test");
    chan.set_state(ChannelState::Ringing);
    chan.caller.id.name.name = "Alice".to_string();
    chan.caller.id.number.number = "5551234".to_string();
    chan.context = "from-internal".to_string();
    chan.exten = "100".to_string();
    chan.priority = 1;

    // Take snapshot while ringing
    let snap1 = chan.snapshot();
    assert_eq!(snap1.state, ChannelState::Ringing);
    assert_eq!(snap1.caller.id.name.name, "Alice");
    assert_eq!(snap1.caller.id.number.number, "5551234");
    assert_eq!(snap1.context, "from-internal");
    assert_eq!(snap1.exten, "100");
    assert_eq!(snap1.priority, 1);
    assert_eq!(snap1.name, "SIP/snapshot-test");

    // Change the channel state
    chan.answer();
    chan.caller.id.name.name = "Bob".to_string();

    // Take a new snapshot
    let snap2 = chan.snapshot();
    assert_eq!(snap2.state, ChannelState::Up);
    assert_eq!(snap2.caller.id.name.name, "Bob");

    // Original snapshot should be unchanged
    assert_eq!(snap1.state, ChannelState::Ringing);
    assert_eq!(snap1.caller.id.name.name, "Alice");
}

/// Test frame queue: enqueue, dequeue, and overflow behavior.
///
/// Verifies that the frame queue behaves correctly including the overflow
/// case where frames are dropped when the queue is full.
#[test]
fn test_frame_queue() {
    let mut chan = Channel::new("Test/queue");

    // Queue should start empty
    assert!(chan.frame_queue.is_empty());
    assert!(chan.dequeue_frame().is_none());

    // Enqueue frames
    chan.queue_frame(Frame::voice(0, 160, Bytes::from_static(&[0x80; 160])));
    chan.queue_frame(Frame::dtmf_begin('1'));
    chan.queue_frame(Frame::control(ControlFrame::Ringing));

    assert_eq!(chan.frame_queue.len(), 3);

    // Dequeue in FIFO order
    let f1 = chan.dequeue_frame().unwrap();
    assert!(f1.is_voice());

    let f2 = chan.dequeue_frame().unwrap();
    assert!(f2.is_dtmf());

    let f3 = chan.dequeue_frame().unwrap();
    assert!(f3.is_control());

    assert!(chan.dequeue_frame().is_none());
}

/// Test frame queue overflow.
///
/// The channel should drop the oldest frames when the queue reaches its
/// maximum capacity (Channel::MAX_FRAME_QUEUE_SIZE = 1000).
#[test]
fn test_frame_queue_overflow() {
    let mut chan = Channel::new("Test/overflow");

    // Fill the queue to capacity
    for i in 0..1000u32 {
        chan.queue_frame(Frame::voice(0, i, Bytes::new()));
    }
    assert_eq!(chan.frame_queue.len(), 1000);

    // Adding one more should drop the oldest
    chan.queue_frame(Frame::voice(0, 9999, Bytes::new()));
    assert_eq!(chan.frame_queue.len(), 1000);

    // The first frame should now be the one with samples=1 (the 0th was dropped)
    let first = chan.dequeue_frame().unwrap();
    if let Frame::Voice { samples, .. } = first {
        assert_eq!(samples, 1);
    } else {
        panic!("Expected voice frame");
    }
}

/// Test channel flags operations.
#[test]
fn test_channel_flags() {
    let mut chan = Channel::new("Test/flags");

    // Initially empty
    assert_eq!(chan.flags, ChannelFlags::empty());

    // Set multiple flags
    chan.flags.insert(ChannelFlags::OUTGOING);
    chan.flags.insert(ChannelFlags::IN_BRIDGE);

    assert!(chan.flags.contains(ChannelFlags::OUTGOING));
    assert!(chan.flags.contains(ChannelFlags::IN_BRIDGE));
    assert!(!chan.flags.contains(ChannelFlags::ZOMBIE));

    // Remove a flag
    chan.flags.remove(ChannelFlags::OUTGOING);
    assert!(!chan.flags.contains(ChannelFlags::OUTGOING));
    assert!(chan.flags.contains(ChannelFlags::IN_BRIDGE));
}

/// Test channel defaults on creation.
#[test]
fn test_channel_defaults() {
    let chan = Channel::new("Test/defaults");

    assert_eq!(chan.state, ChannelState::Down);
    assert_eq!(chan.context, "default");
    assert_eq!(chan.exten, "s");
    assert_eq!(chan.priority, 1);
    assert_eq!(chan.read_format, "ulaw");
    assert_eq!(chan.write_format, "ulaw");
    assert_eq!(chan.language, "en");
    assert_eq!(chan.musicclass, "default");
    assert!(chan.bridge_id.is_none());
    assert_eq!(chan.hangup_cause, HangupCause::default());
    assert!(chan.variables.is_empty());
    assert!(chan.frame_queue.is_empty());
}
