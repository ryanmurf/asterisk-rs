# Full-stack M9 trunk validator (`integration/`) — NOT run in CI

These are the **original, proven** M9 arming-harness validators that drove the
full live media path end-to-end:

```
synthetic Chime UAC ─▶ rustisk PIN gate ─▶ Dial(PJSIP/qa-bridge)
   (chime_caller.py)                          │
                              the REAL pymumble qa-bridge ◀─┘
                                          │  (bridges into Mumble)
                              qa-sip Mumble channel
                                          ▲
                              qa_probe.py joins qa-sip as a deterministic
                              far end: TX 660 Hz, FFT-detect the caller's 440 Hz
```

This proved the whole path **3× off-trunk** during M9 arming. Unlike the
hermetic `tests/e2e-trunk/run.sh` (which replaces the bridge with an in-process
mock), this variant exercises the *actual* pymumble bridge and Mumble server, so
it is the validator for the live cutover.

## Why it is NOT in CI

It requires infrastructure that does not exist on a CI runner:

- the **qa-sip Mumble channel** on the voice server (`voice.murphytek.com`),
- the **dedicated pymumble `qa-bridge`** registered to rustisk,
- `numpy` + `pymumble_py3` (the validators use them directly),
- the real `qa-bridge` auth password and the real 6-digit PIN (secrets — never
  committed; see below).

Running it also touches shared live voice infrastructure and must be sequenced
with the live trunk (see `RUNBOOK-M9`). It is a **manual, operator-run**
validator, not an unattended gate.

## Files

- `chime_caller.py` — synthetic Chime UAC (numpy). Places the call, enters the
  PIN via RFC2833, emits a 440 Hz tone, captures + FFT-analyzes the return RTP.
- `qa_probe.py` — runs inside the `vs-harness` image; joins qa-sip as `m9-probe`,
  transmits 660 Hz into the channel and FFT-detects the caller's 440 Hz.
- `config-live/` — the live rustisk config templates (pjsip / extensions /
  pin_gate / rtp / manager / asterisk). **Sanitized:** the qa-bridge auth
  password is redacted to `<QA_BRIDGE_AUTH_PASSWORD__INJECT_AT_RUNTIME>` and
  `pin_gate.conf` points `secret_file` at a host-local path you stage the real
  PIN into at runtime (see its inline comment). **Never commit the real
  password or PIN.**

## Run (operator, with the voice stack up)

1. Stage the real PIN into the `secret_file` path referenced by
   `config-live/pin_gate.conf` (e.g. from the `voice-sip` k8s secret) — chmod 600.
2. Inject the real qa-bridge auth password into `config-live/pjsip.conf` in
   place of the placeholder.
3. Start rustisk against `config-live/asterisk.conf`, bring up the pymumble
   `qa-bridge` (registers to rustisk) and `qa_probe.py` (in the vs-harness
   image, joined to qa-sip).
4. Drive `chime_caller.py` at rustisk. Assert: caller RX shows 660 Hz (probe →
   caller) and the probe shows 440 Hz (caller → probe) — two-way through the
   real bridge.

For the unattended, hermetic version of the same assertions, use
`../run.sh`.
