# M8a — Cutover mechanism: proof of the LISTENER SWAP

This directory selects and **proves** the mechanism M9 will use to switch the
public UDP trunk endpoint (`tron:45070`, RTP `20000-20100`) between FreeSWITCH
and rustisk. M9 invokes exactly these artifacts.

The whole proof runs inside **one isolated container network namespace**
(`--privileged --network none`) on **synthetic high ports** — it never touches
the host nftables, host ports, the live trunk, port 45070, the router, or the
cluster. See "Isolation" below.

## Selected mechanism: LISTENER SWAP (no NAT)

FreeSWITCH's trunk profile is **stopped** (freeing `hostIP:45070`, FS process
stays up), then rustisk **binds `hostIP:45070` directly**. Delivery is by
per-packet socket lookup; **no NAT, no conntrack, no redirect**. A preinstalled
**fail-closed DROP** on the guarded dport is enabled only across the handover so
the sender sees plain loss (SIP retransmission covers it) instead of ICMP
port-unreachable.

Why this over the alternates (PLAN-v3 design table, §M8a):

| Shape | Stateless? | Verdict |
|---|---|---|
| **Listener swap** — FS profile stop → rustisk binds 45070. No NAT. | **Yes** — per-packet socket lookup; conntrack never steers un-NAT'd traffic. | **SELECTED** |
| prerouting `redirect`/`dnat` to an alt port | No — stateful NAT | DEAD (see RED control) |
| Host→pod DNAT | No — stateful NAT + new forward-hook policy | strictly worse |
| conntrack-zone toggle wrapping a NAT lever | isolates state deliberately | documented alternate |
| tc/eBPF stateless rewrite | Yes | heavy lift, new failure surface |
| re-point the **router** forward | No — NAT we can't inspect/flush | rejected |

Verified reasons the listener swap wins (all confirmed by the proof):
1. **No NAT ⇒ the conntrack objection is structurally absent.** The table this
   mechanism installs (`cutover_lib.sh`) is filter-only: no `ct`, `nat`,
   `dnat`, or `redirect` — same shape as today's `voice-trunk.nft`.
2. **The existing source allowlist keeps protecting `udp dport 45070`
   unchanged** — same port, same host, same rule. No new blast radius. The RED
   control shows a port-rewriting lever silently **bypasses** that drop.
3. **Per-profile stop/start on live FS is already proven in-repo** (the trunk
   watchdog's `sofia profile <name> start`). Rollback is that same command.
4. **rustisk binds the public port directly**, so Via/Contact carry 45070 —
   New-3's external-port hazard N/A on the primary path.

Design constraints honored: **New-5** (no K8s Service on the trunk ports — a
NodePort would make kube-proxy install stateful DNAT and reintroduce the defect;
the mechanism is hostNetwork, no Service); **New-3** (direct bind of the public
port); and **no `flush ruleset` ever** — the cutover table is separate and
independently deletable (`nft delete table inet cutover`).

## What M9 invokes (the artifacts)

| File | Role |
|---|---|
| `cutover_lib.sh` | the nft primitives: `cutover_table_up/down`, `handover_drop_on/off`. **Filter-only table.** The only host-networking surface. |
| `apply-fs-to-rustisk.sh` | **APPLY** (FS→rustisk): drop-on → stop FS → wait released → bind rustisk → wait bound → drop-off. Measures + prints the window. |
| `rollback-rustisk-to-fs.sh` | **ROLLBACK** (rustisk→FS): the exact mirror; `START_NEW_CMD` is the watchdog's `sofia profile start`. |

Both scripts inject the FS/rustisk specifics as commands, so the orchestration
is **identical** in this synthetic proof and in the live M9 cutover — only the
hooks differ:

```
# M8a synthetic (this harness):
STOP_OLD_CMD="kill -USR2 $FS_PID"          # release the port from the stand-in
START_NEW_CMD="kill -USR1 $RUSTISK_PID"    # bind the port on the stand-in

# M9 live (substitute):
PORT=45070 NFT_TABLE=voicefw \
STOP_OLD_CMD="fs_cli -x 'sofia profile <trunk> stop'" \
WAIT_RELEASED_CMD="<poll until 45070 free>" \
START_NEW_CMD="<start rustisk / claim 45070>" \
WAIT_BOUND_CMD="<poll until rustisk holds 45070>" \
  ./apply-fs-to-rustisk.sh
# rollback: STOP_OLD_CMD stops rustisk; START_NEW_CMD = fs_cli -x 'sofia profile <trunk> start'
```

In M9 the source-drops are the **existing** `voice-trunk.nft` allowlist (already
protecting dport 45070) — the cutover adds no new firewall rules. The
`UNTRUSTED_V4/V6` knobs here exist only to *prove* the drop holds across the
handover.

## The proof harness

| File | Role |
|---|---|
| `run-proof.sh` | top-level driver: builds the image, runs it `--privileged --network none`, asserts host-side `NetworkMode=none`, execs the proof, reaps the container + image. |
| `Dockerfile` | debian-slim + nftables/iproute2/conntrack/python3. |
| `in-container-proof.sh` | steps 1–5 over a veth pair + child netns ("chime"→"tron"), so traffic traverses **prerouting+input** like the real trunk (not loopback). |
| `listener.py` | stand-in that binds the port **on command** (SIGUSR1/2), dual-stack, and writes one CSV row per received datagram — the receiver-side ground truth. **No SO_REUSEPORT**: exactly one holder at a time. |
| `sender.py` / `burst.py` | numbered-datagram senders pinning a fixed five-tuple. |
| `assert_boundary.py` | receiver-side assertion: disjoint captures, single clean FS→RUSTISK→FS boundary per transition, and untrusted tags delivered **nowhere**. |
| `red-stateful-variant.sh` | RED control — a stateful `dnat`/redirect lever that **fails** the proof where the listener swap passes (teeth). |
| `detector-selftest.sh` | proves `assert_boundary.py` itself rejects crafted pathologies (overlap, interleave, missing rollback, untrusted delivery). |
| `RESULTS.md` | the committed passing transcript + measured windows. |

## Run it

```
./run-proof.sh                        # build + run + reap, transcript to stdout
TRANSCRIPT=out.txt ./run-proof.sh     # also tee the transcript to out.txt
```

Requires Docker with `--privileged` (for nft + nested netns inside the
container). If privileged Docker is unavailable, the proof **stops** — it never
falls back to the host netns.

## Isolation (the whole risk)

- `--network none` ⇒ the container's netns has only `lo`; the internal veth pair
  lives entirely inside it. There is **no route to the host** (the proof asserts
  `192.168.0.109` is unreachable) and no published ports (`run-proof.sh` asserts
  `NetworkMode=none`).
- The container has its **own** nftables ruleset (empty before setup) — separate
  from the host's. Every `nft`/`ip`/`conntrack`/bind runs inside the container.
- Synthetic ports only (`55070`, `55190/55192`) — never `45070`/`20000-20100`.
- **No `flush ruleset`** anywhere; tables are dropped by name.
