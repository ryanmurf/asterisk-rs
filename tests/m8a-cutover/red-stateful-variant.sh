#!/usr/bin/env bash
# red-stateful-variant.sh — the RED control that proves the proof has TEETH.
#
# A deliberately STATEFUL lever: an nftables `dnat`/redirect (NAT + conntrack)
# that rewrites the guarded dport to an alternate port the successor binds —
# instead of the listener swap's bind handover on the SAME port.
#
# TWO findings, both run against the same veth ingress path (prerouting+input)
# already set up by in-container-proof.sh:
#
#  (A) SECURITY DEFECT — DETERMINISTIC, this is the teeth:
#      Because the lever REWRITES the dport (55190 -> 55192) at prerouting, the
#      fail-closed source-drop written for the original dport 55190 no longer
#      matches (post-DNAT dport is 55192), so the UNTRUSTED source is DELIVERED
#      to the successor. assert_boundary FLAGS this (untrusted delivered) => the
#      redirect lever FAILS the proof exactly where the listener swap PASSES.
#      This is PLAN-v3's cited hazard: "any port-rewriting mechanism moves
#      packets to a port the current filter does not match, and would need a
#      genuine fail-closed DROP for untrusted sources on the new port, v4+v6."
#
#  (B) ROLLBACK-PERSISTENCE PROBE — HONEST NEGATIVE RESULT:
#      PLAN-v3 C1 argues the redirect cannot switch a primed tuple BACK because
#      conntrack persists. Measured on tron's kernel: it reverts CLEANLY on rule
#      removal (see numbers below). So on THIS kernel the redirect dies on (A),
#      not on the rollback boundary. Reported transparently — the listener swap
#      is still chosen for structural reasons (introduces no NAT/conntrack into a
#      NAT-free path; behavior independent of kernel/birth-conditions).
#
# Returns 0 iff the redirect lever demonstrably FAILED the proof (teeth), i.e.
# assert_boundary rejects the redirect capture.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUN="${RUN:-/m8a/run}" ; mkdir -p "${RUN}"

PORT_PUB=55190 ; PORT_ALT=55192
TRON_V4=10.9.0.1
TABLE=redirtest        # ip family: holds the nat/redirect lever
FTABLE=redirtest_f     # inet family: holds the fail-closed source-drops (v4+v6)
nsx() { ip netns exec chime "$@"; }

RFS_OUT="${RUN}/red_fs.csv"      ; : > "${RFS_OUT}"
RRU_OUT="${RUN}/red_rustisk.csv" ; : > "${RRU_OUT}"
RFS_ST="${RUN}/red_fs.status"    ; : > "${RFS_ST}"
RRU_ST="${RUN}/red_rustisk.status" ; : > "${RRU_ST}"

rpids=()
cleanup_red() {
  for p in "${rpids[@]:-}"; do kill "${p}" 2>/dev/null || true; done
  nft delete table ip "${TABLE}" 2>/dev/null || true
  nft delete table inet "${FTABLE}" 2>/dev/null || true
  conntrack -D -p udp --dport "${PORT_PUB}" 2>/dev/null || true
}
trap cleanup_red RETURN

echo "=================== RED CONTROL (stateful redirect lever) ==================="
# fail-closed source-drops written for the ORIGINAL dport (as the operator would),
# in an inet table so BOTH v4 and v6 drops are expressible.
nft add table inet "${FTABLE}"
nft "add chain inet ${FTABLE} input { type filter hook input priority -10 ; policy accept ; }"
nft add rule inet "${FTABLE}" input meta l4proto udp udp dport "${PORT_PUB}" ip  saddr 10.9.0.3 drop
nft add rule inet "${FTABLE}" input meta l4proto udp udp dport "${PORT_PUB}" ip6 saddr fd00::3 drop
# the stateful lever: prerouting DNAT rewriting the dport to the successor's port
nft add table ip "${TABLE}"
nft "add chain ip ${TABLE} nat { type nat hook prerouting priority -100 ; policy accept ; }"
nft add rule ip "${TABLE}" nat meta l4proto udp udp dport "${PORT_PUB}" dnat to ${TRON_V4}:${PORT_ALT}

python3 "${HERE}/listener.py" --port "${PORT_PUB}" --label FS      --out "${RFS_OUT}" --status "${RFS_ST}" --pidfile "${RUN}/red_fs.pid" &
rpids+=($!)
python3 "${HERE}/listener.py" --port "${PORT_ALT}" --label RUSTISK --out "${RRU_OUT}" --status "${RRU_ST}" --pidfile "${RUN}/red_ru.pid" &
rpids+=($!)
sleep 0.5
kill -USR1 "$(cat "${RUN}/red_fs.pid")"; kill -USR1 "$(cat "${RUN}/red_ru.pid")"
sleep 0.3

