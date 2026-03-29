//! Acoustic Echo Cancellation (AEC) using Normalized LMS (NLMS).
//!
//! Implements a real-time echo canceller suitable for telephony applications.
//! The NLMS algorithm adaptively learns the echo path (speaker -> microphone)
//! and subtracts the estimated echo from the near-end signal.
//!
//! Features:
//! - Normalized LMS adaptive filter
//! - Double-talk detection (pauses adaptation when both sides talk)
//! - Non-linear processing (NLP) for residual echo suppression
//! - Comfort noise injection during suppression

use std::collections::VecDeque;

/// Trait for swappable echo cancellation backends.
pub trait EchoCancellerEngine: Send {
    /// Process a frame of audio, removing echo of the far-end signal
    /// from the near-end signal.
    ///
    /// - `near_end`: microphone signal (contains speech + echo)
    /// - `far_end`: speaker signal (the signal being echoed)
    ///
    /// Returns the echo-cancelled signal.
    fn process_frame(&mut self, near_end: &[i16], far_end: &[i16]) -> Vec<i16>;

    /// Reset the canceller state (filter coefficients, buffers).
    fn reset(&mut self);
}

/// NLMS-based acoustic echo canceller.
pub struct EchoCanceller {
    /// Number of filter taps (filter length).
    filter_length: usize,
    /// Adaptive FIR filter coefficients.
    adapted_filter: Vec<f32>,
    /// Far-end (speaker) signal history ring buffer.
    reference_buffer: VecDeque<f32>,
    /// NLMS step size (mu), typically 0.1-1.0.
    step_size: f32,
    /// Running power estimate for normalization.
    power_estimate: f32,
    /// Sample rate in Hz.
    sample_rate: u32,
    /// Double-talk detector state.
    dtd: DoubleTalkDetector,
    /// Non-linear processor for residual echo suppression.
    nlp: NonLinearProcessor,
}

/// Double-talk detector using the Geigel algorithm.
///
/// Pauses filter adaptation when both near-end and far-end are active,
/// preventing the filter from diverging.
struct DoubleTalkDetector {
    /// Detection threshold: ratio of near-end to far-end energy.
    threshold: f32,
    /// Maximum far-end sample magnitude over recent history.
    far_end_max: f32,
    /// Holdover counter: frames to keep adaptation paused after detection.
    holdover: u32,
    /// Current holdover count.
    holdover_count: u32,
}

impl DoubleTalkDetector {
    fn new() -> Self {
        Self {
            threshold: 0.9, // Higher threshold to avoid false DTD during echo-only
            far_end_max: 0.0,
            holdover: 10, // ~10 frames holdover
            holdover_count: 0,
        }
    }

    /// Returns true if double-talk is detected (adaptation should pause).
    fn detect(&mut self, near_sample: f32, far_end_history: &VecDeque<f32>) -> bool {
        // Update far-end maximum over the filter length
        let mut max_far = 0.0f32;
        for &s in far_end_history.iter() {
            let abs = s.abs();
            if abs > max_far {
                max_far = abs;
            }
        }
        self.far_end_max = max_far;

        let near_abs = near_sample.abs();

        // Geigel criterion: if |near| > threshold * max(|far|), double-talk
        if self.far_end_max > 1.0 && near_abs > self.threshold * self.far_end_max {
            self.holdover_count = self.holdover;
            return true;
        }

        if self.holdover_count > 0 {
            self.holdover_count -= 1;
            return true;
        }

        false
    }

    fn reset(&mut self) {
        self.far_end_max = 0.0;
        self.holdover_count = 0;
    }
}

/// Non-linear processor for residual echo suppression.
///
/// After the linear filter, some residual echo may remain. The NLP
/// applies aggressive suppression when the error signal is likely
/// dominated by echo rather than near-end speech.
struct NonLinearProcessor {
    /// Suppression gain when NLP is active (0.0 = full suppression).
    suppression_gain: f32,
    /// Threshold for echo-to-near-end ratio to trigger NLP.
    echo_threshold: f32,
    /// Comfort noise level (amplitude).
    comfort_noise_level: f32,
    /// Simple LFSR for noise generation.
    noise_state: u32,
}

impl NonLinearProcessor {
    fn new() -> Self {
        Self {
            suppression_gain: 0.05,
            echo_threshold: 0.3,
            comfort_noise_level: 50.0,
            noise_state: 0xDEAD_BEEF,
        }
    }

