//! Focused coverage for the WebSocket frame parser's *rejection* branches —
//! the RFC 6455 protocol-compliance and anti-DoS checks that the happy-path
//! round-trip tests in `websocket.rs` don't reach. `WebSocketFrame::parse`
//! runs on unauthenticated SIP-over-WS (WebRTC) sockets, so each of these
//! malformed frames is remotely reachable.

use asterisk_channels::websocket::WebSocketFrame;

#[test]
fn rejects_reserved_rsv_bits() {
    // byte0 = FIN(0x80) | RSV1(0x40); opcode 0 -> rsv != 0 must be an error.
    let frame = [0xC0u8, 0x00];
    assert!(WebSocketFrame::parse(&frame).is_err(), "RSV bits set must be rejected");
}

#[test]
fn rejects_unknown_opcode() {
    // opcode 0x3 is reserved / not a defined opcode.
    let frame = [0x83u8, 0x00];
    assert!(WebSocketFrame::parse(&frame).is_err(), "unknown opcode must be rejected");
}

#[test]
fn rejects_oversized_control_frame() {
    // RFC 6455 §5.5: control frames (here PING=0x9) MUST have payload <= 125.
    // Encode a 126-byte payload via the 16-bit length form.
    let mut frame = vec![0x89u8, 0x7E, 0x00, 0x7E];
    frame.extend(std::iter::repeat_n(0xAA, 126));
    assert!(
        WebSocketFrame::parse(&frame).is_err(),
        "control frame > 125 bytes must be rejected"
    );
}

#[test]
fn rejects_invalid_close_code() {
    // Close (0x8) frame with a body but code 999 (< 1000 is not usable).
    let frame = [0x88u8, 0x02, 0x03, 0xE7]; // 0x03E7 = 999
    assert!(
        WebSocketFrame::parse(&frame).is_err(),
        "close code 999 must be rejected"
    );
}

#[test]
fn accepts_valid_close_code() {
    // 1000 (Normal Closure) is valid on the wire.
    let frame = [0x88u8, 0x02, 0x03, 0xE8]; // 0x03E8 = 1000
    let parsed = WebSocketFrame::parse(&frame).expect("valid close frame");
    assert!(parsed.is_some());
}

#[test]
fn incomplete_masked_frame_returns_none_not_error() {
    // Masked, length 1, but only one of the four mask-key bytes present.
    // The parser needs more data -> Ok(None), not a hard error and not a panic.
    let frame = [0x81u8, 0x81, 0x01];
    assert!(matches!(WebSocketFrame::parse(&frame), Ok(None)));
}

#[test]
fn masked_payload_is_unmasked() {
    // Text frame, masked, 3-byte payload "abc" masked with key 01 02 03 04.
    let key = [0x01u8, 0x02, 0x03, 0x04];
    let plain = *b"abc";
    let masked: Vec<u8> = plain
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % 4])
        .collect();
    let mut frame = vec![0x81u8, 0x83, key[0], key[1], key[2], key[3]];
    frame.extend_from_slice(&masked);
    let (parsed, consumed) = WebSocketFrame::parse(&frame).unwrap().unwrap();
    assert_eq!(consumed, frame.len());
    assert_eq!(parsed.payload.as_ref(), &plain);
}
