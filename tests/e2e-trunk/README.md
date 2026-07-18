# rustisk trunk e2e (`tests/e2e-trunk`)

Durable, repeatable, **hermetic** end-to-end proof of rustisk's inbound-trunk
PIN-gate path — the same path the live voice trunk runs, but with **no external
services**. It is the committed, CI-runnable successor to the one-shot M9 arming
harness (which drove a real Amazon Chime UAC into the real qa-sip Mumble bridge).

```
  synthetic Chime UAC ──INVITE(PCMU+RFC2833)──▶ rustisk [pin-gate]
        (chime_caller.py)                          │ PinGate() collects DTMF PIN
                                                    │
     wrong PIN ─────────────────────────────▶ REJECTED ─▶ play + Hangup, NO Dial
                                                    │
     correct PIN ───────────────────────────▶ GRANTED  ─▶ Dial(PJSIP/qa-bridge)
                                                    │
                          MOCK registered UAS ◀─INVITE─┘  (mock_bridge.py)
                          answers 200 OK + SDP
                          ◀──────── two-way RTP ────────▶
                          caller 440 Hz ─▶ mock RX     (proves A → bridge)
                          mock 660 Hz   ─▶ caller RX    (proves bridge → A)
```

## What it asserts (receiver-side, like M0/M2)

**CASE 1 — REJECTED (wrong PIN must not reach the bridge)**
- rustisk log contains `Verbose: PIN_GATE_RESULT=REJECTED`.
- the mock endpoint's `invite_count == 0` — rustisk **never** Dialed the bridge.

**CASE 2 — GRANTED (correct PIN → Dial → two-way media)**
- rustisk log contains `Verbose: PIN_GATE_RESULT=GRANTED`.
- **A → bridge:** the mock received the caller's relayed tone —
  `invite_count == 1`, `rx_voice ≥ 100` frames, and **440 Hz is the dominant
  band** in the mock's received audio.
- **bridge → A:** the caller received the bridge's distinct tone —
  `voice_rx ≥ 100` frames, and **660 Hz is the dominant band** in the caller's
  received audio.

Distinct tones (caller 440 Hz, mock 660 Hz), each detected on the *far* side,
make each RTP direction independently attributable by frequency — a stronger
proof than an echo (an echo cannot distinguish a relayed frame from a socket
loop). Tone detection is a stdlib Goertzel filter, so **CI needs no numpy**.

## Isolation (no collision with live / m0 / m9)

Everything binds fresh loopback IPs + high ports, asserted free by a preflight
(`ss`) before anything starts:

| role | IP | SIP | RTP | AMI |
|------|-----|-----|-----|-----|
| rustisk (SUT) | `127.0.0.60` | `35060` | `36000-36100` | `35038` |
| synthetic Chime caller | `127.0.0.61` | `35062` | `36200` | — |
| mock qa-bridge | `127.0.0.62` | `35064` | `36300` | — |

None of these touch the reserved live/arming ports (`:45070` live trunk,
`:25060/:25038/:25062/:21000-21100` M9 arming, `:15060/:15038` M0) — the
preflight fails fast if they ever would. Endpoints are identified by distinct
`/32`s so inbound identification is unambiguous. No `external_media_address` is
set, so rustisk advertises its own loopback bind IP for media (avoids the
external-address blackhole in the SDP path).

## Run it

```bash
# uses target/debug/rustisk if present, else `cargo build --bin rustisk`
tests/e2e-trunk/run.sh

# or point at a prebuilt binary
RUSTISK_BIN=/path/to/rustisk tests/e2e-trunk/run.sh
```

Requirements: `python3` (stdlib only), `ss` (iproute2), `stdbuf` (coreutils),
Linux loopback. No docker, no NET_RAW, no root. Exit status **is** the result
(0 = pass). Artifacts (rustisk log, per-leg JSON results, RX WAVs) land in
`target/e2e-trunk/`.

Knobs: `RUSTISK_BIN`, `MIN_VOICE_FRAMES` (default 100), `MIN_TONE_RATIO`
(default 20), `KEEP_RUNTIME` (keep artifacts).

## Proven green — 2× consecutive (calibration runs, rustisk @ 461ad74)

```
RUN 1  ALL CASES PASSED
  REJECTED : PIN_GATE_RESULT=REJECTED, bridge invite_count=0
  GRANTED  : mock  rx_voice=574  tone440=2004457.92  (660=175.59 350=362.93 880=98.49)
             caller voice_rx=695 tone660=1766894.92  (440=216.80 350=154.90 880=155.86)

RUN 2  ALL CASES PASSED
  REJECTED : PIN_GATE_RESULT=REJECTED, bridge invite_count=0
  GRANTED  : mock  rx_voice=574  tone440=2004457.92
             caller voice_rx=696 tone660=1431051.95  (440=308.06)
```

The expected tone dominates the next-strongest band by ~4 orders of magnitude
in both directions on every run (frame counts and dominance are the load-bearing
assertions; the absolute Goertzel magnitude is an unbounded relative metric).

## CI

Wired into `.github/workflows/ci.yml` as the `e2e-trunk` job: it builds the
rustisk binary and runs `run.sh` on `ubuntu-latest`. It is hermetic (loopback +
python3 stdlib only), so it runs unattended in CI with no runner privileges.

## `integration/` — full-stack M9 validator (NOT run in CI)

`integration/` holds the *original* M9 full-bridge validators
(`chime_caller.py` + `qa_probe.py`, numpy) plus sanitized live config. Those
require the real voice stack (qa-sip Mumble channel + the dedicated pymumble
bridge) and are the end-to-end validator for the live cutover — **not**
hermetic, **not** in CI. See `integration/README.md`.
