#!/usr/bin/env bash
# CP5 (B4) container-restart REGISTER acceptance harness.
#
# Proves, end-to-end with a REAL container restart on an isolated `--internal`
# Docker network, that rustisk's dynamic authenticated REGISTER routes an
# outbound Dial to a bridge's CURRENT container IP and FOLLOWS a restart to the
# bridge's NEW IP — verified RECEIVER-SIDE (the INVITE datagram's arrival on the
# bridge/sentinel, never a rustisk TX log).
#
#   GREEN-A  bridge digest-REGISTERs advertising container IP A; Dial(PJSIP/
#            bridge) INVITE arrives at A (captured on the bridge).
#   restart  bridge container is destroyed; a sentinel seizes the vacated A; a
#            fresh bridge container comes up with a NEW IP B and re-REGISTERs.
#   GREEN-B  Dial(PJSIP/bridge) INVITE arrives at B and NOT at stale A (sentinel
#            silent).
#   RED      Dial(PJSIP/bridge_pinned) — routing defeated by a STATIC contact
#            pinned to A — misroutes to stale A; the sentinel catches it and the
#            follow-to-B assertion goes RED. This negative control proves the
#            A-detection is real (the harness can fail).
#
# Isolated Docker only: it never touches the live voice stack, Helm, k8s, the
# carrier trunk, or the real PIN. A throwaway six-digit TEST pin is generated,
# mounted read-only (rustisk fails closed without one), and removed on exit.
#
#   tests/cp5-register-restart/run.sh
#
# Env: CP5_CASE=all|green|red  (default all).  All Docker is reaped on exit.
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/cp5-register-restart"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
PROOF="$RUNTIME_DIR/PROOF.txt"
BRIDGE_CAPTURE="$RUNTIME_DIR/bridge-invites.log"
SENTINEL_CAPTURE="$RUNTIME_DIR/sentinel-invites.log"
BRIDGE_STATUS="$RUNTIME_DIR/bridge-status.log"
SENTINEL_STATUS="$RUNTIME_DIR/sentinel-status.log"

# The rustisk container carries the daemon binary bind-mounted into a pinned
# python image (same pattern as tests/freeswitch-pin-gate). python3 is the SIP
# agents' and AMI driver's only runtime dependency.
RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"

NET="cp5-net-$$"
RUSTISK_CONTAINER="cp5-rustisk-$$"
BRIDGE_CONTAINER="cp5-bridge-$$"
SENTINEL_CONTAINER="cp5-sentinel-$$"
THIRD_OCTET="$((20 + ($$ % 200)))"
SUBNET="10.252.$THIRD_OCTET.0/24"
# Dynamic pool is a small high sub-range so the bridge/sentinel never collide
# with rustisk's fixed low address. A and B are still Docker-assigned (never
# hardcoded) — just constrained to .32-.63.
IP_RANGE="10.252.$THIRD_OCTET.32/27"
RUSTISK_IP="10.252.$THIRD_OCTET.2"   # fixed; A and B stay DYNAMIC / runtime-read
SECRET_DIR=""
CASE="${CP5_CASE:-all}"

# tron's docker is userns-remapped: the daemon cannot SIGKILL a container whose
# process runs as our uid (`docker stop`/`docker rm -f` hang or 'permission
# denied'). The containers run `--user $(id -u)`, so WE own their host PID and
# can signal it. Reap = kill the host PID, then rm.
reap_container() {
    local c="$1" hp i
    docker inspect "$c" >/dev/null 2>&1 || return 0
    # The host PID can read empty for a container still mid-start — retry.
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
    # Wait for the name/endpoint to be released; report a leak if it persists.
    for _ in $(seq 1 20); do docker inspect "$c" >/dev/null 2>&1 || return 0; sleep 0.25; done
    return 1
}

cleanup() {
    # Snapshot rustisk logs before teardown (best-effort).
    docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true
    local leaked=0
    reap_container "$SENTINEL_CONTAINER" || leaked=1
    reap_container "$BRIDGE_CONTAINER" || leaked=1
    reap_container "$RUSTISK_CONTAINER" || leaked=1
    timeout 10 docker network rm "$NET" >/dev/null 2>&1 || true
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/cp5-pin-secret.* ]]; then
        rm -rf "$SECRET_DIR"
    fi
    # An EXIT trap cannot flip an already-set exit code, but a docker leak must
    # be impossible to miss (it can wedge tron).
    local still_net=""
    docker network inspect "$NET" >/dev/null 2>&1 && still_net="$NET"
    if (( leaked == 1 )) || [[ -n "$still_net" ]]; then
        printf 'CLEANUP WARNING: leaked docker resources — reap by hand (kill host PID, docker rm -f, docker network rm %s)\n' "$NET" >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
