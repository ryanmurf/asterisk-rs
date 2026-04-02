//! Softmix bridge technology -- real multi-party audio mixing.
//!
//! Port of bridge_softmix.c from Asterisk C. This bridge technology
//! mixes audio from all participants, producing a unique mix for each
//! channel that excludes that channel's own audio (so you don't hear
//! yourself). The mixing is timer-driven at a configurable interval.
//!
//! Key formulas from C source:
//! - SOFTMIX_DATALEN(rate, interval) = (rate/50) * (interval / 10)
//! - SOFTMIX_SAMPLES(rate, interval) = SOFTMIX_DATALEN(rate, interval) / 2
//!   (but since we work in i16 samples, we just use sample count directly)

use super::{Bridge, BridgeChannel, BridgeTechnology};
use asterisk_types::{AsteriskResult, BridgeCapability, Frame};
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time;
use tracing::{debug, info, trace};

/// Default mixing interval in milliseconds.
const DEFAULT_SOFTMIX_INTERVAL_MS: u32 = 20;

/// Minimum sample rate supported.
const SOFTMIX_MIN_SAMPLE_RATE: u32 = 8000;

/// Default internal sample rate.
const DEFAULT_INTERNAL_RATE: u32 = 8000;

/// Compute the number of samples for a given rate and interval.
///
/// At 8000 Hz with 20ms interval: 160 samples.
/// At 16000 Hz with 20ms interval: 320 samples.
pub fn softmix_samples(rate: u32, interval_ms: u32) -> usize {
    (rate as usize * interval_ms as usize) / 1000
}

/// Per-channel softmix data: audio buffer and state.
#[derive(Debug, Clone)]
pub struct SoftmixChannelData {
    /// The channel's contributed audio samples (SLIN, i16).
    /// This buffer holds samples from the most recent write.
    pub our_buf: Vec<i16>,
    /// Whether this channel has contributed audio in the current interval.
    pub have_audio: bool,
    /// Channel name for debugging.
    pub channel_name: String,
}

impl SoftmixChannelData {
    pub fn new(channel_name: String, num_samples: usize) -> Self {
        Self {
            our_buf: vec![0i16; num_samples],
            have_audio: false,
            channel_name,
        }
    }

    /// Reset the audio buffer for a new mixing interval.
    pub fn clear(&mut self) {
        for s in self.our_buf.iter_mut() {
            *s = 0;
        }
        self.have_audio = false;
    }
}

/// Shared mixing state for the softmix bridge.
///
/// This is the data that the mixing timer task and the bridge technology
/// both access.
#[derive(Debug)]
pub struct SoftmixData {
    /// Internal mixing sample rate.
    pub internal_rate: u32,
    /// Mixing interval in milliseconds.
    pub mixing_interval_ms: u32,
    /// Per-channel audio buffers, keyed by channel_id string.
    pub channel_buffers: HashMap<String, SoftmixChannelData>,
    /// The number of samples per mixing interval at the current rate.
    pub num_samples: usize,
    /// Whether the mixing thread should keep running.
    pub running: bool,
    /// Output frames for each channel after mixing, keyed by channel_id.
    /// These are picked up by the event loop.
    pub output_frames: HashMap<String, Vec<i16>>,
}

impl SoftmixData {
    pub fn new(internal_rate: u32, mixing_interval_ms: u32) -> Self {
        let num_samples = softmix_samples(internal_rate, mixing_interval_ms);
        Self {
            internal_rate,
            mixing_interval_ms,
            channel_buffers: HashMap::new(),
            num_samples,
            running: false,
            output_frames: HashMap::new(),
        }
    }

    /// Perform one mixing iteration.
    ///
    /// For each channel:
    /// 1. Sum all channels' audio into a total mix buffer.
    /// 2. For each channel, subtract that channel's own contribution.
    /// 3. Store the per-channel result in `output_frames`.
    ///
    /// This implements the "mix-minus" algorithm from softmix.
    pub fn mix(&mut self) {
        let num_samples = self.num_samples;
        let channel_ids: Vec<String> = self.channel_buffers.keys().cloned().collect();

        if channel_ids.is_empty() {
            return;
        }

        // Step 1: Compute the total sum of all channels.
        let mut total_mix = vec![0i32; num_samples];
        for chan_data in self.channel_buffers.values() {
            if chan_data.have_audio {
                #[allow(clippy::needless_range_loop)]
                for i in 0..num_samples {
                    if i < chan_data.our_buf.len() {
                        total_mix[i] += chan_data.our_buf[i] as i32;
                    }
                }
            }
        }

        // Step 2: For each channel, produce output = total - own contribution.
        self.output_frames.clear();
        for chan_id in &channel_ids {
            let mut output = vec![0i16; num_samples];
            let chan_data = self.channel_buffers.get(chan_id);

            for i in 0..num_samples {
                let own_contribution = if let Some(cd) = chan_data {
                    if cd.have_audio && i < cd.our_buf.len() {
                        cd.our_buf[i] as i32
                    } else {
                        0
                    }
                } else {
                    0
                };

                // Saturated subtract: mix minus own, clamped to i16 range.
                let mixed = total_mix[i] - own_contribution;
                output[i] = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }

            self.output_frames.insert(chan_id.clone(), output);
        }

        // Step 3: Clear channel buffers for next interval.
        for (_, chan_data) in self.channel_buffers.iter_mut() {
            chan_data.clear();
        }
    }
}

