#!/usr/bin/env bash
# shellcheck disable=SC2016
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/freeswitch-pin-gate"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
PROMPT_DIR="$RUNTIME_DIR/prompts"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
FS_IMAGE="safarov/freeswitch@sha256:b31c743f4c911a19687c61e3214968f2a24f93f9d3d667cc26284192e158ffc6"
RUSTISK_IMAGE="python@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3"
FS_CONTAINER="rustisk-fs-pin-gate-$$"
RUSTISK_CONTAINER="rustisk-m1-gate-$$"
IMPAIRMENT_CONTAINER="rustisk-m4-impairment-$$"
FS_NETWORK="rustisk-fs-pin-gate-net-$$"
NETWORK_THIRD_OCTET="$((20 + ($$ % 200)))"
FS_SUBNET="10.253.$NETWORK_THIRD_OCTET.0/24"
FS_CONTAINER_IP="10.253.$NETWORK_THIRD_OCTET.2"
FS_HOST_IP="10.253.$NETWORK_THIRD_OCTET.3"
IMPAIRMENT_IP="10.253.$NETWORK_THIRD_OCTET.4"
AMI_HOST="$FS_HOST_IP"
RUSTISK_PID=""
AMI_SUBSCRIBER_PID=""
SIP_CAPTURE_PID=""
SECRET_DIR=""
BASELINE_RESOURCES=""
BASELINE_TRANSACTIONS=""
RTP_HELPER_LABEL="rustisk.m2.helper=$FS_CONTAINER"
IMPAIRMENT_CONTROL="$RUNTIME_DIR/impairment-control.json"
IMPAIRMENT_STATE="$RUNTIME_DIR/impairment-state.json"
IMPAIRMENT_GENERATION=0
M4_TIMER_B_MIN_MS=28000
M4_TIMER_B_MAX_MS=38000

