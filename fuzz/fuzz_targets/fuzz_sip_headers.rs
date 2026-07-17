#![no_main]
//! Fuzz the REAL SIP header-value extractors directly on raw strings:
//! `extract_uri`, `extract_tag`, `parse_via`. These slice attacker-controlled
//! From/To/Contact/Via values and were the site of the #109 reversed-bracket
//! remote-crash panic.
use libfuzzer_sys::fuzz_target;

use asterisk_sip::parser::{extract_tag, extract_uri, parse_via};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = extract_uri(s);
        let _ = extract_tag(s);
        let _ = parse_via(s);
    }
});
