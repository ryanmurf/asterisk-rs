//! Port of asterisk/tests/test_message.c
//!
//! Tests out-of-call text message handling:
//! - Message creation with to/from/body
//! - Message variable set/get
//! - Message routing (technology-based)
//! - Message handler registration
//! - Message destination checking
//! - Message sending
//! - Multiple variables per message

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Message system mirroring Asterisk's ast_msg
// ---------------------------------------------------------------------------

/// A text message, mirroring struct ast_msg.
#[derive(Debug, Clone)]
struct Message {
    to: String,
    from: String,
    body: String,
    vars: HashMap<String, String>,
}

impl Message {
    fn new() -> Self {
        Self {
            to: String::new(),
            from: String::new(),
            body: String::new(),
            vars: HashMap::new(),
        }
    }

    fn set_to(&mut self, to: &str) {
        self.to = to.to_string();
    }

    fn set_from(&mut self, from: &str) {
        self.from = from.to_string();
    }

    fn set_body(&mut self, body: &str) {
        self.body = body.to_string();
    }

    fn get_to(&self) -> &str {
        &self.to
    }

    fn get_from(&self) -> &str {
        &self.from
    }

    fn get_body(&self) -> &str {
        &self.body
    }

    fn set_var(&mut self, key: &str, value: &str) {
        self.vars.insert(key.to_string(), value.to_string());
    }

    fn get_var(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(|s| s.as_str())
    }
}

/// Message technology -- handles sending messages for a protocol.
trait MsgTech {
    fn name(&self) -> &str;
    fn msg_send(&self, msg: &Message, to: &str, from: &str) -> Result<(), String>;
}

/// Test message technology.
struct TestMsgTech {
    sent_messages: Arc<Mutex<Vec<(String, String)>>>,
}

impl TestMsgTech {
    fn new() -> Self {
        Self {
            sent_messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn sent_count(&self) -> usize {
        self.sent_messages.lock().unwrap().len()
    }
}

impl MsgTech for TestMsgTech {
    fn name(&self) -> &str {
        "testmsg"
    }

    fn msg_send(&self, _msg: &Message, to: &str, from: &str) -> Result<(), String> {
        self.sent_messages
            .lock()
            .unwrap()
            .push((to.to_string(), from.to_string()));
        Ok(())
    }
}

/// Message handler -- processes incoming messages.
trait MsgHandler {
    fn name(&self) -> &str;
    fn has_destination(&self, msg: &Message) -> bool;
    fn handle_msg(&self, msg: &Message) -> Result<(), String>;
}

struct TestMsgHandler {
    received: Arc<Mutex<Vec<String>>>,
}

impl TestMsgHandler {
    fn new() -> Self {
        Self {
            received: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn received_count(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

impl MsgHandler for TestMsgHandler {
    fn name(&self) -> &str {
        "testmsg"
    }

    fn has_destination(&self, msg: &Message) -> bool {
        msg.get_to() == "foo"
    }

    fn handle_msg(&self, msg: &Message) -> Result<(), String> {
        self.received.lock().unwrap().push(msg.get_body().to_string());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests: Message creation
// ---------------------------------------------------------------------------

#[test]
fn test_message_create() {
    let msg = Message::new();
    assert!(msg.get_to().is_empty());
    assert!(msg.get_from().is_empty());
    assert!(msg.get_body().is_empty());
}

#[test]
fn test_message_set_fields() {
    let mut msg = Message::new();
    msg.set_to("sip:alice@example.com");
    msg.set_from("sip:bob@example.com");
    msg.set_body("Hello, Alice!");

    assert_eq!(msg.get_to(), "sip:alice@example.com");
    assert_eq!(msg.get_from(), "sip:bob@example.com");
    assert_eq!(msg.get_body(), "Hello, Alice!");
}

#[test]
fn test_message_overwrite_fields() {
    let mut msg = Message::new();
    msg.set_to("original");
    msg.set_to("updated");
    assert_eq!(msg.get_to(), "updated");
}

// ---------------------------------------------------------------------------
// Tests: Message variables
// ---------------------------------------------------------------------------

#[test]
fn test_message_variables() {
    let mut msg = Message::new();
    msg.set_var("key1", "value1");
    msg.set_var("key2", "value2");

    assert_eq!(msg.get_var("key1"), Some("value1"));
    assert_eq!(msg.get_var("key2"), Some("value2"));
    assert_eq!(msg.get_var("key3"), None);
}

#[test]
fn test_message_variable_overwrite() {
    let mut msg = Message::new();
    msg.set_var("key", "original");
    msg.set_var("key", "updated");
    assert_eq!(msg.get_var("key"), Some("updated"));
}

#[test]
fn test_message_multiple_variables() {
    let mut msg = Message::new();
    for i in 0..100 {
        msg.set_var(&format!("var{}", i), &format!("val{}", i));
    }

    for i in 0..100 {
        assert_eq!(
            msg.get_var(&format!("var{}", i)),
            Some(format!("val{}", i).as_str())
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: Message technology (sending)
// ---------------------------------------------------------------------------

#[test]
fn test_message_tech_send() {
    let tech = TestMsgTech::new();
    let msg = Message::new();

    let result = tech.msg_send(&msg, "sip:alice@example.com", "sip:bob@example.com");
    assert!(result.is_ok());
    assert_eq!(tech.sent_count(), 1);
}

#[test]
fn test_message_tech_name() {
    let tech = TestMsgTech::new();
    assert_eq!(tech.name(), "testmsg");
}

#[test]
fn test_message_tech_multiple_sends() {
    let tech = TestMsgTech::new();
    let msg = Message::new();

    for _ in 0..5 {
        tech.msg_send(&msg, "to", "from").unwrap();
    }
    assert_eq!(tech.sent_count(), 5);
}

// ---------------------------------------------------------------------------
// Tests: Message handler (routing)
// ---------------------------------------------------------------------------

#[test]
fn test_message_handler_has_destination() {
    let handler = TestMsgHandler::new();
    let mut msg = Message::new();

    // "foo" is our expected destination.
    msg.set_to("foo");
    assert!(handler.has_destination(&msg));

    msg.set_to("bar");
    assert!(!handler.has_destination(&msg));

    msg.set_to("");
    assert!(!handler.has_destination(&msg));
}

#[test]
fn test_message_handler_handle() {
    let handler = TestMsgHandler::new();
    let mut msg = Message::new();
    msg.set_body("test message");

    let result = handler.handle_msg(&msg);
    assert!(result.is_ok());
    assert_eq!(handler.received_count(), 1);
}

#[test]
fn test_message_handler_name() {
    let handler = TestMsgHandler::new();
    assert_eq!(handler.name(), "testmsg");
}

// ---------------------------------------------------------------------------
// Tests: Message serialization/clone
// ---------------------------------------------------------------------------

#[test]
fn test_message_clone() {
    let mut msg = Message::new();
    msg.set_to("to");
    msg.set_from("from");
    msg.set_body("body");
    msg.set_var("key", "value");

    let cloned = msg.clone();
    assert_eq!(cloned.get_to(), msg.get_to());
    assert_eq!(cloned.get_from(), msg.get_from());
    assert_eq!(cloned.get_body(), msg.get_body());
    assert_eq!(cloned.get_var("key"), msg.get_var("key"));
}

#[test]
fn test_message_empty_body() {
    let msg = Message::new();
    assert!(msg.get_body().is_empty());
    assert_eq!(msg.get_body(), "");
}
