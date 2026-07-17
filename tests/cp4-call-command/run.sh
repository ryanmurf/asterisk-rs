#!/usr/bin/env bash
# CP4 (M-k) — bin/call.sh migrated to an AMI operational command.
#
# Exercises the migrated command against the OFFLINE carrier (never live qa-sip),
# proving RECEIVER-SIDE (the carrier's captured datagrams) that:
#   A --dry-run      places NO call (carrier sees no INVITE);
#   B a real call    reaches the carrier (one INVITE);
#   C duplicate guard refuses a second concurrent call to the same dest
#                    (carrier still sees exactly one INVITE);
#   D --hangup-all without --force REFUSES (the hupall foot-gun is guarded — the
#                    live call is untouched, carrier sees no BYE);
#   E --hangup-all --force performs the guarded hupall (carrier sees the BYE).
#
# The AMI secret is read from a mounted file (a k8s Secret in prod), never argv.
# Isolated Docker only; never touches the live voice stack / carrier / real PIN.
#   tests/cp4-call-command/run.sh
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/cp4-call-command"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
PROOF="$RUNTIME_DIR/PROOF.txt"
CAPTURE="$RUNTIME_DIR/carrier.log"

RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"

NET="cp4-net-$$"
RUSTISK_CONTAINER="cp4-rustisk-$$"
CARRIER_CONTAINER="cp4-carrier-$$"
THIRD_OCTET="$((20 + ($$ % 200)))"
SUBNET="10.248.$THIRD_OCTET.0/24"
IP_RANGE="10.248.$THIRD_OCTET.32/27"
RUSTISK_IP="10.248.$THIRD_OCTET.2"
SECRET_DIR=""
DEST=2000

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
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/cp4-secret.* ]]; then
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
# grep -c already prints the count (0 included) and exits 1 on no match; swallow
# only the exit code so we never emit a spurious second "0".
count() { grep -c -- "$1" "$CAPTURE" 2>/dev/null || true; }

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

# call.sh inside the rustisk container, pointed at the local AMI + mounted secret.
callsh() {
    docker exec \
        -e AMI_HOST=127.0.0.1 -e AMI_PORT=15038 -e AMI_USERNAME=operator \
        -e AMI_SECRET_FILE=/run/secrets/rustisk-ami/secret \
        -e CALL_ENDPOINT=carrier -e CALL_CONTEXT=default \
        "$RUSTISK_CONTAINER" bash /bin-call/call.sh "$@"
}

# ---------------------------------------------------------------------------
require_command docker; require_command python3; require_command cargo

say '=== CP4 AMI operational command (bin/call.sh) harness ==='
rm -rf "$RUNTIME_DIR"; mkdir -p "$CONFIG_DIR" "$RUN_DIR"; : >"$CAPTURE"

SECRET_DIR="$(mktemp -d /mnt/data/herodevs-agents/cp4-secret.XXXXXX)"
chmod 700 "$SECRET_DIR"; umask 077
printf '%06d\n' "$(( (RANDOM * 32768 + RANDOM) % 1000000 ))" >"$SECRET_DIR/pin"
printf 'cp4-operator-secret' >"$SECRET_DIR/ami-secret"   # matches manager.conf [operator]

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

say 'Starting offline carrier (answers 200; captures INVITE + BYE)...'
docker run -d --rm --name "$CARRIER_CONTAINER" \
    --network "$NET" --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/carrier.py,dst=/carrier.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /carrier.py --caller "$RUSTISK_IP" --capture /runtime/carrier.log >/dev/null
S=""
for _ in $(seq 1 40); do S="$(container_ip "$CARRIER_CONTAINER")"; [[ -n "$S" ]] && break; sleep 0.25; done
[[ -n "$S" ]] || fail "could not read carrier IP"
wait_for_file_line "$CAPTURE" "READY own=" 15 || fail "carrier never became ready"
say "Carrier IP = $S"

sed -e "s|@CORE_S@|$S|g" "$HARNESS_DIR/config/pjsip.conf.tmpl" >"$CONFIG_DIR/pjsip.conf"

