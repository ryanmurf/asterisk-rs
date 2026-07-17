#!/usr/bin/env bash
# CP5 (M6 routing MINOR-1, folded) — origination signalling-scope regression.
#
# Locks in, RECEIVER-SIDE, that the M7 origination path advertises the configured
# `external_signaling_address` (never the raw `0.0.0.0` bind) on the CORE INVITE
# AND the in-dialog ACK/BYE, when the carrier is outside `local_net`. See
# README.md for the ancillary-path (REFER/NOTIFY/MESSAGE/REGISTER/UPDATE)
# analysis and deferral — none of them is on the Chime origination path.
#
# RED: unset external_signaling_address -> every request leaks `0.0.0.0` (the
# M6 MINOR-1 bind leak) -> the "external present, no raw bind" assertion RED.
#
# Isolated Docker only; never touches the live voice stack / carrier / real PIN.
#   tests/cp5-origination-signaling-scope/run.sh   [CP5_MODE=green|red-noext]
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/cp5-origination-signaling-scope"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
PROOF="$RUNTIME_DIR/PROOF.txt"
CAPTURE="$RUNTIME_DIR/carrier.log"

RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"

NET="cp5s-net-$$"
RUSTISK_CONTAINER="cp5s-rustisk-$$"
CARRIER_CONTAINER="cp5s-carrier-$$"
THIRD_OCTET="$((20 + ($$ % 200)))"
SUBNET="10.247.$THIRD_OCTET.0/24"
IP_RANGE="10.247.$THIRD_OCTET.32/27"
RUSTISK_IP="10.247.$THIRD_OCTET.2"
SECRET_DIR=""
MODE="${CP5_MODE:-green}"
EXPECT_EXT="signaling.example.net"

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
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/cp5s-secret.* ]]; then
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
case "$MODE" in green|red-noext) ;; *) fail "invalid CP5_MODE='$MODE'";; esac

say "=== CP5 origination signalling-scope harness (mode=$MODE) ==="
rm -rf "$RUNTIME_DIR"; mkdir -p "$CONFIG_DIR" "$RUN_DIR"; : >"$CAPTURE"

SECRET_DIR="$(mktemp -d /mnt/data/herodevs-agents/cp5s-secret.XXXXXX)"
chmod 700 "$SECRET_DIR"; umask 077
printf '%06d\n' "$(( (RANDOM * 32768 + RANDOM) % 1000000 ))" >"$SECRET_DIR/pin"

say "Building rustisk (Rust 1.97.0, CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6})..."
( cd "$REPO_DIR" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}" cargo +1.97.0 build -p rustisk-cli )
[[ -x "$REPO_DIR/target/debug/rustisk" ]] || fail "rustisk debug binary not built"

sed -e "s|@CONFIG_DIR@|$CONFIG_DIR|g" -e "s|@RUN_DIR@|$RUN_DIR|g" \
    "$HARNESS_DIR/config/asterisk.conf.tmpl" >"$CONFIG_DIR/asterisk.conf"
cp "$HARNESS_DIR/config/manager.conf" "$CONFIG_DIR/manager.conf"
cp "$HARNESS_DIR/config/extensions.conf" "$CONFIG_DIR/extensions.conf"
cp "$HARNESS_DIR/config/rtp.conf" "$CONFIG_DIR/rtp.conf"
printf '[general]\nsecret_file = /run/secrets/rustisk/pin\n' >"$CONFIG_DIR/pin_gate.conf"

say "Creating isolated --internal network $NET..."
docker network create --internal --subnet "$SUBNET" --ip-range "$IP_RANGE" "$NET" >/dev/null

say 'Starting offline carrier (captures Via/From/Contact host of INVITE, ACK, BYE)...'
docker run -d --rm --name "$CARRIER_CONTAINER" \
    --network "$NET" --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/carrier_scope.py,dst=/carrier_scope.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /carrier_scope.py --caller "$RUSTISK_IP" --capture /runtime/carrier.log >/dev/null
S=""
for _ in $(seq 1 40); do S="$(container_ip "$CARRIER_CONTAINER")"; [[ -n "$S" ]] && break; sleep 0.25; done
[[ -n "$S" ]] || fail "could not read carrier IP"
wait_for_file_line "$CAPTURE" "READY own=" 15 || fail "carrier never became ready"
say "Carrier IP = $S"

sed -e "s|@CORE_S@|$S|g" "$HARNESS_DIR/config/pjsip.conf.tmpl" >"$CONFIG_DIR/pjsip.conf"
if [[ "$MODE" == "red-noext" ]]; then
    # RED: drop external_signaling_address so the origination path leaks the bind.
    sed -i '/external_signaling_address/d' "$CONFIG_DIR/pjsip.conf"
    say "RED mode: external_signaling_address REMOVED (expect a raw-bind leak)."
fi

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
docker exec -i "$RUSTISK_CONTAINER" python3 /ami_originate.py 127.0.0.1 15038 carrier cp5-orig >/dev/null \
    || fail "AMI Originate failed to queue"

wait_for_file_line "$CAPTURE" "^INVITE " 15 || { docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true; fail "carrier never saw the INVITE"; }
wait_for_file_line "$CAPTURE" "^ACK " 15 || true
wait_for_file_line "$CAPTURE" "^BYE " 15 || true
sleep 1

# Every origination request must advertise the external addr, none the raw bind.
LEAK="$(grep -E '^(INVITE|ACK|BYE) ' "$CAPTURE" | grep -E 'host=0\.0\.0\.0' || true)"
SCOPED_INVITE="$(grep -m1 '^INVITE ' "$CAPTURE" | grep -c "via_host=$EXPECT_EXT" || true)"
SCOPED_ACK="$(grep -m1 '^ACK ' "$CAPTURE" | grep -c "via_host=$EXPECT_EXT" || true)"
SCOPED_BYE="$(grep -m1 '^BYE ' "$CAPTURE" | grep -c "via_host=$EXPECT_EXT" || true)"

if [[ -z "$LEAK" && "$SCOPED_INVITE" == "1" && "$SCOPED_ACK" == "1" && "$SCOPED_BYE" == "1" ]]; then
    RESULT="PASS"
else
    RESULT="FAIL"
fi

{
    echo "CP5 origination signalling-scope harness — PROOF (mode=$MODE)"
    echo "generated: $(date -u +%FT%TZ)"
    echo "rustisk HEAD: $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "expected external signalling host: $EXPECT_EXT"
    echo
    echo "INVITE advertises external:  $SCOPED_INVITE"
    echo "ACK    advertises external:  $SCOPED_ACK"
    echo "BYE    advertises external:  $SCOPED_BYE"
    echo "raw-bind (0.0.0.0) leaks:    $([[ -z "$LEAK" ]] && echo none || echo "$LEAK")"
    echo "result:                      $RESULT"
    echo
    echo "--- carrier capture (receiver-side) ---"; cat "$CAPTURE" 2>/dev/null || true
} >"$PROOF"

say ''; say '================ VERDICT ================'; cat "$PROOF"

if [[ "$MODE" == "green" ]]; then
    [[ "$RESULT" == "PASS" ]] || fail "CP5: origination path did not fully scope (see verdict)"
    say ''; say "PASS: INVITE + ACK + BYE all advertise $EXPECT_EXT; no raw-bind leak on the origination path."
    exit 0
else
    [[ "$RESULT" == "FAIL" ]] || fail "CP5 RED mode expected a leak but found none (assertion cannot fail!)"
    say ''; say "RED captured: without external_signaling_address the origination path leaks the raw bind (assertion RED)."
    exit 0
fi
