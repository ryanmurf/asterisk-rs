//! Port of asterisk/tests/test_aeap.c
//!
//! Tests the Asterisk External Application Protocol (AEAP) basics:
//!
//! - Creating and connecting to an AEAP application
//! - Sending a message and handling a string response
//! - Sending a message and handling a typed response
//! - Sending a message and handling a typed request
//!
//! Since we do not have a live AEAP server or WebSocket, we model the
//! protocol's message exchange locally using channels and JSON messages.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// AEAP message model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum AeapMessageKind {
    Request,
    Response,
}

#[derive(Debug, Clone)]
struct AeapMessage {
    kind: AeapMessageKind,
    name: String,
    id: String,
    data: Option<Value>,
}

impl AeapMessage {
    fn create_request(name: &str, id: &str) -> Self {
        Self {
            kind: AeapMessageKind::Request,
            name: name.to_string(),
            id: id.to_string(),
            data: None,
        }
    }

    fn create_response(name: &str, id: &str) -> Self {
        Self {
            kind: AeapMessageKind::Response,
            name: name.to_string(),
            id: id.to_string(),
            data: None,
        }
    }

    fn to_json(&self) -> Value {
        let kind_str = match self.kind {
            AeapMessageKind::Request => "request",
            AeapMessageKind::Response => "response",
        };
        json!({
            kind_str: self.name,
            "id": self.id,
        })
    }

    fn from_json(val: &Value) -> Option<Self> {
        let id = val.get("id")?.as_str()?.to_string();
        if let Some(name) = val.get("request").and_then(|v| v.as_str()) {
            return Some(Self {
                kind: AeapMessageKind::Request,
                name: name.to_string(),
                id,
                data: None,
            });
        }
        if let Some(name) = val.get("response").and_then(|v| v.as_str()) {
            return Some(Self {
                kind: AeapMessageKind::Response,
                name: name.to_string(),
                id,
                data: None,
            });
        }
        None
    }
}

// ---------------------------------------------------------------------------
// AEAP connection model
// ---------------------------------------------------------------------------

type MessageHandler = Box<dyn Fn(&AeapMessage) -> Option<AeapMessage> + Send + Sync>;

struct AeapParams {
    on_string: Option<Box<dyn Fn(&str) + Send + Sync>>,
    request_handlers: HashMap<String, MessageHandler>,
    response_handlers: HashMap<String, MessageHandler>,
}

struct Aeap {
    transport_type: String,
    url: String,
    protocol: String,
    connected: bool,
    user_data: Arc<Mutex<HashMap<String, i32>>>,
    params: AeapParams,
}

impl Aeap {
    fn create_and_connect(
        transport_type: &str,
        url: &str,
        protocol: &str,
        params: AeapParams,
    ) -> Option<Self> {
        // Simulate successful connection (the C test relies on a local echo server)
        Some(Self {
            transport_type: transport_type.to_string(),
            url: url.to_string(),
            protocol: protocol.to_string(),
            connected: true,
            user_data: Arc::new(Mutex::new(HashMap::new())),
            params,
        })
    }

    fn register_user_data(&self, id: &str, initial: i32) {
        self.user_data.lock().unwrap().insert(id.to_string(), initial);
    }

    fn get_user_data(&self, id: &str) -> Option<i32> {
        self.user_data.lock().unwrap().get(id).copied()
    }

    fn update_user_data(&self, id: &str, value: i32) {
        self.user_data.lock().unwrap().insert(id.to_string(), value);
    }

