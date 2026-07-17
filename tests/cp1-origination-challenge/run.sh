#!/usr/bin/env bash
# CP1 (M-f) wire-correct digest challenge harness.
#
# Proves, end-to-end on an isolated `--internal` Docker network, that rustisk's
# origination path handles a carrier's 401 challenge WIRE-CORRECTLY — every
# claim verified RECEIVER-SIDE on the offline carrier's captured datagrams,
# never a rustisk TX log:
#
#   (a) the carrier's 401 is ACKed (same branch) — the carrier captures the ACK
#       and never has to retransmit its 401 / record NO-ACK;
#   (b) the retry INVITE arrives with a NEW Via branch, an incremented CSeq, and
#       a VALID digest Authorization (carrier recomputes the response);
#   (c) after the carrier's 200 (CHANGED Contact + Record-Route pointing at a
#       SEPARATE route target T), the 2xx ACK and the in-dialog BYE land on T —
#       NOT back on the core / original request-URI;
#   (d) the 2xx ACK carries the REAL CSeq (2, matching the authed INVITE) and the
#       BYE the next in-dialog CSeq (3) — not a hardcoded 1.
#
# Isolated Docker only: it never touches the live voice stack, Helm, k8s, the
# carrier trunk, or the real PIN. A throwaway six-digit TEST pin is mounted
# read-only (rustisk fails closed without one) and removed on exit.
#
#   tests/cp1-origination-challenge/run.sh
#
# All Docker is reaped on exit (trap).
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/cp1-origination-challenge"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
PROOF="$RUNTIME_DIR/PROOF.txt"
CORE_CAPTURE="$RUNTIME_DIR/core.log"
TARGET_CAPTURE="$RUNTIME_DIR/target.log"

RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"

NET="cp1-net-$$"
RUSTISK_CONTAINER="cp1-rustisk-$$"
CORE_CONTAINER="cp1-core-$$"
TARGET_CONTAINER="cp1-target-$$"
THIRD_OCTET="$((20 + ($$ % 200)))"
SUBNET="10.251.$THIRD_OCTET.0/24"
IP_RANGE="10.251.$THIRD_OCTET.32/27"
RUSTISK_IP="10.251.$THIRD_OCTET.2"   # fixed; core/target IPs stay DYNAMIC
SECRET_DIR=""

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
    reap_container "$CORE_CONTAINER" || leaked=1
    reap_container "$TARGET_CONTAINER" || leaked=1
    reap_container "$RUSTISK_CONTAINER" || leaked=1
    timeout 10 docker network rm "$NET" >/dev/null 2>&1 || true
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/cp1-pin-secret.* ]]; then
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
        if docker logs "$RUSTISK_CONTAINER" 2>&1 | grep -q 'fully booted'; then return 0; fi
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
        if docker exec "$RUSTISK_CONTAINER" python3 -c \
            "import socket,sys; s=socket.create_connection(('127.0.0.1',15038),2); d=s.recv(64); sys.exit(0 if d else 1)" \
            >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

# ---------------------------------------------------------------------------
require_command docker
require_command python3
require_command cargo

say '=== CP1 wire-correct digest challenge harness ==='
rm -rf "$RUNTIME_DIR"
mkdir -p "$CONFIG_DIR" "$RUN_DIR"
: >"$CORE_CAPTURE"; : >"$TARGET_CAPTURE"

SECRET_DIR="$(mktemp -d /mnt/data/herodevs-agents/cp1-pin-secret.XXXXXX)"
chmod 700 "$SECRET_DIR"
umask 077
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

say "Creating isolated --internal network $NET ($SUBNET, dynamic pool $IP_RANGE)..."
docker network create --internal --subnet "$SUBNET" --ip-range "$IP_RANGE" "$NET" >/dev/null

# --- Bring up the route TARGET first so the core can advertise its IP T -------
say 'Starting carrier route target (holds T; captures 2xx ACK + BYE)...'
docker run -d --rm --name "$TARGET_CONTAINER" \
    --network "$NET" --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/carrier.py,dst=/carrier.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /carrier.py --role target --caller "$RUSTISK_IP" \
        --capture /runtime/target.log >/dev/null
T=""
for _ in $(seq 1 40); do T="$(container_ip "$TARGET_CONTAINER")"; [[ -n "$T" ]] && break; sleep 0.25; done
[[ -n "$T" ]] || fail "could not read route-target container IP T"
wait_for_file_line "$TARGET_CAPTURE" "READY role=target" 15 || fail "route target never became ready"
say "Route target IP T = $T"

# --- Bring up the carrier CORE (challenges, answers with Contact/RR = T) -------
say 'Starting carrier core (challenges 401; answers 200 with Contact + Record-Route = T)...'
docker run -d --rm --name "$CORE_CONTAINER" \
    --network "$NET" --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/carrier.py,dst=/carrier.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /carrier.py --role core --caller "$RUSTISK_IP" \
        --route-target "$T" --capture /runtime/core.log >/dev/null
S=""
for _ in $(seq 1 40); do S="$(container_ip "$CORE_CONTAINER")"; [[ -n "$S" ]] && break; sleep 0.25; done
[[ -n "$S" ]] || fail "could not read carrier core container IP S"
wait_for_file_line "$CORE_CAPTURE" "READY role=core" 15 || fail "carrier core never became ready"
[[ "$S" != "$T" ]] || fail "core S and target T collided ($S) — route-set proof needs distinct addresses"
say "Carrier core IP S = $S (route target T = $T; distinct)"

# --- Template pjsip.conf so Dial(PJSIP/carrier) resolves to the core S --------
sed -e "s|@CORE_S@|$S|g" "$HARNESS_DIR/config/pjsip.conf.tmpl" >"$CONFIG_DIR/pjsip.conf"

