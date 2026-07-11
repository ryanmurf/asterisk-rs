//! ConfBridge audio mixing engine (issue #12).
//!
//! Drives real N-party audio mixing for `ConfBridge()`. The mixing math
//! itself is the existing, tested softmix core
//! ([`asterisk_core::bridge::softmix::SoftmixData`], a port of
//! bridge_softmix.c): sum all contributors, then hand each participant the
//! total minus their own contribution ("mix-minus"), saturation-clamped to
//! i16. What this module adds is the media pump around that core:
//!
//! * one **reader task per participant** pulls frames from the channel
//!   driver (`read_frame` → RTP), decodes G.711 (µ-law/A-law, native ITU
//!   tables from `asterisk-codecs`) to signed linear, and appends the
//!   samples to the participant's pending queue;
//! * one **mixing task per conference** ticks every 20 ms, drains one
//!   frame's worth of samples (160 at 8 kHz) from each queue into the
//!   softmix buffers, runs the mix, re-encodes each participant's
//!   mix-minus output in that leg's negotiated codec, and writes it back
//!   through the channel driver (`write_frame` → RTP).
//!
//! Cadence/timing model: the mixer always emits one 20 ms frame per tick to
//! every (non-deaf) participant — silence when nobody else is talking — so
//! each leg sees a steady 50 pps stream. Inbound legs are not required to be
//! aligned or complete: a participant only contributes to a tick when a full
//! frame of samples is queued (otherwise the partial remainder keeps
//! accumulating for the next tick), and the queue is capped so a stalled
//! reader or a pre-join burst can add at most ~200 ms of latency before old
//! audio is dropped.
//!
//! Scope: G.711 µ-law (PT 0) and A-law (PT 8) legs at 8 kHz. Frames in any
//! other codec are counted and dropped, and legs whose negotiated format is
//! not G.711 receive no mix (logged once). DTMF frames read from a leg are
//! consumed and ignored here (conference DTMF menus are signaling-level and
//! out of scope for the mixing path).

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use asterisk_codecs::alaw_table::{alaw_to_linear, linear_to_alaw_fast};
use asterisk_codecs::ulaw_table::{linear_to_mulaw_fast, mulaw_to_linear};
use asterisk_core::bridge::softmix::{SoftmixChannelData, SoftmixData};
use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::Frame;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Internal mixing sample rate. G.711 is 8 kHz; that is the only leg codec
/// in scope, so the conference mixes at 8 kHz regardless of profile hints.
pub const MIX_RATE: u32 = 8000;

/// Mixing interval: one frame every 20 ms (Asterisk softmix default).
pub const MIX_INTERVAL_MS: u32 = 20;

/// Samples per mixing tick (160 at 8 kHz / 20 ms).
pub const SAMPLES_PER_TICK: usize = (MIX_RATE as usize * MIX_INTERVAL_MS as usize) / 1000;

/// Cap on a participant's pending-sample queue (~200 ms). Bounds both
/// memory and the latency a slow tick / bursty reader can introduce;
/// overflow drops the oldest samples.
const MAX_QUEUE_SAMPLES: usize = SAMPLES_PER_TICK * 10;

/// Global registry of conference mixers, keyed by conference name.
/// Mirrors the `CONFERENCES` registry lifecycle in `confbridge.rs`.
static MIXERS: LazyLock<DashMap<String, Arc<ConferenceMixer>>> = LazyLock::new(DashMap::new);

/// Get the mixer for a conference, creating (and starting) it if needed.
pub fn get_or_create_mixer(conf_name: &str) -> Arc<ConferenceMixer> {
    if let Some(existing) = MIXERS.get(conf_name) {
        if existing.is_running() {
            return existing.value().clone();
        }
    }
    // No mixer, or a previous incarnation already shut down: start fresh.
    let mixer = ConferenceMixer::start(conf_name.to_string());
    MIXERS.insert(conf_name.to_string(), mixer.clone());
    mixer
}

/// Get the mixer for a conference, if one is active.
pub fn get_mixer(conf_name: &str) -> Option<Arc<ConferenceMixer>> {
    MIXERS.get(conf_name).map(|m| m.value().clone())
}

/// Stop and remove the mixer for a conference (last participant left).
pub fn shutdown_mixer(conf_name: &str) {
    if let Some((_, mixer)) = MIXERS.remove(conf_name) {
        mixer.shutdown();
    }
}

