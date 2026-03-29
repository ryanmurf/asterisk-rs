//! Port of asterisk/tests/test_aeap_transport.c
//!
//! Tests AEAP transport layer:
//!
//! - Creating a transport with an invalid type fails
//! - Creating a transport with a valid URL scheme succeeds
//! - Connecting, verifying connection state, and disconnecting
//! - Connection failure with invalid URL or protocol
//! - Binary I/O through the transport
//! - String I/O through the transport
//!
//! Since we do not have a real WebSocket transport, we model the transport
//! as an in-memory loopback buffer.

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Transport model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataType {
    Binary,
    String,
}

struct Transport {
    scheme: String,
    connected: bool,
    buffer: VecDeque<(Vec<u8>, DataType)>,
}

impl Transport {
    /// Create a transport based on the URL scheme.
    /// Returns None for invalid/unsupported schemes.
    fn create(url_or_type: &str) -> Option<Self> {
        let scheme = if url_or_type.starts_with("ws://") || url_or_type.starts_with("wss://") {
            "ws".to_string()
        } else if url_or_type == "ws" || url_or_type == "wss" {
            url_or_type.to_string()
        } else {
            return None;
        };

        Some(Self {
            scheme,
            connected: false,
            buffer: VecDeque::new(),
        })
    }

    /// Create and immediately connect.
    fn create_and_connect(url: &str, _remote_url: &str, protocol: &str, _timeout: u64) -> Option<Self> {
        let mut t = Self::create(url)?;
        if t.connect(_remote_url, protocol, _timeout) {
            Some(t)
        } else {
            None
        }
    }

    /// Connect to the remote endpoint.
    /// Returns true on success.
    fn connect(&mut self, url: &str, protocol: &str, _timeout: u64) -> bool {
        // Simulate failure for invalid URLs or protocols
        if url.contains("/invalid") || protocol == "invalid" {
            return false;
        }
        self.connected = true;
        true
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn disconnect(&mut self) -> bool {
        self.connected = false;
        true
    }

    /// Write data to the transport (loopback: stored in internal buffer).
    fn write(&mut self, data: &[u8], dtype: DataType) -> usize {
        let len = data.len();
        self.buffer.push_back((data.to_vec(), dtype));
        len
    }

    /// Read data from the transport.
    fn read(&mut self, buf: &mut [u8]) -> Option<(usize, DataType)> {
        let (data, dtype) = self.buffer.pop_front()?;
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        Some((len, dtype))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(transport_create_invalid).
///
/// Creating a transport with an invalid type should fail.
#[test]
fn test_transport_create_invalid() {
    let transport = Transport::create("invalid");
    assert!(transport.is_none());
}

/// Port of AST_TEST_DEFINE(transport_create).
///
/// Creating a transport with a valid WebSocket URL should succeed.
#[test]
fn test_transport_create() {
    let transport = Transport::create("ws://127.0.0.1:8088/ws");
    assert!(transport.is_some());
    let t = transport.unwrap();
    assert_eq!(t.scheme, "ws");
    assert!(!t.is_connected());
}

/// Port of AST_TEST_DEFINE(transport_connect).
///
/// Test connecting, checking state, and disconnecting.
#[test]
fn test_transport_connect() {
    let transport = Transport::create_and_connect(
        "ws://127.0.0.1:8088/ws",
        "ws://127.0.0.1:8088/ws",
        "echo",
        2000,
    );
    assert!(transport.is_some());
    let mut t = transport.unwrap();
    assert!(t.is_connected());
    assert!(t.disconnect());
    assert!(!t.is_connected());
}

/// Port of AST_TEST_DEFINE(transport_connect_fail).
///
/// Test connection failure with invalid URL and invalid protocol.
#[test]
fn test_transport_connect_fail() {
    // Invalid address
    let mut t = Transport::create("ws://127.0.0.1:8088/ws").unwrap();
    let result = t.connect("ws://127.0.0.1:8088/invalid", "echo", 2000);
    assert!(!result);
    assert!(!t.is_connected());

    // Invalid protocol
    let mut t = Transport::create("ws://127.0.0.1:8088/ws").unwrap();
    let result = t.connect("ws://127.0.0.1:8088/ws", "invalid", 2000);
    assert!(!result);
    assert!(!t.is_connected());
}

/// Port of AST_TEST_DEFINE(transport_binary).
///
/// Test binary I/O through the transport.
#[test]
fn test_transport_binary() {
    let mut t = Transport::create_and_connect(
        "ws://127.0.0.1:8088/ws",
        "ws://127.0.0.1:8088/ws",
        "echo",
        2000,
    )
    .unwrap();

    let num: i32 = 38;
    let data = num.to_ne_bytes();
    let written = t.write(&data, DataType::Binary);
    assert_eq!(written, 4);

    let mut buf = [0u8; 4];
    let (read_len, rtype) = t.read(&mut buf).unwrap();
    assert_eq!(read_len, 4);
    assert_eq!(rtype, DataType::Binary);
    let result = i32::from_ne_bytes(buf);
    assert_eq!(result, 38);
}

/// Port of AST_TEST_DEFINE(transport_string).
///
/// Test string I/O through the transport.
#[test]
fn test_transport_string() {
    let mut t = Transport::create_and_connect(
        "ws://127.0.0.1:8088/ws",
        "ws://127.0.0.1:8088/ws",
        "echo",
        2000,
    )
    .unwrap();

    let msg = "foo bar baz";
    let written = t.write(msg.as_bytes(), DataType::String);
    assert_eq!(written, 11);

    let mut buf = [0u8; 16];
    let (read_len, rtype) = t.read(&mut buf).unwrap();
    assert_eq!(read_len, 11);
    assert_eq!(rtype, DataType::String);
    let result = std::str::from_utf8(&buf[..read_len]).unwrap();
    assert_eq!(result, "foo bar baz");
}
