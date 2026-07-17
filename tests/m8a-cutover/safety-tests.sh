#!/usr/bin/env bash
# safety-tests.sh — M8a artifact-safety regression tests (task #40 fixes).
#
# Runs ENTIRELY INSIDE the isolated container netns (--privileged --network none):
# only `lo`, no route to the host, its OWN empty nftables ruleset. nft-only,
# synthetic high ports (55071/55072) — NEVER 45070, NEVER RTP 20000-20100, NEVER
# the host firewall / live voicefw / live stack. Driven by run-safety-tests.sh.
#
# Three red-capable tests. Each proves BOTH the bug (faithful pre-fix behavior =>
# RED reproduced) AND the fix (current cutover_lib.sh / apply artifact => GREEN):
#
#   T1  cutover_table_down REFUSES to delete a table it did NOT create.
#       RED : the original unconditional `nft delete table inet voicefw` WIPES a
#             stand-in 'voicefw' firewall it never created.
#       GREEN: the guarded cutover_table_down REFUSES (protected-name denylist +
#             ownership-marker), and the stand-in table SURVIVES. Also proves the
#             ownership gate on a non-denylisted foreign table, and that a table
#             this tool DID create still deletes.
#
#   T2  a mid-sequence apply failure leaves NO lingering handover drop.
#       RED : the pre-fix sequence (drop_on -> STOP fails -> abort, no trap)
#             leaves the blanket handover drop STUCK ON (phone outage).
#       GREEN: the real apply-fs-to-rustisk.sh with a failing STOP_OLD_CMD aborts
#             non-zero, and its EXIT trap clears ALL handover drops (0 left).
#
#   T3  double drop_on then one drop_off leaves ZERO handover rules.
#       RED : pre-fix non-idempotent drop_on (2 calls => 2 rules) + head -n1
#             drop_off (removes ONE) => 1 blanket drop STUCK.
#       GREEN: idempotent drop_on (2 calls => 1 rule) + drop_off removes ALL => 0.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# cutover_lib.sh sets `-e`; we deliberately run commands expected to FAIL (the
# guard returning non-zero, a failing apply), so drop `-e` and drive rc by hand.
source "${HERE}/cutover_lib.sh"
set +e

WORK=/m8a/safety-run ; rm -rf "${WORK}" ; mkdir -p "${WORK}"
PASS=0 ; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }
count_handover() { nft -a list chain inet "$1" input 2>/dev/null | grep -c 'comment "handover"'; }

echo "=================== M8a ARTIFACT-SAFETY TESTS (isolated netns) ==================="
# ---------- ISOLATION assertions (before any nft op) ----------
echo "--- ISOLATION (must all hold before any nft op) ---"
ip link set lo up 2>/dev/null || true
IFACES=$(ip -o link show | awk -F': ' '{print $2}' | cut -d'@' -f1 | sort | tr '\n' ',')
echo "  main-netns links: ${IFACES}"
if ip route get 192.168.0.109 >/dev/null 2>&1; then
  echo "  REFUSING: host LAN 192.168.0.109 is routable — NOT isolated" >&2 ; exit 3
fi
echo "  host LAN 192.168.0.109: NO ROUTE (isolated) OK"
if [ -n "$(nft list tables 2>/dev/null)" ]; then
  echo "  REFUSING: pre-existing nft tables present — not a clean private ruleset" >&2
  nft list tables >&2 ; exit 3
fi
echo "  nft ruleset before tests: EMPTY OK"
echo "  synthetic ports only: 55071 / 55072 (NEVER 45070 / RTP 20000-20100)"

# ============================================================================
echo
echo "=== T1: cutover_table_down refuses to delete a table it did NOT create ==="
# A stand-in 'voicefw' firewall built INSIDE this private netns — its own nft
# ruleset, unrelated to the host's. Same shape as the live firewall: input chain
# (priority -10, policy accept) + a source-allowlist drop. This tool did NOT
# create it (no ownership marker), and the name is on the protected denylist.
make_standin_voicefw() {
  nft add table inet voicefw
  nft "add chain inet voicefw input { type filter hook input priority -10 ; policy accept ; }"
  nft add rule inet voicefw input meta l4proto udp udp dport 55071 ip saddr 10.9.0.3 drop comment '"allowlist-standin"'
}