require_command() { command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"; }
say() { printf '%s\n' "$*"; }

container_ip() {
    docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$1" 2>/dev/null
}

count_lines() { [[ -f "$1" ]] && wc -l <"$1" | tr -d ' ' || echo 0; }

# Wait until $file has a line beyond line number $prev that matches $pattern.
# Echoes the matching line; returns 1 on timeout.
wait_new_line() {
    local file="$1" prev="$2" pattern="$3" timeout="$4"
    local deadline=$(( $(date +%s) + timeout ))
    while (( $(date +%s) < deadline )); do
        if [[ -f "$file" ]]; then
            local total; total="$(count_lines "$file")"
            if (( total > prev )); then
                local hit
                hit="$(tail -n +"$((prev + 1))" "$file" | grep -m1 -- "$pattern" || true)"
                if [[ -n "$hit" ]]; then printf '%s\n' "$hit"; return 0; fi
            fi
        fi
        sleep 0.3
    done
    return 1
}

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

originate() {
    # originate ENDPOINT ACTION_ID
    docker exec -i "$RUSTISK_CONTAINER" python3 /ami_originate.py 127.0.0.1 15038 "$1" "$2"
}

# ---------------------------------------------------------------------------
require_command docker
require_command python3
require_command cargo

# Reject an unknown case up front: an unvalidated selector would skip the case-
# gated assertions and still exit 0 with a full-acceptance message (false pass).
case "$CASE" in
    all|green|red) ;;
    *) fail "invalid CP5_CASE='$CASE' (expected: all|green|red)" ;;
esac

say '=== CP5 container-restart REGISTER harness ==='
rm -rf "$RUNTIME_DIR"
mkdir -p "$CONFIG_DIR" "$RUN_DIR"
: >"$BRIDGE_CAPTURE"; : >"$SENTINEL_CAPTURE"; : >"$BRIDGE_STATUS"; : >"$SENTINEL_STATUS"

# Throwaway random TEST pin (rustisk fails closed without a mounted secret).
# Generated locally, mounted read-only, never logged, removed on exit — a pure
# test value, unrelated to any production secret.
SECRET_DIR="$(mktemp -d /mnt/data/herodevs-agents/cp5-pin-secret.XXXXXX)"
chmod 700 "$SECRET_DIR"
umask 077
printf '%06d\n' "$(( (RANDOM * 32768 + RANDOM) % 1000000 ))" >"$SECRET_DIR/pin"

say "Building rustisk (Rust 1.97.0, CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6})..."
( cd "$REPO_DIR" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}" cargo +1.97.0 build -p rustisk-cli )
[[ -x "$REPO_DIR/target/debug/rustisk" ]] || fail "rustisk debug binary not built"

# Static rustisk config that does not depend on A.
sed -e "s|@CONFIG_DIR@|$CONFIG_DIR|g" -e "s|@RUN_DIR@|$RUN_DIR|g" \
    "$HARNESS_DIR/config/asterisk.conf.tmpl" >"$CONFIG_DIR/asterisk.conf"
cp "$HARNESS_DIR/config/manager.conf" "$CONFIG_DIR/manager.conf"
cp "$HARNESS_DIR/config/extensions.conf" "$CONFIG_DIR/extensions.conf"
cp "$HARNESS_DIR/config/rtp.conf" "$CONFIG_DIR/rtp.conf"
printf '[general]\nsecret_file = /run/secrets/rustisk/pin\n' >"$CONFIG_DIR/pin_gate.conf"

say "Creating isolated --internal network $NET ($SUBNET, dynamic pool $IP_RANGE)..."
docker network create --internal --subnet "$SUBNET" --ip-range "$IP_RANGE" "$NET" >/dev/null

# --- Bring up the bridge FIRST so we can read its Docker-assigned IP A --------
say 'Starting sip-bridge (advertises its own container IP, digest-REGISTERs)...'
docker run -d --rm --name "$BRIDGE_CONTAINER" \
    --network "$NET" \
    --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/sip_agent.py,dst=/sip_agent.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /sip_agent.py --role bridge --registrar "$RUSTISK_IP" \
        --capture /runtime/bridge-invites.log --status /runtime/bridge-status.log >/dev/null

A=""
for _ in $(seq 1 40); do A="$(container_ip "$BRIDGE_CONTAINER")"; [[ -n "$A" ]] && break; sleep 0.25; done
[[ -n "$A" ]] || fail "could not read bridge container IP A"
say "Bridge container IP A = $A"

