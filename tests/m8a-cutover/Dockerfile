# M8a cutover-mechanism proof container.
#
# Built with network (to install packages), then RUN with
#   --network none --cap-add=NET_ADMIN
# so the proof executes in a private, empty network namespace: only `lo`, no
# route to the host, its OWN nftables ruleset. Nothing it does (nft rules, port
# binds, floods, conntrack) can reach the host netns, the live trunk, port
# 45070, the router, or the cluster.
FROM debian:stable-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      nftables \
      iproute2 \
      conntrack \
      python3 \
      procps \
      iputils-ping \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /m8a
COPY listener.py sender.py burst.py assert_boundary.py cutover_lib.sh \
     apply-fs-to-rustisk.sh rollback-rustisk-to-fs.sh red-stateful-variant.sh \
     detector-selftest.sh in-container-proof.sh safety-tests.sh /m8a/
RUN chmod +x /m8a/*.sh /m8a/*.py

# The container just sleeps; the driver drives it via `docker exec`.
CMD ["sleep", "infinity"]
