#!/usr/bin/env bash
# CP3 (flagship) — AMI Originate waits for answer before running the app.
#
# Proves RECEIVER-SIDE, on an isolated `--internal` Docker network, that an AMI
# Originate does NOT run the dialplan app/exten until the far end ANSWERS. The
# offline carrier DELAYS its 200 by --answer-delay and tags every captured
# datagram `phase=pre` (before it sent the 200) or `phase=post`:
#
#   PRE-ANSWER SILENCE: in the pre-answer window the carrier receives ONLY the
#     INVITE — no ACK/BYE/CANCEL and no RTP (the app has not run).
#   POST-ANSWER RUN: after the 200 the answer is ACKed and the app runs — the
#     BYE and the app's Playback RTP appear phase=post. The app plays a real
#     (generated, TEST-only) audio file, so post-answer RTP > 0 is REQUIRED:
#     it proves the media pump delivers, making pre-answer RTP == 0 a real
#     no-media-before-answer guarantee instead of a vacuous one.
#
# RED (revert CP3 -> app runs immediately): the app runs and tears the unanswered
# leg down before the delayed 200; the 200 is orphaned (no post-answer ACK, no
# BYE). Captured below by reverting the wait.
#
# Scenarios (both run by default; select one with CP3_SCENARIO=baseline|early-media):
#   baseline    — carrier sends 100 then the DELAYED 200 (the original CP3 run).
#   early-media — carrier ALSO sends a 183 Session Progress WITH SDP before the
#     delayed 200 (M7 follow-up, WIRE-MINOR-1). A pre-answer media path now
#     EXISTS, giving the `RTP phase=pre == 0` guard independent teeth: if the
#     Originate wait were defeated so the app ran on the 183 (the classic
#     "early media treated as answer" defect), the app's Playback RTP would
#     arrive pre-answer and that guard alone REDs (in the baseline a defeated
#     wait can only surface as an orphaned 200 — the app has no remote media
#     address before the 200 there).
#
# Isolated Docker only; never touches the live voice stack / carrier / real PIN.
#   tests/cp3-originate-wait/run.sh
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"

# With no CP3_SCENARIO set, run BOTH scenarios sequentially.
if [[ -z "${CP3_SCENARIO:-}" ]]; then
    CP3_SCENARIO=baseline "${BASH_SOURCE[0]}"
    CP3_SCENARIO=early-media "${BASH_SOURCE[0]}"
    printf '\nPASS: both CP3 scenarios (baseline + early-media) passed.\n'
    exit 0
fi
case "$CP3_SCENARIO" in
    baseline) EARLY_MEDIA=0 ;;
    early-media) EARLY_MEDIA=1 ;;
    *) printf 'FAIL: unknown CP3_SCENARIO=%s (baseline|early-media)\n' "$CP3_SCENARIO" >&2; exit 2 ;;
esac

RUNTIME_DIR="$REPO_DIR/target/cp3-originate-wait/$CP3_SCENARIO"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
PROOF="$RUNTIME_DIR/PROOF.txt"
CAPTURE="$RUNTIME_DIR/carrier.log"

RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"

NET="cp3-net-$$"
RUSTISK_CONTAINER="cp3-rustisk-$$"
CARRIER_CONTAINER="cp3-carrier-$$"
THIRD_OCTET="$((20 + ($$ % 200)))"
SUBNET="10.249.$THIRD_OCTET.0/24"
IP_RANGE="10.249.$THIRD_OCTET.32/27"
RUSTISK_IP="10.249.$THIRD_OCTET.2"
SECRET_DIR=""
ANSWER_DELAY="${CP3_ANSWER_DELAY:-2.0}"

reap_container() {
    local c="$1" hp i
    docker inspect "$c" >/dev/null 2>&1 || return 0
    hp=""
    for i in 1 2 3 4 5; do
        hp="$(docker inspect -f '{{.State.Pid}}' "$c" 2>/dev/null || true)"
        [[ -n "$hp" && "$hp" != "0" ]] && break
        sleep 0.3
    done
    if [[ -n "$hp" && "$hp" != "0" ]]; then
        kill -TERM "$hp" 2>/dev/null || true
        timeout 3 docker wait "$c" >/dev/null 2>&1 || true
        if docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null | grep -q true; then
            kill -KILL "$hp" 2>/dev/null || true
            timeout 3 docker wait "$c" >/dev/null 2>&1 || true
        fi
    fi
    timeout 10 docker rm -f "$c" >/dev/null 2>&1 || true
    for _ in $(seq 1 20); do docker inspect "$c" >/dev/null 2>&1 || return 0; sleep 0.25; done
    return 1
}

cleanup() {
    docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true
    local leaked=0
    reap_container "$CARRIER_CONTAINER" || leaked=1
    reap_container "$RUSTISK_CONTAINER" || leaked=1
    timeout 10 docker network rm "$NET" >/dev/null 2>&1 || true
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/cp3-pin-secret.* ]]; then
        rm -rf "$SECRET_DIR"
    fi
    local still_net=""
    docker network inspect "$NET" >/dev/null 2>&1 && still_net="$NET"
    if (( leaked == 1 )) || [[ -n "$still_net" ]]; then
        printf 'CLEANUP WARNING: leaked docker resources — reap by hand (docker rm -f, docker network rm %s)\n' "$NET" >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