/// G.711 codec variant of a leg, keyed by RTP payload type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixCodec {
    /// µ-law, payload type 0.
    Ulaw,
    /// A-law, payload type 8.
    Alaw,
}

impl MixCodec {
    /// Map a `Frame::Voice` codec id (RTP payload type) to a mixable codec.
    pub fn from_codec_id(codec_id: u32) -> Option<Self> {
        match codec_id {
            0 => Some(Self::Ulaw),
            8 => Some(Self::Alaw),
            _ => None,
        }
    }

    /// The codec id (RTP payload type) for frames in this codec.
    pub fn codec_id(self) -> u32 {
        match self {
            Self::Ulaw => 0,
            Self::Alaw => 8,
        }
    }

    /// Decode one G.711 byte to a signed-linear sample.
    #[inline]
    pub fn decode(self, byte: u8) -> i16 {
        match self {
            Self::Ulaw => mulaw_to_linear(byte),
            Self::Alaw => alaw_to_linear(byte),
        }
    }

    /// Encode one signed-linear sample to a G.711 byte.
    #[inline]
    pub fn encode(self, sample: i16) -> u8 {
        match self {
            Self::Ulaw => linear_to_mulaw_fast(sample),
            Self::Alaw => linear_to_alaw_fast(sample),
        }
    }
}

/// Per-participant pump state (everything but the softmix buffers, which
/// live in [`SoftmixData`] keyed by the same channel name).
struct ParticipantState {
    /// Driver used for frame I/O on this leg.
    driver: Arc<dyn ChannelDriver>,
    /// Decoded signed-linear samples waiting to be mixed.
    queue: VecDeque<i16>,
    /// The leg's G.711 codec. `None` until known (negotiated format from
    /// the driver, else latched from the first mixable inbound frame);
    /// while `None` the leg receives no mix output.
    codec: Option<MixCodec>,
    /// Contribution muted (admin mute, waiting-for-marked, ...).
    muted: bool,
    /// Receives silence instead of the mix.
    deaf: bool,
    /// The reader task pumping `read_frame` into `queue`.
    reader: JoinHandle<()>,
    /// Whether the unsupported-codec warning was already logged.
    warned_unmixable: bool,
    /// Frames dropped because they were not G.711.
    unmixable_frames: u64,
}

/// Inner (lock-protected) mixer state.
struct MixerInner {
    /// The softmix core: contribution buffers + mix-minus math.
    softmix: SoftmixData,
    /// Pump state per participant, keyed by channel name.
    parts: HashMap<String, ParticipantState>,
}

impl MixerInner {
    /// Remove a participant's pump + softmix state (aborts its reader).
    fn remove(&mut self, channel_name: &str) {
        if let Some(part) = self.parts.remove(channel_name) {
            part.reader.abort();
        }
        self.softmix.channel_buffers.remove(channel_name);
        self.softmix.output_frames.remove(channel_name);
    }
}

/// One conference's audio mixer: participant queues, the softmix core, and
/// the 20 ms mixing task.
pub struct ConferenceMixer {
    conf_name: String,
    inner: Mutex<MixerInner>,
    running: AtomicBool,
    /// The mixing tick task (held so shutdown can abort it).
    tick_task: Mutex<Option<JoinHandle<()>>>,
    /// Completed mixing ticks (observability / tests).
    ticks: AtomicU64,
}

impl fmt::Debug for ConferenceMixer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConferenceMixer")
            .field("conf_name", &self.conf_name)
            .field("running", &self.is_running())
            .field("participants", &self.inner.lock().parts.len())
            .finish()
    }
}

impl ConferenceMixer {
    /// Create a mixer and start its 20 ms mixing task.
    pub fn start(conf_name: String) -> Arc<Self> {
        let mixer = Arc::new(Self {
            conf_name: conf_name.clone(),
            inner: Mutex::new(MixerInner {
                softmix: SoftmixData::new(MIX_RATE, MIX_INTERVAL_MS),
                parts: HashMap::new(),
            }),
            running: AtomicBool::new(true),
            tick_task: Mutex::new(None),
            ticks: AtomicU64::new(0),
        });

        let task = tokio::spawn(Self::run_ticks(mixer.clone()));
        *mixer.tick_task.lock() = Some(task);
        info!(conference = %conf_name, "ConfBridge mixer: started (8 kHz, 20 ms ticks)");
        mixer
    }

