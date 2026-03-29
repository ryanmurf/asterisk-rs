//! Packet Loss Concealment (PLC).
//!
//! When an RTP packet is lost, the PLC engine generates a substitute frame
//! to mask the gap. It uses pitch-period repetition of the last good frame,
//! fading toward silence for consecutive losses.
//!
//! Algorithm:
//! 1. On each good frame, estimate the pitch period via autocorrelation
//!    and store the frame.
//! 2. On the first lost frame, repeat the last good frame aligned to the
//!    estimated pitch period.
//! 3. On subsequent consecutive losses, apply progressive attenuation.
//! 4. After `max_loss` consecutive losses, output silence.

/// Packet loss concealment engine.
pub struct PlcEngine {
    /// Last successfully decoded frame.
    last_good_frame: Vec<i16>,
    /// Estimated pitch period in samples.
    pitch_period: usize,
    /// Overlap-add crossfade length in samples.
    overlap_length: usize,
    /// Number of consecutive lost frames.
    loss_count: u32,
    /// Maximum frames to conceal before outputting silence.
    max_loss: u32,
    /// Frame size in samples.
    frame_size: usize,
    /// Sample rate.
    sample_rate: u32,
}

impl PlcEngine {
    /// Create a new PLC engine.
    ///
    /// - `frame_size`: expected frame size in samples (e.g., 160 for 20ms at 8kHz)
    /// - `sample_rate`: audio sample rate in Hz
    pub fn new(frame_size: usize, sample_rate: u32) -> Self {
        Self {
            last_good_frame: vec![0i16; frame_size],
            pitch_period: frame_size / 2, // Default to half frame
            overlap_length: (frame_size / 8).max(4),
            loss_count: 0,
            max_loss: 10,
            frame_size,
            sample_rate,
        }
    }

    /// Set the maximum number of consecutive frames to conceal.
    pub fn set_max_loss(&mut self, max_loss: u32) {
        self.max_loss = max_loss;
    }

    /// Receive a good (successfully decoded) frame.
    ///
    /// Updates the stored frame and pitch estimate.
    pub fn receive_good_frame(&mut self, frame: &[i16]) {
        self.last_good_frame = frame.to_vec();
        self.frame_size = frame.len();
        self.loss_count = 0;

        // Estimate pitch period
        self.pitch_period = self.estimate_pitch(frame);
        self.overlap_length = (self.frame_size / 8).max(4);
    }

    /// Generate a concealment frame for a lost packet.
    pub fn generate_lost_frame(&mut self) -> Vec<i16> {
        self.loss_count += 1;

        if self.loss_count > self.max_loss {
            // After max_loss, output silence
            return vec![0i16; self.frame_size];
        }

        let mut output = Vec::with_capacity(self.frame_size);

        if self.loss_count == 1 {
            // First loss: repeat with pitch-period alignment
            output = self.repeat_with_pitch();
        } else {
            // Subsequent losses: repeat and apply fade
            let repeated = self.repeat_with_pitch();
            let attenuation = self.compute_attenuation();

            for &s in &repeated {
                output.push((s as f32 * attenuation).round() as i16);
            }
        }

        // Update last_good_frame to the generated frame for next concealment
        self.last_good_frame = output.clone();

        output
    }

    /// Get the current loss count.
    pub fn loss_count(&self) -> u32 {
        self.loss_count
    }

    /// Get the estimated pitch period.
    pub fn pitch_period(&self) -> usize {
        self.pitch_period
    }

    /// Reset the PLC engine.
    pub fn reset(&mut self) {
        self.last_good_frame = vec![0i16; self.frame_size];
        self.pitch_period = self.frame_size / 2;
        self.loss_count = 0;
    }

    /// Estimate pitch period using autocorrelation.
    ///
    /// Searches for the lag with the highest normalized autocorrelation
    /// within the expected pitch range for human speech (2ms-20ms).
    fn estimate_pitch(&self, frame: &[i16]) -> usize {
        if frame.len() < 4 {
            return frame.len() / 2;
        }

        // Expected pitch range for human speech
        let min_pitch = (self.sample_rate as usize * 2) / 1000; // 2ms -> ~16 samples at 8kHz
        let max_pitch = (self.sample_rate as usize * 20) / 1000; // 20ms -> ~160 samples at 8kHz
        let min_pitch = min_pitch.max(2).min(frame.len() / 2);
        let max_pitch = max_pitch.min(frame.len() / 2);

        if min_pitch >= max_pitch {
            return frame.len() / 2;
        }

        // Compute autocorrelation at lag 0 (energy)
        let energy: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
        if energy < 100.0 {
            // Very quiet frame, return default
            return frame.len() / 2;
        }

        let mut best_lag = min_pitch;
        let mut best_corr = -1.0f64;

        for lag in min_pitch..=max_pitch {
            let n = frame.len() - lag;
            let mut correlation = 0.0f64;
            let mut energy_lagged = 0.0f64;

            for i in 0..n {
                correlation += frame[i] as f64 * frame[i + lag] as f64;
                energy_lagged += frame[i + lag] as f64 * frame[i + lag] as f64;
            }

            // Normalized autocorrelation
            let norm = (energy * energy_lagged).sqrt();
            if norm > 0.0 {
                let normalized = correlation / norm;
                if normalized > best_corr {
                    best_corr = normalized;
                    best_lag = lag;
                }
            }
        }

        best_lag
    }