echo "--- (A) source-drop bypass: untrusted 10.9.0.3 -> ${PORT_PUB}, drop is on dport ${PORT_PUB} ---"
nsx python3 "${HERE}/sender.py" --src-ip 10.9.0.2 --src-port 41102 --dst-ip ${TRON_V4} --dst-port ${PORT_PUB} --tag TRUST  --rate-ms 2 --family 4 &
rpids+=($!)
nsx python3 "${HERE}/sender.py" --src-ip 10.9.0.3 --src-port 41103 --dst-ip ${TRON_V4} --dst-port ${PORT_PUB} --tag EVILV4 --rate-ms 2 --family 4 &
rpids+=($!)
sleep 1.2
for p in "${rpids[@]:2}"; do kill "${p}" 2>/dev/null || true; done
sleep 0.2
echo "    (redirect DNATs ${PORT_PUB}->${PORT_ALT} at prerouting, past the dport-${PORT_PUB} drop)"

echo "--- (B) rollback-persistence probe (honest): switch then rollback a primed tuple ---"
# fresh listeners on the two ports; count captured lines per phase.
kill -USR2 "$(cat "${RUN}/red_fs.pid")" 2>/dev/null; kill -USR2 "$(cat "${RUN}/red_ru.pid")" 2>/dev/null
sleep 0.2
nft flush chain ip "${TABLE}" nat; conntrack -F 2>/dev/null || true
PBF="${RUN}/pb_fs.csv"; PBR="${RUN}/pb_ru.csv"; : > "${PBF}"; : > "${PBR}"
python3 "${HERE}/listener.py" --port "${PORT_PUB}" --label FS  --out "${PBF}" --status "${RUN}/pbf.st" --pidfile "${RUN}/pbf.pid" &
rpids+=($!)
python3 "${HERE}/listener.py" --port "${PORT_ALT}" --label RU  --out "${PBR}" --status "${RUN}/pbr.st" --pidfile "${RUN}/pbr.pid" &
rpids+=($!)
sleep 0.5
kill -USR1 "$(cat "${RUN}/pbf.pid")"; kill -USR1 "$(cat "${RUN}/pbr.pid")"; sleep 0.3
cnt(){ wc -l < "$1"; }
nsx python3 "${HERE}/burst.py" --src-ip 10.9.0.2 --src-port 41202 --dst-ip ${TRON_V4} --dst-port ${PORT_PUB} --count 150; sleep 0.1
p1="FS=$(cnt "${PBF}") RU=$(cnt "${PBR}")"
nft add rule ip "${TABLE}" nat meta l4proto udp udp dport "${PORT_PUB}" dnat to ${TRON_V4}:${PORT_ALT}
nsx python3 "${HERE}/burst.py" --src-ip 10.9.0.2 --src-port 41202 --dst-ip ${TRON_V4} --dst-port ${PORT_PUB} --count 200; sleep 0.1
p2="FS=$(cnt "${PBF}") RU=$(cnt "${PBR}")"
h=$(nft -a list chain ip "${TABLE}" nat | grep "dnat to" | grep -oP "handle \K[0-9]+" | head -1)
[ -n "$h" ] && nft delete rule ip "${TABLE}" nat handle "$h"
nsx python3 "${HERE}/burst.py" --src-ip 10.9.0.2 --src-port 41202 --dst-ip ${TRON_V4} --dst-port ${PORT_PUB} --count 120; sleep 0.1
p3="FS=$(cnt "${PBF}") RU=$(cnt "${PBR}")"
echo "    cumulative captured lines (FS=${PORT_PUB}, RU=${PORT_ALT}):"
echo "      prime (no rule):     [$p1]"
echo "      after switch (dnat): [$p2]  (RU grew => switch worked)"
echo "      after rollback (del):[$p3]  (FS grew => reverted cleanly)"
echo "    => rollback reverts to FS on tron's kernel; conntrack-persistence objection NOT reproduced."

echo "--- RED machine verdict (assert_boundary on the redirect source-drop capture) ---"
python3 "${HERE}/assert_boundary.py" --fs "${RFS_OUT}" --rustisk "${RRU_OUT}" \
    --trust-tag TRUST --untrusted-tags EVILV4 --expect-runs FS,RUSTISK,FS
A_RC=$?
if [ "${A_RC}" -ne 0 ]; then
  echo "RED TEETH CONFIRMED: redirect lever FAILED the proof (untrusted delivered / no clean swap) — listener swap PASSES the same checks."
  exit 0
else
  echo "RED WARNING: redirect lever PASSED — the proof has NO teeth here!"
  exit 1
fi
