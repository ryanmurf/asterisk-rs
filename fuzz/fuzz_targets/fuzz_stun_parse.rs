#![no_main]
//! Fuzz the REAL STUN parser (`asterisk_sip::stun::StunMessage::parse` and the
//! single-attribute `RawAttribute::parse`). STUN binding requests arrive
//! unauthenticated on the ICE/RTP socket, so this is pre-auth remote input.
//! `StunMessage::parse` transitively drives the XOR-MAPPED-ADDRESS / ERROR-CODE
//! attribute decoders.
use libfuzzer_sys::fuzz_target;

use asterisk_sip::stun::{RawAttribute, StunMessage};

fuzz_target!(|data: &[u8]| {
    let _ = StunMessage::parse(data);
    let _ = RawAttribute::parse(data);
});
