#![no_main]
//! Fuzz the REAL AMI protocol parsers
//! (`asterisk_ami::protocol::{AmiAction::parse, AmiEvent::parse, read_message}`).
//! AMI accepts CRLF-framed key/value text from manager clients; `read_message`
//! frames it and `AmiAction::parse` interprets it (pre/post-auth).
use libfuzzer_sys::fuzz_target;

use asterisk_ami::protocol::{read_message, AmiAction, AmiEvent};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = read_message(s);
        let _ = AmiAction::parse(s);
        let _ = AmiEvent::parse(s);
    }
});
