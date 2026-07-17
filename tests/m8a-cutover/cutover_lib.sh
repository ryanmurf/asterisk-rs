#!/usr/bin/env bash
# cutover_lib.sh — the nftables primitives the listener-swap cutover uses.
#
# This is the ONLY host-networking surface of the mechanism. It is deliberately
# tiny and it is a FILTER-ONLY table: no `nat`, no `ct`, no `redirect`, no
# `dnat` — so the conntrack objection that kills the redirect design is
# structurally absent (PLAN-v3 C1 / M8a rationale point 1).
#
# Safety, mirroring local-networking/nftables/voice-trunk.nft:
#   * We create our OWN table (`inet ${NFT_TABLE}`), default `policy accept`,
#     and touch nothing else. NEVER `flush ruleset` — the whole table is
#     dropped as a unit and is independently deletable.
#   * The only drops are the ones we NAME: the persistent untrusted-source drops
#     (v4 + v6) and the transient handover drop. A targeted drop on one dport
#     cannot brick the host (M8a runs it in a private netns regardless).
#
# In M9 this runs on the host against PORT=45070; in the M8a proof the harness
# points PORT at a synthetic high port inside an isolated container netns.
#
# Env:
#   NFT_TABLE      nft table name              (default: cutover)
#   PORT           the guarded UDP dport       (required)
#   UNTRUSTED_V4   untrusted v4 source to DROP (optional, for the proof)
#   UNTRUSTED_V6   untrusted v6 source to DROP (optional, for the proof)
#
# M9 MUST use the default, SEPARATE `cutover` table — it coexists with the live
# `voicefw` firewall (both input-hook priority -10, both policy accept). NEVER
# point NFT_TABLE at `voicefw` (or any other pre-existing host table): the live
# allowlist + RTP/SIP drops are not ours to delete. `cutover_table_down` now
# REFUSES any table it did not create (see the guard below), but the correct
# operating discipline is to never aim the tool at the live firewall in the
# first place.
set -euo pipefail

NFT_TABLE="${NFT_TABLE:-cutover}"

# Tables this tool must NEVER delete, even if asked — the live host firewall and
# common system tables. `cutover_table_down` refuses these outright (defence in
# depth on top of the ownership marker below). Space-separated; overridable.
CUTOVER_PROTECTED_TABLES="${CUTOVER_PROTECTED_TABLES:-voicefw filter nat mangle raw security}"

# Ownership sentinel: a regular (never-hooked) chain that `cutover_table_up`
# installs so `cutover_table_down` can prove THIS tool created the table. A table
# lacking it is foreign and will not be deleted. Never carries packets — regular
# chains only run when jumped to, and nothing jumps to this one.
CUTOVER_OWNER_CHAIN="m8a_cutover_owner"

_nft() { nft "$@"; }

# Create the cutover table + input chain, and install the PERSISTENT
# fail-closed source drops (v4 and v6) that hold across BOTH transitions.
# policy accept: trusted traffic falls through; only named sources are dropped.
cutover_table_up() {
  : "${PORT:?PORT required}"
  _nft add table inet "${NFT_TABLE}"
  # Ownership sentinel first: a plain regular chain (never hooked, never jumped
  # to). Its presence is how cutover_table_down proves this table is ours.
  _nft "add chain inet ${NFT_TABLE} ${CUTOVER_OWNER_CHAIN}"
  _nft "add chain inet ${NFT_TABLE} input { type filter hook input priority -10 ; policy accept ; }"
  if [ -n "${UNTRUSTED_V4:-}" ]; then
    _nft add rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" ip  saddr "${UNTRUSTED_V4}" drop comment '"srcdrop-v4"'
  fi
  if [ -n "${UNTRUSTED_V6:-}" ]; then
    _nft add rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" ip6 saddr "${UNTRUSTED_V6}" drop comment '"srcdrop-v6"'
  fi
}

# Drop the whole table (independently deletable; never `flush ruleset`).
#
# GUARDED: only ever deletes a table THIS tool created. A stray NFT_TABLE=voicefw
# (or any pre-existing host table) would otherwise wipe the live firewall — the
# trusted-source allowlist, the SIP 45070 drop, the RTP 20000-20100 drop. Two
# independent gates must both pass before the delete:
#   (1) name is not on the protected/system denylist (voicefw, filter, nat, ...);
#   (2) the table carries our ownership sentinel chain (cutover_table_up put it
#       there) — a foreign table won't have it.
# Refuses (returns non-zero) rather than deleting when either gate fails. A
# missing table is a no-op success.
cutover_table_down() {
  local t="${NFT_TABLE}" p
  # (1) hard refusal: never delete a known host/system table by name.
  for p in ${CUTOVER_PROTECTED_TABLES}; do
    if [ "${t}" = "${p}" ]; then
      echo "REFUSING: 'inet ${t}' is a protected host table; cutover_table_down will not delete it." >&2
      return 3
    fi
  done
  # (2) nothing to delete if the table doesn't exist.
  if ! nft list table inet "${t}" >/dev/null 2>&1; then
    return 0
  fi
  # (3) ownership check: refuse a table that lacks our sentinel chain — it was
  #     not created by this tool and is not ours to delete.
  if ! nft list table inet "${t}" 2>/dev/null \
        | grep -Eq "chain[[:space:]]+${CUTOVER_OWNER_CHAIN}([[:space:]{]|$)"; then
    echo "REFUSING: table 'inet ${t}' exists but lacks the m8a-cutover ownership marker (chain ${CUTOVER_OWNER_CHAIN}); not ours to delete." >&2
    return 3
  fi
  # (4) safe: our table. Drop it by name (never `flush ruleset`).
  _nft delete table inet "${t}"
}

# Enable the transient fail-closed handover drop on the guarded dport.
# Inserted at the head so it is evaluated first. While it is present the sender
# sees plain LOSS (SIP retransmission covers it), NOT ICMP port-unreachable —
# which is the whole reason the port must never be left merely unbound.
#
# IDEMPOTENT: a second call is a no-op. Two calls must never stack two rules, or
# a single handover_drop_off could leave a lingering blanket drop behind.
handover_drop_on() {
  : "${PORT:?PORT required}"
  if _handover_handles | grep -q .; then
    return 0
  fi
  _nft insert rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" drop comment '"handover"'
}

# Emit the handle of EVERY handover rule currently in the chain (one per line).
_handover_handles() {
  nft -a list chain inet "${NFT_TABLE}" input 2>/dev/null \
    | awk '/comment "handover"/ {for(i=1;i<=NF;i++) if($i=="handle") print $(i+1)}'
}

# Disable the handover drop. Removes ALL handover rules (not just the first) so a
# doubled drop_on, or a failed apply that left one stuck, cannot leave a blanket
# drop behind. nft handles are stable within a generation, so one collected pass
# deletes them all.
handover_drop_off() {
  local h
  for h in $(_handover_handles); do
    _nft delete rule inet "${NFT_TABLE}" input handle "${h}" 2>/dev/null || true
  done
}

# Allow being sourced OR invoked as `cutover_lib.sh <fn>`.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  cmd="${1:?usage: cutover_lib.sh <up|down|drop_on|drop_off>}"
  case "${cmd}" in
    up)       cutover_table_up ;;
    down)     cutover_table_down ;;
    drop_on)  handover_drop_on ;;
    drop_off) handover_drop_off ;;
    *) echo "unknown: ${cmd}" >&2; exit 2 ;;
  esac
fi
