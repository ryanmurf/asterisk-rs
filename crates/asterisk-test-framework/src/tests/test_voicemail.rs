//! Port of asterisk/tests/test_voicemail_api.c
//!
//! Tests our voicemail module:
//! - Mailbox creation
//! - Message recording and storage
//! - Message counting per folder
//! - Message move between folders
//! - Message deletion
//! - Greeting management
//! - MWI state after message operations

use asterisk_apps::voicemail::{
    GreetingState, GreetingType, Mailbox, VoiceMessage, VoicemailFolder,
};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helper to create a test mailbox
// ---------------------------------------------------------------------------

fn test_mailbox() -> Mailbox {
    Mailbox::new(
        "100".to_string(),
        "default".to_string(),
        "1234".to_string(),
        "Alice Smith".to_string(),
    )
}

fn test_message(caller: &str, number: &str, duration: u32) -> VoiceMessage {
    VoiceMessage::new(
        caller.to_string(),
        number.to_string(),
        duration,
        PathBuf::from("/tmp/test.wav"),
    )
}

// ---------------------------------------------------------------------------
// Mailbox creation
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_create) from test_voicemail_api.c.
///
/// Test that a mailbox can be created with the correct properties.
#[test]
fn test_mailbox_creation() {
    let mb = test_mailbox();

    assert_eq!(mb.mailbox_number, "100");
    assert_eq!(mb.context, "default");
    assert_eq!(mb.password, "1234");
    assert_eq!(mb.fullname, "Alice Smith");
    assert_eq!(mb.full_id(), "100@default");
}

/// Test mailbox default settings.
#[test]
fn test_mailbox_defaults() {
    let mb = test_mailbox();

    // Greetings state: all initially unrecorded.
    assert!(!mb.greetings.has_greeting(GreetingType::Unavailable));
    assert!(!mb.greetings.has_greeting(GreetingType::Busy));
    assert_eq!(mb.recording_config.max_message_secs, 300);
    assert_eq!(mb.max_messages, 100);
    assert!(!mb.attach_voicemail);
    assert!(mb.email.is_none());
    assert!(mb.pager.is_none());
}

/// Test that all standard folders are created.
#[test]
fn test_mailbox_has_all_folders() {
    let mb = test_mailbox();

    for folder in VoicemailFolder::ALL.iter() {
        assert_eq!(mb.message_count(*folder), 0);
    }
}

// ---------------------------------------------------------------------------
// Message recording and storage
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_store) from test_voicemail_api.c.
///
/// Test that a message can be stored in the INBOX.
#[test]
fn test_message_store() {
    let mut mb = test_mailbox();

    let msg = test_message("Bob", "200", 30);
    assert!(mb.add_message(VoicemailFolder::Inbox, msg));

    assert_eq!(mb.new_message_count(), 1);
    assert_eq!(mb.total_message_count(), 1);
}

/// Test storing multiple messages.
#[test]
fn test_message_store_multiple() {
    let mut mb = test_mailbox();

    for i in 0..5 {
        let msg = test_message(&format!("Caller{}", i), &format!("20{}", i), 15 + i);
        assert!(mb.add_message(VoicemailFolder::Inbox, msg));
    }

    assert_eq!(mb.new_message_count(), 5);
    assert_eq!(mb.total_message_count(), 5);
}

/// Test message properties.
#[test]
fn test_message_properties() {
    let msg = test_message("Bob Jones", "5551234", 45);

    assert_eq!(msg.caller_id, "Bob Jones");
    assert_eq!(msg.caller_number, "5551234");
    assert_eq!(msg.duration, 45);
    assert_eq!(msg.folder, VoicemailFolder::Inbox);
    assert!(!msg.urgent);
    assert_eq!(msg.orig_context, "default");
    assert!(!msg.msg_id.is_empty());
}

// ---------------------------------------------------------------------------
// Message counting per folder
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_msg_count) from test_voicemail_api.c.
///
/// Test that message counts are correct per folder.
#[test]
fn test_message_count_per_folder() {
    let mut mb = test_mailbox();

    // Add 3 to INBOX.
    for _ in 0..3 {
        mb.add_message(VoicemailFolder::Inbox, test_message("A", "1", 10));
    }

    // Add 2 to Old.
    for _ in 0..2 {
        mb.add_message(VoicemailFolder::Old, test_message("B", "2", 20));
    }

    // Add 1 to Work.
    mb.add_message(VoicemailFolder::Work, test_message("C", "3", 30));

    assert_eq!(mb.new_message_count(), 3);
    assert_eq!(mb.old_message_count(), 2);
    assert_eq!(mb.message_count(VoicemailFolder::Work), 1);
    assert_eq!(mb.message_count(VoicemailFolder::Family), 0);
    assert_eq!(mb.message_count(VoicemailFolder::Friends), 0);
    assert_eq!(mb.total_message_count(), 6);
}

