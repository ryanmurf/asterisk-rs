//! Port of asterisk/tests/test_websocket_client.c
//!
//! Tests WebSocket frame parsing in asterisk-channels:
//! - Parse text frame
//! - Parse binary frame
//! - Parse close frame with code and reason
//! - Parse ping/pong
//! - Mask/unmask operations
//! - Maximum payload handling
//! - Frame serialization roundtrip
//! - Handshake accept key computation (RFC 6455)

use asterisk_channels::websocket::{
    close_code, compute_accept_key, build_upgrade_response,
    WebSocketFrame, WebSocketOpcode, MAX_PAYLOAD_SIZE,
};
use bytes::Bytes;

// ---------------------------------------------------------------------------
// Parse text frame
// ---------------------------------------------------------------------------

/// Port of the text frame parsing test from test_websocket_client.c.
///
/// Construct a text frame, serialize it, and parse it back.
#[test]
fn test_parse_text_frame() {
    let frame = WebSocketFrame::text("Hello, WebSocket!");
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert!(parsed.fin);
    assert_eq!(parsed.opcode, WebSocketOpcode::Text);
    assert!(!parsed.masked);
    assert_eq!(
        std::str::from_utf8(&parsed.payload).unwrap(),
        "Hello, WebSocket!"
    );
}

/// Test text frame with empty payload.
#[test]
fn test_parse_text_frame_empty() {
    let frame = WebSocketFrame::text("");
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert_eq!(parsed.opcode, WebSocketOpcode::Text);
    assert!(parsed.payload.is_empty());
}

// ---------------------------------------------------------------------------
// Parse binary frame
// ---------------------------------------------------------------------------

/// Port of the binary frame parsing test from test_websocket_client.c.
#[test]
fn test_parse_binary_frame() {
    let data = Bytes::from(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let frame = WebSocketFrame::binary(data.clone());
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert_eq!(parsed.opcode, WebSocketOpcode::Binary);
    assert_eq!(parsed.payload, data);
}

/// Test binary frame with 1024 bytes.
#[test]
fn test_parse_binary_frame_medium() {
    let data = Bytes::from(vec![0xAB; 1024]);
    let frame = WebSocketFrame::binary(data.clone());
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert_eq!(parsed.payload.len(), 1024);
}

// ---------------------------------------------------------------------------
// Parse close frame with code and reason
// ---------------------------------------------------------------------------

/// Port of the close frame parsing test from test_websocket_client.c.
#[test]
fn test_parse_close_frame_with_reason() {
    let frame = WebSocketFrame::close(close_code::NORMAL, "goodbye");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(parsed.opcode, WebSocketOpcode::Close);
    assert!(parsed.fin);

    // First 2 bytes are the status code.
    let status = u16::from_be_bytes([parsed.payload[0], parsed.payload[1]]);
    assert_eq!(status, 1000);

    // Remaining bytes are the reason.
    let reason = std::str::from_utf8(&parsed.payload[2..]).unwrap();
    assert_eq!(reason, "goodbye");
}

/// Test close frame with different codes.
#[test]
fn test_parse_close_frame_going_away() {
    let frame = WebSocketFrame::close(close_code::GOING_AWAY, "server shutting down");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    let status = u16::from_be_bytes([parsed.payload[0], parsed.payload[1]]);
    assert_eq!(status, close_code::GOING_AWAY);
}

/// Test close frame with no reason.
#[test]
fn test_parse_close_frame_no_reason() {
    let frame = WebSocketFrame::close(close_code::NORMAL, "");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(parsed.opcode, WebSocketOpcode::Close);
    let status = u16::from_be_bytes([parsed.payload[0], parsed.payload[1]]);
    assert_eq!(status, 1000);
    assert_eq!(parsed.payload.len(), 2); // Just the status code.
}

// ---------------------------------------------------------------------------
// Parse ping/pong
// ---------------------------------------------------------------------------

/// Port of the ping/pong test from test_websocket_client.c.
#[test]
fn test_parse_ping() {
    let frame = WebSocketFrame::ping(b"ping-data");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(parsed.opcode, WebSocketOpcode::Ping);
    assert_eq!(&parsed.payload[..], b"ping-data");
}

#[test]
fn test_parse_pong() {
    let frame = WebSocketFrame::pong(b"pong-data");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(parsed.opcode, WebSocketOpcode::Pong);
    assert_eq!(&parsed.payload[..], b"pong-data");
}

/// Test ping with empty payload.
#[test]
fn test_parse_ping_empty() {
    let frame = WebSocketFrame::ping(b"");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(parsed.opcode, WebSocketOpcode::Ping);
    assert!(parsed.payload.is_empty());
}

/// Test pong echoes ping payload.
#[test]
fn test_pong_echoes_ping_payload() {
    let ping = WebSocketFrame::ping(b"echo-me");
    let pong = WebSocketFrame::pong(&ping.payload);

    assert_eq!(pong.opcode, WebSocketOpcode::Pong);
    assert_eq!(pong.payload, ping.payload);
}

// ---------------------------------------------------------------------------
// Mask/unmask operations
// ---------------------------------------------------------------------------

/// Port of the masking test from test_websocket_client.c.
///
/// Test that masking a frame and parsing it back yields the original payload.
#[test]
fn test_mask_unmask_roundtrip() {
    let frame = WebSocketFrame::text("masked-data");
    let mask_key = [0x37, 0xFA, 0x21, 0x3D];
    let bytes = frame.to_bytes_with_mask(true, &mask_key);

    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    // After parsing, the payload should be automatically unmasked.
    assert_eq!(parsed.payload, Bytes::from("masked-data"));
}

/// Test masking with all-zero mask key (identity).
#[test]
fn test_mask_zero_key() {
    let frame = WebSocketFrame::text("no-change");
    let mask_key = [0x00, 0x00, 0x00, 0x00];
    let bytes = frame.to_bytes_with_mask(true, &mask_key);

    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.payload, Bytes::from("no-change"));
}

