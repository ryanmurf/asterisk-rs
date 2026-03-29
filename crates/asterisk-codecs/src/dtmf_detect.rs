//! In-band DTMF Detection using the Goertzel Algorithm.
//!
//! Detects DTMF (Dual-Tone Multi-Frequency) digits from audio samples.
//! Each DTMF digit is a combination of two frequencies: one from a row
//! group and one from a column group.
//!
//! DTMF frequency matrix:
//! ```text
//!          1209 Hz  1336 Hz  1477 Hz  1633 Hz
//! 697 Hz:    1        2        3        A
//! 770 Hz:    4        5        6        B
//! 852 Hz:    7        8        9        C
//! 941 Hz:    *        0        #        D
//! ```
//!
//! The Goertzel algorithm efficiently computes the DFT energy at specific
//! frequencies, making it ideal for DTMF detection without a full FFT.

/// DTMF row frequencies in Hz.
const ROW_FREQS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];

/// DTMF column frequencies in Hz.
const COL_FREQS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];

/// DTMF digit mapping: [row][col] -> char.
const DTMF_MAP: [[char; 4]; 4] = [
    ['1', '2', '3', 'A'],
    ['4', '5', '6', 'B'],
    ['7', '8', '9', 'C'],
    ['*', '0', '#', 'D'],
];

/// State for a single Goertzel filter at one frequency.
#[derive(Debug, Clone)]
pub struct GoertzelState {
    /// Precomputed coefficient: 2 * cos(2 * pi * freq / sample_rate).
    coeff: f32,
    /// First delay element.
    s1: f32,
    /// Second delay element.
    s2: f32,
}

