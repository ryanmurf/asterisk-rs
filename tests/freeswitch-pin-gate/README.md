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

The M1 outbound checks are deliberately asymmetric:

- FreeSWITCH's B endpoint answers and records but remains listen-only for the
  entire call.
- Completed `RTPStats` must report `RTPVoiceFramesRx == 0` and
  `RTPVoiceFramesTx > 0` for the rustisk outbound leg.
- A spectral check on FreeSWITCH's receiver-side WAV must find the expected
  tone; a non-empty recording alone is not accepted.
- A separate one-second `Dial()` timeout must appear as a `CANCEL` in
  FreeSWITCH's SIP trace.
- AMI `CoreStatus` snapshots the channel store, driver map, Call-ID map, and
  call-state map. Every case must return all four to the exact pre-test
  baseline within two seconds.

[RFC 3551]: https://www.rfc-editor.org/info/rfc3551/
[RFC 4733]: https://www.rfc-editor.org/info/rfc4733/
[FreeSWITCH's documented default]: https://developer.signalwire.com/freeswitch/reference/channel-variables/#dtmf
[`uuid_record` documentation]: https://developer.signalwire.com/freeswitch/media-and-codecs/audio-files/#recording-an-established-call
