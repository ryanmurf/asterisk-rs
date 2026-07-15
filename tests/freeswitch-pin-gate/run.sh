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
FS_NETWORK="rustisk-fs-pin-gate-net-$$"
NETWORK_THIRD_OCTET="$((20 + ($$ % 200)))"
FS_SUBNET="10.253.$NETWORK_THIRD_OCTET.0/24"
FS_CONTAINER_IP="10.253.$NETWORK_THIRD_OCTET.2"
FS_HOST_IP="10.253.$NETWORK_THIRD_OCTET.3"
AMI_HOST="$FS_HOST_IP"
RUSTISK_PID=""
BASELINE_RESOURCES=""
BASELINE_TRANSACTIONS=""

cleanup() {
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
    response="$(ami_action $'Action: CoreStatus\r\n\r\n')"
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
    response="$(ami_action $'Action: CoreStatus\r\n\r\n')"
    for field in "${fields[@]}"; do
        local value
        value="$(stat_value "$field" "$response")"
        [[ "$value" =~ ^[0-9]+$ ]] || fail "$field missing from CoreStatus: $response"
        values+=("$value")
    done
    (IFS=/; printf '%s\n' "${values[*]}")
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
        sleep 0.05
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
        sleep 0.05
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

sed \
    -e "s|@PROMPT_DIR@|$PROMPT_DIR|g" \
    -e "s|@FS_CONTAINER_IP@|$FS_CONTAINER_IP|g" \
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

printf '\nPASS: real FreeSWITCH SIP/RTP gate completed PIN and M1 outbound acceptance.\n'
printf 'Proof artifacts: %s\n' "$RUNTIME_DIR"
