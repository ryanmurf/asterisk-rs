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
set -euo pipefail

NFT_TABLE="${NFT_TABLE:-cutover}"

_nft() { nft "$@"; }

# Create the cutover table + input chain, and install the PERSISTENT
# fail-closed source drops (v4 and v6) that hold across BOTH transitions.
# policy accept: trusted traffic falls through; only named sources are dropped.
cutover_table_up() {
  : "${PORT:?PORT required}"
  _nft add table inet "${NFT_TABLE}"
  _nft "add chain inet ${NFT_TABLE} input { type filter hook input priority -10 ; policy accept ; }"
  if [ -n "${UNTRUSTED_V4:-}" ]; then
    _nft add rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" ip  saddr "${UNTRUSTED_V4}" drop comment '"srcdrop-v4"'
  fi
  if [ -n "${UNTRUSTED_V6:-}" ]; then
    _nft add rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" ip6 saddr "${UNTRUSTED_V6}" drop comment '"srcdrop-v6"'
  fi
}

# Drop the whole table (independently deletable; never `flush ruleset`).
cutover_table_down() {
  _nft delete table inet "${NFT_TABLE}" 2>/dev/null || true
}

# Enable the transient fail-closed handover drop on the guarded dport.
# Inserted at the head so it is evaluated first. While it is present the sender
# sees plain LOSS (SIP retransmission covers it), NOT ICMP port-unreachable —
# which is the whole reason the port must never be left merely unbound.
handover_drop_on() {
  : "${PORT:?PORT required}"
  _nft insert rule inet "${NFT_TABLE}" input meta l4proto udp udp dport "${PORT}" drop comment '"handover"'
}

# Disable the handover drop (delete by comment handle).
handover_drop_off() {
  local h
  h="$(nft -a list chain inet "${NFT_TABLE}" input 2>/dev/null \
        | awk '/comment "handover"/ {for(i=1;i<=NF;i++) if($i=="handle") print $(i+1)}' \
        | head -n1)"
  if [ -n "${h}" ]; then
    _nft delete rule inet "${NFT_TABLE}" input handle "${h}"
  fi
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
