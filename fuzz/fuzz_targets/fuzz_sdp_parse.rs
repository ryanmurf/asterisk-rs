#![no_main]
//! Fuzz the REAL SDP parser (`asterisk_sip::sdp::SessionDescription::parse`),
//! which parses the body of every INVITE/UPDATE/ACK offer/answer. This also
//! transitively exercises the ICE candidate, DTLS fingerprint, setup and
//! bandwidth sub-parsers invoked from `a=` attribute handling.
use libfuzzer_sys::fuzz_target;

use asterisk_sip::sdp::SessionDescription;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = SessionDescription::parse(text);
    }
});
