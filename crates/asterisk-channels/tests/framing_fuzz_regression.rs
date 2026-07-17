//! Deterministic mutation / property tests for the channel-driver binary
//! framing parsers (the same real parsers the `fuzz_*` targets wrap). Each
//! parser must return `Err`/`None` on malformed input rather than panicking —
//! these decode untrusted wire bytes on live sockets.

use std::panic::{catch_unwind, AssertUnwindSafe};

use asterisk_channels::iax2::{
    parse_iax2_packet, parse_information_elements, InformationElement, Iax2FullHeader,
    Iax2MetaHeader, Iax2MetaTrunkHeader, Iax2MiniHeader, Iax2TrunkEntry,
};
use asterisk_channels::rtp_channel::{parse_rtp_packet, RtpHeader};
use asterisk_channels::skinny::SkinnyMessage;
use asterisk_channels::unistim::UnistimFrame;
use asterisk_channels::websocket::WebSocketFrame;

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

fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = seed.to_vec();
    for _ in 0..=rng.pick(6) {
        if v.is_empty() {
            v.push(rng.next() as u8);
            continue;
        }
        match rng.pick(7) {
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
                v[i] = 0xff;
            }
            _ => {
                let i = rng.pick(v.len());
                let len = rng.pick(v.len() - i) + 1;
                let region = v[i..i + len].to_vec();
                let at = rng.pick(v.len() + 1);
                for (k, b) in region.into_iter().enumerate() {
                    v.insert(at + k, b);
                }
            }
        }
        if v.len() > 8192 {
            v.truncate(8192);
        }
    }
    v
}

fn assert_never_panics<F>(label: &str, seeds: &[&[u8]], iters: usize, f: F)
where
    F: Fn(&[u8]) + std::panic::RefUnwindSafe,
{
    let mut rng = Rng(0x243F6A8885A308D3);
    let mut inputs: Vec<Vec<u8>> = seeds.iter().map(|s| s.to_vec()).collect();
    // Every length from 0..24 in all-0x00 and all-0xff (header-boundary probing).
    for n in 0..24usize {
        inputs.push(vec![0x00; n]);
        inputs.push(vec![0xff; n]);
    }
    for s in seeds {
        for _ in 0..iters {
            inputs.push(mutate(s, &mut rng));
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

#[test]
fn websocket_frame_parser_never_panics() {
    let small: &[u8] = &[0x81, 0x83, 1, 2, 3, 4, 0xaa, 0xbb, 0xcc];
    let len126: &[u8] = &[0x82, 0xFE, 0, 4, 0xde, 0xad, 0xbe, 0xef];
    let len127: &[u8] = &[0x82, 0xFF, 0, 0, 0, 0, 0, 0, 0, 4, 1, 2, 3, 4];
    let close: &[u8] = &[0x88, 0x02, 0x03, 0xe8];
    assert_never_panics("WebSocketFrame::parse", &[small, len126, len127, close], 20000, |d| {
        let _ = WebSocketFrame::parse(d);
    });
}

#[test]
fn iax2_parsers_never_panic() {
    let full: &[u8] = &[
        0x80, 1, 0, 2, 0, 0, 0, 3, 1, 4, 6, 1, 0x0f, 3, b'a', b'b', b'c',
    ];
    let mini: &[u8] = &[0, 1, 0x12, 0x34, 0xaa, 0xbb];
    let meta: &[u8] = &[0, 0, 1, 0, 0xde, 0xad];
    let trunk: &[u8] = &[0, 1, 0, 4];
    let ies: &[u8] = &[6, 3, b'a', b'b', b'c', 0x0f, 1, 2];
    assert_never_panics("Iax2FullHeader::parse", &[full], 8000, |d| {
        let _ = Iax2FullHeader::parse(d);
    });
    assert_never_panics("Iax2MiniHeader::parse", &[mini], 8000, |d| {
        let _ = Iax2MiniHeader::parse(d);
    });
    assert_never_panics("Iax2MetaHeader::parse", &[meta], 8000, |d| {
        let _ = Iax2MetaHeader::parse(d);
    });
    assert_never_panics("Iax2MetaTrunkHeader::parse", &[trunk], 8000, |d| {
        let _ = Iax2MetaTrunkHeader::parse(d);
    });
    assert_never_panics("Iax2TrunkEntry::parse", &[trunk], 8000, |d| {
        let _ = Iax2TrunkEntry::parse(d);
    });
    assert_never_panics("InformationElement::parse", &[ies], 8000, |d| {
        let _ = InformationElement::parse(d);
    });
    assert_never_panics("parse_information_elements", &[ies], 8000, |d| {
        let _ = parse_information_elements(d);
    });
    assert_never_panics("parse_iax2_packet", &[full, mini, meta], 8000, |d| {
        let _ = parse_iax2_packet(d);
    });
}

#[test]
fn skinny_unistim_rtp_parsers_never_panic() {
    let skinny: &[u8] = &[0x08, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    assert_never_panics("SkinnyMessage::parse", &[skinny], 8000, |d| {
        let _ = SkinnyMessage::parse(d);
    });
    let uni: &[u8] = &[0, 1, 0x11, 3, 0xaa, 0xbb, 0xcc];
    assert_never_panics("UnistimFrame::parse", &[uni], 8000, |d| {
        let _ = UnistimFrame::parse(d);
    });
    let rtp: &[u8] = &[0x80, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0xde, 0xad];
    assert_never_panics("rtp_channel parsers", &[rtp], 8000, |d| {
        let _ = RtpHeader::parse(d);
        let _ = parse_rtp_packet(d);
    });
}
