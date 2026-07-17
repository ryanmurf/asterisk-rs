#!/usr/bin/env bash
# in-container-proof.sh — the M8a proof, executed INSIDE the isolated container
# netns (--privileged --network none). Synthetic ports only; never 45070.
#
# Topology (models Chime -> tron over a real interface, so packets traverse the
# PREROUTING + INPUT hooks exactly like the live trunk — NOT loopback):
#
#     netns "chime"  (the untrusted internet / AWS side)         netns: container main ("tron")
#       veth1: 10.9.0.2 (trusted src)                              veth0: 10.9.0.1  fd00::1
#              10.9.0.3 (untrusted v4)   <==== veth pair ====>     FS-standin / rustisk-standin
#              fd00::2  fd00::3 (untrusted v6)                     nft table `cutover` (filter only)
#
# Proves the LISTENER SWAP:
#   step 1  prime one fixed UDP five-tuple; keep NUMBERED datagrams flowing
#   step 2  switch FS-standin -> rustisk-standin: single clean delivery boundary
#   step 3  rollback rustisk-standin -> FS-standin under the SAME flow
#   step 4  untrusted-source DROP (v4 AND v6) holds throughout both transitions
#   step 5  measure the handover window in both directions
# then runs the RED control (stateful redirect lever) which FAILS the proof
# where the listener swap passes, and a detector self-test.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "${HERE}/cutover_lib.sh"

# ---- synthetic parameters (NOT the real trunk ports 45070/20000-20100) ----
export PORT=55070                 # synthetic analog of hostIP:45070
export NFT_TABLE=cutover
export UNTRUSTED_V4=10.9.0.3
export UNTRUSTED_V6=fd00::3
TRON_V4=10.9.0.1 ; TRON_V6=fd00::1
TRUST_SRC=10.9.0.2 ; TRUST_SPORT=41002
EVIL4_SRC=10.9.0.3 ; EVIL4_SPORT=41003
EVIL6_SRC=fd00::3  ; EVIL6_SPORT=41006
RATE_MS=2

RUN=/m8a/run ; rm -rf "${RUN}" ; mkdir -p "${RUN}"
FS_OUT="${RUN}/fs.csv"        ; : > "${FS_OUT}"
RU_OUT="${RUN}/rustisk.csv"   ; : > "${RU_OUT}"
FS_ST="${RUN}/fs.status"      ; : > "${FS_ST}"
RU_ST="${RUN}/rustisk.status" ; : > "${RU_ST}"

pids=()
cleanup() {
  for p in "${pids[@]:-}"; do kill "${p}" 2>/dev/null || true; done
  cutover_table_down
  ip netns del chime 2>/dev/null || true
}
trap cleanup EXIT

nsx() { ip netns exec chime "$@"; }        # run a command on the "chime" side

wait_status() { # <statusfile> <STATE> <timeout_ms>
  local f="$1" want="$2" to="$3" waited=0
  while :; do
    local last; last="$(awk 'NF{l=$1} END{print l}' "$f" 2>/dev/null || true)"
    [ "${last}" = "${want}" ] && return 0
    sleep 0.001; waited=$((waited+1))
    [ "${waited}" -ge "${to}" ] && { echo "TIMEOUT waiting ${want} in ${f}" >&2; return 1; }
  done
}
export -f wait_status

# ---------------- network setup inside the private netns ----------------
setup_net() {
  ip link set lo up
  ip netns add chime
  ip link add veth0 type veth peer name veth1
  ip link set veth1 netns chime
  ip addr add ${TRON_V4}/24 dev veth0
  ip -6 addr add ${TRON_V6}/64 dev veth0 nodad
  ip link set veth0 up
  nsx ip addr add 10.9.0.2/24 dev veth1
  nsx ip addr add 10.9.0.3/24 dev veth1
  nsx ip -6 addr add fd00::2/64 dev veth1 nodad
  nsx ip -6 addr add fd00::3/64 dev veth1 nodad
  nsx ip link set veth1 up
  nsx ip link set lo up
}

echo "=================== M8a LISTENER-SWAP PROOF ==================="
echo "synthetic dport=${PORT} (NOT 45070)  rate=${RATE_MS}ms  ingress via veth (prerouting+input)"
setup_net

# ---------- ISOLATION assertions (before any traffic) ----------
echo "--- ISOLATION (must all hold before traffic) ---"
IFACES=$(ip -o link show | awk -F': ' '{print $2}' | cut -d'@' -f1 | sort | tr '\n' ',' )
echo "main-netns links: ${IFACES}"
if ip route get 192.168.0.109 >/dev/null 2>&1; then echo "  WARN: host LAN routable"; else echo "  host LAN 192.168.0.109: NO ROUTE (isolated) OK"; fi
echo "  nft ruleset before setup:"; nft list ruleset | sed 's/^/    /' | head

# ---------- the cutover table + PERSISTENT source drops (v4+v6) ----------
cutover_table_up
echo "--- cutover table (filter only; no nat/ct/redirect) ---"
nft list table inet "${NFT_TABLE}" | sed 's/^/  /'