/// Softmix bridge technology with real audio mixing.
///
/// Implements the mix-minus algorithm: for each channel, the output
/// is the sum of all other channels' audio. Mixing runs on a timer
/// at a configurable interval (default 20ms).
#[derive(Debug)]
pub struct SoftmixBridgeTech {
    /// Shared mixing state.
    pub data: Arc<Mutex<SoftmixData>>,
    /// Internal sample rate (8000, 16000, etc.).
    pub internal_sample_rate: u32,
    /// Mixing interval in milliseconds.
    pub mixing_interval_ms: u32,
}

impl SoftmixBridgeTech {
    /// Create a new softmix bridge technology with default parameters.
    pub fn new() -> Self {
        let data = SoftmixData::new(DEFAULT_INTERNAL_RATE, DEFAULT_SOFTMIX_INTERVAL_MS);
        Self {
            data: Arc::new(Mutex::new(data)),
            internal_sample_rate: DEFAULT_INTERNAL_RATE,
            mixing_interval_ms: DEFAULT_SOFTMIX_INTERVAL_MS,
        }
    }

    /// Create with specific sample rate and interval.
    pub fn with_params(internal_rate: u32, interval_ms: u32) -> Self {
        let rate = internal_rate.max(SOFTMIX_MIN_SAMPLE_RATE);
        let interval = if interval_ms == 0 {
            DEFAULT_SOFTMIX_INTERVAL_MS
        } else {
            interval_ms
        };
        let data = SoftmixData::new(rate, interval);
        Self {
            data: Arc::new(Mutex::new(data)),
            internal_sample_rate: rate,
            mixing_interval_ms: interval,
        }
    }

    /// Get a reference to the shared mixing data (for testing / external access).
    pub fn data(&self) -> &Arc<Mutex<SoftmixData>> {
        &self.data
    }

    /// Start the mixing timer task.
    ///
    /// This spawns a tokio task that wakes up every `mixing_interval_ms`
    /// milliseconds and performs a mix iteration on the shared data.
    pub fn start_mixing_task(&self) -> tokio::task::JoinHandle<()> {
        let data = self.data.clone();
        let interval = Duration::from_millis(self.mixing_interval_ms as u64);

        tokio::spawn(async move {
            let mut ticker = time::interval(interval);
            loop {
                ticker.tick().await;

                let mut d = data.lock().await;
                if !d.running {
                    trace!("Softmix mixing task: stopped");
                    break;
                }
                d.mix();
            }
        })
    }
}

impl Default for SoftmixBridgeTech {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BridgeTechnology for SoftmixBridgeTech {
    fn name(&self) -> &str {
        "softmix"
    }

    fn capabilities(&self) -> BridgeCapability {
        BridgeCapability::MULTI_MIX
    }

    fn preference(&self) -> u32 {
        // AST_BRIDGE_PREFERENCE_BASE_MULTIMIX
        50
    }

    async fn create(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        debug!(bridge = %bridge.name, "SoftmixBridgeTech: creating bridge");
        bridge.technology = "softmix".to_string();
        Ok(())
    }

    async fn start(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        info!(bridge = %bridge.name, "SoftmixBridgeTech: starting mixing");
        let mut data = self.data.lock().await;
        data.running = true;
        // The actual mixing task is started separately via start_mixing_task().
        Ok(())
    }

    async fn stop(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        info!(bridge = %bridge.name, "SoftmixBridgeTech: stopping mixing");
        let mut data = self.data.lock().await;
        data.running = false;
        data.channel_buffers.clear();
        data.output_frames.clear();
        Ok(())
    }

