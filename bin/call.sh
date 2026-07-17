#!/usr/bin/env bash
# Place an OUTBOUND call through rustisk and drop the callee into the agents'
# bridge — the AMI-native successor to the FreeSWITCH/ESL bin/call.sh (M-k).
#
#   bin/call.sh 2000                     # ring dest 2000 through the carrier
#   bin/call.sh 2000 --dry-run           # show the Originate, place NO call
#   bin/call.sh --hangup 2000            # hang up the call(s) to dest 2000
#   bin/call.sh --hangup-all --force     # GUARDED hupall: tear down EVERY channel
#
# vs the ESL original this is AMI-native (Action: Originate / CoreShowChannels /
# Hangup), reads its secret from a mounted k8s Secret file (never argv), refuses
# to stack a duplicate concurrent call to the same destination, and turns the old
# unconditional `hupall` foot-gun into an EXPLICIT, DOUBLY-GUARDED operation
# (--hangup-all AND --force both required).
#
# Config via env (all optional):
#   AMI_HOST (127.0.0.1)  AMI_PORT (5038)  AMI_USERNAME (operator)
#   AMI_SECRET_FILE (/run/secrets/rustisk-ami/secret)  — the mounted k8s Secret
#   CALL_ENDPOINT (carrier)  CALL_CONTEXT (default)  CALL_CALLERID ("")
#   CALL_TIMEOUT_MS (30000)
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
AMI_PY="$HERE/ami_call.py"

AMI_HOST="${AMI_HOST:-127.0.0.1}"
AMI_PORT="${AMI_PORT:-5038}"
AMI_USERNAME="${AMI_USERNAME:-operator}"
AMI_SECRET_FILE="${AMI_SECRET_FILE:-/run/secrets/rustisk-ami/secret}"
CALL_ENDPOINT="${CALL_ENDPOINT:-carrier}"
CALL_CONTEXT="${CALL_CONTEXT:-default}"
CALL_CALLERID="${CALL_CALLERID:-}"
CALL_TIMEOUT_MS="${CALL_TIMEOUT_MS:-30000}"

DEST=""
DRY=0
HANGUP=0
HANGUP_ALL=0
FORCE=0
for a in "$@"; do
  case "$a" in
    --dry-run)    DRY=1 ;;
    --hangup)     HANGUP=1 ;;
    --hangup-all) HANGUP_ALL=1 ;;
    --force)      FORCE=1 ;;
    --*)          echo "usage: $0 DEST [--dry-run] | --hangup DEST | --hangup-all --force" >&2; exit 2 ;;
    *)            DEST="$a" ;;
  esac
done

die() { echo "$*" >&2; exit 1; }

ami() { python3 "$AMI_PY" --host "$AMI_HOST" --port "$AMI_PORT" \
                          --username "$AMI_USERNAME" --secret-file "$AMI_SECRET_FILE" "$@"; }

[ -r "$AMI_SECRET_FILE" ] || die "AMI secret file not readable: $AMI_SECRET_FILE (mount the k8s Secret)"

channel_prefix="PJSIP/${CALL_ENDPOINT}-"

# --- --hangup-all: the GUARDED hupall (both --hangup-all AND --force) ---------
if [ "$HANGUP_ALL" = 1 ]; then
  if [ "$FORCE" != 1 ]; then
    echo "REFUSING to hang up EVERY channel." >&2
    echo "This is the old hupall foot-gun. Re-run with an explicit confirmation:" >&2
    echo "  $0 --hangup-all --force" >&2
    exit 3
  fi
  n=0
  while IFS=$'\t' read -r chan _exten _cid _state; do
    [ -n "$chan" ] || continue
    ami hangup --channel "$chan" >/dev/null || true
    echo "hung up $chan"
    n=$((n + 1))
  done < <(ami list)
  echo "hupall complete: $n channel(s) hung up"
  exit 0
fi

# --- --hangup DEST: tear down only the call(s) to DEST ------------------------
if [ "$HANGUP" = 1 ]; then
  [ -n "$DEST" ] || die "usage: $0 --hangup DEST   (to hupall EVERY channel use --hangup-all --force)"
  n=0
  while IFS=$'\t' read -r chan exten _cid _state; do
    [ -n "$chan" ] || continue
    if [ "$exten" = "$DEST" ] && [[ "$chan" == "$channel_prefix"* ]]; then
      ami hangup --channel "$chan" >/dev/null || true
      echo "hung up $chan (dest $DEST)"
      n=$((n + 1))
    fi
  done < <(ami list)
  if [ "$n" = 0 ]; then echo "no active call to $DEST"; fi
  exit 0
fi

# --- place a call ------------------------------------------------------------
[ -n "$DEST" ] || die "usage: $0 DEST [--dry-run] | --hangup DEST | --hangup-all --force"

CHANNEL="PJSIP/${CALL_ENDPOINT}"

# Duplicate-call guard: refuse to stack a second concurrent call to the same
# destination (matched on the active channel's Extension + endpoint prefix).
if ami list | awk -F'\t' -v d="$DEST" -v p="$channel_prefix" \
      '$2==d && index($1,p)==1 {found=1} END{exit found?0:1}'; then
  echo "a call to $DEST is already up -- refusing to place another"
  echo "(use '$0 --hangup $DEST' to clear it)"
  exit 0
fi

if [ "$DRY" = 1 ]; then
  echo "DRY RUN — would AMI Originate:"
  echo "  Channel:  $CHANNEL"
  echo "  Context:  $CALL_CONTEXT"
  echo "  Exten:    $DEST"
  echo "  CallerID: ${CALL_CALLERID:-<default>}"
  echo "  (no call placed)"
  exit 0
fi

echo ">> ringing $DEST via $CHANNEL (context $CALL_CONTEXT)"
if ami originate --channel "$CHANNEL" --context "$CALL_CONTEXT" --exten "$DEST" \
                 --priority 1 --callerid "$CALL_CALLERID" --timeout "$CALL_TIMEOUT_MS" >/dev/null; then
  echo ">> Originate queued for $DEST"
  echo ">> hang up with: $0 --hangup $DEST"
else
  die ">> Originate failed"
fi