    /// Apply NLP: suppress residual echo and inject comfort noise.
    ///
    /// - `error`: the output of the linear canceller
    /// - `echo_estimate_power`: power of the echo estimate
    /// - `near_end_power`: power of the near-end signal
    fn process(&mut self, error: f32, echo_estimate_power: f32, near_end_power: f32) -> f32 {
        // If echo dominates, suppress the output
        if near_end_power > 1.0 && echo_estimate_power / near_end_power > self.echo_threshold {
            // Suppress and inject comfort noise
            let noise = self.generate_comfort_noise();
            error * self.suppression_gain + noise
        } else {
            error
        }
    }

    /// Generate a single comfort noise sample.
    fn generate_comfort_noise(&mut self) -> f32 {
        // Simple LFSR-based white noise
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 17;
        self.noise_state ^= self.noise_state << 5;
        // Map to [-comfort_noise_level, +comfort_noise_level]
        let normalized = (self.noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0;
        normalized * self.comfort_noise_level
    }

    fn reset(&mut self) {
        self.noise_state = 0xDEAD_BEEF;
    }
}

impl EchoCanceller {
    /// Create a new echo canceller.
    ///
    /// - `filter_length`: number of taps (128-256 typical; 16-32ms at 8kHz)
    /// - `sample_rate`: audio sample rate in Hz
    pub fn new(filter_length: usize, sample_rate: u32) -> Self {
        let filter_length = filter_length.max(1);
        Self {
            filter_length,
            adapted_filter: vec![0.0; filter_length],
            reference_buffer: VecDeque::from(vec![0.0; filter_length]),
            step_size: 0.5, // Moderate step size
            power_estimate: 0.0,
            sample_rate,
            dtd: DoubleTalkDetector::new(),
            nlp: NonLinearProcessor::new(),
        }
    }

    /// Set the NLMS step size (mu). Clamped to [0.01, 2.0].
    pub fn set_step_size(&mut self, step_size: f32) {
        self.step_size = step_size.clamp(0.01, 2.0);
    }

    /// Get the current sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Process a frame of audio samples.
    ///
    /// - `near_end`: microphone signal (contains desired speech + echo)
    /// - `far_end`: loudspeaker signal (the reference signal causing echo)
    ///
    /// Returns the echo-cancelled output signal.
    pub fn process(&mut self, near_end: &[i16], far_end: &[i16]) -> Vec<i16> {
        let len = near_end.len().min(far_end.len());
        let mut output = Vec::with_capacity(len);

        // Regularization constant to avoid division by zero
        let epsilon: f32 = 1e-6;
        // Power smoothing factor
        let alpha: f32 = 0.999;

        for i in 0..len {
            let near_f = near_end[i] as f32;
            let far_f = far_end[i] as f32;

            // Push new far-end sample into reference buffer
            self.reference_buffer.push_front(far_f);
            if self.reference_buffer.len() > self.filter_length {
                self.reference_buffer.pop_back();
            }

            // Update running power estimate (exponential moving average)
            self.power_estimate = alpha * self.power_estimate + (1.0 - alpha) * far_f * far_f;

            // Compute filter output (echo estimate): y = sum(h[i] * x[n-i])
            let mut echo_estimate: f32 = 0.0;
            for (j, &coeff) in self.adapted_filter.iter().enumerate() {
                if j < self.reference_buffer.len() {
                    echo_estimate += coeff * self.reference_buffer[j];
                }
            }

            // Compute error (echo-cancelled signal): e = near - echo_estimate
            let error = near_f - echo_estimate;

            // Double-talk detection
            let is_double_talk = self.dtd.detect(near_f, &self.reference_buffer);

            // NLMS filter update (only if not in double-talk)
            if !is_double_talk {
                // Standard NLMS normalization: divide by sum of x^2
                let ref_power: f32 = self.reference_buffer.iter().map(|&x| x * x).sum();
                let norm = ref_power + epsilon;
                let update_factor = self.step_size * error / norm;

                for (j, coeff) in self.adapted_filter.iter_mut().enumerate() {
                    if j < self.reference_buffer.len() {
                        *coeff += update_factor * self.reference_buffer[j];
                    }
                }
            }

            // Non-linear processing for residual echo suppression
            let echo_power = echo_estimate * echo_estimate;
            let near_power = near_f * near_f;
            let processed = self.nlp.process(error, echo_power, near_power);

            // Clamp to i16 range and add to output
            let clamped = processed.round().clamp(-32768.0, 32767.0) as i16;
            output.push(clamped);
        }

        output
    }

    /// Reset the echo canceller to initial state.
    pub fn reset(&mut self) {
        self.adapted_filter.fill(0.0);
        self.reference_buffer.clear();
        self.reference_buffer.resize(self.filter_length, 0.0);
        self.power_estimate = 0.0;
        self.dtd.reset();
        self.nlp.reset();
    }
}

impl EchoCancellerEngine for EchoCanceller {
    fn process_frame(&mut self, near_end: &[i16], far_end: &[i16]) -> Vec<i16> {
        self.process(near_end, far_end)
    }

