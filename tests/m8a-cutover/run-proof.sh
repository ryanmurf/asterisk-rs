#!/usr/bin/env bash
# run-proof.sh — top-level driver for the M8a cutover-mechanism proof.
#
# Builds the proof image and runs the ENTIRE proof inside ONE isolated container
# network namespace (--privileged --network none): the container has only `lo`
# plus an internal veth pair to a child netns, NO route to the host, and its OWN
# nftables ruleset. Nothing it does can touch the host nft/ports, the live
# trunk, port 45070, the router, or the cluster. Synthetic ports only.
#
# Everything is reaped on exit (container + image). Never touches the host netns.
#
#   ./run-proof.sh                 # build + run, transcript to stdout
#   TRANSCRIPT=out.txt ./run-proof.sh   # also tee the transcript to a file
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
IMG=m8a-cutover-proof:local
CTR="m8a-cutover-$$"
TRANSCRIPT="${TRANSCRIPT:-}"

cleanup() {
  docker rm -f "${CTR}" >/dev/null 2>&1 || true
  docker image rm "${IMG}" >/dev/null 2>&1 || true
  # Report any leak so a human can reap by hand.
  if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -qx "${CTR}"; then
    echo "CLEANUP WARNING: container ${CTR} leaked — docker rm -f ${CTR}" >&2
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

command -v docker >/dev/null || { echo "docker required" >&2; exit 2; }

echo "== build proof image =="
docker build -t "${IMG}" "${HERE}" >/dev/null

echo "== launch ISOLATED container (--privileged --network none) =="
docker run -d --rm --name "${CTR}" --privileged --network none "${IMG}" >/dev/null

# Host-side isolation assertion: docker itself reports this container is on the
# 'none' network — no bridge, no veth to the host, no published ports.
NETMODE="$(docker inspect -f '{{.HostConfig.NetworkMode}}' "${CTR}")"
PORTS="$(docker inspect -f '{{json .NetworkSettings.Ports}}' "${CTR}")"
echo "host-side check: NetworkMode=${NETMODE}  PublishedPorts=${PORTS}"
[ "${NETMODE}" = "none" ] || { echo "REFUSING: container is not on network 'none'" >&2; exit 3; }

run() { docker exec "${CTR}" bash /m8a/in-container-proof.sh; }
if [ -n "${TRANSCRIPT}" ]; then run | tee "${TRANSCRIPT}"; rc="${PIPESTATUS[0]}"; else run; rc=$?; fi

echo "== proof exit: ${rc} =="
exit "${rc}"
