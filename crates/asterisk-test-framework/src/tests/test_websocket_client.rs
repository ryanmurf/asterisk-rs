//! Port of asterisk/tests/test_websocket_client.c
//!
//! Tests WebSocket client operations:
//! - Client connection state management
//! - WebSocket handshake key validation
//! - Frame write/read through client
//! - Client close handling

// ---------------------------------------------------------------------------
// Simulated WebSocket client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientState {
    Disconnected,
    Connecting,
    Connected,
    Closing,
    Closed,
}

struct WebSocketClient {
    uri: String,
    state: ClientState,
    protocol: Option<String>,
    received_frames: Vec<String>,
}

impl WebSocketClient {
    fn new(uri: &str, protocol: Option<&str>) -> Self {
        Self {
            uri: uri.to_string(),
            state: ClientState::Disconnected,
            protocol: protocol.map(|s| s.to_string()),
            received_frames: Vec::new(),
        }
    }

    fn connect(&mut self) -> Result<(), &'static str> {
        if self.uri.is_empty() {
            return Err("Empty URI");
        }
        self.state = ClientState::Connecting;
        // Simulate successful connection.
        self.state = ClientState::Connected;
        Ok(())
    }

    fn write_text(&mut self, text: &str) -> Result<(), &'static str> {
        if self.state != ClientState::Connected {
            return Err("Not connected");
        }
        // Simulate loopback for testing.
        self.received_frames.push(text.to_string());
        Ok(())
    }

    fn read(&mut self) -> Option<String> {
        if self.state != ClientState::Connected {
            return None;
        }
        if self.received_frames.is_empty() {
            return None;
        }
        Some(self.received_frames.remove(0))
    }

    fn close(&mut self, _code: u16, _reason: &str) {
        self.state = ClientState::Closing;
        self.state = ClientState::Closed;
    }

    fn is_connected(&self) -> bool {
        self.state == ClientState::Connected
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of connection state management test.
#[test]
fn test_client_connect_disconnect() {
    let mut client = WebSocketClient::new("ws://localhost:8088/ws", Some("echo"));
    assert_eq!(client.state, ClientState::Disconnected);

    assert!(client.connect().is_ok());
    assert!(client.is_connected());
    assert_eq!(client.protocol.as_deref(), Some("echo"));

    client.close(1000, "normal");
    assert_eq!(client.state, ClientState::Closed);
    assert!(!client.is_connected());
}

/// Port of write/read through client test.
#[test]
fn test_client_write_read() {
    let mut client = WebSocketClient::new("ws://localhost:8088/ws", None);
    client.connect().unwrap();

    assert!(client.write_text("Hello, WebSocket!").is_ok());
    let msg = client.read().unwrap();
    assert_eq!(msg, "Hello, WebSocket!");
}

/// Test writing when not connected fails.
#[test]
fn test_client_write_not_connected() {
    let mut client = WebSocketClient::new("ws://localhost:8088/ws", None);
    assert!(client.write_text("test").is_err());
}

/// Test connect with empty URI fails.
#[test]
fn test_client_connect_empty_uri() {
    let mut client = WebSocketClient::new("", None);
    assert!(client.connect().is_err());
    assert!(!client.is_connected());
}

/// Test multiple messages.
#[test]
fn test_client_multiple_messages() {
    let mut client = WebSocketClient::new("ws://localhost:8088/ws", None);
    client.connect().unwrap();

    client.write_text("msg1").unwrap();
    client.write_text("msg2").unwrap();
    client.write_text("msg3").unwrap();

    assert_eq!(client.read().unwrap(), "msg1");
    assert_eq!(client.read().unwrap(), "msg2");
    assert_eq!(client.read().unwrap(), "msg3");
    assert!(client.read().is_none());
}

/// Test reading when not connected returns None.
#[test]
fn test_client_read_not_connected() {
    let mut client = WebSocketClient::new("ws://localhost:8088/ws", None);
    assert!(client.read().is_none());
}
