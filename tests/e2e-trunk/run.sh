#!/usr/bin/env bash
# =============================================================================
# Durable, hermetic e2e for rustisk's TRUNK PATH:
#   synthetic Chime INVITE -> rustisk PIN gate -> (correct PIN) GRANTED ->
#   Dial(PJSIP/qa-bridge) -> a MOCK registered SIP endpoint answers ->
#   two-way RTP (both directions, distinct tones) ; and
#   (wrong PIN) REJECTED -> NO Dial (the bridge is never INVITEd).
#
# Self-contained: needs ONLY the rustisk binary + python3 (stdlib) + loopback.
# NO FreeSWITCH, NO Mumble, NO docker, NO NET_RAW, NO root. The pymumble bridge
# is replaced by tests/e2e-trunk/mock_bridge.py (REGISTER + digest + echo tone).
#
# Everything runs on isolated 127.0.0.6x loopback IPs and 35xxx/36xxx ports so
# it CANNOT collide with the live trunk (:45070), the M9 arming rig (:25060/
# :25038/:25062/:21000-21100) or the M0 instance (:15060/:15038). A preflight
# asserts our ports are free before touching anything.
#
# Exit status is the test result: 0 = all cases passed, non-zero = failure.
#
# Env knobs:
#   RUSTISK_BIN   path to the rustisk binary (default: build/target/debug/rustisk;
#                 if absent it is built with `cargo build --bin rustisk`).
#   KEEP_RUNTIME  set to keep $RUNTIME_DIR artifacts after the run.
# =============================================================================
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/e2e-trunk"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
PROMPT_DIR="$RUNTIME_DIR/prompts"
SECRET_FILE="$RUNTIME_DIR/pin.secret"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
RUSTISK_BIN="${RUSTISK_BIN:-$REPO_DIR/target/debug/rustisk}"

# --- isolated loopback topology (fresh; no overlap with live/m0/m9 ports) ---
RK_IP=127.0.0.60;  RK_SIP=35060;  RK_AMI=35038
RTP_START=36000;   RTP_END=36100
CALLER_IP=127.0.0.61; CALLER_SIP=35062; CALLER_RTP=36200
MOCK_IP=127.0.0.62;   MOCK_SIP=35064;   MOCK_RTP=36300
UNTRUSTED_IP=127.0.0.63; UNTRUSTED_SIP=35066; UNTRUSTED_RTP=36400

# The one production DID this least-privilege trunk accepts. Chime presents it
# in canonical E.164 form, including the leading '+'.
CHIME_DID="+19709601891"
WRONG_CHIME_DID="+19709601892"
CHIME_INVITE_FIXTURE="$HARNESS_DIR/fixtures/chime-invite.txt"

# --- TEST-ONLY secrets (never the real PIN / qa-bridge password) ---
TEST_PIN="246813"
WRONG_PIN="999999"
BRIDGE_USER="qa-bridge"
BRIDGE_PASS="e2e-mock-bridge-secret"

# --- media tones ---
CALLER_TONE=440    # caller -> bridge  (proves A->bridge on the mock RX)
BRIDGE_TONE=660    # bridge -> caller  (proves bridge->A on the caller RX)

# --- assertion thresholds (overridable) ---
# MIN_TONE_RATIO is deliberately set an order of magnitude ABOVE the observed
# Goertzel noise-band floor (~150-360) and ~3 orders BELOW a real relayed tone
# (~2e6), so a silence/comfort-noise "relay" can neither clear the floor nor the
# 10x dominance margin the GRANTED assertions also require.
MIN_VOICE_FRAMES="${MIN_VOICE_FRAMES:-100}"
MIN_TONE_RATIO="${MIN_TONE_RATIO:-5000}"

RUSTISK_PID=""
MOCK_PID=""

