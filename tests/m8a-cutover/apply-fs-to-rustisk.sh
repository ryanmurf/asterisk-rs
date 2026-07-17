#!/usr/bin/env bash
# apply-fs-to-rustisk.sh — THE APPLY ARTIFACT M9 invokes (FS -> rustisk).
#
# Listener swap, no NAT. Sequence:
#   1. handover_drop_on   — fail-closed drop on the guarded dport, so the
#                           sender sees loss (SIP retransmits) not ICMP reject
#                           during the window the port is unbound.
#   2. STOP_OLD_CMD       — release the port from the incumbent (FS).
#                           M9: fs_cli -x 'sofia profile <trunk> stop'
#   3. WAIT_RELEASED_CMD  — block until the port is actually free.
#   4. START_NEW_CMD      — bind the port on the successor (rustisk).
#                           M9: start rustisk / claim hostIP:45070.
#   5. WAIT_BOUND_CMD     — block until the successor holds the port.
#   6. handover_drop_off  — reopen; traffic now lands on rustisk.
#
# The FS/rustisk specifics are injected as commands so THIS orchestration is
# identical in the M8a synthetic proof and in the M9 live cutover — only the
# STOP_OLD/START_NEW hooks differ. The measured wall-clock between step 1 and
# step 6 is the handover window reported as M9's go/no-go input.
#
# Env: PORT, NFT_TABLE (+ UNTRUSTED_V4/6 already installed by the caller),
#      STOP_OLD_CMD, WAIT_RELEASED_CMD, START_NEW_CMD, WAIT_BOUND_CMD,
#      WINDOW_OUT (file to write the measured window, ns).
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=cutover_lib.sh
source "${HERE}/cutover_lib.sh"

: "${STOP_OLD_CMD:?}" ; : "${START_NEW_CMD:?}"
: "${WAIT_RELEASED_CMD:=true}" ; : "${WAIT_BOUND_CMD:=true}"

t0="$(date +%s%N)"
handover_drop_on
eval "${STOP_OLD_CMD}"
eval "${WAIT_RELEASED_CMD}"
eval "${START_NEW_CMD}"
eval "${WAIT_BOUND_CMD}"
handover_drop_off
t1="$(date +%s%N)"

win_ms=$(( (t1 - t0) / 1000000 ))
echo "APPLY FS->rustisk handover window: ${win_ms} ms (${t0} -> ${t1} ns)"
if [ -n "${WINDOW_OUT:-}" ]; then echo "$((t1 - t0))" > "${WINDOW_OUT}"; fi
