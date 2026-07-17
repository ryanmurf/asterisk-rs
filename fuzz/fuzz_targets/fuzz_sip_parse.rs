#![no_main]
//! Fuzz the REAL SIP message parser (`asterisk_sip::parser::SipMessage::parse`)
//! — the exact entrypoint `UdpTransport::recv` / the TCP framer feed with
//! unauthenticated wire bytes. Also drives the header-URI/param extractors on
//! the parsed message, since those run inline on attacker-controlled From/To/
//! Contact values in `handle_request()` (see issues #108 / #109).
use libfuzzer_sys::fuzz_target;

use asterisk_sip::parser::{extract_tag, extract_uri, parse_via, SipMessage};

fuzz_target!(|data: &[u8]| {
    if let Ok(msg) = SipMessage::parse(data) {
        // Exercise the same helpers handle_request() runs on parsed headers.
        for name in ["From", "To", "Contact", "Via", "Route", "Record-Route"] {
            for value in msg.get_headers(name) {
                let _ = extract_uri(value);
                let _ = extract_tag(value);
                let _ = parse_via(value);
            }
        }
    }
});
