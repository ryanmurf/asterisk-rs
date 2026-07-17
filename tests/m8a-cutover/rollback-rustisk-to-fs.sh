#!/usr/bin/env bash
# rollback-rustisk-to-fs.sh — THE ROLLBACK ARTIFACT M9 invokes (rustisk -> FS).
#
# The exact mirror of the apply. Because the mechanism holds NO conntrack/NAT
# state, the rollback is symmetric and works under the SAME primed, continuously
# flowing tuple — this is the step a stateful/redirect lever cannot pass
# (deleting nft rules does not delete conntrack entries; continuing traffic
# refreshes them). See red-stateful-variant.sh for the RED contrast.
#
# Sequence:
#   1. handover_drop_on
#   2. STOP_OLD_CMD       — release the port from rustisk.
#                           M9: stop rustisk's transport.
#   3. WAIT_RELEASED_CMD
#   4. START_NEW_CMD      — rebind the port on FS. This is the trunk watchdog's
#                           already-exercised recovery:
#                           M9: fs_cli -x 'sofia profile <trunk> start'
#   5. WAIT_BOUND_CMD
#   6. handover_drop_off
#
# Env: same shape as apply-fs-to-rustisk.sh (STOP_OLD_CMD now stops rustisk,
#      START_NEW_CMD now starts FS).
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
echo "ROLLBACK rustisk->FS handover window: ${win_ms} ms (${t0} -> ${t1} ns)"
if [ -n "${WINDOW_OUT:-}" ]; then echo "$((t1 - t0))" > "${WINDOW_OUT}"; fi
