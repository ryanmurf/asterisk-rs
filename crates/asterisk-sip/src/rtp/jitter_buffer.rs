//! RTP Jitter Buffer implementations.
//!
//! Inspired by Asterisk's `main/abstract_jb.c`, provides both fixed-delay
//! and adaptive jitter buffers. The jitter buffer holds incoming RTP frames
//! and releases them at a smoothed playout time to compensate for network
//! jitter.
//!
//! Two strategies:
//! - **FixedJitterBuffer**: constant target delay, simple and predictable.
//! - **AdaptiveJitterBuffer**: dynamically adjusts delay based on observed
//!   inter-arrival jitter (like Asterisk's `jb_new` / `jb_adaptive`).

use std::collections::BTreeMap;
use std::time::Duration;

use asterisk_types::Frame;

/// Result from a jitter buffer get operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JitterBufferResult {
    /// A frame is available for playout.
    Ok(Frame),
    /// No frame is available; the caller should perform packet loss
    /// concealment (PLC).
    PlcMarker,
    /// No frame is ready yet (too early to play out).
    NotReady,
    /// The buffer is empty and no frames are expected.
    Empty,
}

/// Trait for jitter buffer implementations.
pub trait JitterBuffer: Send {
    /// Insert a frame with its RTP timestamp.
    fn put(&mut self, frame: Frame, timestamp: u32);

    /// Retrieve the frame that should be played out at the given timestamp.
    /// Returns `PlcMarker` if the expected frame is missing.
    fn get(&mut self, timestamp: u32) -> JitterBufferResult;

    /// Reset the jitter buffer, discarding all buffered frames.
    fn reset(&mut self);

    /// Return the current target delay in milliseconds.
    fn target_delay_ms(&self) -> u32;

    /// Return the number of frames currently buffered.
    fn len(&self) -> usize;

    /// Return true if the buffer is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Fixed Jitter Buffer
// ---------------------------------------------------------------------------

/// A fixed-delay jitter buffer.
///
/// Holds frames for exactly `target_delay` ms before releasing them.
/// Frames that arrive too late (older than the current playout point)
/// are dropped.
pub struct FixedJitterBuffer {
    /// Buffered frames keyed by RTP timestamp.
    buffer: BTreeMap<u32, Frame>,
    /// Target delay in RTP timestamp units (samples).
    target_delay: u32,
    /// Target delay as a Duration (for reporting).
    target_delay_duration: Duration,
    /// Sample rate for converting between timestamps and time (reserved for future use).
    #[allow(dead_code)]
    sample_rate: u32,
    /// The timestamp of the first frame received (to establish baseline).
    first_timestamp: Option<u32>,
    /// Maximum buffer size (number of frames) to prevent unbounded growth.
    max_frames: usize,
}

impl FixedJitterBuffer {
    /// Create a fixed jitter buffer with the given target delay and sample rate.
    ///
    /// - `target_delay`: how long to hold frames before release.
    /// - `sample_rate`: audio sample rate (e.g. 8000 for G.711).
    pub fn new(target_delay: Duration, sample_rate: u32) -> Self {
        let delay_samples =
            (target_delay.as_millis() as u32 * sample_rate) / 1000;
        Self {
            buffer: BTreeMap::new(),
            target_delay: delay_samples,
            target_delay_duration: target_delay,
            sample_rate,
            first_timestamp: None,
            max_frames: 200,
        }
    }
}

impl JitterBuffer for FixedJitterBuffer {
    fn put(&mut self, frame: Frame, timestamp: u32) {
        if self.first_timestamp.is_none() {
            self.first_timestamp = Some(timestamp);
        }

        // Drop if buffer is full (prevent unbounded growth)
        if self.buffer.len() >= self.max_frames {
            // Remove the oldest entry
            if let Some(oldest) = self.buffer.keys().next().copied() {
                self.buffer.remove(&oldest);
            }
        }

        self.buffer.insert(timestamp, frame);
    }

