# CP5 — external signalling scope on origination-relevant ancillary paths

Folded from the M6 routing review (MINOR-1): several **ancillary** signalling
paths still interpolate the raw internal bind (`local_addr`) into Via/From/Contact
instead of the NAT-scoped `advertised_signaling_hostport()`, so an external peer
could see the internal address. This checkpoint answers the M7-scoped question:
**which of those ancillary paths does a Chime origination actually exercise?**

## Finding: none of the ancillary paths is on the M7 Chime origination path

| Ancillary path | Emits raw bind? | On the Chime origination path? |
|---|---|---|
| `outbound_registration.rs` (REGISTER + its keepalive OPTIONS) | yes (`local_addr` in Via/Contact) | **No.** AWS Chime Voice Connector **termination is IP-authenticated** — rustisk does **not** REGISTER to the carrier (the incumbent `call.sh` documents `NOREG`; no outbound registration is started for origination). CP1's carrier auth is a **digest challenge answered on the INVITE**, not a REGISTER. |
| `refer.rs` (REFER / transfer) | yes | **No.** The M7 origination path implements no call transfer, so REFER is never sent. |
| `notify.rs` / `notify_service.rs` (NOTIFY) | yes (`127.0.0.1:5060` fallback / `local_addr`) | **No.** No subscription/event dialog is created on the origination path. |
| `messaging.rs` (MESSAGE) | yes | **No.** SIP MESSAGE is unrelated to origination. |
| `update.rs` (outbound UPDATE / session-timer refresh) | template-only today | **No.** rustisk selects `refresher=uac` and is the session-timer **responder**, not the refresher (`event_handler.rs`: "UAS-side refresh SCHEDULING is deferred"). It answers a peer's UPDATE/re-INVITE but does not originate refreshes, so it never puts an outbound UPDATE on the wire. |

The **core** outbound INVITE (M6 CP2) **and** the in-dialog ACK / BYE / CANCEL
added in M7 CP1 already route through `signaling_hostport()` →
`advertised_signaling_hostport()`, so the entire origination signalling path is
NAT-scoped. There is therefore nothing origination-relevant left to scope in
this checkpoint.

## What this checkpoint delivers instead

1. **This analysis + deferral.** Scoping the genuinely ancillary paths
   (REFER/NOTIFY/MESSAGE/REGISTER/UPDATE) remains a real latent leak but is
   **off the M7 origination path** — deferred as an M6-MINOR-1 follow-up, to be
   done when/if transfer, event subscriptions, MESSAGE, or an authenticated
   outbound REGISTER are put on a carrier path.
2. **A receiver-side regression** (`run.sh`) that **locks in** the origination
   path's scoping so a future change cannot silently reintroduce a bind leak on
   INVITE/ACK/BYE. With `external_signaling_address = signaling.example.net` and
   the carrier outside `local_net`, the offline carrier asserts every origination
   request advertises the external host and **none** carries the raw `0.0.0.0`
   bind.

## Running

```
tests/cp5-origination-signaling-scope/run.sh                 # GREEN: fully scoped
CP5_MODE=red-noext tests/cp5-origination-signaling-scope/run.sh  # RED: drop the
                    # external address -> every request leaks 0.0.0.0 (assertion RED)
```

Isolated Docker only — never the live voice stack, carrier, or real PIN.
