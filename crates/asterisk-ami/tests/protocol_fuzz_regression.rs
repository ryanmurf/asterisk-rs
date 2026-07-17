//! Deterministic mutation / property tests for the AMI protocol parsers (the
//! same real parsers the `fuzz_ami_parse` target wraps). AMI accepts CRLF-
//! framed key/value text from manager clients; the framing + parse must never
//! panic on malformed input.

use std::panic::{catch_unwind, AssertUnwindSafe};

use asterisk_ami::protocol::{read_message, AmiAction, AmiEvent};

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
    const INTERESTING: [u8; 8] = [b':', b'\r', b'\n', b' ', b'\t', 0x80, 0xff, 0x00];
    let mut v = seed.to_vec();
    for _ in 0..=rng.pick(6) {
        if v.is_empty() {
            v.push(rng.next() as u8);
            continue;
        }
        match rng.pick(6) {
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
                v.insert(i, INTERESTING[rng.pick(INTERESTING.len())]);
            }
            3 => {
                let i = rng.pick(v.len());
                v.remove(i);
            }
            4 => {
                let i = rng.pick(v.len());
                v.truncate(i);
            }
            _ => {
                let i = rng.pick(v.len() + 1);
                v.insert(i, rng.next() as u8);
            }
        }
        if v.len() > 4096 {
            v.truncate(4096);
        }
    }
    v
}

fn assert_never_panics<F>(label: &str, seeds: &[&[u8]], iters: usize, f: F)
where
    F: Fn(&str) + std::panic::RefUnwindSafe,
{
    let mut rng = Rng(0xB5026F5AA96619E9);
    let mut inputs: Vec<Vec<u8>> = seeds.iter().map(|s| s.to_vec()).collect();
    for s in seeds {
        for _ in 0..iters {
            inputs.push(mutate(s, &mut rng));
        }
    }
    let mut first_crash = None;
    for inp in &inputs {
        if let Ok(s) = std::str::from_utf8(inp) {
            if catch_unwind(AssertUnwindSafe(|| f(s))).is_err() {
                first_crash = Some(inp.clone());
                break;
            }
        }
    }
    assert!(
        first_crash.is_none(),
        "{label}: parser panicked on input {:02x?}",
        first_crash.unwrap()
    );
}

#[test]
fn ami_parsers_never_panic() {
    let action: &[u8] = b"Action: Login\r\nUsername: admin\r\nSecret: x\r\nActionID: 1\r\n\r\n";
    let event: &[u8] = b"Event: Newchannel\r\nChannel: SIP/1\r\nChannelState: 4\r\n\r\n";
    assert_never_panics("AmiAction::parse", &[action], 25000, |s| {
        let _ = AmiAction::parse(s);
    });
    assert_never_panics("AmiEvent::parse", &[event], 25000, |s| {
        let _ = AmiEvent::parse(s);
    });
    assert_never_panics("read_message", &[action, event], 25000, |s| {
        let _ = read_message(s);
    });
}
