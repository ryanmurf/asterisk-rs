# FreeSWITCH PIN-gate acceptance harness

This harness runs the production-pinned FreeSWITCH 1.10.12 image on an
internal Docker network as an isolated carrier and originates real SIP calls
into an isolated rustisk container. It also accepts a real outbound rustisk call.
It never contacts Kubernetes, a carrier trunk, Chime, Mumble, or the live
phone number.

Run it from the rustisk repository:

```console
tests/freeswitch-pin-gate/run.sh
```

The first call enters `123456#` and must take the `GRANTED` branch. The second
enters `123450#` and must take the `REJECTED` branch. Both calls use negotiated
[RFC 4733] telephone events from FreeSWITCH. [FreeSWITCH's documented default]
when `dtmf_type` is unset is to use the method negotiated in SDP. The harness
fails unless rustisk's authenticated AMI `RTPStats` action proves all of the
following for each completed call:

- RTP packets were transmitted and received.
- Voice frames were transmitted and received.
- DTMF digits were detected by rustisk's receiver.
- Completed-call counters remained available after hangup.

It also retains FreeSWITCH WAV captures, rustisk logs, and the raw AMI proof in
`target/freeswitch-pin-gate/`. The generated prompts are 8 kHz, mono,
signed-16 WAV fixtures; rustisk must decode them and encode the negotiated
G.711 payload. [RFC 3551] assigns PCMU and PCMA static payload types 0 and 8.
FreeSWITCH's [`uuid_record` documentation] describes the established-call WAV
capture used for the retained artifacts.

Set `FREESWITCH_PIN_GATE_CASE=m4-timer-b` to run only the isolated Timer B
negative-control target while developing the impairment harness.

The M1 outbound checks are deliberately asymmetric:

- FreeSWITCH's B endpoint answers and records while its sleeping dialplan sends
  zero media packets to rustisk for the entire call.
- Completed `RTPStats` must report `RTPVoiceFramesRx == 0` and
  `RTPVoiceFramesTx > 0` for the rustisk outbound leg.
- The symmetric-RTP latch exists only in `RtpSession::recv_frame`, which also
  increments `voice_frames_received` for every non-empty voice payload. Any
  socket read capable of latching would therefore make `RTPVoiceFramesRx`
  non-zero and fail the case.
- A spectral check on FreeSWITCH's receiver-side WAV must find the expected
  tone; a non-empty recording alone is not accepted.
- A separate one-second `Dial()` timeout must appear as a `CANCEL` in
  FreeSWITCH's SIP trace.
- AMI `CoreStatus` snapshots the channel store, driver map, Call-ID map,
  call-state map, and NOTIFY channel map. Every case must return all five to
  the exact pre-test baseline within two seconds.
- The same action exposes the INVITE client/server and non-INVITE
  client/server transaction-map sizes. Every case must return all four to the
  exact pre-test baseline after any RFC 3261 UDP absorption timer expires.

The M2 case establishes two FreeSWITCH channels on opposite sides of a real
rustisk `Dial()` bridge and records both of them. It injects 440 Hz on A and
requires it in B's capture, injects 660 Hz on B and requires it in A's capture,
and sends one RFC 4733 digit on A that FreeSWITCH B must decode into its
`RTPDTMFDigitsRx` dialplan variable. A transmit counter or non-empty recording
cannot satisfy any of those assertions.

The same live call supplies the M2 ingress-hygiene and reorder proof. A helper
shares the isolated FreeSWITCH network namespace to send wrong-source,
wrong-payload-type, malformed, and unstable-SSRC datagrams to the negotiated
rustisk RTP port. Each injection must increment only its named discard counter,
leave all accepted-media counters unchanged, and leave `RTPRemoteAddress`
pointing at the SDP-negotiated FreeSWITCH address. A deterministic PCMU stream
then carries a marker plus a four-frequency audio sequence with a gap, a
duplicate, and swapped packet pairs. The far-side FreeSWITCH WAV must recover
at least 44 of 47 ordered audio frames; the passing proof currently reports
all 47.

Finally, the harness proves pump cancellation with both sources silent for two
independent teardown paths: a receiver-originated BYE and the absolute `Dial()`
media deadline. Both must restore the five resource maps to the exact
`0/0/0/0/0` baseline within two seconds. The deadline case additionally
requires zero accepted RTP packets on both completed rustisk legs.

The M4 stage adds a dedicated UDP SIP proxy on the internal network. The proxy
inserts and removes its own top Via and rewrites Contact to itself, so both
transaction responses and later ACK/BYE requests are provably on path; RTP
still flows directly between the endpoints. Its deterministic modes drop,
hold, replay, or rewrite selected messages. Receiver-side assertions cover a
dropped 200, a late INVITE replay with three byte-identical 200s and one call,
dropped ACK and BYE, 180 followed by silence, three forged dialog identities
from the allowed proxy address, ten simultaneous calls with distinct captured
tones, the CANCEL/200 crossing order, and non-2xx or absent BYE finals. Every
individual case restores both the five core maps and four transaction maps to
their exact `0/0/0/0/0` and `0/0/0/0` baselines.

The 180-then-silence case directly originates its outbound leg into a 70-second
hold, proves the provisional response left a live INVITE-client transaction,
then requires that map to drain in Timer B's 28-38 second window before checking
the exact map baselines. This makes the transaction timer, rather than either
the old `Dial()` timeout or a short post-originate hangup's CANCEL/487/Timer-D
path, load-bearing for the case.

[RFC 3551]: https://www.rfc-editor.org/info/rfc3551/
[RFC 4733]: https://www.rfc-editor.org/info/rfc4733/
[FreeSWITCH's documented default]: https://developer.signalwire.com/freeswitch/reference/channel-variables/#dtmf
[`uuid_record` documentation]: https://developer.signalwire.com/freeswitch/media-and-codecs/audio-files/#recording-an-established-call