// ---------------------------------------------------------------------------
// Message move between folders
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_move) from test_voicemail_api.c.
///
/// Test that a message can be moved from INBOX to Old.
#[test]
fn test_message_move() {
    let mut mb = test_mailbox();

    let msg = test_message("Bob", "200", 30);
    let msg_id = msg.msg_id.clone();
    mb.add_message(VoicemailFolder::Inbox, msg);

    assert_eq!(mb.new_message_count(), 1);
    assert_eq!(mb.old_message_count(), 0);

    // Move from INBOX to Old.
    assert!(mb.move_message(&msg_id, VoicemailFolder::Inbox, VoicemailFolder::Old));

    assert_eq!(mb.new_message_count(), 0);
    assert_eq!(mb.old_message_count(), 1);
}

/// Test move non-existent message returns false.
#[test]
fn test_message_move_nonexistent() {
    let mut mb = test_mailbox();

    assert!(!mb.move_message(
        "nonexistent-id",
        VoicemailFolder::Inbox,
        VoicemailFolder::Old
    ));
}

/// Test move between non-Inbox folders.
#[test]
fn test_message_move_between_folders() {
    let mut mb = test_mailbox();

    let msg = test_message("Carol", "300", 20);
    let msg_id = msg.msg_id.clone();
    mb.add_message(VoicemailFolder::Old, msg);

    assert!(mb.move_message(
        &msg_id,
        VoicemailFolder::Old,
        VoicemailFolder::Work,
    ));

    assert_eq!(mb.old_message_count(), 0);
    assert_eq!(mb.message_count(VoicemailFolder::Work), 1);
}

// ---------------------------------------------------------------------------
// Message deletion
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_delete) from test_voicemail_api.c.
///
/// Test that a message can be deleted from a folder.
#[test]
fn test_message_delete() {
    let mut mb = test_mailbox();

    let msg = test_message("Bob", "200", 30);
    let msg_id = msg.msg_id.clone();
    mb.add_message(VoicemailFolder::Inbox, msg);

    assert_eq!(mb.new_message_count(), 1);

    assert!(mb.delete_message(&msg_id, VoicemailFolder::Inbox));
    assert_eq!(mb.new_message_count(), 0);
    assert_eq!(mb.total_message_count(), 0);
}

/// Test delete non-existent message returns false.
#[test]
fn test_message_delete_nonexistent() {
    let mut mb = test_mailbox();

    assert!(!mb.delete_message("nonexistent-id", VoicemailFolder::Inbox));
}

/// Test delete from wrong folder returns false.
#[test]
fn test_message_delete_wrong_folder() {
    let mut mb = test_mailbox();

    let msg = test_message("Bob", "200", 30);
    let msg_id = msg.msg_id.clone();
    mb.add_message(VoicemailFolder::Inbox, msg);

    // Try deleting from Old (wrong folder).
    assert!(!mb.delete_message(&msg_id, VoicemailFolder::Old));
    assert_eq!(mb.new_message_count(), 1); // Still there.
}

/// Test deleting specific message among multiple.
#[test]
fn test_message_delete_specific() {
    let mut mb = test_mailbox();

    let msg1 = test_message("Alice", "100", 10);
    let msg2 = test_message("Bob", "200", 20);
    let msg3 = test_message("Carol", "300", 30);
    let id2 = msg2.msg_id.clone();

    mb.add_message(VoicemailFolder::Inbox, msg1);
    mb.add_message(VoicemailFolder::Inbox, msg2);
    mb.add_message(VoicemailFolder::Inbox, msg3);

    assert_eq!(mb.new_message_count(), 3);

    // Delete Bob's message.
    assert!(mb.delete_message(&id2, VoicemailFolder::Inbox));
    assert_eq!(mb.new_message_count(), 2);

    // Verify remaining messages.
    let messages = mb.get_messages(VoicemailFolder::Inbox);
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.msg_id != id2));
}

// ---------------------------------------------------------------------------
// Greeting management
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(voicemail_api_nominal_greeting) from test_voicemail_api.c.
///
/// Test greeting state management: marking greetings as recorded
/// and resolving which greeting to play.
#[test]
fn test_greeting_management() {
    let mut mb = test_mailbox();

    // Initially no greetings are recorded.
    assert!(!mb.greetings.has_greeting(GreetingType::Unavailable));
    assert!(!mb.greetings.has_greeting(GreetingType::Busy));
    assert!(!mb.greetings.has_greeting(GreetingType::Name));
    assert!(!mb.greetings.has_greeting(GreetingType::Temp));

    // Record an unavailable greeting.
    mb.greetings.has_unavailable = true;
    assert!(mb.greetings.has_greeting(GreetingType::Unavailable));

    // Record a busy greeting.
    mb.greetings.has_busy = true;
    assert!(mb.greetings.has_greeting(GreetingType::Busy));

    // Record a name greeting.
    mb.greetings.has_name = true;
    assert!(mb.greetings.has_greeting(GreetingType::Name));

    // Set a temp greeting (overrides all others).
    mb.greetings.has_temp = true;
    assert!(mb.greetings.has_greeting(GreetingType::Temp));

    // Resolve: temp should take priority.
    let resolved = mb.greetings.resolve_greeting(GreetingType::Unavailable);
    assert_eq!(resolved, GreetingType::Temp);

    // Without temp, the requested type is used if available.
    mb.greetings.has_temp = false;
    let resolved = mb.greetings.resolve_greeting(GreetingType::Busy);
    assert_eq!(resolved, GreetingType::Busy);
}