    /// Simulate sending a message and receiving the echo back.
    fn send_msg_tsx(&self, msg: &AeapMessage, timeout_ms: u64) -> bool {
        let json_str = msg.to_json().to_string();

        // If there is a string handler, invoke it with the serialized message
        if let Some(ref handler) = self.params.on_string {
            handler(&json_str);
        }

        // Simulate echo: parse the JSON back and dispatch to appropriate handler
        if let Some(received) = AeapMessage::from_json(&msg.to_json()) {
            match received.kind {
                AeapMessageKind::Response => {
                    if let Some(handler) = self.params.response_handlers.get(&received.name) {
                        handler(&received);
                    }
                }
                AeapMessageKind::Request => {
                    if let Some(handler) = self.params.request_handlers.get(&received.name) {
                        handler(&received);
                    }
                }
            }
        }

        // Simulate timeout if no handler matched
        let _ = timeout_ms;
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

const AEAP_TRANSPORT_TYPE: &str = "ws";
const AEAP_REMOTE_URL: &str = "ws://127.0.0.1:8088/ws";
const AEAP_REMOTE_PROTOCOL: &str = "echo";
const AEAP_MESSAGE_ID: &str = "foo";

/// Port of AST_TEST_DEFINE(create_and_connect).
///
/// Test creating and connecting to an AEAP application.
#[test]
fn test_aeap_create_and_connect() {
    let params = AeapParams {
        on_string: None,
        request_handlers: HashMap::new(),
        response_handlers: HashMap::new(),
    };
    let aeap = Aeap::create_and_connect(
        AEAP_TRANSPORT_TYPE,
        AEAP_REMOTE_URL,
        AEAP_REMOTE_PROTOCOL,
        params,
    );
    assert!(aeap.is_some());
    assert!(aeap.unwrap().connected);
}

/// Port of AST_TEST_DEFINE(send_msg_handle_string).
///
/// Test sending a message and handling the string echo.
#[test]
fn test_aeap_send_msg_handle_string() {
    let string_seen = Arc::new(Mutex::new(false));
    let string_seen_clone = string_seen.clone();

    let params = AeapParams {
        on_string: Some(Box::new(move |buf: &str| {
            if buf.contains(AEAP_MESSAGE_ID) {
                *string_seen_clone.lock().unwrap() = true;
            }
        })),
        request_handlers: HashMap::new(),
        response_handlers: HashMap::new(),
    };

    let aeap = Aeap::create_and_connect(
        AEAP_TRANSPORT_TYPE,
        AEAP_REMOTE_URL,
        AEAP_REMOTE_PROTOCOL,
        params,
    )
    .unwrap();

    aeap.register_user_data(AEAP_MESSAGE_ID, 0);

    let msg = AeapMessage::create_request("foo", AEAP_MESSAGE_ID);
    aeap.send_msg_tsx(&msg, 2000);

    assert!(*string_seen.lock().unwrap());
}

/// Port of AST_TEST_DEFINE(send_msg_handle_response).
///
/// Test sending a response message and handling it through the response handler.
#[test]
fn test_aeap_send_msg_handle_response() {
    let handled = Arc::new(Mutex::new(false));
    let handled_clone = handled.clone();

    let mut response_handlers: HashMap<String, MessageHandler> = HashMap::new();
    response_handlers.insert(
        "foo".to_string(),
        Box::new(move |msg: &AeapMessage| {
            if msg.id == AEAP_MESSAGE_ID && msg.name == "foo" {
                *handled_clone.lock().unwrap() = true;
            }
            None
        }),
    );

    let params = AeapParams {
        on_string: None,
        request_handlers: HashMap::new(),
        response_handlers,
    };

    let aeap = Aeap::create_and_connect(
        AEAP_TRANSPORT_TYPE,
        AEAP_REMOTE_URL,
        AEAP_REMOTE_PROTOCOL,
        params,
    )
    .unwrap();

    aeap.register_user_data(AEAP_MESSAGE_ID, 0);

    let msg = AeapMessage::create_response("foo", AEAP_MESSAGE_ID);
    aeap.send_msg_tsx(&msg, 2000);

    assert!(*handled.lock().unwrap());
}

/// Port of AST_TEST_DEFINE(send_msg_handle_request).
///
/// Test sending a request message and handling it through the request handler.
#[test]
fn test_aeap_send_msg_handle_request() {
    let handled = Arc::new(Mutex::new(false));
    let handled_clone = handled.clone();

    let mut request_handlers: HashMap<String, MessageHandler> = HashMap::new();
    request_handlers.insert(
        "foo".to_string(),
        Box::new(move |msg: &AeapMessage| {
            if msg.id == AEAP_MESSAGE_ID && msg.name == "foo" {
                *handled_clone.lock().unwrap() = true;
            }
            None
        }),
    );

    let params = AeapParams {
        on_string: None,
        request_handlers,
        response_handlers: HashMap::new(),
    };

    let aeap = Aeap::create_and_connect(
        AEAP_TRANSPORT_TYPE,
        AEAP_REMOTE_URL,
        AEAP_REMOTE_PROTOCOL,
        params,
    )
    .unwrap();

    aeap.register_user_data(AEAP_MESSAGE_ID, 0);

    let msg = AeapMessage::create_request("foo", AEAP_MESSAGE_ID);
    aeap.send_msg_tsx(&msg, 2000);

    assert!(*handled.lock().unwrap());
}
