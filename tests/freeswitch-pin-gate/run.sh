#!/usr/bin/env bash
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd -- "$HARNESS_DIR/../.." && pwd)"
RUNTIME_DIR="$REPO_DIR/target/freeswitch-pin-gate"
CONFIG_DIR="$RUNTIME_DIR/config"
RUN_DIR="$RUNTIME_DIR/run"
PROMPT_DIR="$RUNTIME_DIR/prompts"
RUSTISK_LOG="$RUNTIME_DIR/rustisk.log"
FS_IMAGE="safarov/freeswitch@sha256:b31c743f4c911a19687c61e3214968f2a24f93f9d3d667cc26284192e158ffc6"
FS_CONTAINER="rustisk-fs-pin-gate-$$"
FS_CONTAINER_IP=""
FS_HOST_IP=""
RUSTISK_PID=""

cleanup() {
    if [[ -n "$RUSTISK_PID" ]]; then
        kill "$RUSTISK_PID" 2>/dev/null || true
        wait "$RUSTISK_PID" 2>/dev/null || true
    fi
    if docker inspect "$FS_CONTAINER" >/dev/null 2>&1; then
        docker exec "$FS_CONTAINER" freeswitch -stop >/dev/null 2>&1 || true
        for _ in {1..50}; do
            docker inspect "$FS_CONTAINER" >/dev/null 2>&1 || break
            sleep 0.1
        done
        docker rm -f "$FS_CONTAINER" >/dev/null 2>&1 || true
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

fs_cli() {
    docker exec "$FS_CONTAINER" fs_cli \
        -H 127.0.0.1 -P 8021 -p ClueCon -t 10000 -x "$1"
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
        if nc -z 127.0.0.1 15038 >/dev/null 2>&1; then
            return
        fi
        if ! kill -0 "$RUSTISK_PID" 2>/dev/null; then
            sed -n '1,240p' "$RUSTISK_LOG" >&2
            fail "rustisk exited before AMI became ready"
        fi
        sleep 0.1
    done
    sed -n '1,240p' "$RUSTISK_LOG" >&2
    fail "rustisk AMI did not become ready"
}

ami_rtp_stats() {
    local channel="$1"
    printf 'Action: Login\r\nUsername: harness\r\nSecret: pin-gate-local-only\r\n\r\nAction: RTPStats\r\nChannel: %s\r\n\r\nAction: Logoff\r\n\r\n' "$channel" \
        | nc -w 2 127.0.0.1 15038
}

wait_for_completed_stats() {
    local channel="$1"
    local response=""
    for _ in {1..75}; do
        response="$(ami_rtp_stats "$channel")"
        if grep -q 'Message: RTP statistics' <<<"$response" \
            && grep -q 'RTPActive: false' <<<"$response"; then
            printf '%s\n' "$response"
            return
        fi
        sleep 0.5
    done
    fail "completed RTPStats record was not available for $channel: $response"
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
    printf '%s: PASS (TX voice=%s, RX voice=%s, RX DTMF=%s)\n' \
        "$expected" \
        "$(stat_value RTPVoiceFramesTx "$stats")" \
        "$(stat_value RTPVoiceFramesRx "$stats")" \
        "$(stat_value RTPDTMFDigitsRx "$stats")"
}

require_command cargo
require_command docker
require_command nc
require_command python3

rm -rf "$RUNTIME_DIR"
mkdir -p "$CONFIG_DIR" "$RUN_DIR" "$PROMPT_DIR"
python3 "$HARNESS_DIR/generate_wavs.py" "$PROMPT_DIR"

sed \
    -e "s|@CONFIG_DIR@|$CONFIG_DIR|g" \
    -e "s|@RUN_DIR@|$RUN_DIR|g" \
    "$HARNESS_DIR/config/asterisk.conf" >"$CONFIG_DIR/asterisk.conf"
sed "s|@PROMPT_DIR@|$PROMPT_DIR|g" \
    "$HARNESS_DIR/config/extensions.conf" >"$CONFIG_DIR/extensions.conf"
cp "$HARNESS_DIR/config/manager.conf" "$CONFIG_DIR/manager.conf"
cp "$HARNESS_DIR/config/pjsip.conf" "$CONFIG_DIR/pjsip.conf"
cp "$HARNESS_DIR/config/rtp.conf" "$CONFIG_DIR/rtp.conf"

printf 'Building rustisk with Rust 1.97.0...\n'
(cd "$REPO_DIR" && cargo +1.97.0 build -p rustisk-cli)

printf 'Starting isolated rustisk...\n'
"$REPO_DIR/target/debug/rustisk" -f -vv -C "$CONFIG_DIR/asterisk.conf" \
    >"$RUSTISK_LOG" 2>&1 &
RUSTISK_PID=$!
wait_for_ami

printf 'Starting pinned FreeSWITCH carrier container...\n'
docker run --rm --name "$FS_CONTAINER" \
    --add-host host.docker.internal:host-gateway \
    --mount "type=bind,src=$HARNESS_DIR/config/event_socket.conf.xml,dst=/usr/share/freeswitch/conf/vanilla/autoload_configs/event_socket.conf.xml,readonly" \
    -d "$FS_IMAGE" >/dev/null
wait_for_freeswitch
FS_CONTAINER_IP="$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$FS_CONTAINER")"
FS_HOST_IP="$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.Gateway}}{{end}}' "$FS_CONTAINER")"
[[ -n "$FS_CONTAINER_IP" && -n "$FS_HOST_IP" ]] || fail "could not resolve isolated Docker network addresses"

run_case 1 123456 GRANTED
run_case 2 123450 REJECTED

printf '\nPASS: real FreeSWITCH SIP/RTP PIN gate completed both branches.\n'
printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