# ---------- listeners (bind on command; exactly one holds the port) ----------
python3 "${HERE}/listener.py" --port "${PORT}" --label FS      --out "${FS_OUT}" --status "${FS_ST}" --pidfile "${RUN}/fs.pid" &
pids+=($!)
python3 "${HERE}/listener.py" --port "${PORT}" --label RUSTISK --out "${RU_OUT}" --status "${RU_ST}" --pidfile "${RUN}/ru.pid" &
pids+=($!)
sleep 0.5
FS_PID="$(cat "${RUN}/fs.pid")" ; RU_PID="$(cat "${RUN}/ru.pid")"

# ---------- step 1: prime the fixed five-tuple; keep numbered flow hot ----------
kill -USR1 "${FS_PID}" ; wait_status "${FS_ST}" BOUND 3000
nsx python3 "${HERE}/sender.py" --src-ip "${TRUST_SRC}" --src-port "${TRUST_SPORT}" --dst-ip "${TRON_V4}" --dst-port "${PORT}" --tag TRUST  --rate-ms "${RATE_MS}" --family 4 &
pids+=($!)
nsx python3 "${HERE}/sender.py" --src-ip "${EVIL4_SRC}" --src-port "${EVIL4_SPORT}" --dst-ip "${TRON_V4}" --dst-port "${PORT}" --tag EVILV4 --rate-ms "${RATE_MS}" --family 4 &
pids+=($!)
nsx python3 "${HERE}/sender.py" --src-ip "${EVIL6_SRC}" --src-port "${EVIL6_SPORT}" --dst-ip "${TRON_V6}" --dst-port "${PORT}" --tag EVILV6 --rate-ms "${RATE_MS}" --family 6 &
pids+=($!)
echo "step 1: primed TRUST(v4) five-tuple ${TRUST_SRC}:${TRUST_SPORT} -> ${TRON_V4}:${PORT}"
echo "        untrusted EVILV4(${EVIL4_SRC}) + EVILV6(${EVIL6_SRC}) flooding continuously"
sleep 1.5

# ---------- step 2: switch FS -> rustisk ----------
echo "step 2: APPLY switch FS -> rustisk"
STOP_OLD_CMD="kill -USR2 ${FS_PID}" \
WAIT_RELEASED_CMD="wait_status ${FS_ST} UNBOUND 3000" \
START_NEW_CMD="kill -USR1 ${RU_PID}" \
WAIT_BOUND_CMD="wait_status ${RU_ST} BOUND 3000" \
WINDOW_OUT="${RUN}/apply_window.ns" \
  bash "${HERE}/apply-fs-to-rustisk.sh"
sleep 1.5

# ---------- step 3: rollback rustisk -> FS under the SAME flow ----------
echo "step 3: ROLLBACK rustisk -> FS (same continuously-flowing tuple)"
STOP_OLD_CMD="kill -USR2 ${RU_PID}" \
WAIT_RELEASED_CMD="wait_status ${RU_ST} UNBOUND 3000" \
START_NEW_CMD="kill -USR1 ${FS_PID}" \
WAIT_BOUND_CMD="wait_status ${FS_ST} BOUND 3000" \
WINDOW_OUT="${RUN}/rollback_window.ns" \
  bash "${HERE}/rollback-rustisk-to-fs.sh"
sleep 1.5

# ---------- stop the flow, flush captures ----------
for p in "${pids[@]}"; do kill "${p}" 2>/dev/null || true; done
sleep 0.3

# ---------- steps 2/3/4 assertions (receiver-side) ----------
echo "--------------- RECEIVER-SIDE ASSERTIONS (listener swap) ---------------"
python3 "${HERE}/assert_boundary.py" --fs "${FS_OUT}" --rustisk "${RU_OUT}" \
    --trust-tag TRUST --untrusted-tags EVILV4 EVILV6 --expect-runs FS,RUSTISK,FS
SWAP_RC=$?

# ---------- step 5: report the measured windows ----------
aw=$(cat "${RUN}/apply_window.ns" 2>/dev/null || echo 0)
rw=$(cat "${RUN}/rollback_window.ns" 2>/dev/null || echo 0)
echo "--------------- MEASURED HANDOVER WINDOWS (mechanism floor) ---------------"
awk -v a="$aw" 'BEGIN{printf "FS->rustisk apply    window: %.1f ms\n", a/1e6}'
awk -v r="$rw" 'BEGIN{printf "rustisk->FS rollback window: %.1f ms\n", r/1e6}'
echo "(synthetic floor = nft toggle + socket close/bind + IPC; the LIVE M9 window"
echo " additionally includes FS 'sofia profile stop/start' + DNS re-resolve.)"

# ---------- RED control + detector self-test ----------
echo
bash "${HERE}/red-stateful-variant.sh"; RED_RC=$?
echo
bash "${HERE}/detector-selftest.sh"; DET_RC=$?

echo
echo "=================== SUMMARY ==================="
echo "listener-swap assert  rc=${SWAP_RC}  (0 = clean boundaries + source-drop hold)"
echo "RED control (redirect) rc=${RED_RC}  (0 = redirect FAILED the proof as expected -> teeth)"
echo "detector self-test     rc=${DET_RC}  (0 = assert_boundary rejects every crafted bad capture)"
if [ "${SWAP_RC}" -eq 0 ] && [ "${RED_RC}" -eq 0 ] && [ "${DET_RC}" -eq 0 ]; then
  echo "M8a RESULT: PASS"
  exit 0
else
  echo "M8a RESULT: FAIL"
  exit 1
fi