cleanup() {
    touch "$RUNTIME_DIR/m3-ami.stop" "$RUNTIME_DIR/m3-sip-capture.stop" 2>/dev/null || true
    if [[ -n "$AMI_SUBSCRIBER_PID" ]]; then
        kill "$AMI_SUBSCRIBER_PID" 2>/dev/null || true
        wait "$AMI_SUBSCRIBER_PID" 2>/dev/null || true
    fi
    if [[ -n "$SIP_CAPTURE_PID" ]]; then
        kill "$SIP_CAPTURE_PID" 2>/dev/null || true
        wait "$SIP_CAPTURE_PID" 2>/dev/null || true
    fi
    if docker inspect "$IMPAIRMENT_CONTAINER" >/dev/null 2>&1; then
        local impairment_host_pid
        impairment_host_pid="$(docker inspect -f '{{.State.Pid}}' "$IMPAIRMENT_CONTAINER")"
        kill -TERM "$impairment_host_pid" 2>/dev/null || true
        timeout 3 docker wait "$IMPAIRMENT_CONTAINER" >/dev/null 2>&1 || true
        docker rm -f "$IMPAIRMENT_CONTAINER" >/dev/null 2>&1 || true
    fi
    docker ps -aq --filter "label=$RTP_HELPER_LABEL" \
        | xargs -r docker rm -f >/dev/null 2>&1 || true
    if docker inspect "$RUSTISK_CONTAINER" >/dev/null 2>&1; then
        local rustisk_host_pid
        rustisk_host_pid="$(docker inspect -f '{{.State.Pid}}' "$RUSTISK_CONTAINER")"
        kill -TERM "$rustisk_host_pid" 2>/dev/null || true
        timeout 7 docker wait "$RUSTISK_CONTAINER" >/dev/null 2>&1 || true
        if docker inspect -f '{{.State.Running}}' "$RUSTISK_CONTAINER" 2>/dev/null \
            | grep -q true; then
            kill -KILL "$rustisk_host_pid" 2>/dev/null || true
            timeout 2 docker wait "$RUSTISK_CONTAINER" >/dev/null 2>&1 || true
        fi
        docker rm -f "$RUSTISK_CONTAINER" >/dev/null 2>&1 || true
    fi
    if [[ -n "$RUSTISK_PID" ]]; then
        wait "$RUSTISK_PID" 2>/dev/null || true
    fi
    if docker inspect "$FS_CONTAINER" >/dev/null 2>&1; then
        docker exec "$FS_CONTAINER" freeswitch -stop >/dev/null 2>&1 || true
        timeout 7 docker wait "$FS_CONTAINER" >/dev/null 2>&1 || true
        docker rm -f "$FS_CONTAINER" >/dev/null 2>&1 || true
    fi
    docker network rm "$FS_NETWORK" >/dev/null 2>&1 || true
    if [[ -n "$SECRET_DIR" && "$SECRET_DIR" == /mnt/data/herodevs-agents/m3-pin-secret.* ]]; then
        rm -rf "$SECRET_DIR"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

prove_startup_secret_fail_closed() {
    printf 'Proving startup refuses an absent mounted PIN secret...\n'
    if timeout 10 docker run --rm \
        --network none \
        --user "$(id -u):$(id -g)" \
        --entrypoint /rustisk \
        --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
        --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
        "$RUSTISK_IMAGE" -f -C "$CONFIG_DIR/asterisk.conf" \
        >"$RUNTIME_DIR/startup-absent-secret.log" 2>&1; then
        fail "rustisk started without the required mounted PIN secret"
    fi
    grep -q 'Startup failed: required mounted PIN secret file' \
        "$RUNTIME_DIR/startup-absent-secret.log" \
        || fail "absent-secret startup failure did not identify the missing mounted file"
    grep -q 'Rustisk is fully booted' "$RUNTIME_DIR/startup-absent-secret.log" \
        && fail "absent-secret rustisk reached fully booted"
    printf 'STARTUP_ABSENT_SECRET: PASS (exit nonzero before fully booted)\n'

    printf 'Proving startup refuses an invalid mounted PIN secret...\n'
    if timeout 10 docker run --rm \
        --network none \
        --user "$(id -u):$(id -g)" \
        --entrypoint /rustisk \
        --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
        --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
        --mount "type=bind,src=$SECRET_DIR/invalid,dst=/run/secrets/rustisk/pin,readonly" \
        "$RUSTISK_IMAGE" -f -C "$CONFIG_DIR/asterisk.conf" \
        >"$RUNTIME_DIR/startup-invalid-secret.log" 2>&1; then
        fail "rustisk started with an invalid mounted PIN secret"
    fi
    grep -q 'Startup failed: mounted PIN secret file is not a valid six-digit secret' \
        "$RUNTIME_DIR/startup-invalid-secret.log" \
        || fail "invalid-secret startup failure did not identify invalid contents"
    grep -q 'Rustisk is fully booted' "$RUNTIME_DIR/startup-invalid-secret.log" \
        && fail "invalid-secret rustisk reached fully booted"
    printf 'STARTUP_INVALID_SECRET: PASS (exit nonzero before fully booted)\n'
}

fs_cli() {
    docker exec "$FS_CONTAINER" fs_cli \
        -H 127.0.0.1 -P 8021 -p ClueCon -t 10000 -x "$1"
}

rtp_helper() {
    docker run --rm \
        --label "$RTP_HELPER_LABEL" \
        --network "container:$FS_CONTAINER" \
        --cap-add NET_RAW \
        --mount "type=bind,src=$HARNESS_DIR/rtp_injector.py,dst=/rtp_injector.py,readonly" \
        --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
        "$RUSTISK_IMAGE" python3 /rtp_injector.py "$@"
}

rtp_sniffer() {
    docker run --rm \
        --label "$RTP_HELPER_LABEL" \
        --network "container:$RUSTISK_CONTAINER" \
        --cap-add NET_RAW \
        --mount "type=bind,src=$HARNESS_DIR/rtp_injector.py,dst=/rtp_injector.py,readonly" \
        --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
        "$RUSTISK_IMAGE" python3 /rtp_injector.py "$@"
}

wait_for_freeswitch() {
    for _ in {1..100}; do
        if fs_cli status >/dev/null 2>&1; then
            return
        fi
        sleep 0.1
    done
    docker logs "$FS_CONTAINER" >&2 || true
    fail "FreeSWITCH event socket did not become ready"
}

wait_for_ami() {
    for _ in {1..100}; do
        if printf 'Action: Login\r\nUsername: harness\r\nSecret: pin-gate-local-only\r\n\r\nAction: Logoff\r\n\r\n' \
            | ami_socket 2>/dev/null | grep -q 'Authentication accepted'; then
            return
        fi
        if ! docker inspect -f '{{.State.Running}}' "$RUSTISK_CONTAINER" 2>/dev/null \
            | grep -q true; then
            if kill -0 "$RUSTISK_PID" 2>/dev/null; then
                sleep 0.1
                continue
            fi
            sed -n '1,240p' "$RUSTISK_LOG" >&2
            fail "rustisk exited before AMI became ready"
        fi
        sleep 0.1
    done
    sed -n '1,240p' "$RUSTISK_LOG" >&2
    fail "rustisk AMI did not become ready"
}

ami_socket() {
    docker exec -i "$RUSTISK_CONTAINER" python3 /ami_client.py "$AMI_HOST" 15038
}

ami_rtp_stats() {
    local channel="$1"
    printf 'Action: Login\r\nUsername: harness\r\nSecret: pin-gate-local-only\r\n\r\nAction: RTPStats\r\nChannel: %s\r\n\r\nAction: Logoff\r\n\r\n' "$channel" \
        | ami_socket
}

ami_action() {
    local action="$1"
    printf 'Action: Login\r\nUsername: harness\r\nSecret: pin-gate-local-only\r\n\r\n%bAction: Logoff\r\n\r\n' "$action" \
        | ami_socket
}

resource_snapshot() {
    local response
    local fields=(CoreCurrentCalls SIPDriverChannels SIPCallIdMappings SIPCallStates SIPNotifyChannels)
    local values=()
    response="$(core_status_response)"
    for field in "${fields[@]}"; do
        local value
        value="$(stat_value "$field" "$response")"
        [[ "$value" =~ ^[0-9]+$ ]] || fail "$field missing from CoreStatus: $response"
        values+=("$value")
    done
    (IFS=/; printf '%s\n' "${values[*]}")
}

transaction_snapshot() {
    local response
    local fields=(SIPInviteClientTransactions SIPInviteServerTransactions SIPNonInviteClientTransactions SIPNonInviteServerTransactions)
    local values=()
    response="$(core_status_response)"
    for field in "${fields[@]}"; do
        local value
        value="$(stat_value "$field" "$response")"
        [[ "$value" =~ ^[0-9]+$ ]] || fail "$field missing from CoreStatus: $response"
        values+=("$value")
    done
    (IFS=/; printf '%s\n' "${values[*]}")
}

core_status_response() {
    local response=""
    for _ in {1..10}; do
        if response="$(ami_action $'Action: CoreStatus\r\n\r\n' 2>/dev/null)" \
            && grep -q 'SIPNonInviteServerTransactions:' <<<"$response"; then
            printf '%s\n' "$response"
            return
        fi
        sleep 0.2
    done
    fail "CoreStatus did not respond after retries"
}

# The full M5 soak baseline: EVERY registry the plan enumerates, in one string.
# Core (5) + all four transaction maps (4) + RTP port allocations, registrar
# bindings, and the #122 hangup/answer callback counts (4). Per M-g,
# active_channel_count==0 proves nothing — this asserts each registry exactly.
#
# NOTE (M5 review MINOR-2): the soak call loop uses `originate`/`park` and never
# REGISTERs or expires a binding, so `SIPRegistrarBindings` here is a
# *regression sentinel* (it must return to its exact pre-soak value), NOT a
# proof of registrar lifecycle. Registrar bind/expire/removal correctness is
# M6's dynamic-REGISTER acceptance, not this soak.
full_snapshot() {
    local response value
    local fields=(
        CoreCurrentCalls SIPDriverChannels SIPCallIdMappings SIPCallStates SIPNotifyChannels
        SIPInviteClientTransactions SIPInviteServerTransactions
        SIPNonInviteClientTransactions SIPNonInviteServerTransactions
        SIPRtpSessions SIPRegistrarBindings SIPHangupCallbacks SIPAnswerCallbacks
    )
    local values=()
    response="$(core_status_response)"
    for field in "${fields[@]}"; do
        value="$(stat_value "$field" "$response")"
        [[ "$value" =~ ^[0-9]+$ ]] || fail "$field missing from CoreStatus: $response"
        values+=("$value")
    done
    (IFS=/; printf '%s\n' "${values[*]}")
}

# Poll the full registry snapshot until it returns to the exact baseline, or
# fail after `timeout_ms`. The generous default lets the last calls' RFC 3261
# absorption timers (Timer J/K/I/D) drain before the exact-baseline check.
wait_for_full_baseline() {
    local baseline="$1"
    local timeout_ms="${2:-60000}"
    local deadline snapshot
    deadline=$(( $(now_ms) + timeout_ms ))
    while (( $(now_ms) < deadline )); do
        snapshot="$(full_snapshot)"
        if [[ "$snapshot" == "$baseline" ]]; then
            printf '%s\n' "$snapshot"
            return 0
        fi
        sleep 0.5
    done
    fail "M5 soak did not return to exact baseline within $((timeout_ms/1000))s: baseline=$baseline actual=$(full_snapshot)"
}

set_impairment() {
    local mode="$1"
    local inject_forged="${2:-false}"
    IMPAIRMENT_GENERATION="$((IMPAIRMENT_GENERATION + 1))"
    python3 - "$IMPAIRMENT_CONTROL" "$IMPAIRMENT_GENERATION" "$mode" "$inject_forged" <<'PY'
import json
import os
import sys

path, generation, mode, inject_forged = sys.argv[1:]
tmp = path + ".new"
with open(tmp, "w", encoding="utf-8") as output:
    json.dump({
        "generation": int(generation),
        "mode": mode,
        "inject_forged": inject_forged == "true",
    }, output)
os.replace(tmp, path)
PY
    for _ in {1..100}; do
        if [[ -f "$IMPAIRMENT_STATE" ]] \
            && [[ "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["generation"])' "$IMPAIRMENT_STATE" 2>/dev/null || true)" == "$IMPAIRMENT_GENERATION" ]]; then
            return
        fi
        sleep 0.2
    done
    fail "impairment proxy did not acknowledge generation $IMPAIRMENT_GENERATION ($mode)"
}

impairment_counter() {
    local name="$1"
    python3 - "$IMPAIRMENT_STATE" "$name" <<'PY'
import json
import sys

state = json.load(open(sys.argv[1], encoding="utf-8"))
print(state.get("counters", {}).get(sys.argv[2], 0))
PY
}

wait_for_impairment_counter() {
    local name="$1"
    local minimum="$2"
    local value=0
    for _ in {1..800}; do
        value="$(impairment_counter "$name")"
        if (( value >= minimum )); then
            printf '%s\n' "$value"
            return
        fi
        sleep 0.05
    done
    fail "impairment counter $name must reach $minimum, got $value"
}

assert_impairment_hash_count() {
    local direction="$1"
    local status="$2"
    local cseq_method="$3"
    local expected="$4"
    python3 - "$IMPAIRMENT_STATE" "$direction" "$status" "$cseq_method" "$expected" <<'PY'
import json
import sys

state = json.load(open(sys.argv[1], encoding="utf-8"))
direction, status, cseq_method, expected = sys.argv[2], int(sys.argv[3]), sys.argv[4], int(sys.argv[5])
events = [event for event in state["events"]
          if event["direction"] == direction
          and event["status"] == status
          and event["cseq_method"] == cseq_method]
if len(events) != expected:
    raise SystemExit(f"expected {expected} matching events, got {len(events)}: {events}")
hashes = {event["sha256"] for event in events}
if len(hashes) != 1:
    raise SystemExit(f"matching messages were not byte-identical: {hashes}")
print(f"Identical{status}{cseq_method}={expected} SHA256={next(iter(hashes))}")
PY
}

assert_impairment_hashes_identical_minimum() {
    local direction="$1"
    local status="$2"
    local cseq_method="$3"
    local minimum="$4"
    python3 - "$IMPAIRMENT_STATE" "$direction" "$status" "$cseq_method" "$minimum" <<'PY'
import json
import sys

state = json.load(open(sys.argv[1], encoding="utf-8"))
direction, status, cseq_method, minimum = sys.argv[2], int(sys.argv[3]), sys.argv[4], int(sys.argv[5])
events = [event for event in state["events"]
          if event["direction"] == direction
          and event["status"] == status
          and event["cseq_method"] == cseq_method]
if len(events) < minimum:
    raise SystemExit(f"expected at least {minimum} {status}/{cseq_method} messages, got {len(events)}")
hashes = {event["sha256"] for event in events}
if len(hashes) != 1:
    raise SystemExit(f"{status}/{cseq_method} messages were not byte-identical: {hashes}")
print(f"Identical{status}{cseq_method}={len(events)} SHA256={next(iter(hashes))}")
PY
}

wait_for_transaction_baseline() {
    local label="$1"
    local started_ms
    local snapshot=""
    local elapsed_ms
    started_ms="$(now_ms)"
    while true; do
        snapshot="$(transaction_snapshot)"
        elapsed_ms="$(($(now_ms) - started_ms))"
        if [[ "$snapshot" == "$BASELINE_TRANSACTIONS" ]]; then
            printf '%s: TransactionBaseline=%s RestoredInMs=%d\n' \
                "$label" "$snapshot" "$elapsed_ms"
            return
        fi
        (( elapsed_ms < 40000 )) \
            || fail "$label transactions did not return to exact baseline: baseline=$BASELINE_TRANSACTIONS actual=$snapshot"
        sleep 0.2
    done
}

wait_for_resource_baseline() {
    local label="$1"
    local hangup_observed_ms="$2"
    local snapshot=""
    local elapsed_ms
    while true; do
        snapshot="$(resource_snapshot)"
        elapsed_ms="$(($(now_ms) - hangup_observed_ms))"
        if [[ "$snapshot" == "$BASELINE_RESOURCES" ]]; then
            (( elapsed_ms <= 2000 )) \
                || fail "$label resources reached baseline after the 2s deadline: ${elapsed_ms}ms"
            printf '%s: ResourceBaseline=%s RestoredFromReceiverHangupInMs=%d\n' \
                "$label" "$snapshot" "$elapsed_ms"
            wait_for_transaction_baseline "$label"
            return
        fi
        (( elapsed_ms < 2000 )) \
            || fail "$label resources did not return to exact baseline within 2s: baseline=$BASELINE_RESOURCES actual=$snapshot"
        sleep 0.05
    done
}

wait_for_resource_baseline_eventually() {
    local label="$1"
    local started_ms
    local snapshot=""
    local elapsed_ms
    started_ms="$(now_ms)"
    while true; do
        snapshot="$(resource_snapshot)"
        elapsed_ms="$(($(now_ms) - started_ms))"
        if [[ "$snapshot" == "$BASELINE_RESOURCES" ]]; then
            printf '%s: ResourceBaseline=%s RestoredEventuallyInMs=%d\n' \
                "$label" "$snapshot" "$elapsed_ms"
            wait_for_transaction_baseline "$label"
            return
        fi
        (( elapsed_ms < 40000 )) \
            || fail "$label resources did not eventually return to baseline: baseline=$BASELINE_RESOURCES actual=$snapshot"
        sleep 0.1
    done
}

now_ms() {
    python3 -c 'import time; print(time.monotonic_ns() // 1_000_000)'
}

wait_for_completed_stats() {
    local channel="$1"
    local response=""
    for _ in {1..750}; do
        response="$(ami_rtp_stats "$channel")"
        if grep -q 'Message: RTP statistics' <<<"$response" \
            && grep -q 'RTPActive: false' <<<"$response"; then
            printf '%s\n' "$response"
            return
        fi
        sleep 0.05
    done
    fail "completed RTPStats record was not available for $channel: $response"
}

wait_for_active_stats() {
    local channel="$1"
    local response=""
    for _ in {1..100}; do
        response="$(ami_rtp_stats "$channel")"
        if grep -q 'Message: RTP statistics' <<<"$response" \
            && grep -q 'RTPActive: true' <<<"$response"; then
            printf '%s\n' "$response"
            return
        fi
        sleep 0.05
    done
    fail "active RTPStats record was not available for $channel: $response"
}

stat_value() {
    local field="$1"
    local response="$2"
    awk -F ': ' -v field="$field" '$1 == field { sub(/\r$/, "", $2); value = $2 } END { print value }' <<<"$response"
}

assert_positive_counter() {
    local field="$1"
    local response="$2"
    local value
    value="$(stat_value "$field" "$response")"
    [[ "$value" =~ ^[0-9]+$ ]] || fail "$field missing from AMI RTPStats response"
    (( value > 0 )) || fail "$field must be greater than zero, got $value"
}

assert_counter_equals() {
    local field="$1"
    local expected="$2"
    local response="$3"
    local value
    value="$(stat_value "$field" "$response")"
    [[ "$value" == "$expected" ]] || fail "$field must equal $expected, got ${value:-missing}"
}

assert_counter_delta() {
    local field="$1"
    local expected_delta="$2"
    local before="$3"
    local after="$4"
    local before_value
    local after_value
    before_value="$(stat_value "$field" "$before")"
    after_value="$(stat_value "$field" "$after")"
    [[ "$before_value" =~ ^[0-9]+$ && "$after_value" =~ ^[0-9]+$ ]] \
        || fail "$field missing from AMI RTPStats response"
    (( after_value - before_value == expected_delta )) \
        || fail "$field delta must equal $expected_delta, got $before_value->$after_value"
}

active_sip_channels() {
    ami_action $'Action: CoreShowChannels\r\n\r\n' \
        | awk -F ': ' '$1 == "Channel" { sub(/\r$/, "", $2); if ($2 ~ /^PJSIP\//) print $2 }'
}

wait_for_rustisk_bridge_channels() {
    local outbound_destination="$1"
    local channels=""
    local inbound=""
    local outbound=""
    for _ in {1..100}; do
        channels="$(active_sip_channels)"
        inbound="$(grep '^PJSIP/fs-carrier-' <<<"$channels" | tail -n1 || true)"
        outbound="$(grep "^PJSIP/$outbound_destination-" <<<"$channels" | tail -n1 || true)"
        if [[ -n "$inbound" && -n "$outbound" ]]; then
            printf '%s|%s\n' "$inbound" "$outbound"
            return
        fi
        sleep 0.05
    done
    fail "rustisk bridge channels did not appear for $outbound_destination: $channels"
}

wait_for_fs_destination_uuid() {
    local destination="$1"
    local response=""
    local uuid=""
    for _ in {1..100}; do
        response="$(fs_cli 'show channels as csv')"
        uuid="$(grep "$destination" <<<"$response" \
            | grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' | head -n1 || true)"
        if [[ -n "$uuid" ]]; then
            printf '%s\n' "$uuid"
            return
        fi
        sleep 0.05
    done
    fail "FreeSWITCH destination $destination did not appear: $response"
}

fs_channel_var() {
    local uuid="$1"
    local variable="$2"
    fs_cli "uuid_getvar $uuid $variable" | tr -d '\r\n'
}

wait_for_fs_channel_var() {
    local uuid="$1"
    local variable="$2"
    local expected="$3"
    local value=""
    for _ in {1..100}; do
        value="$(fs_channel_var "$uuid" "$variable")"
        if [[ "$value" == "$expected" ]]; then
            printf '%s\n' "$value"
            return
        fi
        sleep 0.05
    done
    fail "FreeSWITCH $variable on $uuid must equal $expected, got $value"
}

assert_accepted_media_unchanged() {
    local before="$1"
    local after="$2"
    local field
    for field in RTPPacketsRx RTPOctetsRx RTPVoiceFramesRx RTPDTMFDigitsRx; do
        assert_counter_delta "$field" 0 "$before" "$after"
    done
}

assert_media_quiet() {
    local label="$1"
    local inbound_channel="$2"
    local outbound_channel="$3"
    local inbound_before
    local inbound_after
    local outbound_before
    local outbound_after
    inbound_before="$(wait_for_active_stats "$inbound_channel")"
    outbound_before="$(wait_for_active_stats "$outbound_channel")"
    sleep 0.3
    inbound_after="$(wait_for_active_stats "$inbound_channel")"
    outbound_after="$(wait_for_active_stats "$outbound_channel")"
    assert_accepted_media_unchanged "$inbound_before" "$inbound_after"
    assert_accepted_media_unchanged "$outbound_before" "$outbound_after"
    printf '%s: BothRtpSourcesSilent=true AcceptedMediaStable=true\n' "$label"
}

assert_hostile_injection() {
    local kind="$1"
    local expected_field="$2"
    local channel="$3"
    local source_port="$4"
    local destination_port="$5"
    local payload_type="$6"
    local sequence="$7"
    local timestamp="$8"
    local ssrc="$9"
    local before
    local after
    local before_remote
    local field
    local output
    before="$(wait_for_active_stats "$channel")"
    before_remote="$(stat_value RTPRemoteAddress "$before")"
    output="$(rtp_helper inject \
        --kind "$kind" \
        --source-ip "$FS_CONTAINER_IP" \
        --source-port "$source_port" \
        --destination-ip "$FS_HOST_IP" \
        --destination-port "$destination_port" \
        --payload-type "$payload_type" \
        --sequence "$sequence" \
        --timestamp "$timestamp" \
        --ssrc "$ssrc")"
    sleep 0.1
    after="$(wait_for_active_stats "$channel")"
    assert_accepted_media_unchanged "$before" "$after"
    for field in RTPDiscardWrongSource RTPDiscardWrongPayloadType RTPDiscardMalformed RTPDiscardUnstableSSRC; do
        if [[ "$field" == "$expected_field" ]]; then
            assert_counter_delta "$field" 1 "$before" "$after"
        else
            assert_counter_delta "$field" 0 "$before" "$after"
        fi
    done
    [[ "$(stat_value RTPRemoteAddress "$after")" == "$before_remote" ]] \
        || fail "$kind injection repointed RTP remote: $before_remote -> $(stat_value RTPRemoteAddress "$after")"
    printf 'INGRESS_%s: PASS (%s; %s +1; accepted media unchanged; Remote=%s)\n' \
        "${kind^^}" "$output" "$expected_field" "$before_remote"
}

wait_for_call_end() {
    local uuid="$1"
    for _ in {1..150}; do
        if [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" != "true" ]]; then
            return
        fi
        sleep 0.1
    done
    fail "FreeSWITCH call $uuid did not end"
}

fs_b_invite_count() {
    docker exec "$FS_CONTAINER" awk \
        '/INVITE sip:9201@/ { count++ } END { print count + 0 }' \
        /var/log/freeswitch/freeswitch.log
}

wait_for_transaction_baseline_in_timer_b_window() {
    local started_ms="$1"
    local elapsed_ms
    local snapshot
    # CoreStatus uses one AMI connection per sample. The transaction is already
    # proven live above, so wait without polling until just before the window.
    sleep 27
    while true; do
        elapsed_ms="$(($(now_ms) - started_ms))"
        snapshot="$(transaction_snapshot)"
        if [[ "$snapshot" == "$BASELINE_TRANSACTIONS" ]]; then
            (( elapsed_ms >= M4_TIMER_B_MIN_MS )) \
                || fail "INVITE client transaction drained before Timer B: ${elapsed_ms}ms"
            printf '%s\n' "$elapsed_ms"
            return
        fi
        (( elapsed_ms < M4_TIMER_B_MAX_MS )) \
            || fail "INVITE client transaction did not drain in Timer B window (${M4_TIMER_B_MIN_MS}-${M4_TIMER_B_MAX_MS}ms): baseline=$BASELINE_TRANSACTIONS actual=$snapshot; 70s direct-Originate hold is still pending"
        sleep 0.25
    done
}

timer_b_teardown_count() {
    grep -c 'Timer B tore down unanswered outbound INVITE' "$RUSTISK_LOG" || true
}

run_case() {
    local ordinal="$1"
    local digits="$2"
    local expected="$3"
    local channel
    local originate_response
    local uuid
    local stats
    local capture="/var/lib/freeswitch/recordings/pin-gate-${expected,,}.wav"
    local b_invites_before
    local b_invites_after
    local b_uuid=""
    local m3_a_capture_remote="/var/lib/freeswitch/recordings/m3-gate-a-capture.wav"
    local m3_a_capture="$RUNTIME_DIR/m3-gate-a-capture.wav"
    local m3_b_capture="$RUNTIME_DIR/m3-gate-b-capture.wav"

    printf '\nRunning %s case with receiver-side RFC4733 digits...\n' "$expected"
    b_invites_before="$(fs_b_invite_count)"
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9000@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH originate failed: $originate_response"
    fi
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "FreeSWITCH originate failed: $originate_response"

    fs_cli "uuid_record $uuid start $capture" >/dev/null
    fs_cli "uuid_broadcast $uuid tone_stream://%(1000,0,600) aleg" >/dev/null
    sleep 2.2
    fs_cli "uuid_send_dtmf $uuid ${digits}#@200" >/dev/null

    if [[ "$expected" == "REJECTED" ]]; then
        # Fail immediately if the compare is bypassed. Waiting for the call to
        # end first would let the unexpected B leg mask this assertion.
        sleep 5
        b_invites_after="$(fs_b_invite_count)"
        (( b_invites_after == b_invites_before )) \
            || fail "rejected PIN reached FreeSWITCH B: INVITEs $b_invites_before->$b_invites_after"
    else
        b_uuid="$(wait_for_fs_destination_uuid 9201)"
        fs_cli "uuid_record $uuid stop $capture" >/dev/null
        fs_cli "uuid_record $uuid start $m3_a_capture_remote" >/dev/null
        fs_cli "uuid_broadcast $uuid tone_stream://%(1000,0,440) aleg" >/dev/null
        sleep 1.2
        fs_cli "uuid_broadcast $b_uuid tone_stream://%(1000,0,660) aleg" >/dev/null
        sleep 1.2
        fs_cli "uuid_send_dtmf $uuid 7@200" >/dev/null
        wait_for_fs_channel_var "$b_uuid" M2DecodedDigit 7 >/dev/null
        fs_cli "uuid_kill $uuid NORMAL_CLEARING" >/dev/null
    fi
    wait_for_call_end "$uuid"
    if [[ -n "$b_uuid" ]]; then
        wait_for_call_end "$b_uuid"
    fi

    b_invites_after="$(fs_b_invite_count)"
    if [[ "$expected" == "REJECTED" ]]; then
        (( b_invites_after == b_invites_before )) \
            || fail "rejected PIN reached FreeSWITCH B: INVITEs $b_invites_before->$b_invites_after"
        printf 'REJECTED_B_SIDE: PASS (FreeSWITCH-B INVITE delta=0)\n'
    else
        (( b_invites_after > b_invites_before )) \
            || fail "granted PIN never reached FreeSWITCH B"
    fi

    channel="PJSIP/fs-carrier-$(printf '%08d' "$ordinal")"
    stats="$(wait_for_completed_stats "$channel")"
    grep -q 'Message: RTP statistics' <<<"$stats" || fail "RTPStats failed for $channel: $stats"
    grep -q "RTPActive: false" <<<"$stats" || fail "completed stats were not retained for $channel"
    assert_positive_counter RTPPacketsTx "$stats"
    assert_positive_counter RTPPacketsRx "$stats"
    assert_positive_counter RTPVoiceFramesTx "$stats"
    assert_positive_counter RTPVoiceFramesRx "$stats"
    if [[ "$expected" == "REJECTED" ]]; then
        assert_counter_equals RTPDTMFDigitsRx "${#digits}" "$stats"
    else
        (( $(stat_value RTPDTMFDigitsRx "$stats") >= ${#digits} + 1 )) \
            || fail "granted bridge did not receive gate digits plus forwarded M2 DTMF"
    fi
    grep -q "Verbose: PIN_GATE_RESULT=$expected" "$RUSTISK_LOG" \
        || fail "dialplan did not take the $expected branch"

    docker cp "$FS_CONTAINER:$capture" "$RUNTIME_DIR/${expected,,}-capture.wav" >/dev/null
    (( $(wc -c <"$RUNTIME_DIR/${expected,,}-capture.wav") > 44 )) \
        || fail "FreeSWITCH audio capture has no samples"

    printf '%s\n' "$stats" >"$RUNTIME_DIR/${expected,,}-rtp-stats.txt"
    if [[ "$expected" == "GRANTED" ]]; then
        docker cp "$FS_CONTAINER:$m3_a_capture_remote" "$m3_a_capture" >/dev/null
        docker cp "$FS_CONTAINER:/var/lib/freeswitch/recordings/m2-b-capture.wav" \
            "$m3_b_capture" >/dev/null
        python3 "$HARNESS_DIR/assert_tone.py" "$m3_b_capture" 440 \
            | sed 's/^/M3_GRANTED_A_TO_B: /' \
            | tee "$RUNTIME_DIR/m3-granted-a-to-b-tone-proof.txt"
        python3 "$HARNESS_DIR/assert_tone.py" "$m3_a_capture" 660 \
            | sed 's/^/M3_GRANTED_B_TO_A: /' \
            | tee "$RUNTIME_DIR/m3-granted-b-to-a-tone-proof.txt"
        printf 'GRANTED_BRIDGE: PASS (M2 two-way far-side receiver proof)\n'
    fi
    wait_for_resource_baseline_eventually "$expected"
    printf '%s: PASS (TX voice=%s, RX voice=%s, RX DTMF=%s)\n' \
        "$expected" \
        "$(stat_value RTPVoiceFramesTx "$stats")" \
        "$(stat_value RTPVoiceFramesRx "$stats")" \
        "$(stat_value RTPDTMFDigitsRx "$stats")"
}

run_m3_no_input_deadline_case() {
    local originate_response
    local uuid
    local answered_ms
    local ended_ms
    local elapsed_ms

    printf '\nRunning M3 no-input absolute deadline case...\n'
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9002@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH no-input originate failed: $originate_response"
    fi
    answered_ms="$(now_ms)"
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "FreeSWITCH no-input originate failed: $originate_response"
    wait_for_call_end "$uuid"
    ended_ms="$(now_ms)"
    elapsed_ms="$((ended_ms - answered_ms))"
    (( elapsed_ms >= 4000 && elapsed_ms <= 6000 )) \
        || fail "no-input hangup missed the 5s absolute deadline ±1s: ${elapsed_ms}ms"
    wait_for_resource_baseline_eventually "M3_NO_INPUT"
    printf 'NO_INPUT_DEADLINE: PASS (AnsweredToHangupMs=%s DeadlineMs=5000 ToleranceMs=1000)\n' \
        "$elapsed_ms"
}

start_m3_sink_receivers() {
    rm -f "$RUNTIME_DIR/m3-ami.stop" "$RUNTIME_DIR/m3-sip-capture.stop"
    docker exec "$RUSTISK_CONTAINER" python3 /ami_subscriber.py "$AMI_HOST" 15038 \
        --stop-file "$RUNTIME_DIR/m3-ami.stop" \
        >"$RUNTIME_DIR/m3-ami-transcript.txt" 2>&1 &
    AMI_SUBSCRIBER_PID=$!
    docker run --rm \
        --label "$RTP_HELPER_LABEL" \
        --network "container:$RUSTISK_CONTAINER" \
        --cap-add NET_RAW \
        --mount "type=bind,src=$HARNESS_DIR/sip_capture.py,dst=/sip_capture.py,readonly" \
        --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
        "$RUSTISK_IMAGE" python3 /sip_capture.py \
            --output /runtime/m3-sip-only.pcap \
            --stop-file /runtime/m3-sip-capture.stop \
        >"$RUNTIME_DIR/m3-sip-capture-proof.txt" 2>&1 &
    SIP_CAPTURE_PID=$!

    for _ in {1..100}; do
        grep -q 'Authentication accepted' "$RUNTIME_DIR/m3-ami-transcript.txt" 2>/dev/null \
            && return
        sleep 0.05
    done
    fail "authenticated AMI subscriber did not become live"
}

stop_m3_sink_receivers() {
    touch "$RUNTIME_DIR/m3-ami.stop" "$RUNTIME_DIR/m3-sip-capture.stop"
    wait "$AMI_SUBSCRIBER_PID" || fail "AMI subscriber failed"
    wait "$SIP_CAPTURE_PID" || fail "SIP-only capture failed"
    AMI_SUBSCRIBER_PID=""
    SIP_CAPTURE_PID=""

    # POSITIVE CONTROL (load-bearing under AMI default-DENY, issues #126/#127).
    # The zero-hit PIN audit only proves something if the subscriber is actually
    # RECEIVING events. With the AMI read default now DENY, an account without an
    # explicit `read=` would receive NOTHING and the audit would FALSELY PASS
    # (no PIN seen because no events seen). The [harness] account is granted
    # `read = all`; these four assertions require it to have observed benign,
    # non-secret control events (a PinGate Newexten and a PINGATESTATUS VarSet),
    # proving the subscriber is live and its read grant is effective. If the
    # grant were dropped, the subscriber would go silent and these greps fail.
    grep -q 'Event: Newexten' "$RUNTIME_DIR/m3-ami-transcript.txt" \
        || fail "AMI subscriber saw no Newexten events (read grant silenced? subscriber-sees-nothing false pass)"
    grep -q 'Application: PinGate' "$RUNTIME_DIR/m3-ami-transcript.txt" \
        || fail "AMI subscriber did not observe PinGate Newexten"
    grep -q 'Event: VarSet' "$RUNTIME_DIR/m3-ami-transcript.txt" \
        || fail "AMI subscriber saw no VarSet events"
    grep -q 'Variable: PINGATESTATUS' "$RUNTIME_DIR/m3-ami-transcript.txt" \
        || fail "AMI subscriber did not observe the non-secret gate status"
    grep -Eq 'SIPOnlyPackets=[1-9][0-9]*' "$RUNTIME_DIR/m3-sip-capture-proof.txt" \
        || fail "SIP-only pcap captured no packets"
    printf 'AMI_SUBSCRIBER: PASS (authenticated, explicit read=all, positive control: Newexten/PinGate + VarSet/PINGATESTATUS observed)\n'
    printf 'SIP_ONLY_CAPTURE: PASS (%s)\n' \
        "$(tr -d '\r\n' <"$RUNTIME_DIR/m3-sip-capture-proof.txt")"
}

assert_m3_zero_hit_audit() {
    local hits=""
    local file
    local pattern

    rm -rf "$RUNTIME_DIR/freeswitch-artifacts"
    docker cp "$FS_CONTAINER:/var/log/freeswitch" \
        "$RUNTIME_DIR/freeswitch-artifacts" >/dev/null
    {
        printf 'Rustisk CDR/CEL wiring: no files emitted by current runtime.\n'
        printf 'FreeSWITCH CDR/CEL files copied under freeswitch-artifacts:\n'
        find "$RUNTIME_DIR/freeswitch-artifacts" -type f \
            \( -iname '*cdr*' -o -iname '*cel*' -o -name '*.csv' \) -print
    } >"$RUNTIME_DIR/cdr-cel-manifest.txt"

    for pattern in "$GRANTED_TEST_PIN" "$WRONG_TEST_PIN"; do
        while IFS= read -r -d '' file; do
            if grep -aFq -- "$pattern" "$file"; then
                hits+="$file"$'\n'
            fi
        done < <(find "$RUNTIME_DIR" -type f -print0)
    done
    [[ -z "$hits" ]] || fail "secret audit found a test PIN in artifacts: $hits"

    # PER-DIGIT LEAK AUDIT (REVIEW-M3 MINOR-1). The concatenated grep above is
    # blind to a sink that emits the PIN one digit at a time (e.g. a
    # `debug!(digit = %digit, ...)` tracing field per DTMF event): no artifact
    # ever contains the 6-digit string, yet the full PIN is trivially
    # reassembled by reading the digit fields in order. For every runtime
    # artifact, extract each individually-logged DTMF digit value (shapes:
    # `digit=5`, `digit='5'`, `digit: 5`, `Digit: 5`, plus the
    # message-embedded quoted shape `DTMF '5'` / `by DTMF '5'` used by
    # playback barge-in — REVIEW-BUNDLEB: the original extractor returned
    # empty on that shape, so such a leak stayed silently GREEN) preserving
    # file order,
    # and fail if either test PIN appears in that file's digit stream — raw,
    # or with consecutive duplicates collapsed (a begin+end pair logs every
    # digit twice: 112233… would defeat the raw check alone).
    #
    # Scope: every artifact EXCEPT freeswitch-artifacts/. The FreeSWITCH
    # carrier simulator is the DTMF *sender* — `uuid_send_dtmf` hands it the
    # digits, and its own debug log records each digit it transmits
    # (`digit=7 ms=200 samples=1600`), exactly like the mounted secret input
    # (which also lives outside the audited tree, see SecretInputArtifact).
    # That send-side record is harness input, not a rustisk sink; proven
    # empirically — the first run of this audit flagged it. The concatenated
    # scan above still covers freeswitch-artifacts/ unreduced. If the live
    # FreeSWITCH pod runs at a loglevel this chatty, its pod logs leak PIN
    # digits FS-side — flagged separately; not fixable from this repo.
    # Two alternatives: the field shape (`digit=5` etc., quote optional) and
    # the message-embedded shape (`DTMF '5'`, quote REQUIRED — unquoted
    # `DTMF<x>` would false-match benign counters like `RTPDTMFDigitsRx=1`,
    # polluting the stream and masking real leaks). Both alternatives end on
    # the digit char so the `.$` extraction below works for either.
    local perdigit_extractor="([Dd]igit *[=:] *['\"]?|[Dd][Tt][Mm][Ff] *['\"])[0-9A-D#*]"
    # ANSI SGR sequences must be stripped BEFORE matching: the tracing fmt
    # writer colors its output even into the redirected log file, so a leaked
    # field arrives as `digit\e[0m\e[2m=\e[0m7` — the literal bytes "digit="
    # never appear and an unstripped grep stays silently GREEN on a real
    # leak (caught by this audit's own red-proof run).
    local strip_ansi='s/\x1b\[[0-9;]*m//g'
    # Extractor positive control: prove the pattern still captures the log
    # shapes — including the ANSI-colored tracing shape — before trusting a
    # zero-hit result (guards against silent rot the same way the AMI
    # positive control above guards the subscriber).
    local perdigit_selftest
    perdigit_selftest="$(printf "digit=4 a\nx digit: 2 b\nDigit: 4\ndigit='2'\n\x1b[3mdigit\x1b[0m\x1b[2m=\x1b[0m7\ninterrupted by DTMF '3' during file 'x'\nby DTMF \"8\"\n\x1b[2mDTMF\x1b[0m '9'\nRTPDTMFDigitsRx=1\n" \
        | sed -e "$strip_ansi" \
        | grep -aoE "$perdigit_extractor" | grep -o '.$' | tr -d '\n')"
    [[ "$perdigit_selftest" == "42427389" ]] \
        || fail "per-digit extractor self-test failed (positive control): got '$perdigit_selftest'"
    local perdigit_stream
    local perdigit_squeezed
    local perdigit_hits=""
    while IFS= read -r -d '' file; do
        perdigit_stream="$(sed -e "$strip_ansi" "$file" \
            | grep -aoE -- "$perdigit_extractor" \
            | grep -o '.$' | tr -d '\n' || true)"
        [[ -n "$perdigit_stream" ]] || continue
        perdigit_squeezed="$(printf '%s' "$perdigit_stream" | tr -s '0-9A-D#*')"
        for pattern in "$GRANTED_TEST_PIN" "$WRONG_TEST_PIN"; do
            if [[ "$perdigit_stream" == *"$pattern"* \
                || "$perdigit_squeezed" == *"$pattern"* ]]; then
                perdigit_hits+="$file (per-digit stream: $perdigit_stream)"$'\n'
            fi
        done
    done < <(find "$RUNTIME_DIR" -type f \
        ! -path "$RUNTIME_DIR/freeswitch-artifacts/*" -print0)
    [[ -z "$perdigit_hits" ]] \
        || fail "per-digit audit reassembled a test PIN from individually-logged digits: $perdigit_hits"

    {
        printf 'GrantedPatternHits=0\n'
        printf 'RejectedPatternHits=0\n'
        printf 'PerDigitPatternHits=0\n'
        printf 'PerDigitExtractorSelfTest=%s\n' "$perdigit_selftest"
        printf 'PerDigitScanExcluded=freeswitch-artifacts (carrier simulator send-side log; concatenated scan still covers it)\n'
        printf 'ArtifactClasses=rustisk-trace-log,freeswitch-logs,sip-only-pcap,ami-transcript,cdr-cel,audio,stats\n'
        printf 'AMIObserved=Newexten,VarSet,PINGATESTATUS\n'
        printf 'SecretInputArtifact=false (mounted input lived outside artifact tree)\n'
    } >"$RUNTIME_DIR/m3-zero-hit-proof.txt"
    printf 'ZERO_HIT_SECRET_AUDIT: PASS (both test patterns, concatenated + per-digit; every runtime artifact; AMI live)\n'
}

run_outbound_listen_only_case() {
    local destination="9100@$FS_CONTAINER_IP:5060"
    local response
    local action
    # The two PIN inbound calls, the granted call's B leg, and the no-input
    # deadline call consume the first four process-global channel suffixes.
    # The harness is serial and starts a fresh rustisk process for every run.
    local channel="PJSIP/$destination-00000005"
    local stats
    local capture="$RUNTIME_DIR/m1-listen-only.wav"
    local hangup_observed_ms

    printf '\nRunning outbound listen-only receiver case...\n'
    printf -v action 'Action: Originate\r\nActionID: m1-listen-only\r\nChannel: PJSIP/%s\r\nContext: outbound-proof\r\nExten: 9100\r\nPriority: 1\r\nTimeout: 5000\r\nAsync: true\r\n\r\n' \
        "$destination"
    response="$(ami_action "$action")"
    grep -q 'Message: Originate successfully queued' <<<"$response" \
        || fail "AMI outbound Originate was not queued: $response"

    stats="$(wait_for_completed_stats "$channel")"
    hangup_observed_ms="$(now_ms)"
    assert_counter_equals RTPVoiceFramesRx 0 "$stats"
    assert_positive_counter RTPVoiceFramesTx "$stats"
    printf '%s\n' "$stats" >"$RUNTIME_DIR/listen-only-rtp-stats.txt"
    wait_for_resource_baseline "LISTEN_ONLY" "$hangup_observed_ms"

    for _ in {1..20}; do
        if docker cp "$FS_CONTAINER:/var/lib/freeswitch/recordings/m1-listen-only.wav" \
            "$capture" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done
    [[ -f "$capture" ]] || fail "FreeSWITCH did not produce the far-side capture"
    python3 "$HARNESS_DIR/assert_tone.py" "$capture" 660 \
        | tee "$RUNTIME_DIR/listen-only-tone-proof.txt"

    printf 'LISTEN_ONLY: PASS (RTPVoiceFramesRx=%s, RTPVoiceFramesTx=%s)\n' \
        "$(stat_value RTPVoiceFramesRx "$stats")" \
        "$(stat_value RTPVoiceFramesTx "$stats")"
}

run_dial_timeout_case() {
    local before_cancel_count
    local after_cancel_count
    local originate_response
    local uuid
    local hangup_observed_ms

    printf '\nRunning Dial timeout CANCEL case...\n'
    before_cancel_count="$(docker exec "$FS_CONTAINER" awk \
        '/CANCEL sip:9199@/ { count++ } END { print count + 0 }' \
        /var/log/freeswitch/freeswitch.log)"
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9001@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH timeout-case originate failed: $originate_response"
    fi
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "FreeSWITCH timeout-case originate failed: $originate_response"
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "DIAL_TIMEOUT" "$hangup_observed_ms"

    after_cancel_count="$(docker exec "$FS_CONTAINER" awk \
        '/CANCEL sip:9199@/ { count++ } END { print count + 0 }' \
        /var/log/freeswitch/freeswitch.log)"
    (( after_cancel_count > before_cancel_count )) \
        || fail "FreeSWITCH SIP trace did not observe CANCEL for the timed-out Dial leg"
    docker exec "$FS_CONTAINER" grep 'CANCEL sip:9199@' \
        /var/log/freeswitch/freeswitch.log \
        >"$RUNTIME_DIR/dial-timeout-cancel-proof.txt"

    printf 'DIAL_TIMEOUT: PASS (FreeSWITCH observed CANCEL, count delta=%d)\n' \
        "$((after_cancel_count - before_cancel_count))"
}

run_m2_two_way_bye_case() {
    local originate_response
    local a_uuid
    local b_uuid
    local channels
    local inbound_channel
    local outbound_channel
    local a_source_port
    local a_destination_port
    local negotiated_remote
    local sniff_pid
    local metadata_file="$RUNTIME_DIR/m2-a-rtp-metadata.json"
    local sniffer_ready="$RUNTIME_DIR/m2-sniffer-ready"
    local sniffer_go="$RUNTIME_DIR/m2-sniffer-go"
    local metadata
    local payload_type
    local sequence
    local timestamp
    local ssrc
    local pattern_output
    local inbound_stats
    local outbound_stats
    local hangup_observed_ms
    local a_capture_remote="/var/lib/freeswitch/recordings/m2-a-capture.wav"
    local a_capture="$RUNTIME_DIR/m2-a-capture.wav"
    local b_capture="$RUNTIME_DIR/m2-b-capture.wav"

    printf '\nRunning M2 two-way receiver, ingress hygiene, and BYE-silent teardown case...\n'
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9200@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH M2 originate failed: $originate_response"
    fi
    a_uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$a_uuid" ]] || fail "FreeSWITCH M2 originate failed: $originate_response"
    b_uuid="$(wait_for_fs_destination_uuid 9201)"
    channels="$(wait_for_rustisk_bridge_channels "9201@$FS_CONTAINER_IP:5060")"
    IFS='|' read -r inbound_channel outbound_channel <<<"$channels"
    wait_for_active_stats "$inbound_channel" >/dev/null
    wait_for_active_stats "$outbound_channel" >/dev/null

    fs_cli "uuid_record $a_uuid start $a_capture_remote" >/dev/null
    a_source_port="$(fs_channel_var "$a_uuid" local_media_port)"
    a_destination_port="$(fs_channel_var "$a_uuid" remote_media_port)"
    [[ "$a_source_port" =~ ^[0-9]+$ && "$a_destination_port" =~ ^[0-9]+$ ]] \
        || fail "FreeSWITCH did not expose A RTP ports: local=$a_source_port remote=$a_destination_port"
    negotiated_remote="$(stat_value RTPRemoteAddress "$(wait_for_active_stats "$inbound_channel")")"
    [[ "$negotiated_remote" == "$FS_CONTAINER_IP:$a_source_port" ]] \
        || fail "inbound negotiated remote mismatch: expected=$FS_CONTAINER_IP:$a_source_port actual=$negotiated_remote"

    rm -f "$sniffer_ready" "$sniffer_go"
    rtp_sniffer sniff \
        --source-ip "$FS_CONTAINER_IP" \
        --source-port "$a_source_port" \
        --destination-ip "$FS_HOST_IP" \
        --destination-port "$a_destination_port" \
        --ready-file /runtime/m2-sniffer-ready \
        --go-file /runtime/m2-sniffer-go \
        --timeout 5 >"$metadata_file" &
    sniff_pid=$!
    for _ in {1..100}; do
        [[ -f "$sniffer_ready" ]] && break
        sleep 0.05
    done
    [[ -f "$sniffer_ready" ]] || fail "RTP sniffer did not become ready"
    # The sniffer's 5s match deadline does not start until it observes
    # $sniffer_go (see rtp_injector.py's priming phase). Only touch it after
    # fs_cli's synchronous ESL round trip confirms FreeSWITCH actually
    # accepted uuid_broadcast — this closes the race where docker-exec/ESL
    # setup latency was silently consumed out of the match window before any
    # real media left FreeSWITCH (previously: deadline started at socket
    # open, so a slow `docker exec`+ESL round trip could eat the whole
    # window and fail with "no matching RTP packet observed" while real
    # frames were still incoming or about to arrive).
    fs_cli "uuid_broadcast $a_uuid tone_stream://%(1000,0,440) aleg" >/dev/null
    touch "$sniffer_go"
    wait "$sniff_pid" || fail "RTP sniffer did not observe A's negotiated source"
    sleep 1.2
    fs_cli "uuid_broadcast $b_uuid tone_stream://%(1000,0,660) aleg" >/dev/null
    sleep 1.2
    fs_cli "uuid_send_dtmf $a_uuid 7@200" >/dev/null
    wait_for_fs_channel_var "$b_uuid" M2DecodedDigit 7 >/dev/null
    wait_for_fs_channel_var "$b_uuid" RTPDTMFDigitsRx 1 >/dev/null
    printf 'M2_DTMF_A_TO_B: PASS (FreeSWITCH-B RTPDTMFDigitsRx=1 Digit=7)\n'
    sleep 0.5

    metadata="$(<"$metadata_file")"
    payload_type="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["payload_type"])' <<<"$metadata")"
    sequence="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["sequence"])' <<<"$metadata")"
    timestamp="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["timestamp"])' <<<"$metadata")"
    ssrc="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["ssrc"])' <<<"$metadata")"
    sequence="$(((sequence + 200) & 65535))"
    timestamp="$(((timestamp + 32000) & 4294967295))"

    assert_media_quiet "M2_PRE_INJECTION" "$inbound_channel" "$outbound_channel"
    assert_hostile_injection wrong-source RTPDiscardWrongSource \
        "$inbound_channel" "$a_source_port" "$a_destination_port" \
        "$payload_type" "$sequence" "$timestamp" "$ssrc"
    assert_hostile_injection wrong-pt RTPDiscardWrongPayloadType \
        "$inbound_channel" "$a_source_port" "$a_destination_port" \
        "$payload_type" "$((sequence + 1))" "$((timestamp + 160))" "$ssrc"
    assert_hostile_injection malformed RTPDiscardMalformed \
        "$inbound_channel" "$a_source_port" "$a_destination_port" \
        "$payload_type" "$((sequence + 2))" "$((timestamp + 320))" "$ssrc"
    assert_hostile_injection unstable-ssrc RTPDiscardUnstableSSRC \
        "$inbound_channel" "$a_source_port" "$a_destination_port" \
        "$payload_type" "$((sequence + 3))" "$((timestamp + 480))" "$ssrc"

    pattern_output="$(rtp_helper pattern \
        --source-ip "$FS_CONTAINER_IP" \
        --source-port "$a_source_port" \
        --destination-ip "$FS_HOST_IP" \
        --destination-port "$a_destination_port" \
        --payload-type "$payload_type" \
        --sequence "$((sequence + 20))" \
        --timestamp "$((timestamp + 3200))" \
        --ssrc "$ssrc")"
    printf 'M2_PATTERN_INJECTION: %s\n' "$pattern_output"
    sleep 0.8
    assert_media_quiet "M2_BYE" "$inbound_channel" "$outbound_channel"
    fs_cli "uuid_kill $a_uuid NORMAL_CLEARING" >/dev/null
    wait_for_call_end "$a_uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_call_end "$b_uuid"
    wait_for_resource_baseline "M2_BYE_SILENT" "$hangup_observed_ms"

    inbound_stats="$(wait_for_completed_stats "$inbound_channel")"
    outbound_stats="$(wait_for_completed_stats "$outbound_channel")"
    printf '%s\n' "$inbound_stats" >"$RUNTIME_DIR/m2-inbound-rtp-stats.txt"
    printf '%s\n' "$outbound_stats" >"$RUNTIME_DIR/m2-outbound-rtp-stats.txt"
    assert_positive_counter RTPVoiceFramesRx "$inbound_stats"
    assert_positive_counter RTPVoiceFramesRx "$outbound_stats"

    docker cp "$FS_CONTAINER:$a_capture_remote" "$a_capture" >/dev/null
    docker cp "$FS_CONTAINER:/var/lib/freeswitch/recordings/m2-b-capture.wav" "$b_capture" >/dev/null
    python3 "$HARNESS_DIR/assert_tone.py" "$b_capture" 440 \
        | sed 's/^/M2_A_TO_B: /' | tee "$RUNTIME_DIR/m2-a-to-b-tone-proof.txt"
    python3 "$HARNESS_DIR/assert_tone.py" "$a_capture" 660 \
        | sed 's/^/M2_B_TO_A: /' | tee "$RUNTIME_DIR/m2-b-to-a-tone-proof.txt"
    python3 "$HARNESS_DIR/assert_recovered_pattern.py" "$b_capture" \
        | tee "$RUNTIME_DIR/m2-recovered-pattern-proof.txt"
    printf 'M2_TWO_WAY_BYE: PASS (AReceiver=%s BReceiver=%s)\n' "$a_capture" "$b_capture"
}

run_m2_deadline_silent_case() {
    local originate_response
    local a_uuid
    local b_uuid
    local channels
    local inbound_channel
    local outbound_channel
    local inbound_stats
    local outbound_stats
    local hangup_observed_ms

    printf '\nRunning M2 absolute-deadline teardown with both RTP sources silent...\n'
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9202@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH M2 deadline originate failed: $originate_response"
    fi
    a_uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$a_uuid" ]] || fail "FreeSWITCH M2 deadline originate failed: $originate_response"
    b_uuid="$(wait_for_fs_destination_uuid 9203)"
    channels="$(wait_for_rustisk_bridge_channels "9203@$FS_CONTAINER_IP:5060")"
    IFS='|' read -r inbound_channel outbound_channel <<<"$channels"
    wait_for_call_end "$a_uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_call_end "$b_uuid"
    wait_for_resource_baseline "M2_DEADLINE_SILENT" "$hangup_observed_ms"
    inbound_stats="$(wait_for_completed_stats "$inbound_channel")"
    outbound_stats="$(wait_for_completed_stats "$outbound_channel")"
    assert_counter_equals RTPPacketsRx 0 "$inbound_stats"
    assert_counter_equals RTPPacketsRx 0 "$outbound_stats"
    printf 'M2_DEADLINE_SILENT: PASS (InboundRTPPacketsRx=0 OutboundRTPPacketsRx=0)\n'
}

queue_m4_originate() {
    local destination="$1"
    local context="${2:-m4-wait}"
    local extension="${3:-s}"
    local timeout_ms="${4:-5000}"
    local action
    local response
    printf -v action 'Action: Originate\r\nActionID: m4-%s\r\nChannel: PJSIP/%s@%s:5060\r\nContext: %s\r\nExten: %s\r\nPriority: 1\r\nTimeout: %s\r\nAsync: true\r\n\r\n' \
        "$destination" "$destination" "$IMPAIRMENT_IP" "$context" "$extension" "$timeout_ms"
    response="$(ami_action "$action")"
    grep -q 'Message: Originate successfully queued' <<<"$response" \
        || fail "M4 Originate $destination was not queued: $response"
}

run_m4_dropped_200_case() {
    local uuid
    local hangup_observed_ms
    printf '\nRunning M4 dropped-200 retransmission case through Contact proxy...\n'
    set_impairment drop_first_invite_200
    queue_m4_originate 9300
    uuid="$(wait_for_fs_destination_uuid 9300)"
    wait_for_impairment_counter fs_to_rustisk_response_200_INVITE 2 >/dev/null
    wait_for_impairment_counter rustisk_to_fs_request_ACK 1 >/dev/null
    [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" == "true" ]] \
        || fail "FreeSWITCH call ended before the retransmitted 200 was ACKed"
    assert_impairment_hashes_identical_minimum fs_to_rustisk 200 INVITE 2 \
        | tee "$RUNTIME_DIR/m4-dropped-200-proof.txt"
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "M4_DROPPED_200" "$hangup_observed_ms"
    printf 'M4_DROPPED_200: PASS (FreeSWITCH retransmitted identical 200 and received ACK)\n'
}

run_m4_late_invite_replay_case() {
    local originate_response
    local uuid
    local live_snapshot
    local baseline_calls
    local live_calls
    local hangup_observed_ms
    printf '\nRunning M4 late duplicate-INVITE replay case...\n'
    set_impairment replay_invite_after_200
    originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9401@$IMPAIRMENT_IP:5060 &park()")"
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "M4 replay originate failed: $originate_response"
    wait_for_impairment_counter rustisk_to_fs_response_200_INVITE 3 >/dev/null
    assert_impairment_hash_count rustisk_to_fs 200 INVITE 3 \
        | tee "$RUNTIME_DIR/m4-late-invite-replay-proof.txt"
    live_snapshot="$(resource_snapshot)"
    baseline_calls="${BASELINE_RESOURCES%%/*}"
    live_calls="${live_snapshot%%/*}"
    (( live_calls == baseline_calls + 1 )) \
        || fail "late INVITEs created more than one call: baseline=$BASELINE_RESOURCES live=$live_snapshot"
    [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" == "true" ]] \
        || fail "FreeSWITCH did not retain the single established call"
    fs_cli "uuid_kill $uuid NORMAL_CLEARING" >/dev/null
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "M4_LATE_INVITE_REPLAY" "$hangup_observed_ms"
    printf 'M4_LATE_INVITE_REPLAY: PASS (Identical200s=3 Calls=1)\n'
}

run_m4_dropped_ack_case() {
    local originate_response
    local uuid
    printf '\nRunning M4 dropped-ACK Timer-H teardown case...\n'
    set_impairment drop_all_ack
    originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9402@$IMPAIRMENT_IP:5060 &park()")"
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "M4 dropped-ACK originate failed: $originate_response"
    wait_for_impairment_counter fs_to_rustisk_request_ACK 2 >/dev/null
    wait_for_impairment_counter rustisk_to_fs_response_200_INVITE 2 >/dev/null
    wait_for_resource_baseline_eventually "M4_DROPPED_ACK_TIMER_H"
    fs_cli "uuid_kill $uuid NORMAL_CLEARING" >/dev/null 2>&1 || true
    printf 'M4_DROPPED_ACK_TIMER_H: PASS (ACKsDropped=%s Retransmitted200s=%s)\n' \
        "$(impairment_counter fs_to_rustisk_request_ACK)" \
        "$(impairment_counter rustisk_to_fs_response_200_INVITE)"
}

run_m4_dropped_bye_case() {
    local uuid
    local hangup_observed_ms
    printf '\nRunning M4 dropped-BYE retransmission case...\n'
    set_impairment drop_first_bye
    queue_m4_originate 9301
    uuid="$(wait_for_fs_destination_uuid 9301)"
    wait_for_impairment_counter rustisk_to_fs_request_BYE 2 >/dev/null
    python3 - "$IMPAIRMENT_STATE" <<'PY' | tee "$RUNTIME_DIR/m4-dropped-bye-proof.txt"
import json
import sys
state = json.load(open(sys.argv[1], encoding="utf-8"))
events = [event for event in state["events"]
          if event["direction"] == "rustisk_to_fs" and event["method"] == "BYE"]
if len(events) < 2 or len({event["sha256"] for event in events}) != 1:
    raise SystemExit(f"BYE was not retransmitted byte-identically: {events}")
print(f"IdenticalBYEs={len(events)} SHA256={events[0]['sha256']}")
PY
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "M4_DROPPED_BYE" "$hangup_observed_ms"
    printf 'M4_DROPPED_BYE: PASS (FreeSWITCH received retransmitted BYE)\n'
}

run_m4_provisional_silence_case() {
    local inner_uuid
    local active_transactions
    local active_invite_clients
    local baseline_invite_clients
    local timer_b_started_ms
    local timer_b_elapsed_ms
    local timer_b_logs_before
    local timer_b_logs_after
    printf '\nRunning M4 180-then-silence Timer-B case...\n'
    set_impairment none
    timer_b_logs_before="$(timer_b_teardown_count)"
    queue_m4_originate 9303 m4-timer-b-hold s 70000
    inner_uuid="$(wait_for_fs_destination_uuid 9303)"
    wait_for_impairment_counter fs_to_rustisk_response_180_INVITE 1 >/dev/null
    active_transactions="$(transaction_snapshot)"
    active_invite_clients="${active_transactions%%/*}"
    baseline_invite_clients="${BASELINE_TRANSACTIONS%%/*}"
    (( active_invite_clients == baseline_invite_clients + 1 )) \
        || fail "180 response did not leave exactly one live INVITE client transaction: baseline=$BASELINE_TRANSACTIONS actual=$active_transactions"
    timer_b_started_ms="$(now_ms)"
    timer_b_elapsed_ms="$(wait_for_transaction_baseline_in_timer_b_window "$timer_b_started_ms")"
    wait_for_resource_baseline_eventually "M4_180_SILENCE_TIMER_B"
    timer_b_logs_after="$(timer_b_teardown_count)"
    (( timer_b_logs_after == timer_b_logs_before + 1 )) \
        || fail "Timer B teardown log count must increase by one: $timer_b_logs_before->$timer_b_logs_after"
    fs_cli "uuid_kill $inner_uuid NORMAL_CLEARING" >/dev/null 2>&1 || true
    printf 'M4_180_SILENCE_TIMER_B: PASS (FreeSWITCH sent 180; TimerBTransactionDrainMs=%s; DirectOriginateHoldSec=70)\n' \
        "$timer_b_elapsed_ms"
}

run_m4_forged_dialog_case() {
    local originate_response
    local uuid
    local live_snapshot
    local baseline_calls
    local live_calls
    local hangup_observed_ms
    printf '\nRunning M4 allowed-source forged dialog identity case...\n'
    set_impairment none
    originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9405@$IMPAIRMENT_IP:5060 &park()")"
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "M4 forged-dialog originate failed: $originate_response"
    wait_for_impairment_counter fs_to_rustisk_request_ACK 1 >/dev/null
    [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" == "true" ]] \
        || fail "forged-dialog setup call was not established at FreeSWITCH"
    set_impairment inject_forged true
    wait_for_impairment_counter forged_injected 3 >/dev/null
    sleep 0.3
    [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" == "true" ]] \
        || fail "forged Call-ID/tag/CSeq tore down the live FreeSWITCH call"
    live_snapshot="$(resource_snapshot)"
    baseline_calls="${BASELINE_RESOURCES%%/*}"
    live_calls="${live_snapshot%%/*}"
    (( live_calls == baseline_calls + 1 )) \
        || fail "forged dialog requests corrupted live resources: $live_snapshot"
    fs_cli "uuid_kill $uuid NORMAL_CLEARING" >/dev/null
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "M4_FORGED_DIALOG" "$hangup_observed_ms"
    printf 'M4_FORGED_DIALOG: PASS (AllowedSource=true ForgedCallIdTagCSeq=3 LiveCallPreserved=true)\n'
}

run_m4_concurrency_case() {
    local index
    local destination
    local frequency
    local uuid
    local capture
    local uuids=()
    printf '\nRunning M4 10-way distinct-tone concurrency case...\n'
    set_impairment none
    for index in {0..9}; do
        destination="$((9310 + index))"
        queue_m4_originate "$destination" m4-concurrency "$index"
    done
    for index in {0..9}; do
        destination="$((9310 + index))"
        uuid="$(wait_for_fs_destination_uuid "$destination")"
        uuids+=("$uuid")
    done
    for uuid in "${uuids[@]}"; do
        wait_for_call_end "$uuid"
    done
    wait_for_resource_baseline_eventually "M4_CONCURRENCY_10"
    for index in {0..9}; do
        destination="$((9310 + index))"
        frequency="$((400 + index * 50))"
        capture="$RUNTIME_DIR/m4-$destination.wav"
        docker cp "$FS_CONTAINER:/var/lib/freeswitch/recordings/m4-$destination.wav" \
            "$capture" >/dev/null
        python3 "$HARNESS_DIR/assert_tone.py" "$capture" "$frequency" \
            | sed "s/^/M4_CONCURRENCY_$destination: /"
    done | tee "$RUNTIME_DIR/m4-concurrency-tone-proof.txt"
    printf 'M4_CONCURRENCY_10: PASS (Calls=10 DistinctReceiverTones=10)\n'
}

run_m4_abandon_200_race_case() {
    local originate_response
    local outer_uuid
    local hangup_observed_ms
    printf '\nRunning M4 CANCEL/200 crossing case...\n'
    set_impairment hold_invite_200_until_cancel
    originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9404@$FS_HOST_IP:15060 &park()")"
    outer_uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$outer_uuid" ]] || fail "M4 abandon race originate failed: $originate_response"
    wait_for_impairment_counter held_200_released_after_cancel 1 >/dev/null
    wait_for_impairment_counter rustisk_to_fs_request_BYE 1 >/dev/null
    python3 - "$IMPAIRMENT_STATE" <<'PY' | tee "$RUNTIME_DIR/m4-abandon-200-order-proof.txt"
import json
import sys
state = json.load(open(sys.argv[1], encoding="utf-8"))
methods = [event["method"] for event in state["events"]
           if event["direction"] == "rustisk_to_fs" and event["method"] in ("CANCEL", "ACK", "BYE")]
try:
    positions = [methods.index(name) for name in ("CANCEL", "ACK", "BYE")]
except ValueError as error:
    raise SystemExit(f"missing crossing method: {methods}") from error
if positions != sorted(positions):
    raise SystemExit(f"crossing order was not CANCEL then ACK then BYE: {methods}")
print(f"ReceiverMethodOrder={methods}")
PY
    wait_for_call_end "$outer_uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_resource_baseline "M4_ABANDON_200" "$hangup_observed_ms"
    printf 'M4_ABANDON_200: PASS (FreeSWITCH observed CANCEL then ACK then BYE)\n'
}

run_m4_bye_final_failure_cases() {
    local uuid
    local hangup_observed_ms
    printf '\nRunning M4 non-2xx BYE-final consumption case...\n'
    set_impairment rewrite_bye_final_481
    queue_m4_originate 9305
    uuid="$(wait_for_fs_destination_uuid 9305)"
    wait_for_call_end "$uuid"
    hangup_observed_ms="$(now_ms)"
    wait_for_impairment_counter fs_to_rustisk_response_200_BYE 1 >/dev/null
    wait_for_resource_baseline "M4_BYE_FINAL_481" "$hangup_observed_ms"
    grep -q '"action": "rewrite-481"' "$IMPAIRMENT_STATE" \
        || fail "proxy did not rewrite the BYE final to 481"
    printf 'M4_BYE_FINAL_481: PASS (ForwardedStatus=481)\n'

    printf '\nRunning M4 dropped BYE-final Timer-F reaper case...\n'
    set_impairment drop_all_bye_final
    queue_m4_originate 9306
    uuid="$(wait_for_fs_destination_uuid 9306)"
    wait_for_call_end "$uuid"
    wait_for_impairment_counter fs_to_rustisk_response_200_BYE 1 >/dev/null
    wait_for_resource_baseline_eventually "M4_BYE_FINAL_DROPPED"
    printf 'M4_BYE_FINAL_DROPPED: PASS (ResponsesDropped=%s TimerFReaped=true)\n' \
        "$(impairment_counter fs_to_rustisk_response_200_BYE)"
}

run_m5_soak_case() {
    local count="${M5_SOAK_CALLS:-500}"
    local baseline after uuid resp
    printf '\nRunning M5 %d-call exact-baseline soak (every registry)...\n' "$count"
    baseline="$(full_snapshot)"
    printf 'M5_SOAK baseline (core/txn/rtp/registrar/hangup-cb/answer-cb): %s\n' "$baseline"

    local i
    for ((i = 1; i <= count; i++)); do
        if ! resp="$(fs_cli "originate {ignore_early_media=true,origination_caller_id_number=15551230000,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9500@$FS_HOST_IP:15060 &park()" 2>&1)"; then
            fail "M5 soak originate #$i failed: $resp"
        fi
        uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$resp" | head -n1)"
        [[ -n "$uuid" ]] || fail "M5 soak originate #$i produced no A-leg uuid: $resp"
        wait_for_call_end "$uuid"
        if (( i % 50 == 0 )); then
            printf '  M5_SOAK progress: %d/%d complete; live=%s\n' "$i" "$count" "$(full_snapshot)"
        fi
    done

    # EXACT baseline restoration across every registry — zero drift. A per-call
    # leak of even one unit in any map (a stranded outbound register-store
    # channel per M-g, a leaked RTP socket, an unfreed callback closure) shows
    # as non-zero drift after `count` calls. (SIPRegistrarBindings is a
    # regression sentinel only — this soak drives no REGISTER lifecycle; see the
    # full_snapshot note. Registrar bind/expire proof is M6's job.)
    after="$(wait_for_full_baseline "$baseline")"
    printf 'M5_SOAK: PASS (%d calls; exact baseline restored across all registries: %s)\n' \
        "$count" "$after"
}

require_command cargo
require_command docker
require_command python3

AGENT_TMP="${AGENT_TMP:-/mnt/data/herodevs-agents}"
[[ "$AGENT_TMP" == /mnt/data/herodevs-agents* ]] \
    || fail "AGENT_TMP must stay under /mnt/data/herodevs-agents"
SECRET_DIR="$(mktemp -d "$AGENT_TMP/m3-pin-secret.XXXXXX")"
printf -v GRANTED_TEST_PIN '%06d' "$(( (RANDOM * 32768 + RANDOM) % 1000000 ))"
last_digit="${GRANTED_TEST_PIN: -1}"
WRONG_TEST_PIN="${GRANTED_TEST_PIN:0:5}$(( (10#$last_digit + 1) % 10 ))"
umask 077
printf '%s\n' "$GRANTED_TEST_PIN" >"$SECRET_DIR/pin"
printf 'invalid\n' >"$SECRET_DIR/invalid"

rm -rf "$RUNTIME_DIR"
mkdir -p "$CONFIG_DIR" "$RUN_DIR" "$PROMPT_DIR"
python3 "$HARNESS_DIR/generate_wavs.py" "$PROMPT_DIR"
docker run --rm --entrypoint /bin/cat "$FS_IMAGE" \
    /usr/share/freeswitch/conf/vanilla/vars.xml \
    | sed \
        -e 's|cmd="stun-set" data="external_rtp_ip=stun:stun.freeswitch.org"|cmd="set" data="external_rtp_ip=$${local_ip_v4}"|' \
        -e 's|cmd="stun-set" data="external_sip_ip=stun:stun.freeswitch.org"|cmd="set" data="external_sip_ip=$${local_ip_v4}"|' \
        >"$CONFIG_DIR/freeswitch-vars.xml"
docker run --rm --entrypoint /bin/cat "$FS_IMAGE" \
    /usr/share/freeswitch/conf/vanilla/sip_profiles/internal.xml \
    | sed \
        -e "s|\$\${local_ip_v4}|$FS_CONTAINER_IP|g" \
        -e "s|\$\${external_rtp_ip}|$FS_CONTAINER_IP|g" \
        -e "s|\$\${external_sip_ip}|$FS_CONTAINER_IP|g" \
        -e 's|<param name="context" value="public"/>|<param name="context" value="default"/>|' \
        -e 's|<param name="auth-calls" value="$${internal_auth_calls}"/>|<param name="auth-calls" value="false"/>|' \
        -e '/<param name="apply-inbound-acl" value="domains"\/>/d' \
        >"$CONFIG_DIR/freeswitch-internal.xml"

sed \
    -e "s|@CONFIG_DIR@|$CONFIG_DIR|g" \
    -e "s|@RUN_DIR@|$RUN_DIR|g" \
    "$HARNESS_DIR/config/asterisk.conf" >"$CONFIG_DIR/asterisk.conf"
sed 's/bindaddr = 127.0.0.1/bindaddr = 0.0.0.0/' \
    "$HARNESS_DIR/config/manager.conf" >"$CONFIG_DIR/manager.conf"
sed \
    -e "s|@FS_CONTAINER_IP@|$FS_CONTAINER_IP|g" \
    -e "s|@IMPAIRMENT_IP@|$IMPAIRMENT_IP|g" \
    -e "s|bind = 0.0.0.0:15060|bind = $FS_HOST_IP:15060|" \
    "$HARNESS_DIR/config/pjsip.conf" >"$CONFIG_DIR/pjsip.conf"
cp "$HARNESS_DIR/config/rtp.conf" "$CONFIG_DIR/rtp.conf"
printf '[general]\nsecret_file = /run/secrets/rustisk/pin\n' \
    >"$CONFIG_DIR/pin_gate.conf"

printf 'Building rustisk with Rust 1.97.0...\n'
(cd "$REPO_DIR" && cargo +1.97.0 build -p rustisk-cli)
prove_startup_secret_fail_closed

printf 'Starting pinned FreeSWITCH carrier container...\n'
docker network create --internal --subnet "$FS_SUBNET" "$FS_NETWORK" >/dev/null
docker run --rm --name "$FS_CONTAINER" \
    --network "$FS_NETWORK" \
    --ip "$FS_CONTAINER_IP" \
    --add-host host.docker.internal:host-gateway \
    --mount "type=bind,src=$CONFIG_DIR/freeswitch-vars.xml,dst=/usr/share/freeswitch/conf/vanilla/vars.xml,readonly" \
    --mount "type=bind,src=$CONFIG_DIR/freeswitch-internal.xml,dst=/usr/share/freeswitch/conf/vanilla/sip_profiles/internal.xml,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/config/event_socket.conf.xml,dst=/usr/share/freeswitch/conf/vanilla/autoload_configs/event_socket.conf.xml,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/config/m1-endpoints.xml,dst=/usr/share/freeswitch/conf/vanilla/dialplan/default/00-rustisk-m1.xml,readonly" \
    -d "$FS_IMAGE" >/dev/null
wait_for_freeswitch
fs_cli "sofia global siptrace on" >/dev/null

printf 'Starting deterministic on-path SIP impairment proxy...\n'
docker run --rm --name "$IMPAIRMENT_CONTAINER" \
    --network "$FS_NETWORK" \
    --ip "$IMPAIRMENT_IP" \
    --user "$(id -u):$(id -g)" \
    --mount "type=bind,src=$HARNESS_DIR/sip_impairment_proxy.py,dst=/sip_impairment_proxy.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=/runtime" \
    -d "$RUSTISK_IMAGE" python3 /sip_impairment_proxy.py \
        --listen 0.0.0.0:5060 \
        --freeswitch "$FS_CONTAINER_IP:5060" \
        --rustisk "$FS_HOST_IP:15060" \
        --proxy-host "$IMPAIRMENT_IP" \
        --control /runtime/impairment-control.json \
        --state /runtime/impairment-state.json >/dev/null
set_impairment none

sed \
    -e "s|@PROMPT_DIR@|$PROMPT_DIR|g" \
    -e "s|@FS_CONTAINER_IP@|$FS_CONTAINER_IP|g" \
    -e "s|@IMPAIRMENT_IP@|$IMPAIRMENT_IP|g" \
    "$HARNESS_DIR/config/extensions.conf" >"$CONFIG_DIR/extensions.conf"

printf 'Starting isolated rustisk...\n'
docker run --rm --name "$RUSTISK_CONTAINER" \
    --network "$FS_NETWORK" \
    --ip "$FS_HOST_IP" \
    --ulimit nofile=65536:65536 \
    --user "$(id -u):$(id -g)" \
    --entrypoint /rustisk \
    --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/ami_client.py,dst=/ami_client.py,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/ami_subscriber.py,dst=/ami_subscriber.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
    --mount "type=bind,src=$SECRET_DIR/pin,dst=/run/secrets/rustisk/pin,readonly" \
    "$RUSTISK_IMAGE" -f -vvv -C "$CONFIG_DIR/asterisk.conf" >"$RUSTISK_LOG" 2>&1 &
RUSTISK_PID=$!
wait_for_ami
BASELINE_RESOURCES="$(resource_snapshot)"
BASELINE_TRANSACTIONS="$(transaction_snapshot)"
printf 'Exact resource baseline: %s (store/driver/call-id/state/notify)\n' "$BASELINE_RESOURCES"
printf 'Exact transaction baseline: %s (invite-client/invite-server/non-invite-client/non-invite-server)\n' "$BASELINE_TRANSACTIONS"

if [[ "${FREESWITCH_PIN_GATE_CASE:-all}" == "m4-timer-b" ]]; then
    run_m4_provisional_silence_case
    printf '\nPASS: isolated M4 Timer B impairment acceptance.\n'
    printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
    exit 0
fi

if [[ "${FREESWITCH_PIN_GATE_CASE:-all}" == "m5-soak" ]]; then
    run_m5_soak_case
    printf '\nPASS: isolated M5 exact-baseline soak.\n'
    printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
    exit 0
fi

if [[ "${FREESWITCH_PIN_GATE_CASE:-all}" == "m2-standalone" ]]; then
    run_m2_two_way_bye_case
    printf '\nPASS: isolated M2 two-way / ingress-hygiene / BYE-silent re-run.\n'
    printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
    exit 0
fi

start_m3_sink_receivers
run_case 1 "$WRONG_TEST_PIN" REJECTED
run_case 2 "$GRANTED_TEST_PIN" GRANTED
run_m3_no_input_deadline_case
stop_m3_sink_receivers

if [[ "${FREESWITCH_PIN_GATE_CASE:-all}" == "m3" ]]; then
    assert_m3_zero_hit_audit
    printf '\nPASS: isolated M3 receiver-side and zero-hit acceptance.\n'
    printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
    exit 0
fi

run_outbound_listen_only_case
run_dial_timeout_case
run_m2_two_way_bye_case
run_m2_deadline_silent_case
run_m4_dropped_200_case
run_m4_late_invite_replay_case
run_m4_dropped_ack_case
run_m4_dropped_bye_case
run_m4_provisional_silence_case
run_m4_forged_dialog_case
run_m4_concurrency_case
run_m4_abandon_200_race_case
run_m4_bye_final_failure_cases
run_m5_soak_case

assert_m3_zero_hit_audit
printf '\nPASS: real FreeSWITCH SIP/RTP gate completed PIN, M1, M2, and M4 impairment acceptance.\n'
printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