    /// Whether the mixer is still running (not shut down).
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Completed mixing ticks.
    pub fn tick_count(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// Current participant count in the mixer.
    pub fn participant_count(&self) -> usize {
        self.inner.lock().parts.len()
    }

    /// Add a participant by channel name, resolving the driver from the
    /// technology registry (channel names are `TECH/resource-seq`).
    pub async fn add_participant(self: &Arc<Self>, channel_name: &str) {
        let tech = channel_name.split('/').next().unwrap_or("");
        let Some(driver) = asterisk_core::channel::tech_registry::TECH_REGISTRY.find(tech) else {
            warn!(
                channel = %channel_name,
                conference = %self.conf_name,
                "ConfBridge mixer: no channel driver for tech '{tech}', leg will carry no audio"
            );
            return;
        };
        self.add_participant_with_driver(channel_name, driver).await;
    }

    /// Add a participant using an explicit driver (also used by tests).
    pub async fn add_participant_with_driver(
        self: &Arc<Self>,
        channel_name: &str,
        driver: Arc<dyn ChannelDriver>,
    ) {
        // Query the negotiated format before taking the lock (async).
        let format_chan = Channel::new(channel_name.to_string());
        let negotiated = driver.audio_format(&format_chan).await;
        let codec = negotiated.and_then(MixCodec::from_codec_id);
        if let Some(id) = negotiated {
            if codec.is_none() {
                warn!(
                    channel = %channel_name,
                    conference = %self.conf_name,
                    codec_id = id,
                    "ConfBridge mixer: negotiated codec is not G.711, leg joins without audio"
                );
            }
        }

        let reader = tokio::spawn(Self::reader_loop(
            Arc::downgrade(self),
            channel_name.to_string(),
            driver.clone(),
        ));

        let mut inner = self.inner.lock();
        // Re-join with the same channel name replaces the old state.
        inner.remove(channel_name);
        inner.softmix.channel_buffers.insert(
            channel_name.to_string(),
            SoftmixChannelData::new(channel_name.to_string(), SAMPLES_PER_TICK),
        );
        inner.parts.insert(
            channel_name.to_string(),
            ParticipantState {
                driver,
                queue: VecDeque::new(),
                codec,
                muted: false,
                deaf: false,
                reader,
                warned_unmixable: false,
                unmixable_frames: 0,
            },
        );
        debug!(
            channel = %channel_name,
            conference = %self.conf_name,
            codec = ?codec,
            "ConfBridge mixer: participant added"
        );
    }

    /// Remove a participant (leg left the conference). Stops its reader and
    /// frees its buffers; remaining legs keep mixing undisturbed.
    pub fn remove_participant(&self, channel_name: &str) {
        let mut inner = self.inner.lock();
        inner.remove(channel_name);
        debug!(
            channel = %channel_name,
            conference = %self.conf_name,
            "ConfBridge mixer: participant removed"
        );
    }

    /// Update a participant's mute/deaf flags (normally synced each tick
    /// from the conference's participant list).
    pub fn set_participant_flags(&self, channel_name: &str, muted: bool, deaf: bool) {
        let mut inner = self.inner.lock();
        if let Some(part) = inner.parts.get_mut(channel_name) {
            part.muted = muted;
            part.deaf = deaf;
        }
    }

    /// Stop the mixer: end the tick task and abort all readers.
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(task) = self.tick_task.lock().take() {
            task.abort();
        }
        let mut inner = self.inner.lock();
        let names: Vec<String> = inner.parts.keys().cloned().collect();
        for name in names {
            inner.remove(&name);
        }
        info!(conference = %self.conf_name, "ConfBridge mixer: shut down");
    }

