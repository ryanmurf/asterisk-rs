//! Tone detection using Goertzel algorithm.
//!
//! Port of `res/res_tonedetect.c`. Provides the TONE_DETECT() dialplan
//! function and the WaitForTone/ToneScan applications. Uses the Goertzel
//! algorithm for efficient single-frequency detection in audio frames.

use std::f64::consts::PI;

use tracing::debug;

// ---------------------------------------------------------------------------
// Goertzel detector
// ---------------------------------------------------------------------------

/// A Goertzel algorithm frequency detector.
///
/// The Goertzel algorithm is an efficient method for computing a single
/// DFT bin, making it ideal for detecting a specific frequency in an
/// audio stream. It requires O(N) multiplications per block versus
/// O(N log N) for a full FFT.
#[derive(Debug, Clone)]
pub struct GoertzelDetector {
    /// Target frequency in Hz.
    pub frequency: f64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Block size (number of samples per detection cycle).
    pub block_size: u32,
    /// Precomputed coefficient (2 * cos(2*PI*k/N)).
    coeff: f64,
    /// Internal state: s1 (previous sample).
    s1: f64,
    /// Internal state: s2 (two samples ago).
    s2: f64,
    /// Samples processed in current block.
    samples_processed: u32,
}

impl GoertzelDetector {
    /// Create a new Goertzel detector for the given frequency.
    pub fn new(frequency: f64, sample_rate: u32, block_size: u32) -> Self {
        let k = (0.5 + (block_size as f64 * frequency / sample_rate as f64)).floor();
        let omega = 2.0 * PI * k / block_size as f64;
        let coeff = 2.0 * omega.cos();

        Self {
            frequency,
            sample_rate,
            block_size,
            coeff,
            s1: 0.0,
            s2: 0.0,
            samples_processed: 0,
        }
    }

    /// Process a single audio sample.
    pub fn process_sample(&mut self, sample: f64) {
        let s0 = self.coeff * self.s1 - self.s2 + sample;
        self.s2 = self.s1;
        self.s1 = s0;
        self.samples_processed += 1;
    }

    /// Process a block of 16-bit linear PCM samples.
    pub fn process_samples(&mut self, samples: &[i16]) {
        for &sample in samples {
            self.process_sample(sample as f64);
        }
    }

    /// Check if a full block has been processed.
    pub fn block_complete(&self) -> bool {
        self.samples_processed >= self.block_size
    }

    /// Compute the magnitude squared of the detected frequency.
    ///
    /// Call this after processing a complete block of samples.
    pub fn magnitude_squared(&self) -> f64 {
        self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2
    }

    /// Compute the magnitude in dB relative to full scale.
    pub fn magnitude_db(&self) -> f64 {
        let mag_sq = self.magnitude_squared();
        if mag_sq <= 0.0 {
            return -96.0; // floor
        }
        10.0 * mag_sq.log10()
    }

    /// Reset the detector state for the next block.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
        self.samples_processed = 0;
    }
}

// ---------------------------------------------------------------------------
// Tone detection configuration
// ---------------------------------------------------------------------------

/// Default detection threshold in dB.
pub const DEFAULT_THRESHOLD_DB: f64 = 16.0;

/// Default minimum duration in milliseconds.
pub const DEFAULT_DURATION_MS: u32 = 500;

/// Configuration for the TONE_DETECT() function.
#[derive(Debug, Clone)]
pub struct ToneDetectConfig {
    /// Frequency to detect (Hz).
    pub frequency: f64,
    /// Minimum duration in milliseconds.
    pub duration_ms: u32,
    /// Detection threshold in dB.
    pub threshold_db: f64,
    /// Maximum time to wait (seconds, 0 = forever).
    pub timeout_secs: u32,
    /// Number of times the tone should be detected.
    pub times: u32,
    /// Whether to squelch (remove) the detected tone.
    pub squelch: bool,
    /// Goto destination when tone is detected.
    pub goto_target: Option<String>,
}