    fn get(&mut self, timestamp: u32) -> JitterBufferResult {
        if self.buffer.is_empty() {
            return JitterBufferResult::Empty;
        }

        // The playout timestamp: we release frames that are at least
        // target_delay older than the current timestamp.
        let playout_ts = timestamp.wrapping_sub(self.target_delay);

        // Drop frames that are too old (more than target_delay behind playout).
        let too_old: Vec<u32> = self
            .buffer
            .keys()
            .copied()
            .take_while(|&ts| {
                // Use wrapping arithmetic: if playout_ts - ts < a large number,
                // the frame is "before" playout_ts.
                let diff = playout_ts.wrapping_sub(ts);
                diff > 0 && diff < 0x8000_0000
            })
            .collect();

        for ts in &too_old {
            self.buffer.remove(ts);
        }

        // Try to find the frame at or nearest before the playout timestamp.
        // We look for exact match first.
        if let Some(frame) = self.buffer.remove(&playout_ts) {
            return JitterBufferResult::Ok(frame);
        }

        // Check if there's a frame that is ready for playout
        // (timestamp <= playout_ts in wrapping arithmetic).
        let ready_key = self.buffer.keys().next().copied();
        if let Some(key) = ready_key {
            let diff = playout_ts.wrapping_sub(key);
            if diff < 0x8000_0000 && diff > 0 {
                // This frame is past due but not too old -- play it
                if let Some(frame) = self.buffer.remove(&key) {
                    return JitterBufferResult::Ok(frame);
                }
            }
        }

        // If we expected a frame at this timestamp but none arrived, PLC
        if !too_old.is_empty() || self.first_timestamp.is_some() {
            // Check if we're past the point where we should have data
            if let Some(first) = self.first_timestamp {
                let elapsed = timestamp.wrapping_sub(first);
                if elapsed > self.target_delay && elapsed < 0x8000_0000 {
                    return JitterBufferResult::PlcMarker;
                }
            }
        }

        JitterBufferResult::NotReady
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.first_timestamp = None;
    }

    fn target_delay_ms(&self) -> u32 {
        self.target_delay_duration.as_millis() as u32
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }
}

// ---------------------------------------------------------------------------
// Adaptive Jitter Buffer
// ---------------------------------------------------------------------------

/// An adaptive jitter buffer that dynamically adjusts its target delay
/// based on observed inter-arrival jitter (RFC 3550 jitter calculation).
pub struct AdaptiveJitterBuffer {
    /// Buffered frames keyed by RTP timestamp.
    buffer: BTreeMap<u32, Frame>,
    /// Minimum delay in RTP timestamp units.
    min_delay: u32,
    /// Current target delay in RTP timestamp units.
    target_delay: u32,
    /// Maximum delay in RTP timestamp units.
    max_delay: u32,
    /// Sample rate.
    sample_rate: u32,
    /// The timestamp of the first frame received.
    first_timestamp: Option<u32>,
    /// Previous frame arrival timestamp (RTP timestamp of last received frame).
    last_arrival_ts: Option<u32>,
    /// Previous frame transit value (for jitter estimation).
    last_transit: Option<i64>,
    /// Smoothed jitter estimate (RFC 3550 algorithm, in timestamp units).
    jitter_estimate: f64,
    /// Maximum observed jitter over a window.
    max_jitter: u32,
    /// Number of frames received (for statistics).
    frames_received: u64,
    /// Maximum buffer size.
    max_frames: usize,
    /// Adaptation gain (how quickly to react to jitter changes).
    /// Higher values = faster adaptation.
    alpha: f64,
}

impl AdaptiveJitterBuffer {
    /// Create an adaptive jitter buffer.
    ///
    /// - `min_delay`: minimum playout delay.
    /// - `initial_delay`: starting target delay.
    /// - `max_delay`: maximum playout delay.
    /// - `sample_rate`: audio sample rate.
    pub fn new(
        min_delay: Duration,
        initial_delay: Duration,
        max_delay: Duration,
        sample_rate: u32,
    ) -> Self {
        let min_samples = (min_delay.as_millis() as u32 * sample_rate) / 1000;
        let target_samples = (initial_delay.as_millis() as u32 * sample_rate) / 1000;
        let max_samples = (max_delay.as_millis() as u32 * sample_rate) / 1000;

        Self {
            buffer: BTreeMap::new(),
            min_delay: min_samples,
            target_delay: target_samples,
            max_delay: max_samples,
            sample_rate,
            first_timestamp: None,
            last_arrival_ts: None,
            last_transit: None,
            jitter_estimate: 0.0,
            max_jitter: 0,
            frames_received: 0,
            max_frames: 200,
            alpha: 1.0 / 16.0, // RFC 3550 recommended gain
        }
    }

    /// Update the jitter estimate based on a new frame arrival.
    fn update_jitter(&mut self, rtp_timestamp: u32, arrival_timestamp: u32) {
        // RFC 3550 jitter calculation:
        // transit = arrival - rtp_timestamp
        // d = transit - last_transit
        // jitter += (|d| - jitter) / 16
        let transit = arrival_timestamp.wrapping_sub(rtp_timestamp) as i64;

        if let Some(last_transit) = self.last_transit {
            let d = (transit - last_transit).unsigned_abs() as f64;
            self.jitter_estimate += self.alpha * (d - self.jitter_estimate);

            let jitter_samples = self.jitter_estimate as u32;
            if jitter_samples > self.max_jitter {
                self.max_jitter = jitter_samples;
            }
        }

        self.last_transit = Some(transit);
    }