# --- Start rustisk (fixed IP R) ----------------------------------------------
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
wait_for_ami || fail "rustisk AMI (127.0.0.1:15038) never became reachable"
say 'rustisk booted; AMI reachable.'

# --- Trigger one origination via AMI Originate --------------------------------
say 'AMI Originate PJSIP/carrier ...'
docker exec -i "$RUSTISK_CONTAINER" python3 /ami_originate.py 127.0.0.1 15038 carrier cp1-orig >/dev/null \
    || fail "AMI Originate failed to queue"

# Wait for the full receiver-side cycle to complete on the carrier captures.
wait_for_file_line "$CORE_CAPTURE" "RETRY-INVITE " 15 || { docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true; fail "carrier core never saw the credentialed retry INVITE"; }
wait_for_file_line "$TARGET_CAPTURE" "ACK-2XX-AT-TARGET " 15 || fail "route target never saw the 2xx ACK"
wait_for_file_line "$TARGET_CAPTURE" "BYE-AT-TARGET " 15 || fail "route target never saw the in-dialog BYE"
sleep 1   # let any stray datagram to the core land before asserting silence

# ============================================================================
# Receiver-side assertions
# ============================================================================
VERDICT_OK=1
note() { printf '%s\n' "$*"; }

# (a) challenge ACKed — core captured ACK-CHALLENGE, and never recorded NO-ACK.
if grep -q "ACK-CHALLENGE " "$CORE_CAPTURE" && ! grep -q "NO-ACK " "$CORE_CAPTURE"; then
    A="PASS"
else
    A="FAIL"; VERDICT_OK=0
fi

# (b) retry INVITE: NEW branch, incremented CSeq, VALID digest.
# NB: match " branch=" (leading space) so the retry line's own branch is read,
# not its "prev_branch=" field.
INV_BRANCH="$(grep -m1 "INVITE own=.* auth=no " "$CORE_CAPTURE" | sed -n 's/.* branch=\([^ ]*\).*/\1/p')"
RETRY_LINE="$(grep -m1 "RETRY-INVITE " "$CORE_CAPTURE" || true)"
RETRY_BRANCH="$(printf '%s' "$RETRY_LINE" | sed -n 's/.* branch=\([^ ]*\).*/\1/p')"
RETRY_CSEQ="$(printf '%s' "$RETRY_LINE" | sed -n 's/.*cseq=\([0-9]*\).*/\1/p')"
RETRY_VALID="$(printf '%s' "$RETRY_LINE" | sed -n 's/.*valid=\([a-z]*\).*/\1/p')"
if [[ -n "$INV_BRANCH" && -n "$RETRY_BRANCH" && "$RETRY_BRANCH" != "$INV_BRANCH" \
      && "$RETRY_CSEQ" == "2" && "$RETRY_VALID" == "yes" ]]; then
    B="PASS"
else
    B="FAIL"; VERDICT_OK=0
fi

# (c) route-set/Contact targeting: ACK + BYE at T, NOT at core.
if grep -q "ACK-2XX-AT-TARGET " "$TARGET_CAPTURE" && grep -q "BYE-AT-TARGET " "$TARGET_CAPTURE" \
   && ! grep -q "ACK-2XX-AT-CORE " "$CORE_CAPTURE" && ! grep -q "BYE-AT-CORE " "$CORE_CAPTURE"; then
    C="PASS"
else
    C="FAIL"; VERDICT_OK=0
fi

# (d) real CSeq on the in-dialog requests at T: ACK = "2 ACK", BYE = "3 BYE".
if grep -q "ACK-2XX-AT-TARGET .*cseq=2 ACK" "$TARGET_CAPTURE" \
   && grep -q "BYE-AT-TARGET .*cseq=3 BYE" "$TARGET_CAPTURE"; then
    D="PASS"
else
    D="FAIL"; VERDICT_OK=0
fi

{
    echo "CP1 wire-correct digest challenge harness — PROOF"
    echo "generated: $(date -u +%FT%TZ)"
    echo "rustisk HEAD: $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo
    echo "rustisk IP (fixed):    $RUSTISK_IP"
    echo "carrier core IP S:     $S   (INVITE target; challenges; answers 200)"
    echo "route target IP T:     $T   (Contact + Record-Route target)"
    echo "S != T:                $([[ "$S" != "$T" ]] && echo yes || echo NO)"
    echo
    echo "(a) 401 challenge ACKed (same branch), no retransmit:   $A"
    echo "(b) retry INVITE new-branch + CSeq++ + valid digest:    $B"
    echo "      first INVITE branch:  $INV_BRANCH"
    echo "      retry INVITE branch:  $RETRY_BRANCH (cseq=$RETRY_CSEQ valid=$RETRY_VALID)"
    echo "(c) 2xx ACK + BYE land on route target T, not core:     $C"
    echo "(d) in-dialog CSeq real (ACK=2, BYE=3), not hardcoded 1:$D"
    echo
    echo "--- carrier CORE capture (receiver-side) ---"
    cat "$CORE_CAPTURE" 2>/dev/null || true
    echo "--- route TARGET capture (receiver-side) ---"
    cat "$TARGET_CAPTURE" 2>/dev/null || true
} >"$PROOF"

say ''
say '================ VERDICT ================'
cat "$PROOF"

if (( VERDICT_OK == 1 )); then
    say ''
    say "PASS: CP1 origination challenge handled wire-correctly (receiver-side: (a) ACK, (b) new-branch+CSeq++ +digest, (c) route-set target, (d) real CSeq)."
    say "Proof: $PROOF"
    exit 0
else
    fail "CP1 harness verdict FAILED (see verdict above)"
fi