# --- RED (pre-fix): the ORIGINAL unconditional cutover_table_down ---
old_cutover_table_down() { nft delete table inet "${NFT_TABLE}" 2>/dev/null || true; }
make_standin_voicefw
NFT_TABLE=voicefw old_cutover_table_down
if nft list table inet voicefw >/dev/null 2>&1; then
  bad "RED not reproduced: pre-fix cutover_table_down should have DELETED foreign voicefw"
else
  echo "  RED (pre-fix): unconditional 'nft delete table inet voicefw' WIPED the foreign firewall (the footgun)"
fi

# --- GREEN (fixed): guarded cutover_table_down must REFUSE, table survives ---
make_standin_voicefw
NFT_TABLE=voicefw cutover_table_down 2>"${WORK}/t1a.err"
rc=$?
if [ "${rc}" -ne 0 ] && nft list table inet voicefw >/dev/null 2>&1; then
  ok "T1a guarded cutover_table_down REFUSED foreign 'voicefw' (rc=${rc}); table SURVIVED"
  echo "       $(head -1 "${WORK}/t1a.err")"
else
  bad "T1a expected refuse+survive for voicefw; rc=${rc}, survived=$(nft list table inet voicefw >/dev/null 2>&1 && echo yes || echo no)"
fi

# --- GREEN: ownership gate on a NON-denylisted foreign table (proves the marker,
#     not just the name list) ---
nft add table inet foreign_tbl
nft "add chain inet foreign_tbl input { type filter hook input priority -10 ; policy accept ; }"
NFT_TABLE=foreign_tbl cutover_table_down 2>"${WORK}/t1b.err"
rc=$?
if [ "${rc}" -ne 0 ] && nft list table inet foreign_tbl >/dev/null 2>&1; then
  ok "T1b REFUSED a non-denylisted foreign table lacking the owner marker (rc=${rc}); survived"
  echo "       $(head -1 "${WORK}/t1b.err")"
else
  bad "T1b expected refuse+survive for foreign_tbl; rc=${rc}"
fi

# --- GREEN: positive control — a table THIS tool created IS deletable ---
PORT=55071 NFT_TABLE=owned_cutover cutover_table_up
NFT_TABLE=owned_cutover cutover_table_down
rc=$?
if [ "${rc}" -eq 0 ] && ! nft list table inet owned_cutover >/dev/null 2>&1; then
  ok "T1c cutover_table_down DELETED a table it created (owner marker present, rc=0)"
else
  bad "T1c expected clean delete of owned table; rc=${rc}"
fi
nft delete table inet voicefw 2>/dev/null
nft delete table inet foreign_tbl 2>/dev/null

# ============================================================================
echo
echo "=== T2: mid-sequence apply failure leaves NO lingering handover drop (EXIT trap) ==="
TT=cutover_trap_test
PORT=55072 NFT_TABLE="${TT}" cutover_table_up

# --- RED (pre-fix): drop_on -> STOP fails -> set -e aborts BEFORE drop_off, no
#     trap => the blanket handover drop is left STUCK ON. Reproduced in a set -e
#     subshell so the parent test survives the abort. ---
(
  set -e
  export NFT_TABLE="${TT}" PORT=55072
  handover_drop_on     # drop goes ON
  false                # STOP_OLD_CMD fails mid-sequence
  handover_drop_off    # NEVER reached (no trap) => drop stuck ON
) 2>/dev/null
red_left=$(count_handover "${TT}")
if [ "${red_left}" -ge 1 ]; then
  echo "  RED (pre-fix): mid-sequence failure with NO trap left ${red_left} handover drop(s) STUCK ON (outage)"
else
  bad "RED not reproduced: pre-fix should have left the handover drop stuck ON (left=${red_left})"
fi
NFT_TABLE="${TT}" handover_drop_off   # clear the stuck drop before GREEN