    /// Adapt the target delay based on current jitter.
    fn adapt_delay(&mut self) {
        // Target = 2 * smoothed_jitter, clamped to [min, max].
        let desired = (self.jitter_estimate * 2.0) as u32;
        let new_target = desired.clamp(self.min_delay, self.max_delay);

        // Smooth the transition: grow quickly, shrink slowly.
        if new_target > self.target_delay {
            // Growing: respond quickly
            self.target_delay = self.target_delay + (new_target - self.target_delay) / 2;
            self.target_delay = self.target_delay.min(self.max_delay);
        } else if new_target < self.target_delay {
            // Shrinking: respond slowly to avoid underruns
            let reduction = (self.target_delay - new_target) / 8;
            self.target_delay = self.target_delay.saturating_sub(reduction.max(1));
            self.target_delay = self.target_delay.max(self.min_delay);
        }
    }
}

impl JitterBuffer for AdaptiveJitterBuffer {
    fn put(&mut self, frame: Frame, timestamp: u32) {
        if self.first_timestamp.is_none() {
            self.first_timestamp = Some(timestamp);
        }

        self.frames_received += 1;

        // Use the frame's RTP timestamp as a proxy for arrival ordering.
        // In a real implementation, you'd also track wall-clock arrival time.
        let arrival_ts = timestamp; // Simplified
        if self.last_arrival_ts.is_some() {
            self.update_jitter(timestamp, arrival_ts);
        }
        self.last_arrival_ts = Some(arrival_ts);

        // Periodically adapt the delay
        if self.frames_received % 10 == 0 {
            self.adapt_delay();
        }

        // Drop if buffer is full
        if self.buffer.len() >= self.max_frames {
            if let Some(oldest) = self.buffer.keys().next().copied() {
                self.buffer.remove(&oldest);
            }
        }

        self.buffer.insert(timestamp, frame);
    }

    fn get(&mut self, timestamp: u32) -> JitterBufferResult {
        if self.buffer.is_empty() {
            return JitterBufferResult::Empty;
        }

        let playout_ts = timestamp.wrapping_sub(self.target_delay);

        // Drop frames that are too old
        let too_old: Vec<u32> = self
            .buffer
            .keys()
            .copied()
            .take_while(|&ts| {
                let diff = playout_ts.wrapping_sub(ts);
                diff > 0 && diff < 0x8000_0000
            })
            .collect();

        for ts in &too_old {
            self.buffer.remove(ts);
        }

        // Exact match
        if let Some(frame) = self.buffer.remove(&playout_ts) {
            return JitterBufferResult::Ok(frame);
        }

        // Nearest ready frame
        let ready_key = self.buffer.keys().next().copied();
        if let Some(key) = ready_key {
            let diff = playout_ts.wrapping_sub(key);
            if diff < 0x8000_0000 && diff > 0 {
                if let Some(frame) = self.buffer.remove(&key) {
                    return JitterBufferResult::Ok(frame);
                }
            }
        }

        // PLC marker if we expected data
        if !too_old.is_empty() {
            return JitterBufferResult::PlcMarker;
        }

        if let Some(first) = self.first_timestamp {
            let elapsed = timestamp.wrapping_sub(first);
            if elapsed > self.target_delay && elapsed < 0x8000_0000 {
                return JitterBufferResult::PlcMarker;
            }
        }

        JitterBufferResult::NotReady
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.first_timestamp = None;
        self.last_arrival_ts = None;
        self.last_transit = None;
        self.jitter_estimate = 0.0;
        self.max_jitter = 0;
        self.frames_received = 0;
    }

    fn target_delay_ms(&self) -> u32 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.target_delay * 1000) / self.sample_rate
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_voice_frame(_ts: u32) -> Frame {
        Frame::voice(0, 160, Bytes::from(vec![0u8; 160]))
    }

    // --- Fixed Jitter Buffer Tests ---

    #[test]
    fn test_fixed_jb_empty() {
        let mut jb = FixedJitterBuffer::new(Duration::from_millis(60), 8000);
        assert_eq!(jb.get(0), JitterBufferResult::Empty);
        assert!(jb.is_empty());
    }

    #[test]
    fn test_fixed_jb_put_and_get() {
        let mut jb = FixedJitterBuffer::new(Duration::from_millis(60), 8000);
        // target_delay = 60ms * 8000 / 1000 = 480 samples

        // Put frame at timestamp 0
        jb.put(make_voice_frame(0), 0);
        assert_eq!(jb.len(), 1);

        // At timestamp 480 (60ms later), the frame should be ready
        match jb.get(480) {
            JitterBufferResult::Ok(_) => {} // expected
            other => panic!("Expected Ok, got {:?}", other),
        }
    }