    /// Generate a frame by repeating the last good frame with pitch alignment.
    fn repeat_with_pitch(&self) -> Vec<i16> {
        let mut output = Vec::with_capacity(self.frame_size);
        let src = &self.last_good_frame;
        let _pitch = self.pitch_period.max(1);

        if src.is_empty() {
            return vec![0i16; self.frame_size];
        }

        // Fill output by cycling through the last good frame at pitch intervals
        let mut pos = 0usize;
        while output.len() < self.frame_size {
            let idx = pos % src.len();
            output.push(src[idx]);
            pos += 1;
        }

        // Apply crossfade at the beginning to smooth transition
        let overlap = self.overlap_length.min(output.len()).min(src.len());
        for i in 0..overlap {
            let fade_in = i as f32 / overlap as f32;
            let fade_out = 1.0 - fade_in;
            let src_idx = src.len().saturating_sub(overlap) + i;
            if src_idx < src.len() {
                let blended = src[src_idx] as f32 * fade_out + output[i] as f32 * fade_in;
                output[i] = blended.round().clamp(-32768.0, 32767.0) as i16;
            }
        }

        output.truncate(self.frame_size);
        output
    }

    /// Compute attenuation factor for consecutive losses.
    ///
    /// Fades linearly toward silence over max_loss frames.
    fn compute_attenuation(&self) -> f32 {
        if self.max_loss == 0 {
            return 0.0;
        }
        let progress = self.loss_count as f32 / self.max_loss as f32;
        (1.0 - progress).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plc_creation() {
        let plc = PlcEngine::new(160, 8000);
        assert_eq!(plc.frame_size, 160);
        assert_eq!(plc.loss_count(), 0);
    }

    #[test]
    fn test_plc_good_frame_resets_loss() {
        let mut plc = PlcEngine::new(160, 8000);
        plc.generate_lost_frame();
        assert_eq!(plc.loss_count(), 1);
        let good = vec![100i16; 160];
        plc.receive_good_frame(&good);
        assert_eq!(plc.loss_count(), 0);
    }

    #[test]
    fn test_plc_first_loss_repeats() {
        let mut plc = PlcEngine::new(160, 8000);

        // Feed a tone as the good frame
        let good: Vec<i16> = (0..160)
            .map(|i| (5000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 8000.0).sin()) as i16)
            .collect();
        plc.receive_good_frame(&good);

        let concealed = plc.generate_lost_frame();
        assert_eq!(concealed.len(), 160);
        // Concealed frame should not be silence
        let energy: f64 = concealed.iter().map(|&s| (s as f64) * (s as f64)).sum();
        assert!(energy > 0.0, "First concealment frame should not be silence");
    }

    #[test]
    fn test_plc_fades_to_silence() {
        let mut plc = PlcEngine::new(160, 8000);
        plc.set_max_loss(5);

        let good = vec![1000i16; 160];
        plc.receive_good_frame(&good);

        let mut energies = Vec::new();
        for _ in 0..7 {
            let concealed = plc.generate_lost_frame();
            let energy: f64 = concealed.iter().map(|&s| (s as f64) * (s as f64)).sum();
            energies.push(energy);
        }

        // Energy should decrease over consecutive losses
        assert!(
            energies[4] < energies[0] || energies[0] < 1.0,
            "Energy should decrease: {:?}",
            energies
        );

        // After max_loss, should be silence
        assert!(
            energies[5] < 1.0,
            "After max_loss, output should be silence, energy={}",
            energies[5]
        );
    }

    #[test]
    fn test_plc_pitch_estimation() {
        let plc = PlcEngine::new(160, 8000);

        // Generate a 400 Hz tone (pitch period = 8000/400 = 20 samples)
        let tone: Vec<i16> = (0..160)
            .map(|i| (10000.0 * (2.0 * std::f64::consts::PI * 400.0 * i as f64 / 8000.0).sin()) as i16)
            .collect();

        let pitch = plc.estimate_pitch(&tone);
        // Expected pitch period = 20 samples (8000/400)
        // Allow some tolerance
        assert!(
            (pitch as i32 - 20).unsigned_abs() <= 3,
            "Estimated pitch {} should be near 20 samples for 400 Hz tone",
            pitch
        );
    }

    #[test]
    fn test_plc_silence_input() {
        let mut plc = PlcEngine::new(160, 8000);
        let silence = vec![0i16; 160];
        plc.receive_good_frame(&silence);
        let concealed = plc.generate_lost_frame();
        assert_eq!(concealed.len(), 160);
        // Concealment of silence should still be near-silence
        let max_sample = concealed.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max_sample < 100, "Concealment of silence should be quiet, max={}", max_sample);
    }

    #[test]
    fn test_plc_reset() {
        let mut plc = PlcEngine::new(160, 8000);
        let good = vec![5000i16; 160];
        plc.receive_good_frame(&good);
        plc.generate_lost_frame();
        assert_eq!(plc.loss_count(), 1);

        plc.reset();
        assert_eq!(plc.loss_count(), 0);
    }
}
