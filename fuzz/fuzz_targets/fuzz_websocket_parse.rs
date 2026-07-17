#![no_main]
//! Fuzz the REAL WebSocket frame parser
//! (`asterisk_channels::websocket::WebSocketFrame::parse`) used for SIP-over-WS
//! (RFC 7118) WebRTC signalling. Drives the FIN/RSV/opcode bits, the 7/16/64-bit
//! payload-length encodings, masking-key handling and control-frame rules — all
//! attacker-controlled framing arithmetic.
use libfuzzer_sys::fuzz_target;

use asterisk_channels::websocket::WebSocketFrame;

fuzz_target!(|data: &[u8]| {
    let _ = WebSocketFrame::parse(data);
});
