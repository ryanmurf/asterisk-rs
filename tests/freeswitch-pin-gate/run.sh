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
BASELINE_RESOURCES=""
BASELINE_TRANSACTIONS=""
RTP_HELPER_LABEL="rustisk.m2.helper=$FS_CONTAINER"
IMPAIRMENT_CONTROL="$RUNTIME_DIR/impairment-control.json"
IMPAIRMENT_STATE="$RUNTIME_DIR/impairment-state.json"
IMPAIRMENT_GENERATION=0

cleanup() {
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

wait_for_call_end_eventually() {
    local uuid="$1"
    for _ in {1..450}; do
        if [[ "$(fs_cli "uuid_exists $uuid" | tr -d '\r\n')" != "true" ]]; then
            return
        fi
        sleep 0.1
    done
    fail "FreeSWITCH call $uuid did not end within the transaction timeout window"
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

    printf '\nRunning %s case with receiver-side RFC4733 digits...\n' "$expected"
    if ! originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9000@$FS_HOST_IP:15060 &park()" 2>&1)"; then
        fail "FreeSWITCH originate failed: $originate_response"
    fi
    uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$uuid" ]] || fail "FreeSWITCH originate failed: $originate_response"

    fs_cli "uuid_record $uuid start $capture" >/dev/null
    fs_cli "uuid_broadcast $uuid tone_stream://%(1000,0,600) aleg" >/dev/null
    sleep 2.2
    fs_cli "uuid_send_dtmf $uuid ${digits}#@200" >/dev/null
    wait_for_call_end "$uuid"

    channel="PJSIP/fs-carrier-$(printf '%08d' "$ordinal")"
    stats="$(wait_for_completed_stats "$channel")"
    grep -q 'Message: RTP statistics' <<<"$stats" || fail "RTPStats failed for $channel: $stats"
    grep -q "RTPActive: false" <<<"$stats" || fail "completed stats were not retained for $channel"
    assert_positive_counter RTPPacketsTx "$stats"
    assert_positive_counter RTPPacketsRx "$stats"
    assert_positive_counter RTPVoiceFramesTx "$stats"
    assert_positive_counter RTPVoiceFramesRx "$stats"
    assert_counter_equals RTPDTMFDigitsRx "${#digits}" "$stats"
    grep -q "Verbose: PIN_GATE_RESULT=$expected" "$RUSTISK_LOG" \
        || fail "dialplan did not take the $expected branch"

    docker cp "$FS_CONTAINER:$capture" "$RUNTIME_DIR/${expected,,}-capture.wav" >/dev/null
    (( $(wc -c <"$RUNTIME_DIR/${expected,,}-capture.wav") > 44 )) \
        || fail "FreeSWITCH audio capture has no samples"

    printf '%s\n' "$stats" >"$RUNTIME_DIR/${expected,,}-rtp-stats.txt"
    wait_for_resource_baseline_eventually "$expected"
    printf '%s: PASS (TX voice=%s, RX voice=%s, RX DTMF=%s)\n' \
        "$expected" \
        "$(stat_value RTPVoiceFramesTx "$stats")" \
        "$(stat_value RTPVoiceFramesRx "$stats")" \
        "$(stat_value RTPDTMFDigitsRx "$stats")"
}

run_outbound_listen_only_case() {
    local destination="9100@$FS_CONTAINER_IP:5060"
    local response
    local action
    # The two PIN calls consume the first two process-global channel suffixes.
    # The harness is serial and starts a fresh rustisk process for every run.
    local channel="PJSIP/$destination-00000003"
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

    rm -f "$sniffer_ready"
    rtp_sniffer sniff \
        --source-ip "$FS_CONTAINER_IP" \
        --source-port "$a_source_port" \
        --destination-ip "$FS_HOST_IP" \
        --destination-port "$a_destination_port" \
        --ready-file /runtime/m2-sniffer-ready \
        --timeout 5 >"$metadata_file" &
    sniff_pid=$!
    for _ in {1..100}; do
        [[ -f "$sniffer_ready" ]] && break
        sleep 0.05
    done
    [[ -f "$sniffer_ready" ]] || fail "RTP sniffer did not become ready"
    fs_cli "uuid_broadcast $a_uuid tone_stream://%(1000,0,440) aleg" >/dev/null
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
    local action
    local response
    printf -v action 'Action: Originate\r\nActionID: m4-%s\r\nChannel: PJSIP/%s@%s:5060\r\nContext: %s\r\nExten: %s\r\nPriority: 1\r\nTimeout: 5000\r\nAsync: true\r\n\r\n' \
        "$destination" "$destination" "$IMPAIRMENT_IP" "$context" "$extension"
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
    local originate_response
    local outer_uuid
    local inner_uuid
    printf '\nRunning M4 180-then-silence Timer-B case...\n'
    set_impairment none
    originate_response="$(fs_cli "originate {origination_caller_id_number=15551234567,ignore_early_media=true,rtp_adv_audio_ip=$FS_CONTAINER_IP}sofia/internal/9403@$FS_HOST_IP:15060 &park()")"
    outer_uuid="$(grep -Eo '[0-9a-f]{8}-[0-9a-f-]{27}' <<<"$originate_response" | head -n1)"
    [[ -n "$outer_uuid" ]] || fail "M4 provisional originate failed: $originate_response"
    inner_uuid="$(wait_for_fs_destination_uuid 9303)"
    wait_for_impairment_counter fs_to_rustisk_response_180_INVITE 1 >/dev/null
    wait_for_call_end_eventually "$outer_uuid"
    wait_for_resource_baseline_eventually "M4_180_SILENCE_TIMER_B"
    fs_cli "uuid_kill $inner_uuid NORMAL_CLEARING" >/dev/null 2>&1 || true
    printf 'M4_180_SILENCE_TIMER_B: PASS (FreeSWITCH sent 180; Timer B ended caller)\n'
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

require_command cargo
require_command docker
require_command python3

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

printf 'Building rustisk with Rust 1.97.0...\n'
(cd "$REPO_DIR" && cargo +1.97.0 build -p rustisk-cli)

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
    --user "$(id -u):$(id -g)" \
    --entrypoint /rustisk \
    --mount "type=bind,src=$REPO_DIR/target/debug/rustisk,dst=/rustisk,readonly" \
    --mount "type=bind,src=$HARNESS_DIR/ami_client.py,dst=/ami_client.py,readonly" \
    --mount "type=bind,src=$RUNTIME_DIR,dst=$RUNTIME_DIR" \
    "$RUSTISK_IMAGE" -f -vv -C "$CONFIG_DIR/asterisk.conf" >"$RUSTISK_LOG" 2>&1 &
RUSTISK_PID=$!
wait_for_ami
BASELINE_RESOURCES="$(resource_snapshot)"
BASELINE_TRANSACTIONS="$(transaction_snapshot)"
printf 'Exact resource baseline: %s (store/driver/call-id/state/notify)\n' "$BASELINE_RESOURCES"
printf 'Exact transaction baseline: %s (invite-client/invite-server/non-invite-client/non-invite-server)\n' "$BASELINE_TRANSACTIONS"

run_case 1 123456 GRANTED
run_case 2 123450 REJECTED
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

printf '\nPASS: real FreeSWITCH SIP/RTP gate completed PIN, M1, M2, and M4 impairment acceptance.\n'
printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
