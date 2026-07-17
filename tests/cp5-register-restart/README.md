# CP5 (B4) container-restart REGISTER acceptance harness

This on-demand harness proves rustisk's **dynamic authenticated REGISTER**
end-to-end with a **real container restart** on an isolated `--internal` Docker
network — the literal CP5/B4 acceptance that PR #154 landed only as an
in-process mechanism proof (`crates/asterisk-integration-tests/tests/e2e_dynamic_register.rs`).

It never touches Kubernetes, the live voice stack, the carrier trunk, Chime,
Mumble, the live phone number, or the real PIN. It mirrors the isolation and
Docker-reap discipline of `tests/freeswitch-pin-gate/`.

```console
tests/cp5-register-restart/run.sh
```

`CP5_CASE=all` (default) runs GREEN + RED. `green`/`red` run a subset. Every
container and network is reaped on exit via a trap.

## What it proves (receiver-side)

The core rule of B4: **a static contact is wrong because a pod restart gives the
bridge a new IP.** The harness watches the INVITE *arrive* on the bridge/sentinel
(actual datagram receipt), never a rustisk TX log.

1. **GREEN-A.** A `sip-bridge` container digest-REGISTERs (`401 → digest → 200`)
   advertising its Docker-assigned container IP **A**. `Dial(PJSIP/bridge)` (via
   AMI `Originate`) sends an INVITE that the bridge captures at **A**.
2. **restart.** The bridge container is destroyed. A **sentinel** container seizes
   the now-vacated **A** (so a stale-route INVITE has a receiver *and* Docker
   cannot hand A back). A fresh bridge container comes up with a **new IP B**
   (`B ≠ A`, enforced) and re-REGISTERs from B.
3. **GREEN-B.** `Dial(PJSIP/bridge)` now lands at **B** (captured on the new
   bridge) and **not** at stale A (the sentinel stays silent). rustisk holds both
   the stale-A and fresh-B bindings live; `Registrar::best_contact` returns the
   newest (B), so this is a genuine discrimination, not a trivial single-binding
   route.
4. **RED (captured negative control).** Routing is defeated by a **static
   contact pinned to A** (`bridge_pinned`, whose AoR carries `contact =
   sip:pinned@A:5060` — a distinct user-part — and never re-registers).
   `Dial(PJSIP/bridge_pinned)` misroutes to stale A; the sentinel catches the
   INVITE (correlated by its `sip:pinned@A` Request-URI so no stray `bridge`
   datagram can be mistaken for it) and the follow-to-B assertion goes **RED**.
   This proves the A-detection is real — the harness can fail — so a green
   GREEN-B is meaningful. (The complementary `best_contact` oldest-wins RED is
   covered directly in-process by `e2e_dynamic_register.rs`.)

A and B are **read at runtime** (`docker inspect`), never hardcoded. rustisk's
own address is fixed and kept out of the dynamic IP pool via `--ip-range`.

## Addresses and IP assignment

- Network: `--internal`, `10.252.<octet>.0/24`, dynamic pool `…​.32/27`.
- rustisk: fixed `…​.2` (outside the dynamic pool).
- bridge A, bridge B, sentinel: Docker-assigned from the dynamic pool.

## Files

| File | Role |
|------|------|
| `run.sh` | orchestrator: network, containers, phases, PASS/FAIL verdict, reap trap, `PROOF.txt` |
| `sip_agent.py` | UDP SIP agent — `--role bridge` (digest REGISTER + receiver-side INVITE capture, answers 100/486) or `--role sentinel` (seizes vacated A, captures stray INVITEs) |
| `ami_originate.py` | one authenticated AMI `Originate` to trigger a Dial |
| `config/pjsip.conf.tmpl` | `bridge` (dynamic AoR, digest auth) + `bridge_pinned` (static-A RED control); `@PINNED_A@` filled at runtime |
| `config/{asterisk,manager,extensions,rtp}.conf*` | rustisk runtime config |

## Notes / environment

- rustisk fails closed without a mounted PIN secret, so the harness generates a
  throwaway random six-digit **test** PIN, mounts it read-only, and removes it on
  exit. No PIN value is logged or committed.
- tron's docker is userns-remapped: the daemon cannot `SIGKILL` a container
  running as our uid (`docker stop`/`rm -f` hang). Containers run
  `--user $(id -u)`, so the reaper signals the **host PID** we own, then removes
  the container. Do not replace `reap_container` with a bare `docker rm -f`.
- Build context and volumes live under `target/` (ext4), never `/tmp` (a tmpfs
  RAM disk on tron).
- Requires `docker`, `python3`, and `cargo +1.97.0`.
