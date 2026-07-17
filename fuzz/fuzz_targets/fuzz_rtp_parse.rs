#![no_main]
//! Fuzz the REAL RTP header/packet parser
//! (`asterisk_sip::rtp::{RtpHeader::parse, parse_rtp_header}`) — the code that
//! runs on every inbound media packet. `parse_rtp_header` is the fuller path:
//! it handles CSRC counts, the extension header length field, and RTP padding
//! (all attacker-controlled length arithmetic).
use libfuzzer_sys::fuzz_target;

use asterisk_sip::rtp::{parse_rtp_header, RtpHeader};

fuzz_target!(|data: &[u8]| {
    let _ = RtpHeader::parse(data);
    let _ = parse_rtp_header(data);
});