    /// Ingest raw G.711 payload from a leg: decode to signed linear and
    /// queue for the next mixing ticks. Latches the leg codec from the
    /// frame's codec id when it was not known at join time.
    pub fn ingest(&self, channel_name: &str, codec_id: u32, payload: &[u8]) {
        let mut inner = self.inner.lock();
        let Some(part) = inner.parts.get_mut(channel_name) else {
            return;
        };

        let Some(frame_codec) = MixCodec::from_codec_id(codec_id) else {
            part.unmixable_frames += 1;
            if !part.warned_unmixable {
                part.warned_unmixable = true;
                warn!(
                    channel = %channel_name,
                    conference = %self.conf_name,
                    codec_id,
                    "ConfBridge mixer: dropping non-G.711 voice frames from leg"
                );
            }
            return;
        };
        if part.codec.is_none() {
            part.codec = Some(frame_codec);
        }

        for &byte in payload {
            part.queue.push_back(frame_codec.decode(byte));
        }
        // Bound latency/memory: drop the oldest samples on overflow.
        while part.queue.len() > MAX_QUEUE_SAMPLES {
            part.queue.pop_front();
        }
    }

    /// One mixing iteration: refresh flags from the conference, drain each
    /// participant's queue into the softmix buffers, mix, and encode each
    /// leg's output. Pure CPU under the lock; returns the frames to write.
    ///
    /// Exposed (crate-visible) so unit tests can drive ticks deterministically.
    pub(crate) fn mix_tick(&self) -> Vec<(String, Arc<dyn ChannelDriver>, Frame)> {
        // Snapshot mute/deaf flags and the live participant set from the
        // conference registry (none held while the mixer lock is taken).
        // `None` when the conference is not registered (unit tests): keep
        // the current mixer set as-is.
        let conf_flags = crate::confbridge::conference_participant_flags(&self.conf_name);

        let mut inner = self.inner.lock();

        if let Some(ref flags) = conf_flags {
            // Drop legs no longer in the conference (kicks, end_marked, ...
            // any removal path), then refresh flags for the rest.
            let stale: Vec<String> = inner
                .parts
                .keys()
                .filter(|name| !flags.contains_key(*name))
                .cloned()
                .collect();
            for name in stale {
                inner.remove(&name);
            }
            for (name, &(muted, deaf)) in flags {
                if let Some(part) = inner.parts.get_mut(name) {
                    part.muted = muted;
                    part.deaf = deaf;
                }
            }
        }

        let MixerInner { softmix, parts } = &mut *inner;

        // Load one frame of contribution per talking participant.
        for (name, part) in parts.iter_mut() {
            let Some(buf) = softmix.channel_buffers.get_mut(name) else {
                continue;
            };
            if part.muted {
                // Muted: discard pending audio so nothing stale bursts out
                // on unmute; contributes silence this tick.
                part.queue.clear();
                continue;
            }
            if part.queue.len() >= SAMPLES_PER_TICK {
                for slot in buf.our_buf.iter_mut().take(SAMPLES_PER_TICK) {
                    // Queue length was checked; pop_front cannot fail here.
                    *slot = part.queue.pop_front().unwrap_or(0);
                }
                buf.have_audio = true;
            }
            // Fewer than a full frame queued: leave it accumulating and
            // contribute silence this tick (have_audio stays false).
        }

        // Mix-minus + clamp (clears contribution buffers for the next tick).
        softmix.mix();

        // Encode each leg's output in its own codec.
        let mut out = Vec::with_capacity(parts.len());
        for (name, part) in parts.iter() {
            let Some(codec) = part.codec else {
                // Unknown leg codec: nothing safe to write.
                continue;
            };
            let payload: Vec<u8> = if part.deaf {
                vec![codec.encode(0); SAMPLES_PER_TICK]
            } else {
                match softmix.output_frames.get(name) {
                    Some(samples) => samples.iter().map(|&s| codec.encode(s)).collect(),
                    None => vec![codec.encode(0); SAMPLES_PER_TICK],
                }
            };
            out.push((
                name.clone(),
                part.driver.clone(),
                Frame::voice(codec.codec_id(), SAMPLES_PER_TICK as u32, Bytes::from(payload)),
            ));
        }
        out
    }

    /// The 20 ms mixing task: tick, mix, write each leg's frame.
    async fn run_ticks(mixer: Arc<Self>) {
        // Writer handles are only ever touched by this task; the drivers
        // only use the channel name for frame routing.
        let mut writer_chans: HashMap<String, Channel> = HashMap::new();
        let mut interval = tokio::time::interval(Duration::from_millis(MIX_INTERVAL_MS as u64));
        // If a tick runs late, do not burst to catch up -- stay at cadence.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if !mixer.is_running() {
                break;
            }

