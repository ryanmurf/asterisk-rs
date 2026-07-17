# M8a — Results (measured windows + passing transcript)

Mechanism: **LISTENER SWAP** (no NAT). Verdict: **GO** for M9, gated on M0a's
Chime endpoint-down tolerance vs. the measured window (below).

Run it yourself: `./run-proof.sh`. The transcript below is representative;
windows vary a few ms run to run. Reproduced cleanly across repeated runs.

## Measured handover windows (both directions)

| Direction | Mechanism-floor window | Receiver-observed delivery gap |
|---|---|---|
| **FS → rustisk** (apply) | **~47–53 ms** | ~26–28 ms (12–13 datagrams @ 2 ms) |
| **rustisk → FS** (rollback) | **~38–52 ms** | ~28–30 ms (13–14 datagrams @ 2 ms) |

The **mechanism-floor** window is nft-drop-toggle + socket close + socket bind +
IPC/status-poll, measured in an isolated netns. It is the *floor*: the **live M9
window additionally includes FS `sofia profile stop/start` + DNS re-resolve**,
which this synthetic proof cannot measure. Feed the live number from M0a.

### bind-on-command note (go/no-go)

The synthetic floor (~50 ms) is well under any plausible SIP retransmission /
endpoint-down tolerance, and the fail-closed drop converts the gap to loss (not
ICMP reject). **On this evidence, bind-on-command is NOT required** — do not
build it yet. Revisit only if M0a's live measurement (FS profile stop/start +
DNS) pushes the *rustisk-side* bind past Chime's tolerance; then the mitigation
is bind-on-command for rustisk (today the SIP transport binds at startup and a
bind failure is fatal by design — `crates/rustisk-cli/src/main.rs:2189-2192`,
PR #65) so rustisk can claim `45070` the instant FS releases it. Flagged for the
coordinator as a **conditional** work item, not scheduled.

## Passing transcript (steps 1–5 + RED control + detector self-test)

```
=================== M8a LISTENER-SWAP PROOF ===================
synthetic dport=55070 (NOT 45070)  rate=2ms  ingress via veth (prerouting+input)
--- ISOLATION (must all hold before traffic) ---
main-netns links: lo,veth0,
  host LAN 192.168.0.109: NO ROUTE (isolated) OK
  nft ruleset before setup:
--- cutover table (filter only; no nat/ct/redirect) ---
  table inet cutover {
  	chain input {
  		type filter hook input priority filter - 10; policy accept;
  		udp dport 55070 ip saddr 10.9.0.3 drop comment "srcdrop-v4"
  		udp dport 55070 ip6 saddr fd00::3 drop comment "srcdrop-v6"
  	}
  }
step 1: primed TRUST(v4) five-tuple 10.9.0.2:41002 -> 10.9.0.1:55070
        untrusted EVILV4(10.9.0.3) + EVILV6(fd00::3) flooding continuously
step 2: APPLY switch FS -> rustisk
APPLY FS->rustisk handover window: 47 ms
step 3: ROLLBACK rustisk -> FS (same continuously-flowing tuple)
ROLLBACK rustisk->FS handover window: 52 ms
--------------- RECEIVER-SIDE ASSERTIONS (listener swap) ---------------
PASS source-drop EVILV4: delivered_to_FS=0 delivered_to_RUSTISK=0 (must be 0/0)
PASS source-drop EVILV6: delivered_to_FS=0 delivered_to_RUSTISK=0 (must be 0/0)
PASS disjoint captures (no split-brain): overlap_count=0
PASS ownership run pattern by seq = [FS,RUSTISK,FS] (expected [FS,RUSTISK,FS])
PASS boundary FS->RUSTISK: last FS seq=745, first RUSTISK seq=758 (clean: FS.max < RUSTISK.min); handover gap = 12 datagrams / 26.0 ms delivery gap
PASS boundary RUSTISK->FS: last RUSTISK seq=1519, first FS seq=1533 (clean: RUSTISK.max < FS.min); handover gap = 13 datagrams / 28.0 ms delivery gap
trusted captured: FS=1509 RUSTISK=762 seq_span=1..2296
RESULT: PASS
--------------- MEASURED HANDOVER WINDOWS (mechanism floor) ---------------
FS->rustisk apply    window: 47.1 ms
rustisk->FS rollback window: 52.3 ms

=================== RED CONTROL (stateful redirect lever) ===================
--- (A) source-drop bypass: untrusted 10.9.0.3 -> 55190, drop is on dport 55190 ---
    (redirect DNATs 55190->55192 at prerouting, past the dport-55190 drop)
--- (B) rollback-persistence probe (honest): switch then rollback a primed tuple ---
    cumulative captured lines (FS=55190, RU=55192):
      prime (no rule):     [FS=822 RU=0]
      after switch (dnat): [FS=824 RU=1556]  (RU grew => switch worked)
      after rollback (del):[FS=1373 RU=1582]  (FS grew => reverted cleanly)
    => rollback reverts to FS on tron's kernel; conntrack-persistence objection NOT reproduced.
--- RED machine verdict (assert_boundary on the redirect source-drop capture) ---
FAIL source-drop EVILV4: delivered_to_FS=0 delivered_to_RUSTISK=695 (must be 0/0)
FAIL ownership run pattern by seq = [RUSTISK] (expected [FS,RUSTISK,FS])
RESULT: FAIL
RED TEETH CONFIRMED: redirect lever FAILED the proof (untrusted delivered / no clean swap) — listener swap PASSES the same checks.

=================== DETECTOR SELF-TEST (assert_boundary teeth) ===================
  detector[good]: expected PASS, got PASS  OK
  detector[overlap]: expected FAIL, got FAIL  OK
  detector[interleave]: expected FAIL, got FAIL  OK
  detector[norollback]: expected FAIL, got FAIL  OK
  detector[untrusted]: expected FAIL, got FAIL  OK
detector self-test: ALL OK (accepts clean, rejects every pathology)

=================== SUMMARY ===================
listener-swap assert  rc=0  (0 = clean boundaries + source-drop hold)
RED control (redirect) rc=0  (0 = redirect FAILED the proof as expected -> teeth)
detector self-test     rc=0  (0 = assert_boundary rejects every crafted bad capture)
M8a RESULT: PASS
```

## What each step proved

1. **Primed tuple** `10.9.0.2:41002 → 10.9.0.1:55070`, numbered datagrams flowing
   continuously (seq 1…2296), never restarted across both transitions.
2. **Switch FS→rustisk**: single clean boundary — FS captured through seq **745**,
   rustisk from seq **758**; the 12-datagram gap is the handover drop window. No
   overlap, no interleaving, no split-brain (`overlap_count=0`).
3. **Rollback rustisk→FS** under the *same* flow: reverse boundary just as clean —
   rustisk through **1519**, FS from **1533**. (This is the step a stateful/redirect
   lever is claimed to fail; the no-NAT swap passes it.)
4. **Source drop holds throughout** both transitions for **v4 and v6**: the
   untrusted `10.9.0.3` (v4) and `fd00::3` (v6) sources were delivered to
   **neither** stand-in at any point (`0/0`). A genuine DROP in an accept-policy
   chain, not an allow rule.
5. **Windows** measured both directions (above).

## RED control — teeth, and an honest finding

The RED control runs a **stateful `dnat`/redirect lever** through the same path:

- **(A) It FAILS the proof deterministically.** Because it rewrites the dport
  (`55190 → 55192`) at prerouting, the fail-closed source-drop written for dport
  `55190` no longer matches, so the **untrusted source is delivered** to the
  successor (`delivered_to_RUSTISK=695`). `assert_boundary` flags it → **RED**.
  The listener swap keeps the same port, so its drop holds (`0/0`). This is
  exactly PLAN-v3's hazard: *"any port-rewriting mechanism moves packets to a
  port the current filter does not match… would need a genuine fail-closed DROP
  for untrusted sources on the new port, v4 and v6."*

- **(B) Honest negative result on the conntrack-persistence claim.** PLAN-v3 C1
  argues the redirect *cannot switch a primed tuple back* because conntrack
  persists. **Measured on tron's kernel, it reverts cleanly on rule removal**
  (the probe shows FS reclaiming the flow after the rule is deleted; a directly
  injected stale conntrack entry was ignored). So on this kernel the redirect
  dies on **(A)** — the security regression — not on the rollback boundary. The
  listener swap is still the right choice: it introduces **no** NAT/conntrack
  into a NAT-free path, and its behavior does not depend on kernel/conntrack
  semantics that were observed to vary by birth-condition during this work.

- The **detector self-test** proves `assert_boundary` is not rigged to always
  pass: it rejects overlap/split-brain, interleaving, a missing rollback
  boundary, and any untrusted delivery, while accepting a clean capture.