# --- Generate pjsip.conf pinning bridge_pinned's STATIC contact at A ----------
sed -e "s|@PINNED_A@|$A|g" "$HARNESS_DIR/config/pjsip.conf.tmpl" >"$CONFIG_DIR/pjsip.conf"

# --- Start rustisk (fixed IP R) ----------------------------------------------
say "Starting isolated rustisk at $RUSTISK_IP..."
docker run -d --rm --name "$RUSTISK_CONTAINER" \
    --network "$NET" \
    --ip "$RUSTISK_IP" \
    --ulimit nofile=65536:65536 \
    --user "$(id -u):$(id -g)" \
    --entrypoint /rustisk \
    --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/ami_originate.py,dst=/ami_originate.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
    --mount "type=bind,src=$SECRET_DIR/pin,dst=/run/secrets/rustisk/pin,readonly" \
    "$RUSTISK_IMAGE" -f -vvv -C "$CONFIG_DIR/asterisk.conf" >/dev/null

wait_for_rustisk_boot || fail "rustisk did not report fully booted"
wait_for_ami || fail "rustisk AMI (127.0.0.1:15038) never became reachable"
say 'rustisk booted; AMI reachable.'

wait_for_file_line "$BRIDGE_STATUS" "REGISTERED own=$A" 90 \
    || fail "bridge never completed digest REGISTER from A=$A"
say "Bridge digest-REGISTERed from A=$A (401 -> digest -> 200)."

# ============================================================================
# GREEN-A: with the bridge live at A, Dial(PJSIP/bridge) must arrive at A.
# ============================================================================
GREEN_A=SKIP
prev="$(count_lines "$BRIDGE_CAPTURE")"
originate bridge cp5-green-a >/dev/null || fail "AMI Originate (green-a) failed"
if line="$(wait_new_line "$BRIDGE_CAPTURE" "$prev" "own=$A" 15)"; then
    say "GREEN-A pass: INVITE arrived RECEIVER-SIDE at A. [$line]"
    GREEN_A=PASS
else
    GREEN_A=FAIL
    docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true
    fail "GREEN-A: no INVITE datagram observed at bridge A=$A"
fi

# ============================================================================
# RESTART: destroy the bridge, seize the vacated A with a sentinel, bring up a
# fresh bridge that MUST get a new IP B, then re-REGISTER from B.
# ============================================================================
say 'Restarting bridge container (new IP, re-REGISTER)...'
reap_container "$BRIDGE_CONTAINER"

# Seize A so Docker cannot hand it back to the new bridge and so a stale-route
# INVITE to A has a RECEIVER that captures it.
docker run -d --rm --name "$SENTINEL_CONTAINER" \
    --network "$NET" \
    --ip "$A" \
    --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/sip_agent.py,dst=/sip_agent.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /sip_agent.py --role sentinel --registrar "$RUSTISK_IP" \
        --capture /runtime/sentinel-invites.log --status /runtime/sentinel-status.log >/dev/null
wait_for_file_line "$SENTINEL_STATUS" "REGISTERED own=$A" 15 \
    || fail "sentinel did not come up holding vacated A=$A"
say "Sentinel seized vacated A=$A."

docker run -d --rm --name "$BRIDGE_CONTAINER" \
    --network "$NET" \
    --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/sip_agent.py,dst=/sip_agent.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    "$RUSTISK_IMAGE" python3 /sip_agent.py --role bridge --registrar "$RUSTISK_IP" \
        --capture /runtime/bridge-invites.log --status /runtime/bridge-status.log >/dev/null

B=""
for _ in $(seq 1 40); do B="$(container_ip "$BRIDGE_CONTAINER")"; [[ -n "$B" ]] && break; sleep 0.25; done
[[ -n "$B" ]] || fail "could not read restarted bridge container IP B"
[[ "$B" != "$A" ]] || fail "restarted bridge came back with the SAME IP ($B); A/B not distinct"
say "Restarted bridge container IP B = $B (distinct from A=$A)."

wait_for_file_line "$BRIDGE_STATUS" "REGISTERED own=$B" 90 \
    || fail "restarted bridge never completed digest REGISTER from B=$B"
say "Bridge re-REGISTERed from B=$B (401 -> digest -> 200)."