# --- GREEN (fixed): the REAL apply artifact with a failing STOP_OLD_CMD. Its
#     EXIT trap must clear ALL handover drops even though the sequence aborts. ---
NFT_TABLE="${TT}" PORT=55072 \
STOP_OLD_CMD="false" START_NEW_CMD="true" \
WAIT_RELEASED_CMD="true" WAIT_BOUND_CMD="true" \
  bash "${HERE}/apply-fs-to-rustisk.sh" >"${WORK}/t2.out" 2>&1
rc=$?
green_left=$(count_handover "${TT}")
if [ "${rc}" -ne 0 ] && [ "${green_left}" -eq 0 ]; then
  ok "T2 apply FAILED mid-sequence (rc=${rc}) yet EXIT trap cleared ALL handover drops (0 left)"
else
  bad "T2 expected failing apply (rc!=0) + 0 lingering drops; rc=${rc} left=${green_left}"
fi
NFT_TABLE="${TT}" cutover_table_down

# ============================================================================
echo
echo "=== T3: double drop_on + one drop_off leaves ZERO handover rules ==="
T3=cutover_idem_test
PORT=55071 NFT_TABLE="${T3}" cutover_table_up

# --- RED (pre-fix): non-idempotent drop_on (plain insert) + head -n1 drop_off ---
old_drop_on()  { nft insert rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" drop comment '"handover"'; }
old_drop_off() {
  local h
  h="$(nft -a list chain inet "${NFT_TABLE}" input 2>/dev/null \
        | awk '/comment "handover"/ {for(i=1;i<=NF;i++) if($i=="handle") print $(i+1)}' \
        | head -n1)"
  [ -n "${h}" ] && nft delete rule inet "${NFT_TABLE}" input handle "${h}"
}
NFT_TABLE="${T3}" PORT=55071 old_drop_on
NFT_TABLE="${T3}" PORT=55071 old_drop_on          # second call STACKS a duplicate
red_on=$(count_handover "${T3}")
NFT_TABLE="${T3}" old_drop_off                    # head -n1 removes only ONE
red_off=$(count_handover "${T3}")
if [ "${red_on}" -eq 2 ] && [ "${red_off}" -eq 1 ]; then
  echo "  RED (pre-fix): 2x old_drop_on => ${red_on} rules; one old_drop_off => ${red_off} left (blanket drop STUCK)"
else
  bad "RED not reproduced: expected on=2/off=1, got on=${red_on}/off=${red_off}"
fi
NFT_TABLE="${T3}" old_drop_off ; NFT_TABLE="${T3}" old_drop_off   # clean leftover

# --- GREEN (fixed): idempotent drop_on + drop_off removes ALL ---
NFT_TABLE="${T3}" PORT=55071 handover_drop_on
NFT_TABLE="${T3}" PORT=55071 handover_drop_on     # no-op (idempotent)
green_on=$(count_handover "${T3}")
NFT_TABLE="${T3}" handover_drop_off
green_off=$(count_handover "${T3}")
if [ "${green_on}" -eq 1 ] && [ "${green_off}" -eq 0 ]; then
  ok "T3 idempotent: 2x drop_on => ${green_on} rule; one drop_off => ${green_off} left"
else
  bad "T3 expected on=1/off=0, got on=${green_on}/off=${green_off}"
fi
NFT_TABLE="${T3}" cutover_table_down

# ============================================================================
echo
echo "=================== SUMMARY ==================="
echo "PASS=${PASS} FAIL=${FAIL}  (expect 5 GREEN asserts, 0 fail; each RED reproduced above)"
# Leave the container's ruleset clean.
for t in voicefw foreign_tbl owned_cutover "${TT}" "${T3}"; do
  nft delete table inet "$t" 2>/dev/null
done
if [ "${FAIL}" -eq 0 ] && [ "${PASS}" -ge 5 ]; then
  echo "M8a SAFETY TESTS: PASS"
  exit 0
else
  echo "M8a SAFETY TESTS: FAIL"
  exit 1
fi