/// Test masking with binary data.
#[test]
fn test_mask_binary_data() {
    let data = Bytes::from(vec![0x00, 0xFF, 0xAA, 0x55, 0x12, 0x34]);
    let frame = WebSocketFrame::binary(data.clone());
    let mask_key = [0xAB, 0xCD, 0xEF, 0x01];
    let bytes = frame.to_bytes_with_mask(true, &mask_key);

    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.payload, data);
}

// ---------------------------------------------------------------------------
// Maximum payload handling
// ---------------------------------------------------------------------------

/// Port of the large payload test from test_websocket_client.c.
///
/// Test that a payload at the maximum allowed size works.
#[test]
fn test_max_payload_size() {
    let data = Bytes::from(vec![0xCD; MAX_PAYLOAD_SIZE]);
    let frame = WebSocketFrame::binary(data.clone());
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert_eq!(parsed.payload.len(), MAX_PAYLOAD_SIZE);
}

/// Test that a payload exceeding the maximum is rejected.
#[test]
fn test_payload_exceeds_max_rejected() {
    // Craft a header claiming a payload larger than MAX_PAYLOAD_SIZE.
    let mut buf = vec![0u8; 14];
    buf[0] = 0x82; // FIN + Binary
    buf[1] = 127; // 64-bit length
    let huge_len = (MAX_PAYLOAD_SIZE as u64) + 1;
    buf[2..10].copy_from_slice(&huge_len.to_be_bytes());
    // Add some fake data so the length check doesn't return Ok(None).
    buf.extend_from_slice(&vec![0u8; 1024]);

    let result = WebSocketFrame::parse(&buf);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Frame serialization roundtrip
// ---------------------------------------------------------------------------

/// Test complete roundtrip: create -> serialize -> parse for every frame type.
#[test]
fn test_roundtrip_all_types() {
    // Text
    let frame = WebSocketFrame::text("roundtrip");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.opcode, WebSocketOpcode::Text);
    assert_eq!(parsed.payload, Bytes::from("roundtrip"));

    // Binary
    let frame = WebSocketFrame::binary(Bytes::from(vec![1, 2, 3]));
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.opcode, WebSocketOpcode::Binary);

    // Close
    let frame = WebSocketFrame::close(close_code::NORMAL, "bye");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.opcode, WebSocketOpcode::Close);

    // Ping
    let frame = WebSocketFrame::ping(b"test");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.opcode, WebSocketOpcode::Ping);

    // Pong
    let frame = WebSocketFrame::pong(b"test");
    let bytes = frame.to_bytes();
    let (parsed, _) = WebSocketFrame::parse(&bytes).unwrap().unwrap();
    assert_eq!(parsed.opcode, WebSocketOpcode::Pong);
}

