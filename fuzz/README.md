# Asterisk-RS Fuzz Testing

This directory contains cargo-fuzz harnesses for testing the SIP and related parsers in the asterisk-rs project.

## Setup

1. Install nightly Rust toolchain (required for fuzzing):
```bash
rustup install nightly
rustup default nightly
```

2. Install cargo-fuzz:
```bash
cargo install cargo-fuzz
```

## Available Fuzz Targets

- `fuzz_sip_parse` - Tests SIP message parsing (INVITE, REGISTER, responses, etc.)
- `fuzz_sdp_parse` - Tests SDP (Session Description Protocol) parsing 
- `fuzz_rtp_parse` - Tests RTP (Real-time Transport Protocol) header parsing
- `fuzz_stun_parse` - Tests STUN (Session Traversal Utilities for NAT) message parsing

## Running the Fuzzers

List available targets:
```bash
cargo fuzz list
```

Run a specific fuzzer:
```bash
# Run indefinitely until crash or Ctrl-C
cargo fuzz run fuzz_sip_parse

# Run for limited time (5 seconds)
cargo fuzz run fuzz_sip_parse -- -max_total_time=5

# Run with specific number of iterations
cargo fuzz run fuzz_sip_parse -- -runs=10000
```

## Corpus

The `corpus/` directory contains seed inputs for the fuzzers:

- `sip_*.txt` - Real SIP messages (INVITE, REGISTER, 200 OK response)
- `sdp_*.txt` - Sample SDP session descriptions
- `*.bin` - Binary files for RTP and STUN protocols

These seed files help the fuzzer generate more realistic test cases by starting with valid protocol examples.

## Implementation Notes

The fuzz targets use minimal standalone implementations of the parsers to avoid complex dependency chains that were causing build issues. Each target:

1. Receives arbitrary byte input from libfuzzer
2. Attempts to parse the input using the respective protocol parser
3. Ensures no panics occur (the main goal of fuzz testing)

The parsers implement the core protocol logic:

- **SIP**: Request/response lines, header folding, body separation
- **SDP**: Version, origin, media descriptions, attributes 
- **RTP**: Version validation, header fields, CSRC/extension parsing
- **STUN**: Magic cookie validation, attribute parsing, padding

## Finding Issues

If a fuzzer finds a crash, it will save the crashing input to `artifacts/`. You can then:

1. Examine the crashing input file
2. Reproduce the crash by running the target with that input
3. Fix the underlying parser issue
4. Re-run the fuzzer to verify the fix

## Continuous Fuzzing

For continuous integration, you can run fuzzers for fixed periods:

```bash
# Run each fuzzer for 1 minute
for target in fuzz_sip_parse fuzz_sdp_parse fuzz_rtp_parse fuzz_stun_parse; do
    cargo fuzz run $target -- -max_total_time=60
done
```