impl ToneDetectConfig {
    pub fn new(frequency: f64) -> Self {
        Self {
            frequency,
            duration_ms: DEFAULT_DURATION_MS,
            threshold_db: DEFAULT_THRESHOLD_DB,
            timeout_secs: 0,
            times: 1,
            squelch: false,
            goto_target: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tone detection state
// ---------------------------------------------------------------------------

/// State tracker for continuous tone detection.
#[derive(Debug)]
pub struct ToneDetectState {
    /// Goertzel detector.
    pub detector: GoertzelDetector,
    /// Configuration.
    pub config: ToneDetectConfig,
    /// How many consecutive blocks the tone has been detected.
    pub consecutive_hits: u32,
    /// How many times the tone has been detected in total.
    pub total_detections: u32,
    /// Whether we are currently detecting a tone.
    pub detecting: bool,
}

impl ToneDetectState {
    pub fn new(config: ToneDetectConfig, sample_rate: u32) -> Self {
        let block_size = sample_rate / 50; // 20ms blocks
        Self {
            detector: GoertzelDetector::new(config.frequency, sample_rate, block_size),
            config,
            consecutive_hits: 0,
            total_detections: 0,
            detecting: false,
        }
    }

    /// Process audio samples and check for tone detection.
    ///
    /// Returns true if the tone has been detected the required number of times.
    pub fn process(&mut self, samples: &[i16]) -> bool {
        self.detector.process_samples(samples);

        if self.detector.block_complete() {
            let db = self.detector.magnitude_db();
            let detected = db >= self.config.threshold_db;

            if detected {
                self.consecutive_hits += 1;
                let required_blocks = (self.config.duration_ms as f64
                    / (self.detector.block_size as f64 / self.detector.sample_rate as f64 * 1000.0))
                    .ceil() as u32;

                if self.consecutive_hits >= required_blocks && !self.detecting {
                    self.detecting = true;
                    self.total_detections += 1;
                    debug!(
                        freq = self.config.frequency,
                        count = self.total_detections,
                        "Tone detected"
                    );
                }
            } else {
                self.consecutive_hits = 0;
                self.detecting = false;
            }

            self.detector.reset();
        }

        self.total_detections >= self.config.times
    }
}

// ---------------------------------------------------------------------------
// WaitForTone result
// ---------------------------------------------------------------------------

/// Result of a WaitForTone operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitForToneResult {
    Success,
    Timeout,
    Hangup,
    Error,
}

impl WaitForToneResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Timeout => "TIMEOUT",
            Self::Hangup => "HANGUP",
            Self::Error => "ERROR",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goertzel_creation() {
        let det = GoertzelDetector::new(440.0, 8000, 160);
        assert_eq!(det.frequency, 440.0);
        assert_eq!(det.sample_rate, 8000);
        assert_eq!(det.block_size, 160);
        assert_eq!(det.samples_processed, 0);
    }

    #[test]
    fn test_goertzel_reset() {
        let mut det = GoertzelDetector::new(440.0, 8000, 160);
        det.process_sample(1000.0);
        det.process_sample(2000.0);
        assert_ne!(det.s1, 0.0);
        det.reset();
        assert_eq!(det.s1, 0.0);
        assert_eq!(det.s2, 0.0);
        assert_eq!(det.samples_processed, 0);
    }

    #[test]
    fn test_goertzel_block_complete() {
        let mut det = GoertzelDetector::new(440.0, 8000, 4);
        assert!(!det.block_complete());
        for _ in 0..4 {
            det.process_sample(0.0);
        }
        assert!(det.block_complete());
    }

    #[test]
    fn test_goertzel_silence() {
        let mut det = GoertzelDetector::new(440.0, 8000, 160);
        let silence = vec![0i16; 160];
        det.process_samples(&silence);
        assert!(det.block_complete());
        // Silence should give very low magnitude
        assert!(det.magnitude_db() < DEFAULT_THRESHOLD_DB);
    }

    #[test]
    fn test_tone_detect_config() {
        let config = ToneDetectConfig::new(2600.0);
        assert_eq!(config.frequency, 2600.0);
        assert_eq!(config.duration_ms, DEFAULT_DURATION_MS);
        assert_eq!(config.times, 1);
    }

    #[test]
    fn test_wait_for_tone_result() {
        assert_eq!(WaitForToneResult::Success.as_str(), "SUCCESS");
        assert_eq!(WaitForToneResult::Timeout.as_str(), "TIMEOUT");
    }
}