log()  { printf '%s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
    [[ -n "$MOCK_PID" ]] && { kill -TERM "$MOCK_PID" 2>/dev/null || true; wait "$MOCK_PID" 2>/dev/null || true; }
    if [[ -n "$RUSTISK_PID" ]]; then
        kill -TERM "$RUSTISK_PID" 2>/dev/null || true
        for _ in {1..20}; do kill -0 "$RUSTISK_PID" 2>/dev/null || break; sleep 0.1; done
        kill -KILL "$RUSTISK_PID" 2>/dev/null || true
        wait "$RUSTISK_PID" 2>/dev/null || true
    fi
    if [[ -z "${KEEP_RUNTIME:-}" ]]; then rm -f "$SECRET_FILE" 2>/dev/null || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require() { command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"; }

preflight_ports() {
    # Prove none of OUR ports are already bound (guards against colliding with
    # a stray instance, and makes a port typo fail fast rather than mysteriously).
    local busy=""
    local udp tcp
    udp="$(ss -Huan 2>/dev/null || true)"
    tcp="$(ss -Htan 2>/dev/null || true)"
    local p
    for p in "$RK_SIP" "$CALLER_SIP" "$MOCK_SIP" "$UNTRUSTED_SIP" "$CALLER_RTP" "$MOCK_RTP" "$UNTRUSTED_RTP" "$RTP_START" "$RTP_END"; do
        grep -qE "[.:]$p( |\$)|:$p " <<<"$udp" && busy+="udp/$p "
    done
    grep -qE "[.:]$RK_AMI( |\$)|:$RK_AMI " <<<"$tcp" && busy+="tcp/$RK_AMI "
    # Also assert we are NOT about to touch any known live/m0/m9 port.
    local forbidden="45070 25060 25038 25062 21000 21100 15060 15038"
    for p in "$RK_SIP" "$RK_AMI" "$CALLER_SIP" "$MOCK_SIP" "$UNTRUSTED_SIP" "$RTP_START" "$RTP_END" "$CALLER_RTP" "$MOCK_RTP" "$UNTRUSTED_RTP"; do
        for f in $forbidden; do
            [[ "$p" == "$f" ]] && fail "port $p collides with a reserved live/m0/m9 port"
        done
    done
    [[ -z "$busy" ]] || fail "ports already in use: $busy (is a stray rustisk/mock running?)"
    log "PREFLIGHT: OK — SIP $RK_SIP / AMI $RK_AMI / RTP $RTP_START-$RTP_END + caller $CALLER_SIP/$CALLER_RTP + mock $MOCK_SIP/$MOCK_RTP + untrusted $UNTRUSTED_SIP/$UNTRUSTED_RTP all free"
}

ensure_binary() {
    if [[ ! -x "$RUSTISK_BIN" ]]; then
        log "rustisk binary not found at $RUSTISK_BIN — building (cargo build --bin rustisk)..."
        ( cd "$REPO_DIR" && cargo build --bin rustisk )
        RUSTISK_BIN="$REPO_DIR/target/debug/rustisk"
    fi
    [[ -x "$RUSTISK_BIN" ]] || fail "no usable rustisk binary at $RUSTISK_BIN"
    log "RUSTISK_BIN=$RUSTISK_BIN"
}

write_configs() {
    rm -rf "$RUNTIME_DIR"
    mkdir -p "$CONFIG_DIR" "$RUN_DIR" "$PROMPT_DIR"
    printf '%s\n' "$TEST_PIN" >"$SECRET_FILE"; chmod 600 "$SECRET_FILE"
    python3 "$HARNESS_DIR/gen_prompts.py" "$PROMPT_DIR" >/dev/null

    cat >"$CONFIG_DIR/asterisk.conf" <<EOF
[directories]
astetcdir = $CONFIG_DIR
astrundir = $RUN_DIR
EOF

    cat >"$CONFIG_DIR/pin_gate.conf" <<EOF
[general]
secret_file = $SECRET_FILE
EOF

    cat >"$CONFIG_DIR/rtp.conf" <<EOF
[general]
rtpstart = $RTP_START
rtpend = $RTP_END
EOF

    cat >"$CONFIG_DIR/manager.conf" <<EOF
[general]
enabled = yes
bindaddr = $RK_IP
port = $RK_AMI

[harness]
secret = e2e-local-only
read = all
write = system
EOF

    # Exercise the external-signaling Contact path with a reachable loopback
    # address/port. external_media_address remains unset so the media answer
    # uses its routed loopback address (avoids an artificial media blackhole).
    # Each endpoint is identified by a DISTINCT /32 so an
    # inbound INVITE from the caller maps to chime-trunk and the mock's
    # REGISTER/traffic maps to qa-bridge with no ambiguity.
    cat >"$CONFIG_DIR/pjsip.conf" <<EOF
[transport-udp]
type = transport
protocol = udp
bind = $RK_IP:$RK_SIP
external_signaling_address = $RK_IP
external_signaling_port = $RK_SIP

[chime-trunk]
type = endpoint
context = pin-gate
disallow = all
allow = ulaw
direct_media = no
rtp_symmetric = yes
dtmf_mode = rfc4733

[chime-trunk-identify]
type = identify
endpoint = chime-trunk
match = $CALLER_IP/32

[$BRIDGE_USER]
type = endpoint
context = pin-gate
disallow = all
allow = ulaw
direct_media = no
rtp_symmetric = yes
dtmf_mode = rfc4733
auth = qa-bridge-auth

[qa-bridge-auth]
type = auth
auth_type = userpass
username = $BRIDGE_USER
password = $BRIDGE_PASS

[qa-bridge-identify]
type = identify
endpoint = $BRIDGE_USER
match = $MOCK_IP/32
EOF

    cat >"$CONFIG_DIR/extensions.conf" <<EOF
; Owned Chime DID -> PIN gate. On GRANTED, Dial(PJSIP/qa-bridge) bridges
; the caller into the registered mock endpoint. On REJECTED, play + hang up
; WITHOUT dialing (the bridge must never be INVITEd).
[pin-gate]
exten => $CHIME_DID,1,Answer()
 same => n,PinGate($PROMPT_DIR/pin-prompt.wav,20,7)
 same => n,GotoIf(\$["\${PINGATESTATUS}" = "GRANTED"]?10:20)
 same => 10,Verbose(0,PIN_GATE_RESULT=GRANTED)
 same => n,Playback($PROMPT_DIR/granted.wav)
 same => n,Dial(PJSIP/$BRIDGE_USER,30)
 same => n,Hangup()
 same => 20,Verbose(0,PIN_GATE_RESULT=REJECTED)
 same => n,Playback($PROMPT_DIR/rejected.wav)
 same => n,Hangup()
EOF
    log "CONFIG: wrote hermetic pjsip/extensions/pin_gate/rtp/manager to $CONFIG_DIR"
}

start_rustisk() {
    RUST_LOG=info stdbuf -oL -eL "$RUSTISK_BIN" -f -C "$CONFIG_DIR/asterisk.conf" \
        >"$RUSTISK_LOG" 2>&1 &
    RUSTISK_PID=$!
    for _ in {1..100}; do
        kill -0 "$RUSTISK_PID" 2>/dev/null || fail "rustisk exited during boot; log:$(printf '\n'; cat "$RUSTISK_LOG")"
        grep -q 'Rustisk is fully booted' "$RUSTISK_LOG" && { log "RUSTISK: booted (pid $RUSTISK_PID) on $RK_IP:$RK_SIP"; return 0; }
        sleep 0.1
    done
    fail "rustisk did not report fully booted within 10s; log:$(printf '\n'; tail -40 "$RUSTISK_LOG")"
}

start_mock() {
    local ready="$1" result="$2" runsecs="$3"
    rm -f "$ready" "$result"
    python3 "$HARNESS_DIR/mock_bridge.py" \
        --reg-ip "$RK_IP" --reg-port "$RK_SIP" \
        --src-ip "$MOCK_IP" --sip-port "$MOCK_SIP" --rtp-port "$MOCK_RTP" \
        --username "$BRIDGE_USER" --password "$BRIDGE_PASS" \
        --tx-tone "$BRIDGE_TONE" --detect "$CALLER_TONE" \
        --run-secs "$runsecs" --ready-file "$ready" --result-file "$result" \
        >"$RUNTIME_DIR/$(basename "$result").mock.log" 2>&1 &
    MOCK_PID=$!
    for _ in {1..100}; do
        kill -0 "$MOCK_PID" 2>/dev/null || fail "mock_bridge exited before registering; log:$(printf '\n'; cat "$RUNTIME_DIR/$(basename "$result").mock.log")"
        [[ -f "$ready" ]] && { log "MOCK: registered as $BRIDGE_USER (pid $MOCK_PID)"; return 0; }
        sleep 0.1
    done
    fail "mock_bridge did not register within 10s; log:$(printf '\n'; tail -20 "$RUNTIME_DIR/$(basename "$result").mock.log")"
}

stop_mock() {
    [[ -n "$MOCK_PID" ]] || return 0
    kill -TERM "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
    MOCK_PID=""
}

run_caller() {
    local pin="$1" rxwav="$2" result="$3" callsecs="$4"
    rm -f "$result"
    python3 "$HARNESS_DIR/chime_caller.py" \
        --dst-ip "$RK_IP" --dst-port "$RK_SIP" \
        --src-ip "$CALLER_IP" --sip-port "$CALLER_SIP" --rtp-port "$CALLER_RTP" \
        --exten "$CHIME_DID" --invite-fixture "$CHIME_INVITE_FIXTURE" --pin "$pin" \
        --tone "$CALLER_TONE" --detect "$BRIDGE_TONE" \
        --call-secs "$callsecs" --rx-wav "$rxwav" --result-file "$result" \
        2>"$RUNTIME_DIR/$(basename "$result").caller.log" || true
}

run_status_probe() {
    local src_ip="$1" sip_port="$2" rtp_port="$3" exten="$4" status="$5" result="$6"
    rm -f "$result"
    python3 "$HARNESS_DIR/chime_caller.py" \
        --dst-ip "$RK_IP" --dst-port "$RK_SIP" \
        --src-ip "$src_ip" --sip-port "$sip_port" --rtp-port "$rtp_port" \
        --exten "$exten" --invite-fixture "$CHIME_INVITE_FIXTURE" \
        --expect-status "$status" --pin "$WRONG_PIN" \
        --call-secs 0.1 --rx-wav "$RUNTIME_DIR/probe-unused.wav" --result-file "$result" \
        2>"$result.caller.log" || true
    assert_json "$result" "captured INVITE expected SIP $status" \
        "d.get('sip_status') == $status" >/dev/null \
        || fail "captured INVITE probe from $src_ip to $exten did not receive SIP $status"
}

# JSON assertion helper: python3 predicate over a result file. Exits non-zero
# (with a message) on failure so `set -e` propagates.
assert_json() {
    local file="$1" desc="$2"; shift 2
    python3 - "$file" "$desc" "$@" <<'PY'
import json, sys
path, desc = sys.argv[1], sys.argv[2]
checks = sys.argv[3:]
try:
    d = json.load(open(path))
except Exception as e:
    print(f"FAIL: {desc}: cannot read {path}: {e}", file=sys.stderr); sys.exit(1)
def ratio(hz):
    return float((d.get("tone_ratios") or {}).get(str(hz), 0) or 0)
env = {"d": d, "ratio": ratio, "tr": d.get("tone_ratios") or {}}
for chk in checks:
    try:
        ok = eval(chk, {}, env)
    except Exception as e:
        print(f"FAIL: {desc}: check {chk!r} errored: {e}  (result={d})", file=sys.stderr); sys.exit(1)
    if not ok:
        print(f"FAIL: {desc}: check FAILED: {chk}  (result={d})", file=sys.stderr); sys.exit(1)
print(f"OK: {desc}: {d}")
PY
}

# =============================================================================
main() {
    require python3; require ss; require stdbuf
    log "=== rustisk e2e-trunk (hermetic) ==="
    python3 "$HARNESS_DIR/test_chime_fixture.py"
    preflight_ports
    ensure_binary
    write_configs
    start_rustisk

    # ---------- ingress policy: exact DID + source ACL ----------
    # These are the two negative controls for the sanitized production-derived
    # Chime request shape. A neighboring E.164 DID must not enter the gate,
    # and even the owned DID must be rejected when its source is not identified.
    log ""
    log "--- INGRESS POLICY: exact E.164 DID + allowlisted Chime source ---"
    run_status_probe "$CALLER_IP" "$CALLER_SIP" "$CALLER_RTP" \
        "$WRONG_CHIME_DID" 404 "$RUNTIME_DIR/wrong-did.json"
    log "WRONG DID: PASS ($WRONG_CHIME_DID -> SIP 404)"
    run_status_probe "$UNTRUSTED_IP" "$UNTRUSTED_SIP" "$UNTRUSTED_RTP" \
        "$CHIME_DID" 403 "$RUNTIME_DIR/untrusted-source.json"
    log "UNTRUSTED SOURCE: PASS ($UNTRUSTED_IP -> SIP 403)"

    # ---------- CASE 1: REJECTED (wrong PIN -> NO Dial) ----------
    log ""
    log "--- CASE 1: REJECTED (wrong PIN must NOT reach the bridge) ---"
    local rej_ready="$RUNTIME_DIR/mock-reject.ready" rej_res="$RUNTIME_DIR/mock-reject.json"
    start_mock "$rej_ready" "$rej_res" 25
    run_caller "$WRONG_PIN" "$RUNTIME_DIR/reject-rx.wav" "$RUNTIME_DIR/caller-reject.json" 9
    sleep 1.0
    stop_mock
    grep -q 'Verbose: PIN_GATE_RESULT=REJECTED' "$RUSTISK_LOG" \
        || fail "dialplan did not take the REJECTED branch (no 'Verbose: PIN_GATE_RESULT=REJECTED' in rustisk log)"
    assert_json "$rej_res" "REJECTED bridge never INVITEd" \
        "d['registered'] is True" "d['invite_count'] == 0" >/dev/null \
        || fail "REJECTED case: bridge WAS INVITEd (invite_count != 0) — reject leaked to Dial"
    log "REJECTED: PASS (PIN_GATE_RESULT=REJECTED, bridge invite_count=0)"

    # ---------- CASE 2: GRANTED (correct PIN -> Dial -> two-way media) ----------
    log ""
    log "--- CASE 2: GRANTED (correct PIN -> Dial -> two-way RTP) ---"
    local grt_ready="$RUNTIME_DIR/mock-grant.ready" grt_res="$RUNTIME_DIR/mock-grant.json"
    local caller_res="$RUNTIME_DIR/caller-grant.json"
    start_mock "$grt_ready" "$grt_res" 40
    run_caller "$TEST_PIN" "$RUNTIME_DIR/grant-rx.wav" "$caller_res" 15
    # caller sent BYE; give rustisk time to relay the hangup so the mock finalizes
    for _ in {1..50}; do [[ -f "$grt_res" ]] && break; sleep 0.2; done
    stop_mock

    grep -q 'Verbose: PIN_GATE_RESULT=GRANTED' "$RUSTISK_LOG" \
        || fail "dialplan did not take the GRANTED branch (no 'Verbose: PIN_GATE_RESULT=GRANTED' in rustisk log)"

    # A -> bridge: the mock received the caller's relayed 440 Hz tone
    assert_json "$grt_res" "GRANTED mock RX (A->bridge)" \
        "d['invite_count'] == 1" \
        "d['rx_voice'] >= $MIN_VOICE_FRAMES" \
        "d.get('rtp_src') == '$RK_IP'" \
        "ratio($CALLER_TONE) >= $MIN_TONE_RATIO" \
        "ratio($CALLER_TONE) >= 10 * max((ratio($BRIDGE_TONE), ratio(350), ratio(880), 1))" \
        || fail "GRANTED case: A->bridge media not proven on the mock"

    # bridge -> A: the caller received the bridge's distinct 660 Hz tone
    assert_json "$caller_res" "GRANTED caller RX (bridge->A)" \
        "d.get('voice_rx',0) >= $MIN_VOICE_FRAMES" \
        "d.get('rtp_src') == '$RK_IP'" \
        "ratio($BRIDGE_TONE) >= $MIN_TONE_RATIO" \
        "ratio($BRIDGE_TONE) >= 10 * max((ratio($CALLER_TONE), ratio(350), ratio(880), 1))" \
        || fail "GRANTED case: bridge->A media not proven on the caller"

    log "GRANTED: PASS (two-way RTP: A->bridge 440 Hz + bridge->A 660 Hz, both directions)"

    log ""
    log "================= ALL CASES PASSED ================="
    log "Evidence: $(python3 -c "import json;m=json.load(open('$grt_res'));c=json.load(open('$caller_res'));print(f\"mock rx_voice={m['rx_voice']} tone440={m['tone_ratios'].get('440')} tone660={m['tone_ratios'].get('660')} | caller voice_rx={c['voice_rx']} tone660={c['tone_ratios'].get('660')} tone440={c['tone_ratios'].get('440')}\")")"
    log "Artifacts in $RUNTIME_DIR"
}

main "$@"