    async fn join(
        &self,
        bridge: &mut Bridge,
        channel: &BridgeChannel,
    ) -> AsteriskResult<()> {
        let mut data = self.data.lock().await;
        let num_samples = data.num_samples;
        let chan_data =
            SoftmixChannelData::new(channel.channel_name.clone(), num_samples);
        data.channel_buffers
            .insert(channel.channel_id.as_str().to_string(), chan_data);
        debug!(
            channel = %channel.channel_name,
            bridge = %bridge.name,
            num_samples = num_samples,
            "SoftmixBridgeTech: channel joined, buffer allocated"
        );
        Ok(())
    }

    async fn leave(
        &self,
        bridge: &mut Bridge,
        channel: &BridgeChannel,
    ) -> AsteriskResult<()> {
        let mut data = self.data.lock().await;
        data.channel_buffers
            .remove(channel.channel_id.as_str());
        data.output_frames
            .remove(channel.channel_id.as_str());
        debug!(
            channel = %channel.channel_name,
            bridge = %bridge.name,
            "SoftmixBridgeTech: channel left, buffer freed"
        );
        Ok(())
    }

    async fn write_frame(
        &self,
        _bridge: &mut Bridge,
        from_channel: &BridgeChannel,
        frame: &Frame,
    ) -> AsteriskResult<()> {
        // Only process voice frames for mixing.
        if let Frame::Voice { data, samples: _, .. } = frame {
            let mut mix_data = self.data.lock().await;
            let chan_id = from_channel.channel_id.as_str();

            if let Some(chan_data) = mix_data.channel_buffers.get_mut(chan_id) {
                // Convert the raw audio bytes to i16 samples (assuming SLIN/PCM16LE).
                let sample_count = data.len() / 2;
                let num_to_copy = sample_count.min(chan_data.our_buf.len());

                for i in 0..num_to_copy {
                    chan_data.our_buf[i] =
                        i16::from_le_bytes([data[i * 2], data[i * 2 + 1]]);
                }

                chan_data.have_audio = true;
                trace!(
                    channel = %from_channel.channel_name,
                    samples = sample_count,
                    "SoftmixBridgeTech: buffered audio for mixing"
                );
            }

            Ok(())
        } else {
            // Non-voice frames in softmix: pass to all other channels.
            // In a full implementation, non-voice frames (DTMF, text, control)
            // would be queued directly to all other bridge channels.
            Ok(())
        }
    }
}

/// Retrieve the mixed output for a specific channel after a mix iteration.
///
/// Returns the mixed audio as a Voice frame (SLIN format) or None
/// if no output is available.
pub async fn get_mixed_output(
    data: &Arc<Mutex<SoftmixData>>,
    channel_id: &str,
    codec_id: u32,
) -> Option<Frame> {
    let d = data.lock().await;
    if let Some(samples) = d.output_frames.get(channel_id) {
        // Check if the output is all silence.
        let is_silence = samples.iter().all(|&s| s == 0);
        if is_silence {
            return None;
        }

        // Convert i16 samples back to bytes (little-endian PCM16).
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            bytes.push(sample as u8);
            bytes.push((sample >> 8) as u8);
        }

        Some(Frame::Voice {
            codec_id,
            samples: samples.len() as u32,
            data: Bytes::from(bytes),
            timestamp_ms: 0,
            seqno: -1,
            stream_num: 0,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softmix_samples_8k_20ms() {
        assert_eq!(softmix_samples(8000, 20), 160);
    }

    #[test]
    fn test_softmix_samples_16k_20ms() {
        assert_eq!(softmix_samples(16000, 20), 320);
    }

    #[test]
    fn test_softmix_samples_8k_40ms() {
        assert_eq!(softmix_samples(8000, 40), 320);
    }

    #[test]
    fn test_softmix_data_new() {
        let data = SoftmixData::new(8000, 20);
        assert_eq!(data.internal_rate, 8000);
        assert_eq!(data.mixing_interval_ms, 20);
        assert_eq!(data.num_samples, 160);
        assert!(data.channel_buffers.is_empty());
    }

    #[test]
    fn test_softmix_mix_empty() {
        let mut data = SoftmixData::new(8000, 20);
        data.mix(); // Should not panic with no channels.
        assert!(data.output_frames.is_empty());
    }

    #[test]
    fn test_softmix_mix_single_channel_silence() {
        let mut data = SoftmixData::new(8000, 20);
        data.channel_buffers.insert(
            "chan1".to_string(),
            SoftmixChannelData::new("chan1".to_string(), 160),
        );
        // No audio contributed.
        data.mix();
        let output = data.output_frames.get("chan1").unwrap();
        // Output should be all zeros (silence) since the only channel
        // contributes nothing and there is nothing else to mix.
        assert!(output.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_softmix_mix_two_channels() {
        let mut data = SoftmixData::new(8000, 20);
        let num_samples = 160;

        // Channel A: contributes a constant tone of 1000.
        let mut chan_a = SoftmixChannelData::new("chanA".to_string(), num_samples);
        for s in chan_a.our_buf.iter_mut() {
            *s = 1000;
        }
        chan_a.have_audio = true;
        data.channel_buffers
            .insert("chanA".to_string(), chan_a);

        // Channel B: contributes a constant tone of 2000.
        let mut chan_b = SoftmixChannelData::new("chanB".to_string(), num_samples);
        for s in chan_b.our_buf.iter_mut() {
            *s = 2000;
        }
        chan_b.have_audio = true;
        data.channel_buffers
            .insert("chanB".to_string(), chan_b);

        data.mix();

        // Channel A should hear B only (total=3000, minus own=1000, result=2000).
        let output_a = data.output_frames.get("chanA").unwrap();
        assert_eq!(output_a[0], 2000);

        // Channel B should hear A only (total=3000, minus own=2000, result=1000).
        let output_b = data.output_frames.get("chanB").unwrap();
        assert_eq!(output_b[0], 1000);
    }

    #[test]
    fn test_softmix_mix_three_channels() {
        let mut data = SoftmixData::new(8000, 20);
        let num_samples = 160;

        let mut chan_a = SoftmixChannelData::new("chanA".to_string(), num_samples);
        for s in chan_a.our_buf.iter_mut() {
            *s = 100;
        }
        chan_a.have_audio = true;
        data.channel_buffers.insert("chanA".to_string(), chan_a);

        let mut chan_b = SoftmixChannelData::new("chanB".to_string(), num_samples);
        for s in chan_b.our_buf.iter_mut() {
            *s = 200;
        }
        chan_b.have_audio = true;
        data.channel_buffers.insert("chanB".to_string(), chan_b);

        let mut chan_c = SoftmixChannelData::new("chanC".to_string(), num_samples);
        for s in chan_c.our_buf.iter_mut() {
            *s = 300;
        }
        chan_c.have_audio = true;
        data.channel_buffers.insert("chanC".to_string(), chan_c);

        data.mix();

        // Total = 100 + 200 + 300 = 600
        // A hears B+C = 600 - 100 = 500
        let output_a = data.output_frames.get("chanA").unwrap();
        assert_eq!(output_a[0], 500);

        // B hears A+C = 600 - 200 = 400
        let output_b = data.output_frames.get("chanB").unwrap();
        assert_eq!(output_b[0], 400);

        // C hears A+B = 600 - 300 = 300
        let output_c = data.output_frames.get("chanC").unwrap();
        assert_eq!(output_c[0], 300);
    }

    #[test]
    fn test_softmix_saturation() {
        let mut data = SoftmixData::new(8000, 20);
        let num_samples = 4;

        let mut chan_a = SoftmixChannelData::new("chanA".to_string(), num_samples);
        chan_a.our_buf[0] = i16::MAX;
        chan_a.have_audio = true;
        data.channel_buffers.insert("chanA".to_string(), chan_a);

        let mut chan_b = SoftmixChannelData::new("chanB".to_string(), num_samples);
        chan_b.our_buf[0] = i16::MAX;
        chan_b.have_audio = true;
        data.channel_buffers.insert("chanB".to_string(), chan_b);

        data.mix();

        // A hears B: MAX (total = 2*MAX, minus own MAX = MAX).
        let output_a = data.output_frames.get("chanA").unwrap();
        assert_eq!(output_a[0], i16::MAX);

        // B hears A: MAX.
        let output_b = data.output_frames.get("chanB").unwrap();
        assert_eq!(output_b[0], i16::MAX);
    }

    #[test]
    fn test_softmix_channel_data_clear() {
        let mut cd = SoftmixChannelData::new("test".to_string(), 160);
        cd.our_buf[0] = 1234;
        cd.have_audio = true;
        cd.clear();
        assert!(!cd.have_audio);
        assert_eq!(cd.our_buf[0], 0);
    }

    #[test]
    fn test_softmix_bridge_tech_params() {
        let tech = SoftmixBridgeTech::with_params(16000, 20);
        assert_eq!(tech.internal_sample_rate, 16000);
        assert_eq!(tech.mixing_interval_ms, 20);
    }

    #[test]
    fn test_softmix_bridge_tech_default() {
        let tech = SoftmixBridgeTech::new();
        assert_eq!(tech.internal_sample_rate, 8000);
        assert_eq!(tech.mixing_interval_ms, 20);
        assert_eq!(tech.name(), "softmix");
        assert_eq!(tech.capabilities(), BridgeCapability::MULTI_MIX);
        assert_eq!(tech.preference(), 50);
    }
}