// ---------------------------------------------------------------------------
// MWI state after message operations
// ---------------------------------------------------------------------------

/// Port of the MWI (Message Waiting Indicator) state test from test_voicemail_api.c.
///
/// Verify that new_message_count and old_message_count correctly reflect
/// the MWI state (new, old counts) as messages are added, moved, and deleted.
#[test]
fn test_mwi_state_lifecycle() {
    let mut mb = test_mailbox();

    // Initially: 0 new, 0 old.
    assert_eq!(mb.new_message_count(), 0);
    assert_eq!(mb.old_message_count(), 0);

    // Record a new message -> 1 new, 0 old.
    let msg1 = test_message("Bob", "200", 30);
    let id1 = msg1.msg_id.clone();
    mb.add_message(VoicemailFolder::Inbox, msg1);
    assert_eq!(mb.new_message_count(), 1);
    assert_eq!(mb.old_message_count(), 0);

    // Record another -> 2 new, 0 old.
    let msg2 = test_message("Carol", "300", 20);
    let id2 = msg2.msg_id.clone();
    mb.add_message(VoicemailFolder::Inbox, msg2);
    assert_eq!(mb.new_message_count(), 2);
    assert_eq!(mb.old_message_count(), 0);

    // User listens to msg1, move to Old -> 1 new, 1 old.
    mb.move_message(&id1, VoicemailFolder::Inbox, VoicemailFolder::Old);
    assert_eq!(mb.new_message_count(), 1);
    assert_eq!(mb.old_message_count(), 1);

    // User deletes msg2 from Inbox -> 0 new, 1 old.
    mb.delete_message(&id2, VoicemailFolder::Inbox);
    assert_eq!(mb.new_message_count(), 0);
    assert_eq!(mb.old_message_count(), 1);

    // User deletes msg1 from Old -> 0 new, 0 old.
    mb.delete_message(&id1, VoicemailFolder::Old);
    assert_eq!(mb.new_message_count(), 0);
    assert_eq!(mb.old_message_count(), 0);
    assert_eq!(mb.total_message_count(), 0);
}

// ---------------------------------------------------------------------------
// Mailbox password and optional fields
// ---------------------------------------------------------------------------

/// Test password verification.
#[test]
fn test_mailbox_password() {
    let mb = test_mailbox();

    assert!(mb.verify_password("1234"));
    assert!(!mb.verify_password("wrong"));
    assert!(!mb.verify_password(""));
}

/// Test mailbox email/pager settings.
#[test]
fn test_mailbox_email_settings() {
    let mut mb = test_mailbox();

    mb.email = Some("alice@example.com".to_string());
    mb.pager = Some("pager@example.com".to_string());
    mb.attach_voicemail = true;

    assert_eq!(mb.email.as_deref(), Some("alice@example.com"));
    assert_eq!(mb.pager.as_deref(), Some("pager@example.com"));
    assert!(mb.attach_voicemail);
}

/// Test mailbox base directory.
#[test]
fn test_mailbox_base_dir() {
    let mb = test_mailbox();
    let base = mb.base_dir();
    assert_eq!(
        base,
        PathBuf::from("/var/spool/asterisk/voicemail/default/100")
    );
}

// ---------------------------------------------------------------------------
// VoicemailFolder tests
// ---------------------------------------------------------------------------

/// Test folder name/number conversions.
#[test]
fn test_folder_conversions() {
    for folder in VoicemailFolder::ALL.iter() {
        let num = folder.number();
        let recovered = VoicemailFolder::from_number(num);
        assert_eq!(recovered, Some(*folder));
    }

    assert!(VoicemailFolder::from_number(99).is_none());
}

/// Test folder display names.
#[test]
fn test_folder_display_names() {
    assert_eq!(VoicemailFolder::Inbox.name(), "INBOX");
    assert_eq!(VoicemailFolder::Old.name(), "Old");
    assert_eq!(VoicemailFolder::Work.name(), "Work");
    assert_eq!(VoicemailFolder::Family.name(), "Family");
    assert_eq!(VoicemailFolder::Friends.name(), "Friends");
}

/// Test folder dir_name.
#[test]
fn test_folder_dir_name() {
    assert_eq!(VoicemailFolder::Inbox.dir_name(), "INBOX");
    assert_eq!(VoicemailFolder::Old.dir_name(), "Old");
}
