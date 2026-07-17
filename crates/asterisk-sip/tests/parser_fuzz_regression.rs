//! Deterministic mutation / property tests for the SIP-stack wire parsers.
//!
//! `cargo fuzz` needs a nightly toolchain + libFuzzer, so it does not run in
//! the pinned-stable CI. These tests exercise the **same real parsers** the
//! fuzz targets wrap, feeding them a large battery of malformed and
//! byte-mutated inputs and asserting they return an `Err`/`None` (or a valid
//! value) rather than panicking. A remotely-triggerable parser panic is a DoS
//! (the datagram parse runs inline on the single SIP event loop), so "never
//! panic on any input" is the invariant under test.
//!
//! The battery is seeded from a fixed PRNG, so a failure is reproducible: the
//! offending input bytes are printed.

use std::panic::{catch_unwind, AssertUnwindSafe};

use asterisk_sip::auth::DigestChallenge;
use asterisk_sip::authenticator::parse_authorization;
use asterisk_sip::diversion::{parse_diversion_header, parse_history_info};
use asterisk_sip::geolocation::parse_geolocation_header;
use asterisk_sip::messaging::MessageContentType;
use asterisk_sip::multipart::parse_multipart;
use asterisk_sip::parser::{extract_tag, extract_uri, parse_via, SipMessage, SipUri};
use asterisk_sip::rtp::avpf::RtcpFeedback;
use asterisk_sip::rtp::{parse_rtp_header, RtpHeader};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::stun::{RawAttribute, StunMessage};
use asterisk_sip::turn::decode_channel_data;

/// Small deterministic xorshift PRNG (reproducible battery).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// Apply a short random chain of byte-level mutations to `seed`.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    const INTERESTING: [u8; 14] = [
        b'<', b'>', b':', b';', b'@', b'?', b'[', b']', 0x80, 0xff, 0x00, b'\r', b'\n', b'=',
    ];
    let mut v = seed.to_vec();
    for _ in 0..=rng.pick(6) {
        if v.is_empty() {
            v.push(rng.next() as u8);
            continue;
        }
        match rng.pick(8) {
            0 => {
                let i = rng.pick(v.len());
                v[i] ^= 1u8 << rng.pick(8);
            }
            1 => {
                let i = rng.pick(v.len());
                v[i] = rng.next() as u8;
            }
            2 => {
                let i = rng.pick(v.len() + 1);
                v.insert(i, rng.next() as u8);
            }
            3 => {
                let i = rng.pick(v.len());
                v.remove(i);
            }
            4 => {
                let i = rng.pick(v.len());
                v.truncate(i);
            }
            5 => {
                let i = rng.pick(v.len());
                let len = rng.pick(v.len() - i) + 1;
                let region = v[i..i + len].to_vec();
                let at = rng.pick(v.len() + 1);
                for (k, b) in region.into_iter().enumerate() {
                    v.insert(at + k, b);
                }
            }
            6 => {
                let i = rng.pick(v.len() + 1);
                v.insert(i, INTERESTING[rng.pick(INTERESTING.len())]);
            }
            _ => {
                let i = rng.pick(v.len());
                v[i] = 0xff;
            }
        }
        if v.len() > 4096 {
            v.truncate(4096);
        }
    }
    v
}

/// Hand-written structural edge cases that random mutation rarely reaches.
const ADVERSARIAL: &[&[u8]] = &[
    b"",
    b"<",
    b">",
    b"><",
    b"<>",
    b">sip:a@b<",
    b"a>b<c",
    b"sip:",
    b"sips:",
    b"tel:",
    b"sip:@",
    b"sip:a@",
    b"sip:@b",
    b"sip:[::1",
    b"sip:[]",
    b"sip:[::1]:",
    b"sip:[::1]:99999999999",
    b"sip:a@[::1]:x",
    b";tag=",
    b"tag=",
    b"SIP/2.0 999999999999 x",
    b"INVITE  SIP/2.0",
    &[0xff, 0xfe, 0xfd],
    &[0x80],
];