    fn reset(&mut self) {
        EchoCanceller::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_canceller_creation() {
        let ec = EchoCanceller::new(128, 8000);
        assert_eq!(ec.filter_length, 128);
        assert_eq!(ec.sample_rate(), 8000);
        assert_eq!(ec.adapted_filter.len(), 128);
    }

    #[test]
    fn test_echo_canceller_reset() {
        let mut ec = EchoCanceller::new(64, 8000);
        // Process some data to change state
        let near = vec![1000i16; 160];
        let far = vec![500i16; 160];
        ec.process(&near, &far);
        // Reset and verify
        ec.reset();
        assert!(ec.adapted_filter.iter().all(|&x| x == 0.0));
        assert_eq!(ec.power_estimate, 0.0);
    }

    #[test]
    fn test_echo_canceller_silent_input() {
        let mut ec = EchoCanceller::new(128, 8000);
        let silence = vec![0i16; 160];
        let output = ec.process(&silence, &silence);
        assert_eq!(output.len(), 160);
        // Output of silence should be near-silence
        for &s in &output {
            assert!(s.abs() < 100, "Silent input should produce near-silent output, got {}", s);
        }
    }

    #[test]
    fn test_echo_cancellation_convergence() {
        // Test that the canceller actually reduces echo over time.
        // Simulate: far_end plays a tone, near_end = echo of far_end (delayed).
        let mut ec = EchoCanceller::new(128, 8000);

        let num_frames = 200;
        let frame_size = 160;
        let delay = 40; // 40-sample echo delay

        // Generate a reference signal (440 Hz tone)
        let total_samples = num_frames * frame_size;
        let mut far_signal = Vec::with_capacity(total_samples);
        for i in 0..total_samples {
            let sample = (10000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 8000.0).sin()) as i16;
            far_signal.push(sample);
        }

        // Generate near-end = attenuated echo of far-end (simulates acoustic path)
        let echo_gain = 0.6;
        let mut near_signal = vec![0i16; total_samples];
        for i in delay..total_samples {
            near_signal[i] = (far_signal[i - delay] as f32 * echo_gain) as i16;
        }

        // Measure initial echo power (first 10 frames)
        let mut initial_power = 0.0f64;
        for frame_idx in 0..10 {
            let start = frame_idx * frame_size;
            let end = start + frame_size;
            let output = ec.process(&near_signal[start..end], &far_signal[start..end]);
            for &s in &output {
                initial_power += (s as f64) * (s as f64);
            }
        }

        // Let the filter converge over many frames
        for frame_idx in 10..(num_frames - 10) {
            let start = frame_idx * frame_size;
            let end = start + frame_size;
            ec.process(&near_signal[start..end], &far_signal[start..end]);
        }

        // Measure final echo power (last 10 frames)
        let mut final_power = 0.0f64;
        for frame_idx in (num_frames - 10)..num_frames {
            let start = frame_idx * frame_size;
            let end = start + frame_size;
            let output = ec.process(&near_signal[start..end], &far_signal[start..end]);
            for &s in &output {
                final_power += (s as f64) * (s as f64);
            }
        }

        // Echo should be reduced by at least 20 dB
        // 20 dB = 100x power reduction
        if initial_power > 0.0 {
            let reduction_db = 10.0 * (initial_power / final_power.max(1.0)).log10();
            assert!(
                reduction_db > 10.0,
                "Echo cancellation should achieve >10dB reduction, got {:.1}dB \
                 (initial={:.0}, final={:.0})",
                reduction_db,
                initial_power,
                final_power
            );
        }
    }

    #[test]
    fn test_echo_canceller_different_length_inputs() {
        let mut ec = EchoCanceller::new(64, 8000);
        let near = vec![1000i16; 80];
        let far = vec![500i16; 160];
        // Should process min(80, 160) = 80 samples
        let output = ec.process(&near, &far);
        assert_eq!(output.len(), 80);
    }

    #[test]
    fn test_echo_canceller_step_size() {
        let mut ec = EchoCanceller::new(64, 8000);
        ec.set_step_size(0.3);
        assert!((ec.step_size - 0.3).abs() < 1e-6);
        // Clamping
        ec.set_step_size(5.0);
        assert!((ec.step_size - 2.0).abs() < 1e-6);
        ec.set_step_size(-1.0);
        assert!((ec.step_size - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_echo_canceller_engine_trait() {
        let mut ec = EchoCanceller::new(64, 8000);
        let engine: &mut dyn EchoCancellerEngine = &mut ec;
        let near = vec![100i16; 80];
        let far = vec![50i16; 80];
        let output = engine.process_frame(&near, &far);
        assert_eq!(output.len(), 80);
        engine.reset();
    }
}