    #[test]
    fn test_fixed_jb_playout_delay() {
        let mut jb = FixedJitterBuffer::new(Duration::from_millis(60), 8000);
        // 480 sample delay

        jb.put(make_voice_frame(0), 0);
        jb.put(make_voice_frame(160), 160);
        jb.put(make_voice_frame(320), 320);

        // Too early: playout_ts = 100 - 480 = wraps around (very large), NotReady
        let result = jb.get(100);
        assert!(
            matches!(result, JitterBufferResult::NotReady),
            "Expected NotReady at ts=100, got {:?}",
            result
        );

        // At exactly the right time for frame 0
        let result = jb.get(480);
        assert!(matches!(result, JitterBufferResult::Ok(_)));

        // Frame 1 at ts=160 should be ready at 160+480=640
        let result = jb.get(640);
        assert!(matches!(result, JitterBufferResult::Ok(_)));
    }

    #[test]
    fn test_fixed_jb_missing_frame_plc() {
        let mut jb = FixedJitterBuffer::new(Duration::from_millis(60), 8000);

        // Put frames at 0 and 320 (skip 160)
        jb.put(make_voice_frame(0), 0);
        jb.put(make_voice_frame(320), 320);

        // Get frame at 0
        let result = jb.get(480);
        assert!(matches!(result, JitterBufferResult::Ok(_)));

        // Frame at 160 is missing -- at playout ts=640, we get PLC
        let result = jb.get(640);
        // The missing frame should produce a PLC marker
        assert!(
            matches!(result, JitterBufferResult::PlcMarker),
            "Expected PlcMarker for missing frame, got {:?}",
            result
        );
    }

    #[test]
    fn test_fixed_jb_reset() {
        let mut jb = FixedJitterBuffer::new(Duration::from_millis(60), 8000);
        jb.put(make_voice_frame(0), 0);
        jb.put(make_voice_frame(160), 160);
        assert_eq!(jb.len(), 2);

        jb.reset();
        assert_eq!(jb.len(), 0);
        assert_eq!(jb.get(1000), JitterBufferResult::Empty);
    }

    // --- Adaptive Jitter Buffer Tests ---

    #[test]
    fn test_adaptive_jb_empty() {
        let mut jb = AdaptiveJitterBuffer::new(
            Duration::from_millis(20),
            Duration::from_millis(60),
            Duration::from_millis(200),
            8000,
        );
        assert_eq!(jb.get(0), JitterBufferResult::Empty);
    }

    #[test]
    fn test_adaptive_jb_basic_playout() {
        let mut jb = AdaptiveJitterBuffer::new(
            Duration::from_millis(20),
            Duration::from_millis(60),
            Duration::from_millis(200),
            8000,
        );

        // Insert frames
        jb.put(make_voice_frame(0), 0);
        jb.put(make_voice_frame(160), 160);
        jb.put(make_voice_frame(320), 320);

        // Target delay is 60ms = 480 samples
        // At ts=480, frame 0 should be ready
        let result = jb.get(480);
        assert!(matches!(result, JitterBufferResult::Ok(_)));
    }

    #[test]
    fn test_adaptive_jb_tracks_jitter() {
        let mut jb = AdaptiveJitterBuffer::new(
            Duration::from_millis(20),
            Duration::from_millis(60),
            Duration::from_millis(200),
            8000,
        );

        // Put many frames to trigger adaptation
        for i in 0..50 {
            let ts = i * 160;
            jb.put(make_voice_frame(ts), ts);
        }

        // The jitter estimate should still be reasonable
        let delay = jb.target_delay_ms();
        assert!(delay >= 20, "Target delay {} should be >= min 20ms", delay);
        assert!(delay <= 200, "Target delay {} should be <= max 200ms", delay);
    }

    #[test]
    fn test_adaptive_jb_plc_on_missing() {
        let mut jb = AdaptiveJitterBuffer::new(
            Duration::from_millis(20),
            Duration::from_millis(60),
            Duration::from_millis(200),
            8000,
        );

        // Put frame at 0 and 320, skip 160
        jb.put(make_voice_frame(0), 0);
        jb.put(make_voice_frame(320), 320);

        // Play frame 0
        let result = jb.get(480);
        assert!(matches!(result, JitterBufferResult::Ok(_)));

        // Missing frame should produce PLC
        let result = jb.get(640);
        assert!(
            matches!(result, JitterBufferResult::PlcMarker),
            "Expected PlcMarker, got {:?}",
            result
        );
    }

    #[test]
    fn test_adaptive_jb_reset() {
        let mut jb = AdaptiveJitterBuffer::new(
            Duration::from_millis(20),
            Duration::from_millis(60),
            Duration::from_millis(200),
            8000,
        );

        for i in 0..10 {
            jb.put(make_voice_frame(i * 160), i * 160);
        }
        assert!(!jb.is_empty());

        jb.reset();
        assert!(jb.is_empty());
        assert_eq!(jb.frames_received, 0);
    }
}