say "Starting isolated rustisk at $RUSTISK_IP..."
docker run -d --rm --name "$RUSTISK_CONTAINER" \
    --network "$NET" --ip "$RUSTISK_IP" \
    --ulimit nofile=65536:65536 --user "$(id -u):$(id -g)" \
    --entrypoint /rustisk \
    --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
    --mount "type=bind,src=$REPO_DIR/bin,dst=/bin-call,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
    --mount "type=bind,src=$SECRET_DIR/pin,dst=/run/secrets/rustisk/pin,readonly" \
    --mount "type=bind,src=$SECRET_DIR/ami-secret,dst=/run/secrets/rustisk-ami/secret,readonly" \
    "$RUSTISK_IMAGE" -f -vvv -C "$CONFIG_DIR/asterisk.conf" >/dev/null

wait_for_rustisk_boot || fail "rustisk did not report fully booted"
wait_for_ami || fail "rustisk AMI never became reachable"
say 'rustisk booted; AMI reachable.'

# ============================================================================
# A. --dry-run places NO call.
# ============================================================================
A_OUT="$(callsh "$DEST" --dry-run 2>&1 || true)"
sleep 1
if echo "$A_OUT" | grep -q "DRY RUN" && [[ "$(count 'INVITE-FROM ')" == "0" ]]; then
    A="PASS"; else A="FAIL"; fi

# ============================================================================
# B. a real call reaches the carrier (exactly one INVITE).
# ============================================================================
callsh "$DEST" >/dev/null 2>&1 || true
wait_for_file_line "$CAPTURE" "INVITE-FROM " 10 || { docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true; fail "real call never reached the carrier"; }
sleep 1
if [[ "$(count 'INVITE-FROM ')" == "1" ]]; then B="PASS"; else B="FAIL"; fi

# ============================================================================
# C. duplicate-call guard refuses a second concurrent call to the same dest.
# ============================================================================
C_OUT="$(callsh "$DEST" 2>&1 || true)"
sleep 1
if echo "$C_OUT" | grep -q "already up" && [[ "$(count 'INVITE-FROM ')" == "1" ]]; then
    C="PASS"; else C="FAIL"; fi

# ============================================================================
# D. --hangup-all WITHOUT --force refuses (foot-gun guarded; call untouched).
# ============================================================================
set +e
D_OUT="$(callsh --hangup-all 2>&1)"; D_RC=$?
set -e
sleep 1
if echo "$D_OUT" | grep -q "REFUSING" && [[ "$D_RC" != "0" ]] && [[ "$(count 'BYE ')" == "0" ]]; then
    D="PASS"; else D="FAIL"; fi

# ============================================================================
# E. --hangup-all --force performs the guarded hupall (carrier sees the BYE).
# ============================================================================
callsh --hangup-all --force >/dev/null 2>&1 || true
wait_for_file_line "$CAPTURE" "BYE " 10 || true
sleep 1
if [[ "$(count 'BYE ')" -ge 1 ]]; then E="PASS"; else E="FAIL"; fi

VERDICT_OK=1
for r in "$A" "$B" "$C" "$D" "$E"; do [[ "$r" == "PASS" ]] || VERDICT_OK=0; done

{
    echo "CP4 AMI operational command (bin/call.sh) — PROOF"
    echo "generated: $(date -u +%FT%TZ)"
    echo "rustisk HEAD: $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo
    echo "A --dry-run places no call:                       $A"
    echo "B real call reaches carrier (1 INVITE):           $B"
    echo "C duplicate-call guard refuses 2nd identical call: $C"
    echo "D --hangup-all without --force REFUSES (guarded):  $D  (rc=$D_RC)"
    echo "E --hangup-all --force performs guarded hupall:    $E"
    echo
    echo "--- A (dry-run) output ---"; echo "$A_OUT"
    echo "--- C (duplicate) output ---"; echo "$C_OUT"
    echo "--- D (hangup-all refuse) output ---"; echo "$D_OUT"
    echo "--- carrier capture (receiver-side) ---"; cat "$CAPTURE" 2>/dev/null || true
} >"$PROOF"

say ''; say '================ VERDICT ================'; cat "$PROOF"

if (( VERDICT_OK == 1 )); then
    say ''
    say "PASS: bin/call.sh AMI migration — dry-run, duplicate guard, and guarded hupall all proven receiver-side."
    exit 0
else
    fail "CP4 harness verdict FAILED (see verdict above)"
fi