/// Run `f` over each seed, adversarial case, and thousands of mutations; record
/// any input that makes `f` panic. `f` must accept arbitrary bytes gracefully.
fn assert_never_panics<F>(label: &str, seeds: &[&[u8]], iters: usize, f: F)
where
    F: Fn(&[u8]) + std::panic::RefUnwindSafe,
{
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut inputs: Vec<Vec<u8>> = Vec::new();
    inputs.extend(seeds.iter().map(|s| s.to_vec()));
    inputs.extend(ADVERSARIAL.iter().map(|s| s.to_vec()));
    for s in seeds {
        for _ in 0..iters {
            inputs.push(mutate(s, &mut rng));
        }
    }
    for a in ADVERSARIAL {
        for _ in 0..(iters / 8).max(1) {
            inputs.push(mutate(a, &mut rng));
        }
    }

    let mut first_crash = None;
    for inp in &inputs {
        if catch_unwind(AssertUnwindSafe(|| f(inp))).is_err() {
            first_crash = Some(inp.clone());
            break;
        }
    }
    assert!(
        first_crash.is_none(),
        "{label}: parser panicked on input {:02x?}",
        first_crash.unwrap()
    );
}

fn as_str_then<F: Fn(&str)>(d: &[u8], f: F) {
    if let Ok(s) = std::str::from_utf8(d) {
        f(s);
    }
}

const SIP_SEED: &[u8] = b"INVITE sip:bob@biloxi.example.com SIP/2.0\r\nVia: SIP/2.0/UDP pc33.atlanta.example.com;branch=z9hG4bKnashds8\r\nTo: Bob <sip:bob@biloxi.example.com>\r\nFrom: Alice <sip:alice@atlanta.example.com>;tag=1928301774\r\nCall-ID: a84b4c76e66710@pc33.atlanta.example.com\r\nCSeq: 314159 INVITE\r\nContact: <sip:alice@pc33.atlanta.example.com>\r\nContent-Type: application/sdp\r\nContent-Length: 4\r\n\r\nv=0\n";
const SIP_RESP_SEED: &[u8] = b"SIP/2.0 200 OK\r\nVia: SIP/2.0/UDP server;branch=z9hG4bK\r\nTo: Bob <sip:bob@b>;tag=a6\r\nContent-Length: 0\r\n\r\n";
const SDP_SEED: &[u8] = b"v=0\r\no=alice 2890 2890 IN IP4 10.0.0.1\r\ns=call\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\nm=audio 5004 RTP/AVP 0 8\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\nb=AS:512\r\na=fingerprint:sha-256 AB:CD\r\na=setup:actpass\r\na=candidate:1 1 UDP 2130706431 10.0.0.1 5004 typ host\r\n";

#[test]
fn sip_message_and_header_parsers_never_panic() {
    assert_never_panics("SipMessage::parse", &[SIP_SEED, SIP_RESP_SEED], 20000, |d| {
        if let Ok(msg) = SipMessage::parse(d) {
            for name in ["From", "To", "Contact", "Via", "Route"] {
                for v in msg.get_headers(name) {
                    let _ = extract_uri(v);
                    let _ = extract_tag(v);
                    let _ = parse_via(v);
                }
            }
        }
    });
    assert_never_panics(
        "extract_uri/tag/via",
        &[b"Alice <sip:a@b>;tag=1", b"<sip:x@y>", b"SIP/2.0/UDP [::1]:5060;branch=z"],
        20000,
        |d| {
            as_str_then(d, |s| {
                let _ = extract_uri(s);
                let _ = extract_tag(s);
                let _ = parse_via(s);
            });
        },
    );
    assert_never_panics(
        "SipUri::parse",
        &[b"sip:alice@atlanta.example.com:5060;transport=tcp", b"sips:[::1]:5061", b"tel:+15551234"],
        20000,
        |d| {
            as_str_then(d, |s| {
                if let Ok(u) = SipUri::parse(s) {
                    let _ = u.to_string();
                    let _ = u.host_display();
                }
            });
        },
    );
}