            let outputs = mixer.mix_tick();

            for (name, driver, frame) in &outputs {
                let chan = writer_chans
                    .entry(name.clone())
                    .or_insert_with(|| Channel::new(name.clone()));
                if let Err(e) = driver.write_frame(chan, frame).await {
                    // Transient during join/teardown races; the tick sync
                    // removes truly-gone legs.
                    debug!(channel = %name, "ConfBridge mixer: write_frame failed: {e}");
                }
            }
            writer_chans.retain(|name, _| outputs.iter().any(|(n, _, _)| n == name));

            mixer.ticks.fetch_add(1, Ordering::Relaxed);
        }
        debug!(conference = %mixer.conf_name, "ConfBridge mixer: tick task ended");
    }

    /// Per-participant reader: pump `read_frame` into the ingest queue
    /// until the participant is removed (task aborted) or the mixer stops.
    async fn reader_loop(mixer: Weak<Self>, channel_name: String, driver: Arc<dyn ChannelDriver>) {
        // The drivers only use the channel name for frame routing.
        let mut chan = Channel::new(channel_name.clone());
        loop {
            let Some(strong) = mixer.upgrade() else {
                break;
            };
            if !strong.is_running() {
                break;
            }
            // Bound the read so mixer shutdown is noticed even on a silent leg.
            let read = tokio::time::timeout(
                Duration::from_millis(500),
                driver.read_frame(&mut chan),
            );
            // Do not hold the Arc across the await: a blocked reader must
            // not keep a shut-down mixer alive.
            drop(strong);

            match read.await {
                Ok(Ok(Frame::Voice { codec_id, data, .. })) => {
                    if let Some(m) = mixer.upgrade() {
                        m.ingest(&channel_name, codec_id, &data);
                    } else {
                        break;
                    }
                }
                // DTMF / control / other frames: not part of the audio mix.
                Ok(Ok(_)) => {}
                Ok(Err(_)) => {
                    // No RTP session (yet) or the leg is being torn down.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(_) => {} // read timeout: loop to re-check state
            }
        }
        debug!(channel = %channel_name, "ConfBridge mixer: reader ended");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_types::{AsteriskResult, ChannelState};
    use parking_lot::Mutex as PlMutex;

    /// Mock driver: `read_frame` never resolves (tests feed `ingest`
    /// directly); `write_frame` records written voice frames per channel.
    #[derive(Debug, Default)]
    struct MockDriver {
        format: Option<u32>,
        written: PlMutex<Vec<(String, u32, Vec<u8>)>>,
    }

    impl MockDriver {
        fn with_format(format: Option<u32>) -> Arc<Self> {
            Arc::new(Self {
                format,
                written: PlMutex::new(Vec::new()),
            })
        }

        fn written_for(&self, name: &str) -> Vec<(u32, Vec<u8>)> {
            self.written
                .lock()
                .iter()
                .filter(|(n, _, _)| n == name)
                .map(|(_, id, data)| (*id, data.clone()))
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl ChannelDriver for MockDriver {
        fn name(&self) -> &str {
            "MOCK"
        }
        fn description(&self) -> &str {
            "mock driver for mixer tests"
        }
        async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
            Ok(Channel::new(format!("MOCK/{dest}")))
        }
        async fn call(&self, _c: &mut Channel, _d: &str, _t: i32) -> AsteriskResult<()> {
            Ok(())
        }
        async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.set_state(ChannelState::Down);
            Ok(())
        }
        async fn answer(&self, _c: &mut Channel) -> AsteriskResult<()> {
            Ok(())
        }
        async fn read_frame(&self, _c: &mut Channel) -> AsteriskResult<Frame> {
            std::future::pending().await
        }
        async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
            if let Frame::Voice { codec_id, data, .. } = frame {
                self.written
                    .lock()
                    .push((channel.name.clone(), *codec_id, data.to_vec()));
            }
            Ok(())
        }
        async fn audio_format(&self, _channel: &Channel) -> Option<u32> {
            self.format
        }
    }

    /// Encode a constant-amplitude 20 ms G.711 payload.
    fn tone(codec: MixCodec, amplitude: i16) -> Vec<u8> {
        vec![codec.encode(amplitude); SAMPLES_PER_TICK]
    }

    /// Decode a written payload back to linear samples.
    fn decode_all(codec: MixCodec, payload: &[u8]) -> Vec<i16> {
        payload.iter().map(|&b| codec.decode(b)).collect()
    }

    fn mean(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        samples.iter().map(|&s| s as f64).sum::<f64>() / samples.len() as f64
    }

    /// Drive one deterministic tick and deliver the writes to the drivers.
    async fn tick_and_write(mixer: &Arc<ConferenceMixer>) {
        let outputs = mixer.mix_tick();
        for (name, driver, frame) in outputs {
            let mut chan = Channel::new(name);
            driver.write_frame(&mut chan, &frame).await.unwrap();
        }
    }

    #[tokio::test]
    async fn two_party_mix_minus_ulaw() {
        let mixer = ConferenceMixer::start("t-two-party".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;

        mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 1000));
        mixer.ingest("MOCK/b-1", 0, &tone(MixCodec::Ulaw, 2000));
        tick_and_write(&mixer).await;

        // A hears only B (~2000), B hears only A (~1000); G.711 quantizes,
        // so compare with tolerance.
        let a = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/a-1")[0].1);
        let b = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/b-1")[0].1);
        assert!(
            (mean(&a) - 2000.0).abs() < 100.0,
            "A must hear B's 2000 tone, got mean {}",
            mean(&a)
        );
        assert!(
            (mean(&b) - 1000.0).abs() < 100.0,
            "B must hear A's 1000 tone, got mean {}",
            mean(&b)
        );
        // Mix-minus: A's own 1000 must not appear in A's mix (it would read
        // ~3000 if the full sum leaked through).
        assert!(mean(&a) < 2500.0, "A's own audio leaked into its mix");
        mixer.shutdown();
    }

    #[tokio::test]
    async fn three_party_mix_minus_with_alaw_leg() {
        let mixer = ConferenceMixer::start("t-three-party".into());
        let drv = MockDriver::with_format(None);
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/c-1", drv.clone()).await;

        // A and B are ulaw; C is an alaw leg (codec latched from frames).
        mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 1000));
        mixer.ingest("MOCK/b-1", 0, &tone(MixCodec::Ulaw, 2000));
        mixer.ingest("MOCK/c-1", 8, &tone(MixCodec::Alaw, 3000));
        tick_and_write(&mixer).await;

        let a = drv.written_for("MOCK/a-1");
        let b = drv.written_for("MOCK/b-1");
        let c = drv.written_for("MOCK/c-1");
        // Each leg's output is stamped with that leg's codec id.
        assert_eq!(a[0].0, 0);
        assert_eq!(b[0].0, 0);
        assert_eq!(c[0].0, 8);

        let a = decode_all(MixCodec::Ulaw, &a[0].1);
        let b = decode_all(MixCodec::Ulaw, &b[0].1);
        let c = decode_all(MixCodec::Alaw, &c[0].1);
        // A hears B+C ~5000, B hears A+C ~4000, C hears A+B ~3000.
        assert!((mean(&a) - 5000.0).abs() < 200.0, "A heard {}", mean(&a));
        assert!((mean(&b) - 4000.0).abs() < 200.0, "B heard {}", mean(&b));
        assert!((mean(&c) - 3000.0).abs() < 200.0, "C heard {}", mean(&c));
        mixer.shutdown();
    }

    #[tokio::test]
    async fn own_samples_absent_from_own_mix() {
        let mixer = ConferenceMixer::start("t-own-absent".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/talker-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/silent-1", drv.clone()).await;

        // Only the talker sends audio.
        mixer.ingest("MOCK/talker-1", 0, &tone(MixCodec::Ulaw, 8000));
        tick_and_write(&mixer).await;

        // The silent leg hears the talker; the talker hears silence (their
        // own samples are excluded, and nobody else is talking).
        let talker = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/talker-1")[0].1);
        let silent = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/silent-1")[0].1);
        assert!(
            talker.iter().all(|&s| s.abs() < 50),
            "talker must not hear their own audio echoed back"
        );
        assert!((mean(&silent) - 8000.0).abs() < 300.0);
        mixer.shutdown();
    }

    #[tokio::test]
    async fn saturation_clamps_loud_mix() {
        let mixer = ConferenceMixer::start("t-clamp".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/c-1", drv.clone()).await;

        // B and C are both near max; their sum must clamp, not wrap.
        mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 0));
        mixer.ingest("MOCK/b-1", 0, &tone(MixCodec::Ulaw, 30000));
        mixer.ingest("MOCK/c-1", 0, &tone(MixCodec::Ulaw, 30000));
        tick_and_write(&mixer).await;

        let a = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/a-1")[0].1);
        // Clamped to i16::MAX then µ-law-quantized (µ-law max ~32124):
        // strongly positive, never wrapped negative.
        assert!(a.iter().all(|&s| s > 30000), "mix must clamp, got {:?}", &a[..4]);
        mixer.shutdown();
    }

    #[tokio::test]
    async fn one_party_hears_silence_at_cadence() {
        let mixer = ConferenceMixer::start("t-one-party".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/solo-1", drv.clone()).await;

        mixer.ingest("MOCK/solo-1", 0, &tone(MixCodec::Ulaw, 5000));
        tick_and_write(&mixer).await;

        // A frame IS written every tick (steady cadence), but it is silence.
        let solo = drv.written_for("MOCK/solo-1");
        assert_eq!(solo.len(), 1);
        let samples = decode_all(MixCodec::Ulaw, &solo[0].1);
        assert!(samples.iter().all(|&s| s.abs() < 50));
        mixer.shutdown();
    }

    #[tokio::test]
    async fn muted_leg_contributes_nothing_deaf_leg_hears_silence() {
        let mixer = ConferenceMixer::start("t-mute-deaf".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/muted-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/deaf-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/norm-1", drv.clone()).await;

        mixer.set_participant_flags("MOCK/muted-1", true, false);
        mixer.set_participant_flags("MOCK/deaf-1", false, true);

        mixer.ingest("MOCK/muted-1", 0, &tone(MixCodec::Ulaw, 12000));
        mixer.ingest("MOCK/deaf-1", 0, &tone(MixCodec::Ulaw, 3000));
        mixer.ingest("MOCK/norm-1", 0, &tone(MixCodec::Ulaw, 1000));
        tick_and_write(&mixer).await;

        // The muted leg's 12000 tone must reach nobody.
        let norm = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/norm-1")[0].1);
        assert!(
            (mean(&norm) - 3000.0).abs() < 150.0,
            "normal leg must hear only the deaf leg's 3000 tone (muted leg suppressed), got {}",
            mean(&norm)
        );
        // The deaf leg still gets a frame, but it is silence.
        let deaf = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/deaf-1")[0].1);
        assert!(deaf.iter().all(|&s| s.abs() < 50), "deaf leg must hear silence");
        mixer.shutdown();
    }

    #[tokio::test]
    async fn partial_frame_accumulates_until_complete() {
        let mixer = ConferenceMixer::start("t-partial".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;

        // B sends only half a frame: contributes silence this tick.
        mixer.ingest("MOCK/b-1", 0, &[MixCodec::Ulaw.encode(4000); SAMPLES_PER_TICK / 2]);
        tick_and_write(&mixer).await;
        let a = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/a-1")[0].1);
        assert!(a.iter().all(|&s| s.abs() < 50), "half a frame must not mix yet");

        // Second half arrives: the full frame mixes on the next tick.
        mixer.ingest("MOCK/b-1", 0, &[MixCodec::Ulaw.encode(4000); SAMPLES_PER_TICK / 2]);
        tick_and_write(&mixer).await;
        let a = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/a-1")[1].1);
        assert!((mean(&a) - 4000.0).abs() < 200.0, "full frame must mix, got {}", mean(&a));
        mixer.shutdown();
    }

    #[tokio::test]
    async fn queue_overflow_drops_oldest() {
        let mixer = ConferenceMixer::start("t-overflow".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;

        // Old audio (1000) then far more than the cap of new audio (7000).
        mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 1000));
        for _ in 0..30 {
            mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 7000));
        }
        tick_and_write(&mixer).await;

        // The oldest (1000) samples were dropped; B hears the recent 7000s.
        let b = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/b-1")[0].1);
        assert!((mean(&b) - 7000.0).abs() < 300.0, "stale audio must be dropped, got {}", mean(&b));
        mixer.shutdown();
    }

    #[tokio::test]
    async fn non_g711_frames_dropped() {
        let mixer = ConferenceMixer::start("t-non-g711".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;

        // Opus-ish payload type: dropped, never mixed.
        mixer.ingest("MOCK/a-1", 96, &[0x55u8; 320]);
        tick_and_write(&mixer).await;
        let b = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/b-1")[0].1);
        assert!(b.iter().all(|&s| s.abs() < 50));
        mixer.shutdown();
    }

    #[tokio::test]
    async fn leave_keeps_mixing_for_remaining() {
        let mixer = ConferenceMixer::start("t-leave".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/c-1", drv.clone()).await;
        assert_eq!(mixer.participant_count(), 3);

        mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 6000));
        mixer.ingest("MOCK/b-1", 0, &tone(MixCodec::Ulaw, 2000));
        tick_and_write(&mixer).await;
        let c = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/c-1")[0].1);
        assert!((mean(&c) - 8000.0).abs() < 300.0);

        // A leaves; B and C keep mixing, and A receives nothing further.
        mixer.remove_participant("MOCK/a-1");
        assert_eq!(mixer.participant_count(), 2);
        let a_writes_before = drv.written_for("MOCK/a-1").len();

        mixer.ingest("MOCK/b-1", 0, &tone(MixCodec::Ulaw, 2000));
        tick_and_write(&mixer).await;
        let c = decode_all(MixCodec::Ulaw, &drv.written_for("MOCK/c-1")[1].1);
        assert!(
            (mean(&c) - 2000.0).abs() < 150.0,
            "C must keep hearing B after A left, got {}",
            mean(&c)
        );
        assert_eq!(
            drv.written_for("MOCK/a-1").len(),
            a_writes_before,
            "a removed leg must receive no more frames"
        );
        mixer.shutdown();
    }

    #[tokio::test]
    async fn shutdown_stops_tick_task_and_readers() {
        let mixer = ConferenceMixer::start("t-shutdown".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;

        // Grab the reader handle state through shutdown.
        assert!(mixer.is_running());
        mixer.shutdown();
        assert!(!mixer.is_running());
        assert_eq!(mixer.participant_count(), 0);

        // The tick task was aborted/ended; ticks stop advancing.
        let ticks_a = mixer.tick_count();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(mixer.tick_count(), ticks_a, "ticks must stop after shutdown");
    }

    #[tokio::test]
    async fn live_tick_task_writes_at_cadence() {
        // End-to-end through the real spawned tick task (wall-clock).
        let mixer = ConferenceMixer::start("t-live-cadence".into());
        let drv = MockDriver::with_format(Some(0));
        mixer.add_participant_with_driver("MOCK/a-1", drv.clone()).await;
        mixer.add_participant_with_driver("MOCK/b-1", drv.clone()).await;

        // Feed A ~500 ms of tone, in advance (cap keeps the last ~200 ms;
        // plenty for several live ticks).
        for _ in 0..10 {
            mixer.ingest("MOCK/a-1", 0, &tone(MixCodec::Ulaw, 5000));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;

        let b_writes = drv.written_for("MOCK/b-1");
        // ~7 ticks in 150 ms; accept generous slop for CI schedulers.
        assert!(
            b_writes.len() >= 3,
            "expected several 20 ms frames from the live mixer task, got {}",
            b_writes.len()
        );
        let voiced: Vec<i16> = b_writes
            .iter()
            .flat_map(|(_, p)| decode_all(MixCodec::Ulaw, p))
            .filter(|s| s.abs() > 500)
            .collect();
        assert!(
            !voiced.is_empty() && (mean(&voiced) - 5000.0).abs() < 300.0,
            "B must hear A's tone from the live task"
        );
        mixer.shutdown();
    }

    #[tokio::test]
    async fn registry_get_or_create_and_shutdown() {
        let name = "t-registry-lifecycle";
        let m1 = get_or_create_mixer(name);
        let m2 = get_or_create_mixer(name);
        assert!(Arc::ptr_eq(&m1, &m2), "same conference must share a mixer");
        assert!(get_mixer(name).is_some());

        shutdown_mixer(name);
        assert!(get_mixer(name).is_none());
        assert!(!m1.is_running());

        // A new conference under the same name gets a fresh mixer.
        let m3 = get_or_create_mixer(name);
        assert!(!Arc::ptr_eq(&m1, &m3));
        assert!(m3.is_running());
        shutdown_mixer(name);
    }
}
