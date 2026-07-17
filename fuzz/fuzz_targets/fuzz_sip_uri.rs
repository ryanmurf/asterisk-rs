#![no_main]
//! Fuzz the REAL SIP URI parser (`asterisk_sip::parser::SipUri::parse`), which
//! runs on the request-line URI of every inbound request and on Contact/Route
//! URIs. Exercises userinfo/host/port/param/header splitting incl. IPv6.
use libfuzzer_sys::fuzz_target;

use asterisk_sip::parser::SipUri;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(uri) = SipUri::parse(s) {
            // Round-trip the Display impl; must not panic on any parsed value.
            let _ = uri.to_string();
            let _ = uri.host_display();
        }
    }
});