impl GoertzelState {
    /// Create a new Goertzel filter for the given frequency and sample rate.
    fn new(freq: f32, sample_rate: u32) -> Self {
        let k = (2.0 * std::f32::consts::PI * freq) / sample_rate as f32;
        Self {
            coeff: 2.0 * k.cos(),
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Reset the filter state.
    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    /// Process a single sample.
    #[inline(always)]
    fn process_sample(&mut self, sample: f32) {
        let s0 = sample + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
    }

    /// Compute the energy (magnitude squared) at the target frequency.
    ///
    /// Call this after processing all samples in a block.
    fn energy(&self) -> f32 {
        // |X(k)|^2 = s1^2 + s2^2 - coeff * s1 * s2
        self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2
    }
}

/// DTMF detector using the Goertzel algorithm.
pub struct DtmfDetector {
    /// Audio sample rate.
    sample_rate: u32,
    /// Goertzel filters for all 8 DTMF frequencies (4 row + 4 col).
    goertzel_states: [GoertzelState; 8],
    /// Minimum energy threshold for detection (squared magnitude).
    detection_threshold: f32,
    /// Maximum twist: allowed dB difference between row and column energy.
    /// Normal twist: row > col. Reverse twist: col > row.
    twist_threshold: f32,
    /// Minimum DTMF duration in milliseconds.
    min_duration_ms: u32,
    /// Number of samples in the current detection block.
    block_size: usize,
    /// Sample buffer for accumulating samples.
    sample_buffer: Vec<f32>,
    /// Currently detected digit (None if no digit active).
    current_digit: Option<char>,
    /// Number of consecutive blocks with the same digit detected.
    consecutive_count: u32,
    /// Required consecutive detections before confirming.
    #[allow(dead_code)]
    min_consecutive: u32,
    /// Previously confirmed digit (for edge detection).
    last_confirmed: Option<char>,
}

impl DtmfDetector {
    /// Create a new DTMF detector.
    ///
    /// - `sample_rate`: audio sample rate in Hz (typically 8000)
    pub fn new(sample_rate: u32) -> Self {
        // Block size: ~10ms of audio for analysis
        // Standard Goertzel detection uses 205 samples at 8kHz (Bellcore standard)
        let block_size = (sample_rate as usize * 205) / 8000;
        let block_size = block_size.max(40);

        let states = [
            GoertzelState::new(ROW_FREQS[0], sample_rate),
            GoertzelState::new(ROW_FREQS[1], sample_rate),
            GoertzelState::new(ROW_FREQS[2], sample_rate),
            GoertzelState::new(ROW_FREQS[3], sample_rate),
            GoertzelState::new(COL_FREQS[0], sample_rate),
            GoertzelState::new(COL_FREQS[1], sample_rate),
            GoertzelState::new(COL_FREQS[2], sample_rate),
            GoertzelState::new(COL_FREQS[3], sample_rate),
        ];

        Self {
            sample_rate,
            goertzel_states: states,
            detection_threshold: 1e6,
            twist_threshold: 8.0,     // 8 dB twist allowance
            min_duration_ms: 40,
            block_size,
            sample_buffer: Vec::with_capacity(block_size),
            current_digit: None,
            consecutive_count: 0,
            min_consecutive: 2,
            last_confirmed: None,
        }
    }

    /// Set the detection energy threshold.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.detection_threshold = threshold;
    }

    /// Process audio samples and return any detected DTMF digits.
    ///
    /// May return multiple digits if the input is long enough.
    pub fn process(&mut self, samples: &[i16]) -> Vec<char> {
        let mut detected = Vec::new();

        for &sample in samples {
            self.sample_buffer.push(sample as f32);

            if self.sample_buffer.len() >= self.block_size {
                if let Some(digit) = self.analyze_block() {
                    detected.push(digit);
                }
                self.sample_buffer.clear();
            }
        }

        detected
    }

    /// Analyze a complete block of samples for DTMF.
    ///
    /// Returns a confirmed digit if detected for sufficient duration.
    fn analyze_block(&mut self) -> Option<char> {
        // Reset all Goertzel states
        for state in self.goertzel_states.iter_mut() {
            state.reset();
        }

        // Process all samples through all 8 filters
        for &sample in &self.sample_buffer {
            for state in self.goertzel_states.iter_mut() {
                state.process_sample(sample);
            }
        }

        // Compute energies
        let row_energies: [f32; 4] = [
            self.goertzel_states[0].energy(),
            self.goertzel_states[1].energy(),
            self.goertzel_states[2].energy(),
            self.goertzel_states[3].energy(),
        ];
        let col_energies: [f32; 4] = [
            self.goertzel_states[4].energy(),
            self.goertzel_states[5].energy(),
            self.goertzel_states[6].energy(),
            self.goertzel_states[7].energy(),
        ];

        // Find the strongest row and column
        let (best_row, best_row_energy) = row_energies
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();

        let (best_col, best_col_energy) = col_energies
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();

        // Check energy threshold
        if *best_row_energy < self.detection_threshold || *best_col_energy < self.detection_threshold
        {
            // No DTMF detected in this block
            self.current_digit = None;
            self.consecutive_count = 0;
            return None;
        }

        // Check twist (ratio between row and column energy)
        let twist_db = 10.0 * (best_row_energy / best_col_energy.max(1e-10)).log10();
        if twist_db.abs() > self.twist_threshold {
            self.current_digit = None;
            self.consecutive_count = 0;
            return None;
        }

        // Ensure the detected frequencies are dominant (reject harmonics/noise)
        // Check that no other row/col has more than 50% of the best energy
        for (i, &e) in row_energies.iter().enumerate() {
            if i != best_row && e > *best_row_energy * 0.5 {
                self.current_digit = None;
                self.consecutive_count = 0;
                return None;
            }
        }
        for (i, &e) in col_energies.iter().enumerate() {
            if i != best_col && e > *best_col_energy * 0.5 {
                self.current_digit = None;
                self.consecutive_count = 0;
                return None;
            }
        }

        let digit = DTMF_MAP[best_row][best_col];

        // Duration validation via consecutive block counting
        if self.current_digit == Some(digit) {
            self.consecutive_count += 1;
        } else {
            self.current_digit = Some(digit);
            self.consecutive_count = 1;
        }

        // Check if we've met the minimum duration requirement
        let block_duration_ms = (self.block_size as u32 * 1000) / self.sample_rate;
        let total_duration = self.consecutive_count * block_duration_ms;

        if total_duration >= self.min_duration_ms && self.last_confirmed != Some(digit) {
            self.last_confirmed = Some(digit);
            return Some(digit);
        }

        // Reset last_confirmed when no digit is present
        if self.current_digit.is_none() {
            self.last_confirmed = None;
        }

        None
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        for state in self.goertzel_states.iter_mut() {
            state.reset();
        }
        self.sample_buffer.clear();
        self.current_digit = None;
        self.consecutive_count = 0;
        self.last_confirmed = None;
    }
}

/// Generate a DTMF tone for testing purposes.
///
/// Returns samples at the given sample rate for the specified duration.
pub fn generate_dtmf_tone(digit: char, sample_rate: u32, duration_ms: u32) -> Vec<i16> {
    let row_freq = match digit {
        '1' | '2' | '3' | 'A' => ROW_FREQS[0],
        '4' | '5' | '6' | 'B' => ROW_FREQS[1],
        '7' | '8' | '9' | 'C' => ROW_FREQS[2],
        '*' | '0' | '#' | 'D' => ROW_FREQS[3],
        _ => return Vec::new(),
    };

    let col_freq = match digit {
        '1' | '4' | '7' | '*' => COL_FREQS[0],
        '2' | '5' | '8' | '0' => COL_FREQS[1],
        '3' | '6' | '9' | '#' => COL_FREQS[2],
        'A' | 'B' | 'C' | 'D' => COL_FREQS[3],
        _ => return Vec::new(),
    };

    let num_samples = (sample_rate * duration_ms / 1000) as usize;
    let mut samples = Vec::with_capacity(num_samples);
    let amplitude = 8000.0; // Per-tone amplitude

    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let row_sample = amplitude * (2.0 * std::f64::consts::PI * row_freq as f64 * t).sin();
        let col_sample = amplitude * (2.0 * std::f64::consts::PI * col_freq as f64 * t).sin();
        let combined = (row_sample + col_sample).round().clamp(-32768.0, 32767.0) as i16;
        samples.push(combined);
    }

    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goertzel_energy_at_target_frequency() {
        // Generate a 697 Hz tone and verify Goertzel detects it
        let sample_rate = 8000u32;
        let n = 205;
        let freq = 697.0f32;

        let samples: Vec<f32> = (0..n)
            .map(|i| 10000.0 * (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect();

        let mut gs = GoertzelState::new(freq, sample_rate);
        for &s in &samples {
            gs.process_sample(s);
        }
        let energy_on = gs.energy();

        // Compute energy at a non-DTMF frequency
        let mut gs_off = GoertzelState::new(500.0, sample_rate);
        for &s in &samples {
            gs_off.process_sample(s);
        }
        let energy_off = gs_off.energy();

        assert!(
            energy_on > energy_off * 10.0,
            "Energy at target freq ({}) should be much higher than off-freq ({})",
            energy_on,
            energy_off
        );
    }

    #[test]
    fn test_dtmf_detect_all_digits() {
        let sample_rate = 8000;
        let digits = [
            '1', '2', '3', 'A',
            '4', '5', '6', 'B',
            '7', '8', '9', 'C',
            '*', '0', '#', 'D',
        ];

        for &expected_digit in &digits {
            let mut detector = DtmfDetector::new(sample_rate);
            detector.set_threshold(1e5);

            // Generate 100ms of the DTMF tone
            let tone = generate_dtmf_tone(expected_digit, sample_rate, 100);

            // Prepend silence to reset state
            let silence = vec![0i16; 400];
            detector.process(&silence);

            let detected = detector.process(&tone);
            assert!(
                detected.contains(&expected_digit),
                "Failed to detect digit '{}', detected: {:?}",
                expected_digit,
                detected
            );
        }
    }

    #[test]
    fn test_dtmf_no_false_positive_silence() {
        let mut detector = DtmfDetector::new(8000);
        let silence = vec![0i16; 1600]; // 200ms of silence
        let detected = detector.process(&silence);
        assert!(
            detected.is_empty(),
            "Should not detect DTMF in silence, got: {:?}",
            detected
        );
    }

    #[test]
    fn test_dtmf_no_false_positive_speech() {
        let mut detector = DtmfDetector::new(8000);

        // Generate a non-DTMF tone (500 Hz, not a DTMF frequency)
        let non_dtmf: Vec<i16> = (0..1600)
            .map(|i| (8000.0 * (2.0 * std::f64::consts::PI * 500.0 * i as f64 / 8000.0).sin()) as i16)
            .collect();

        let detected = detector.process(&non_dtmf);
        assert!(
            detected.is_empty(),
            "Should not detect DTMF from non-DTMF tone, got: {:?}",
            detected
        );
    }

    #[test]
    fn test_dtmf_tone_generation() {
        let tone = generate_dtmf_tone('5', 8000, 100);
        assert_eq!(tone.len(), 800); // 100ms at 8kHz
        // Should not be silence
        let max = tone.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max > 1000, "Generated tone should have significant amplitude");
    }

    #[test]
    fn test_dtmf_detector_reset() {
        let mut detector = DtmfDetector::new(8000);
        let tone = generate_dtmf_tone('5', 8000, 100);
        detector.process(&tone);
        detector.reset();
        assert!(detector.current_digit.is_none());
        assert_eq!(detector.consecutive_count, 0);
    }

    #[test]
    fn test_generate_invalid_digit() {
        let tone = generate_dtmf_tone('X', 8000, 100);
        assert!(tone.is_empty());
    }
}