# ============================================================================
# GREEN-B: Dial(PJSIP/bridge) INVITE must FOLLOW to B and NOT hit stale A.
# ============================================================================
GREEN_B=SKIP
if [[ "$CASE" == "all" || "$CASE" == "green" ]]; then
    bprev="$(count_lines "$BRIDGE_CAPTURE")"
    sprev="$(count_lines "$SENTINEL_CAPTURE")"
    originate bridge cp5-green-b >/dev/null || fail "AMI Originate (green-b) failed"
    if line="$(wait_new_line "$BRIDGE_CAPTURE" "$bprev" "own=$B" 15)"; then
        # Give any stray datagram to A a moment to have landed, then assert none.
        sleep 1
        snow="$(count_lines "$SENTINEL_CAPTURE")"
        if (( snow > sprev )); then
            GREEN_B=FAIL
            fail "GREEN-B: a datagram reached STALE A after restart: $(tail -n1 "$SENTINEL_CAPTURE")"
        fi
        say "GREEN-B pass: INVITE FOLLOWED to B receiver-side; stale A silent. [$line]"
        GREEN_B=PASS
    else
        GREEN_B=FAIL
        docker logs "$RUSTISK_CONTAINER" >"$RUSTISK_LOG" 2>&1 || true
        fail "GREEN-B: INVITE did not follow to B=$B"
    fi
fi

# ============================================================================
# RED negative control: defeat routing via the STATIC contact pinned to A.
# The SAME follow-to-B assertion must go RED (INVITE reaches stale A).
# ============================================================================
RED=SKIP
if [[ "$CASE" == "all" || "$CASE" == "red" ]]; then
    say 'RED negative control: Dial(PJSIP/bridge_pinned) — static contact pinned to stale A...'
    bprev="$(count_lines "$BRIDGE_CAPTURE")"
    sprev="$(count_lines "$SENTINEL_CAPTURE")"
    originate bridge_pinned cp5-red >/dev/null || fail "AMI Originate (red) failed"
    # Correlate on the pinned Request-URI so a stray `bridge` datagram at A cannot
    # false-pass RED.
    if red_line="$(wait_new_line "$SENTINEL_CAPTURE" "$sprev" "ruri=sip:pinned@$A" 15)"; then
        # Confirm the follow-to-B assertion would have FAILED: nothing reached B.
        bnow="$(count_lines "$BRIDGE_CAPTURE")"
        if (( bnow > bprev )); then
            fail "RED: unexpected datagram at B during the pinned-static call"
        fi
        say "RED captured: defeated routing misrouted the INVITE to STALE A; follow-to-B assertion RED. [$red_line]"
        RED=PASS_AS_RED
    else
        RED=FAIL
        fail "RED negative control did NOT fire: sentinel saw no INVITE at A (A-detection is broken / false-green risk)"
    fi
fi

# ============================================================================
# Proof + verdict
# ============================================================================
{
    echo "CP5 container-restart REGISTER harness — PROOF"
    echo "generated: $(date -u +%FT%TZ)"
    echo "rustisk HEAD: $(cd "$REPO_DIR" && git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo
    echo "rustisk container IP (fixed): $RUSTISK_IP"
    echo "bridge IP A (pre-restart, runtime-read): $A"
    echo "bridge IP B (post-restart, runtime-read): $B"
    echo "A != B: $([[ "$A" != "$B" ]] && echo yes || echo NO)"
    echo
    echo "GREEN-A (INVITE arrives at registered A):        $GREEN_A"
    echo "GREEN-B (INVITE follows to B, stale A silent):   $GREEN_B"
    echo "RED     (static-pin defeats routing -> A caught): $RED"
    echo
    echo "--- bridge receiver-side INVITE captures ---"
    cat "$BRIDGE_CAPTURE" 2>/dev/null || true
    echo "--- sentinel (stale-A) INVITE captures ---"
    cat "$SENTINEL_CAPTURE" 2>/dev/null || true
    echo "--- rustisk registrar binding events ---"
    docker logs "$RUSTISK_CONTAINER" 2>&1 | grep -Ei 'Contact registered|Contact removed|Too many contacts|Handled REGISTER' || true
} >"$PROOF"

say ''
say '================ VERDICT ================'
cat "$PROOF"

verdict_ok=1
[[ "$GREEN_A" == "PASS" ]] || verdict_ok=0
if [[ "$CASE" == "all" || "$CASE" == "green" ]]; then [[ "$GREEN_B" == "PASS" ]] || verdict_ok=0; fi
if [[ "$CASE" == "all" || "$CASE" == "red" ]]; then [[ "$RED" == "PASS_AS_RED" ]] || verdict_ok=0; fi

if (( verdict_ok == 1 )); then
    say ''
    say "PASS: CP5 dynamic REGISTER follows a real container restart A=$A -> B=$B (receiver-side); RED negative control captured."
    say "Proof: $PROOF"
    exit 0
else
    fail "CP5 harness verdict FAILED (see verdict above)"
fi
