#!/usr/bin/env bash
# run-safety-tests.sh — driver for the M8a artifact-safety regression tests.
#
# Builds the proof image and runs safety-tests.sh inside ONE isolated container
# network namespace (--privileged --network none): the container has only `lo`,
# NO route to the host, and its OWN empty nftables ruleset. Nothing it does can
# touch the host nft, the live voicefw firewall, port 45070, RTP 20000-20100, the
# router, or the cluster. Synthetic ports only. Container + image reaped on exit.
#
#   ./run-safety-tests.sh                    # build + run, transcript to stdout
#   TRANSCRIPT=out.txt ./run-safety-tests.sh # also tee the transcript to a file
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
IMG=m8a-cutover-safety:local
CTR="m8a-cutover-safety-$$"
TRANSCRIPT="${TRANSCRIPT:-}"

cleanup() {
  docker rm -f "${CTR}" >/dev/null 2>&1 || true
  docker image rm "${IMG}" >/dev/null 2>&1 || true
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

# Host-side isolation assertion: docker itself reports 'none' network — no bridge,
# no veth to the host, no published ports.
NETMODE="$(docker inspect -f '{{.HostConfig.NetworkMode}}' "${CTR}")"
PORTS="$(docker inspect -f '{{json .NetworkSettings.Ports}}' "${CTR}")"
echo "host-side check: NetworkMode=${NETMODE}  PublishedPorts=${PORTS}"
[ "${NETMODE}" = "none" ] || { echo "REFUSING: container is not on network 'none'" >&2; exit 3; }

run() { docker exec "${CTR}" bash /m8a/safety-tests.sh; }
if [ -n "${TRANSCRIPT}" ]; then run | tee "${TRANSCRIPT}"; rc="${PIPESTATUS[0]}"; else run; rc=$?; fi

echo "== safety tests exit: ${rc} =="
exit "${rc}"