// ---------------------------------------------------------------------------
// Handshake tests (RFC 6455)
// ---------------------------------------------------------------------------

/// Test the accept key computation with the RFC 6455 test vector.
#[test]
fn test_compute_accept_key_rfc6455() {
    let key = compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==");
    assert_eq!(key, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

/// Test upgrade response generation.
#[test]
fn test_build_upgrade_response() {
    let response = build_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==", Some("sip"));
    assert!(response.contains("101 Switching Protocols"));
    assert!(response.contains("Upgrade: websocket"));
    assert!(response.contains("Connection: Upgrade"));
    assert!(response.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    assert!(response.contains("Sec-WebSocket-Protocol: sip"));
}

/// Test upgrade response without protocol.
#[test]
fn test_build_upgrade_response_no_protocol() {
    let response = build_upgrade_response("dGhlIHNhbXBsZSBub25jZQ==", None);
    assert!(response.contains("101 Switching Protocols"));
    assert!(!response.contains("Sec-WebSocket-Protocol"));
}

// ---------------------------------------------------------------------------
// Opcode classification
// ---------------------------------------------------------------------------

/// Test opcode is_control classification.
#[test]
fn test_opcode_is_control() {
    assert!(WebSocketOpcode::Close.is_control());
    assert!(WebSocketOpcode::Ping.is_control());
    assert!(WebSocketOpcode::Pong.is_control());
    assert!(!WebSocketOpcode::Text.is_control());
    assert!(!WebSocketOpcode::Binary.is_control());
    assert!(!WebSocketOpcode::Continuation.is_control());
}

/// Test opcode from_u8.
#[test]
fn test_opcode_from_u8() {
    assert_eq!(WebSocketOpcode::from_u8(0x0), Some(WebSocketOpcode::Continuation));
    assert_eq!(WebSocketOpcode::from_u8(0x1), Some(WebSocketOpcode::Text));
    assert_eq!(WebSocketOpcode::from_u8(0x2), Some(WebSocketOpcode::Binary));
    assert_eq!(WebSocketOpcode::from_u8(0x8), Some(WebSocketOpcode::Close));
    assert_eq!(WebSocketOpcode::from_u8(0x9), Some(WebSocketOpcode::Ping));
    assert_eq!(WebSocketOpcode::from_u8(0xA), Some(WebSocketOpcode::Pong));
    assert_eq!(WebSocketOpcode::from_u8(0x3), None);
    assert_eq!(WebSocketOpcode::from_u8(0xF), None);
}

// ---------------------------------------------------------------------------
// Incomplete frame handling
// ---------------------------------------------------------------------------

/// Test that incomplete data returns None (needs more data).
#[test]
fn test_incomplete_frame() {
    // Only 1 byte -- not enough.
    let result = WebSocketFrame::parse(&[0x81]).unwrap();
    assert!(result.is_none());
}

/// Test that an empty buffer returns None.
#[test]
fn test_empty_buffer() {
    let result = WebSocketFrame::parse(&[]).unwrap();
    assert!(result.is_none());
}

/// Test payload lengths requiring 16-bit length field.
#[test]
fn test_16bit_length_field() {
    let data = Bytes::from(vec![0xAB; 300]);
    let frame = WebSocketFrame::binary(data.clone());
    let bytes = frame.to_bytes();
    let (parsed, consumed) = WebSocketFrame::parse(&bytes).unwrap().unwrap();

    assert_eq!(consumed, bytes.len());
    assert_eq!(parsed.payload.len(), 300);
}

/// Test close frame with invalid status code is rejected.
#[test]
fn test_close_frame_invalid_status() {
    // Status code 0 is invalid per RFC 6455 sec 7.4.1.
    let mut buf = Vec::new();
    buf.push(0x88); // FIN + Close
    buf.push(0x02); // payload length = 2
    buf.push(0x00); // status code high byte
    buf.push(0x00); // status code low byte (code = 0)

    let result = WebSocketFrame::parse(&buf);
    assert!(result.is_err());
}