#[test]
fn sdp_rtp_stun_parsers_never_panic() {
    assert_never_panics("SessionDescription::parse", &[SDP_SEED], 20000, |d| {
        as_str_then(d, |s| {
            let _ = SessionDescription::parse(s);
        });
    });

    let rtp: &[u8] = &[0x80, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0xde, 0xad];
    let rtp_ext: &[u8] = &[
        0x90, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0xBE, 0xDE, 0, 1, 0x11, 0x22, 0x33, 0x44, 0xaa,
    ];
    let rtp_pad: &[u8] = &[0xA0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 2];
    assert_never_panics("RtpHeader/parse_rtp_header", &[rtp, rtp_ext, rtp_pad], 20000, |d| {
        let _ = RtpHeader::parse(d);
        let _ = parse_rtp_header(d);
    });

    let stun: &[u8] = &[
        0, 1, 0, 8, 0x21, 0x12, 0xa4, 0x42, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0xa, 0xb, 0, 6, 0, 4, b'u',
        b's', b'e', b'r',
    ];
    assert_never_panics("StunMessage/RawAttribute::parse", &[stun], 20000, |d| {
        let _ = StunMessage::parse(d);
        let _ = RawAttribute::parse(d);
    });
}

#[test]
fn other_untrusted_parsers_never_panic() {
    assert_never_panics("turn::decode_channel_data", &[&[0x40, 1, 0, 4, 1, 2, 3, 4]], 8000, |d| {
        let _ = decode_channel_data(d);
    });
    let fb: &[u8] = &[0x81, 0xCD, 0, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0xaa, 0xbb, 0xcc, 0xdd];
    assert_never_panics("avpf::RtcpFeedback::parse", &[fb], 8000, |d| {
        let _ = RtcpFeedback::parse(d);
    });
    let mp: &[u8] = b"--b\r\nContent-Type: application/sdp\r\n\r\nv=0\r\n--b--\r\n";
    assert_never_panics("parse_multipart", &[mp], 8000, |d| {
        let _ = parse_multipart("multipart/mixed;boundary=b", d);
    });
    assert_never_panics(
        "DigestChallenge/parse_authorization",
        &[
            b"Digest realm=\"a\", nonce=\"n\", algorithm=MD5, qop=\"auth\"",
            b"Digest username=\"a\", realm=\"r\", nonce=\"n\", uri=\"sip:x\", response=\"deadbeef\"",
        ],
        8000,
        |d| {
            as_str_then(d, |s| {
                let _ = DigestChallenge::parse(s);
                let _ = parse_authorization(s);
            });
        },
    );
    assert_never_panics(
        "diversion/history-info/geolocation/content-type",
        &[b"<sip:bob@example.com>;reason=user-busy;counter=2", b"<cid:t@e>;inserted-by=\"p\""],
        8000,
        |d| {
            as_str_then(d, |s| {
                let _ = parse_diversion_header(s);
                let _ = parse_history_info(s);
                let _ = parse_geolocation_header(s);
                let _ = MessageContentType::parse(s);
            });
        },
    );
}

// --------------------------------------------------------------------------
// Pinned regressions for the two historical remote-crash panics. These lock in
// the fixes: reverting either fix in the source makes exactly these asserts go
// RED (verified). See issues #108 and #109.
// --------------------------------------------------------------------------

#[test]
fn regression_issue_108_non_char_boundary_content_length() {
    // `Content-Length: 1` but the body's first char is the 2-byte UTF-8 'é'.
    // The old code did `body[..1]`, slicing inside the multibyte char -> panic.
    let mut msg = b"INVITE sip:a@b SIP/2.0\r\nContent-Length: 1\r\n\r\n".to_vec();
    msg.extend_from_slice("é".as_bytes()); // 0xC3 0xA9
    let parsed = SipMessage::parse(&msg).expect("must parse without panicking");
    // The fix keeps the full (length-bounded) body rather than aborting.
    assert_eq!(parsed.body, "é");
}

#[test]
fn regression_issue_109_reversed_angle_brackets() {
    // '>' before '<' produced a reversed slice range -> panic in extract_uri.
    for bad in [">sip:a@b<", "><", "a>b<c", "\">\"<sip:x"] {
        // Must not panic; returns Some(fallback) or a value.
        let _ = extract_uri(bad);
    }
    // Ordered brackets still extract the enclosed URI.
    assert_eq!(extract_uri("Bob <sip:bob@b>"), Some("sip:bob@b".to_string()));
}
