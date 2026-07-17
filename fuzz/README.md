# Asterisk-RS Fuzz Testing

This directory contains cargo-fuzz harnesses for the wire-facing parsers in
the asterisk-rs project.

**Every target drives the REAL production parser** — the same code the wire
hits — via path dependencies on `asterisk-sip`, `asterisk-channels` and
`asterisk-ami`. They are **not** standalone reimplementations. (The old targets
were private copies of the parsers, which is why the remote-crash panics in
issues #108 and #109 were never surfaced by fuzzing.)

## Setup

1. Install nightly Rust toolchain (required by cargo-fuzz / libFuzzer):
```bash
rustup install nightly
```

2. Install cargo-fuzz:
```bash
cargo install cargo-fuzz
```

## Available Fuzz Targets

| Target | Real parser exercised |
|--------|-----------------------|
| `fuzz_sip_parse` | `asterisk_sip::parser::SipMessage::parse` + the `extract_uri`/`extract_tag`/`parse_via` header helpers on parsed values |
| `fuzz_sip_uri` | `asterisk_sip::parser::SipUri::parse` (+ `Display` round-trip) |
| `fuzz_sip_headers` | `extract_uri` / `extract_tag` / `parse_via` on raw header strings |
| `fuzz_sdp_parse` | `asterisk_sip::sdp::SessionDescription::parse` (+ ICE/DTLS/bandwidth sub-parsers) |
| `fuzz_rtp_parse` | `asterisk_sip::rtp::{RtpHeader::parse, parse_rtp_header}` (CSRC/extension/padding) |
| `fuzz_stun_parse` | `asterisk_sip::stun::{StunMessage::parse, RawAttribute::parse}` |
| `fuzz_ami_parse` | `asterisk_ami::protocol::{read_message, AmiAction::parse, AmiEvent::parse}` |
| `fuzz_websocket_parse` | `asterisk_channels::websocket::WebSocketFrame::parse` (SIP-over-WS framing) |
| `fuzz_srtp_unprotect` | `asterisk_sip::srtp::SrtpCrypto::{unprotect_rtp, unprotect_rtcp}` (fixed non-secret test key) |

## Running the Fuzzers

List available targets:
```bash
cargo fuzz list
```

Run a specific fuzzer:
```bash
# Run indefinitely until crash or Ctrl-C
cargo fuzz run fuzz_sip_parse

# Run for a bounded time (60 seconds)
cargo fuzz run fuzz_sip_parse -- -max_total_time=60
```

## Compile check without nightly

`cargo fuzz build` needs nightly, but the targets compile against the real
crates on the pinned stable toolchain too, which is a fast way to catch API
drift (a parser signature change that would otherwise silently bitrot a
harness):
```bash
cargo check --manifest-path fuzz/Cargo.toml
```

## Corpus

The `corpus/` directory holds seed inputs (real SIP/SDP messages and binary
RTP/STUN packets). libFuzzer starts from these and writes new coverage-
increasing inputs back into the same directory.

## Finding Issues

A crash is saved to `artifacts/`. Reproduce with:
```bash
cargo fuzz run <target> artifacts/<target>/crash-<hash>
```
Fix the underlying parser, add a regression test built from the crashing
input, then re-run to confirm.

## Deterministic coverage under `cargo test`

Because `cargo fuzz` needs nightly, the same real parsers are *also* exercised
by deterministic mutation/property tests that run on the pinned stable
toolchain under `cargo test --workspace` (see `*_fuzz_regression.rs` in the
`tests/` directories of `asterisk-sip`, `asterisk-channels` and `asterisk-ami`).
Those feed a large battery of malformed + byte-mutated inputs to the production
parsers and assert graceful `Err`/no-panic, so panic regressions are caught in
CI even without a libFuzzer run.