require_command() { command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"; }
say() { printf '%s\n' "$*"; }
container_ip() { docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$1" 2>/dev/null; }

wait_for_file_line() {
    local file="$1" pattern="$2" timeout="$3"
    local deadline=$(( $(date +%s) + timeout ))
    while (( $(date +%s) < deadline )); do
        [[ -f "$file" ]] && grep -q -- "$pattern" "$file" && return 0
        sleep 0.3
    done
    return 1
}

wait_for_rustisk_boot() {
    local deadline=$(( $(date +%s) + 40 ))
    while (( $(date +%s) < deadline )); do
        docker logs "$RUSTISK_CONTAINER" 2>&1 | grep -q 'fully booted' && return 0
        if ! docker inspect -f '{{.State.Running}}' "$RUSTISK_CONTAINER" 2>/dev/null | grep -q true; then
            docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true
            fail "rustisk container exited during boot; see $RUSTISK_LOG"
        fi
        sleep 0.5
    done
    return 1
}

wait_for_ami() {
    local deadline=$(( $(date +%s) + 20 ))
    while (( $(date +%s) < deadline )); do
        docker exec "$RUSTISK_CONTAINER" python3 -c \
            "import socket,sys; s=socket.create_connection(('127.0.0.1',15038),2); d=s.recv(64); sys.exit(0 if d else 1)" \
            >/dev/null 2>&1 && return 0
        sleep 0.5
    done
    return 1
}

# ---------------------------------------------------------------------------
require_command docker; require_command python3; require_command cargo

say '=== CP3 wait-for-answer harness ==='
rm -rf "$RUNTIME_DIR"; mkdir -p "$CONFIG_DIR" "$RUN_DIR"; : >"$CAPTURE"

SECRET_DIR="$(mktemp -d /mnt/data/herodevs-agents/cp3-pin-secret.XXXXXX)"
chmod 700 "$SECRET_DIR"; umask 077
printf '%06d\n' "$(( (RANDOM * 32768 + RANDOM) % 1000000 ))" >"$SECRET_DIR/pin"

say "Building rustisk (Rust 1.97.0, CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6})..."
( cd "$REPO_DIR" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}" cargo +1.97.0 build -p rustisk-cli )
[[ -x "$REPO_DIR/target/debug/rustisk" ]] || fail "rustisk debug binary not built"

sed -e "s|@CONFIG_DIR@|$CONFIG_DIR|g" -e "s|@RUN_DIR@|$RUN_DIR|g" \
    "$HARNESS_DIR/config/asterisk.conf.tmpl" >"$CONFIG_DIR/asterisk.conf"
cp "$HARNESS_DIR/config/manager.conf" "$CONFIG_DIR/manager.conf"
# The app plays a real (generated, TEST-only) 1s G.711u file so app media
# genuinely flows: post-answer RTP > 0 proves the media pump works, which is
# what gives the pre-answer RTP == 0 guard its teeth (see verdict).
MEDIA_FILE="$RUNTIME_DIR/media/app-tone.ulaw"
mkdir -p "$RUNTIME_DIR/media"
python3 -c "import sys; sys.stdout.buffer.write(bytes(range(256)) * 32)" >"$MEDIA_FILE"
sed -e "s|@MEDIA_FILE@|$MEDIA_FILE|g" \
    "$HARNESS_DIR/config/extensions.conf.tmpl" >"$CONFIG_DIR/extensions.conf"
cp "$HARNESS_DIR/config/rtp.conf" "$CONFIG_DIR/rtp.conf"
printf '[general]\nsecret_file = /run/secrets/rustisk/pin\n' >"$CONFIG_DIR/pin_gate.conf"

say "Creating isolated --internal network $NET..."
docker network create --internal --subnet "$SUBNET" --ip-range "$IP_RANGE" "$NET" >/dev/null

EM_FLAG=""
(( EARLY_MEDIA )) && EM_FLAG="--early-media"
say "Starting offline carrier (scenario=$CP3_SCENARIO; delays 200 by ${ANSWER_DELAY}s; captures SIP + RTP by phase)..."
docker run -d --rm --name "$CARRIER_CONTAINER" \
    --network "$NET" --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/carrier_delay.py,dst=/carrier_delay.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /carrier_delay.py --caller "$RUSTISK_IP" \
        --capture /runtime/carrier.log --answer-delay "$ANSWER_DELAY" $EM_FLAG >/dev/null
S=""
for _ in $(seq 1 40); do S="$(container_ip "$CARRIER_CONTAINER")"; [[ -n "$S" ]] && break; sleep 0.25; done
[[ -n "$S" ]] || fail "could not read carrier container IP"
wait_for_file_line "$CAPTURE" "READY own=" 15 || fail "carrier never became ready"
say "Carrier IP = $S"

sed -e "s|@CORE_S@|$S|g" "$HARNESS_DIR/config/pjsip.conf.tmpl" >"$CONFIG_DIR/pjsip.conf"

say "Starting isolated rustisk at $RUSTISK_IP..."
docker run -d --rm --name "$RUSTISK_CONTAINER" \
    --network "$NET" --ip "$RUSTISK_IP" \
    --ulimit nofile=65536:65536 --user "$(id -u):$(id -g)" \
    --entrypoint /rustisk \
    --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/ami_originate.py,dst=/ami_originate.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
    --mount "type=bind,src=$SECRET_DIR/pin,dst=/run/secrets/rustisk/pin,readonly" \
    "$RUSTISK_IMAGE" -f -vvv -C "$CONFIG_DIR/asterisk.conf" >/dev/null

wait_for_rustisk_boot || fail "rustisk did not report fully booted"
wait_for_ami || fail "rustisk AMI never became reachable"
say 'rustisk booted; AMI reachable.'

say 'AMI Originate PJSIP/carrier ...'
docker exec -i "$RUSTISK_CONTAINER" python3 /ami_originate.py 127.0.0.1 15038 carrier cp3-orig >/dev/null \
    || fail "AMI Originate failed to queue"

# Wait for the carrier to send its (delayed) 200 and for the post-answer BYE.
wait_for_file_line "$CAPTURE" "SENT-200 " 15 || { docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true; fail "carrier never sent its delayed 200"; }
wait_for_file_line "$CAPTURE" "BYE phase=post" 15 || true   # asserted below (RED path won't have it)
sleep 1

# ============================================================================
# Receiver-side assertions
# ============================================================================
# Pre-answer window MUST be silent except for the INVITE (no ACK/BYE/CANCEL/RTP).
PRE_NOISE="$(grep -E 'phase=pre' "$CAPTURE" | grep -E '^(ACK|BYE|CANCEL|RTP)' || true)"
if [[ -z "$PRE_NOISE" ]]; then SILENCE="PASS"; else SILENCE="FAIL"; fi

# Post-answer: the answer is ACKed and the app runs (BYE) AFTER the 200.
if grep -q "ACK phase=post" "$CAPTURE" && grep -q "BYE phase=post" "$CAPTURE"; then
    POST="PASS"
else
    POST="FAIL"
fi

# App media (the app's Playback RTP) must have arrived — and only after answer.
# RTP_POST > 0 proves the app's media pump actually delivers to the carrier, so
# RTP_PRE == 0 is a real no-media-before-answer guarantee, not a vacuous one.
RTP_PRE="$(grep -c 'RTP phase=pre' "$CAPTURE" || true)"
RTP_POST="$(grep -c 'RTP phase=post' "$CAPTURE" || true)"

# Early-media scenario: the pre-answer media path must have EXISTED (183 with
# SDP sent BEFORE the 200) — otherwise the RTP phase=pre check has no teeth.
EARLY="n/a"
if (( EARLY_MEDIA )); then
    L183="$(grep -n -m1 'SENT-183 ' "$CAPTURE" | cut -d: -f1 || true)"
    L200="$(grep -n -m1 'SENT-200 ' "$CAPTURE" | cut -d: -f1 || true)"
    if [[ -n "$L183" && -n "$L200" ]] && (( L183 < L200 )); then EARLY="PASS"; else EARLY="FAIL"; fi
fi

VERDICT_OK=1
[[ "$SILENCE" == "PASS" ]] || VERDICT_OK=0
[[ "$POST" == "PASS" ]] || VERDICT_OK=0
[[ "${RTP_PRE:-0}" == "0" ]] || VERDICT_OK=0
[[ "${RTP_POST:-0}" != "0" ]] || VERDICT_OK=0
[[ "$EARLY" == "FAIL" ]] && VERDICT_OK=0

{
    echo "CP3 wait-for-answer harness — PROOF (scenario=$CP3_SCENARIO)"
    echo "generated: $(date -u +%FT%TZ)"
    echo "rustisk HEAD: $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "answer delay: ${ANSWER_DELAY}s"
    echo
    echo "PRE-ANSWER SILENCE (only INVITE before the 200, no ACK/BYE/CANCEL/RTP): $SILENCE"
    echo "POST-ANSWER RUN   (answer ACKed + app BYE after the 200):              $POST"
    if (( EARLY_MEDIA )); then
        echo "EARLY-MEDIA PATH  (183 with SDP sent before the 200):                  $EARLY"
    fi
    echo "RTP datagrams pre-answer (must be 0):            ${RTP_PRE:-0}"
    echo "RTP datagrams post-answer (app media, must be >0): ${RTP_POST:-0}"
    echo
    echo "--- carrier capture (receiver-side, phase-tagged, rel = seconds since start) ---"
    cat "$CAPTURE" 2>/dev/null || true
} >"$PROOF"

say ''
say '================ VERDICT ================'
cat "$PROOF"

if (( VERDICT_OK == 1 )); then
    say ''
    say "PASS ($CP3_SCENARIO): Originate stayed SILENT before answer and ran the app only after the delayed 200 (receiver-side)."
    exit 0
else
    fail "CP3 harness verdict FAILED (scenario=$CP3_SCENARIO; see verdict above)"
fi
