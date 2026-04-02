//! Telephony Tone Generator.
//!
//! Generates standard telephony tones (dial tone, busy, ringback, etc.)
//! as PCM audio samples with proper phase continuity.
//!
//! Standard North American tones:
//! - Dial tone: 350+440 Hz continuous
//! - Busy tone: 480+620 Hz, 500ms on / 500ms off
//! - Ringback: 440+480 Hz, 2000ms on / 4000ms off
//! - Congestion: 480+620 Hz, 250ms on / 250ms off
//! - SIT (Special Information Tones): 3 ascending tones

use std::f64::consts::PI;

/// A segment of a tone pattern.
#[derive(Debug, Clone)]
pub struct ToneSegment {
    /// Frequencies and their amplitudes: (freq_hz, amplitude).
    pub frequencies: Vec<(f64, f64)>,
    /// Duration of the tone in milliseconds.
    pub duration_ms: u32,
    /// Duration of silence after the tone in milliseconds.
    pub silence_ms: u32,
}

/// Special Information Tone types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SitType {
    /// Intercept: number not in service.
    Intercept,
    /// Vacant circuit.
    VacantCircuit,
    /// Reorder: switch congestion.
    Reorder,
    /// No circuit found.
    NoCircuit,
}

/// Standard telephony tone patterns.
#[derive(Debug, Clone)]
pub enum TonePattern {
    /// Dial tone: 350+440 Hz continuous.
    DialTone,
    /// Busy tone: 480+620 Hz, 500ms on / 500ms off.
    BusyTone,
    /// Ringback: 440+480 Hz, 2s on / 4s off.
    RingbackTone,
    /// Congestion/reorder: 480+620 Hz, 250ms on / 250ms off.
    CongestionTone,
    /// Reorder tone (alias for Congestion).
    Reorder,
    /// Special Information Tones (3 ascending tones).
    Sit(SitType),
    /// 1004 Hz milliwatt test tone.
    Milliwatt,
    /// Custom tone pattern defined by segments.
    Custom(Vec<ToneSegment>),
}

impl TonePattern {
    /// Convert the pattern to a sequence of tone segments.
    fn to_segments(&self) -> Vec<ToneSegment> {
        match self {
            TonePattern::DialTone => vec![ToneSegment {
                frequencies: vec![(350.0, 0.5), (440.0, 0.5)],
                duration_ms: 10000, // 10 seconds continuous
                silence_ms: 0,
            }],
            TonePattern::BusyTone => vec![ToneSegment {
                frequencies: vec![(480.0, 0.5), (620.0, 0.5)],
                duration_ms: 500,
                silence_ms: 500,
            }],
            TonePattern::RingbackTone => vec![ToneSegment {
                frequencies: vec![(440.0, 0.5), (480.0, 0.5)],
                duration_ms: 2000,
                silence_ms: 4000,
            }],
            TonePattern::CongestionTone | TonePattern::Reorder => vec![ToneSegment {
                frequencies: vec![(480.0, 0.5), (620.0, 0.5)],
                duration_ms: 250,
                silence_ms: 250,
            }],
            TonePattern::Sit(sit_type) => {
                let (f1, f2, f3) = match sit_type {
                    SitType::Intercept => (913.8, 1370.6, 1776.7),
                    SitType::VacantCircuit => (985.2, 1370.6, 1776.7),
                    SitType::Reorder => (913.8, 1428.5, 1776.7),
                    SitType::NoCircuit => (985.2, 1428.5, 1776.7),
                };
                vec![
                    ToneSegment {
                        frequencies: vec![(f1, 0.8)],
                        duration_ms: 330,
                        silence_ms: 0,
                    },
                    ToneSegment {
                        frequencies: vec![(f2, 0.8)],
                        duration_ms: 330,
                        silence_ms: 0,
                    },
                    ToneSegment {
                        frequencies: vec![(f3, 0.8)],
                        duration_ms: 330,
                        silence_ms: 1000,
                    },
                ]
            }
            TonePattern::Milliwatt => vec![ToneSegment {
                frequencies: vec![(1004.0, 0.9)],
                duration_ms: 10000,
                silence_ms: 0,
            }],
            TonePattern::Custom(segments) => segments.clone(),
        }
    }
}

/// Telephony tone generator with proper phase continuity.
pub struct ToneGenerator {
    /// Audio sample rate.
    sample_rate: u32,
    /// Current phase for each oscillator (radians).
    phases: Vec<f64>,
    /// Current tone pattern being generated.
    pattern: Option<TonePattern>,
    /// Segments of the current pattern.
    segments: Vec<ToneSegment>,
    /// Current segment index.
    segment_index: usize,
    /// Position within the current segment (in samples).
    sample_position: u32,
    /// Whether we're in the silence portion of the segment.
    in_silence: bool,
    /// Peak amplitude for output scaling (prevents clipping when mixing tones).
    peak_amplitude: f64,
}

impl ToneGenerator {
    /// Create a new tone generator.
    ///
    /// - `sample_rate`: audio sample rate in Hz
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            phases: Vec::new(),
            pattern: None,
            segments: Vec::new(),
            segment_index: 0,
            sample_position: 0,
            in_silence: false,
            peak_amplitude: 8000.0,
        }
    }

    /// Set the peak output amplitude (default 8000).
    pub fn set_amplitude(&mut self, amplitude: f64) {
        self.peak_amplitude = amplitude;
    }

    /// Start generating a tone pattern.
    pub fn start(&mut self, pattern: TonePattern) {
        self.segments = pattern.to_segments();
        self.pattern = Some(pattern);
        self.segment_index = 0;
        self.sample_position = 0;
        self.in_silence = false;

        // Initialize phases for the maximum number of frequencies
        let max_freqs = self.segments.iter().map(|s| s.frequencies.len()).max().unwrap_or(0);
        self.phases = vec![0.0; max_freqs];
    }

    /// Stop tone generation.
    pub fn stop(&mut self) {
        self.pattern = None;
        self.segments.clear();
    }

    /// Check if a tone is currently being generated.
    pub fn is_active(&self) -> bool {
        self.pattern.is_some()
    }

    /// Generate audio samples.
    ///
    /// Returns the requested number of samples, or fewer if the tone ends.
    pub fn generate(&mut self, num_samples: usize) -> Vec<i16> {
        if self.segments.is_empty() {
            return vec![0i16; num_samples];
        }

        let mut output = Vec::with_capacity(num_samples);

        while output.len() < num_samples {
            if self.segment_index >= self.segments.len() {
                // Wrap around for repeating patterns
                self.segment_index = 0;
                self.sample_position = 0;
                self.in_silence = false;
            }

            let segment = &self.segments[self.segment_index];

            if self.in_silence {
                // Generate silence
                let silence_samples = segment.silence_ms * self.sample_rate / 1000;
                let remaining = silence_samples.saturating_sub(self.sample_position);
                let to_generate = remaining.min((num_samples - output.len()) as u32);

                #[allow(clippy::same_item_push)]
                for _ in 0..to_generate {
                    output.push(0);
                }
                self.sample_position += to_generate;

                if self.sample_position >= silence_samples {
                    // Move to next segment
                    self.segment_index += 1;
                    self.sample_position = 0;
                    self.in_silence = false;
                }
            } else {
                // Generate tone
                let tone_samples = segment.duration_ms * self.sample_rate / 1000;
                let remaining = tone_samples.saturating_sub(self.sample_position);
                let to_generate = remaining.min((num_samples - output.len()) as u32);

                // Ensure phases vector is large enough
                while self.phases.len() < segment.frequencies.len() {
                    self.phases.push(0.0);
                }

                for _ in 0..to_generate {
                    let mut sample = 0.0f64;
                    for (i, &(freq, amplitude)) in segment.frequencies.iter().enumerate() {
                        sample += amplitude * (self.phases[i]).sin();
                        // Advance phase
                        self.phases[i] += 2.0 * PI * freq / self.sample_rate as f64;
                        // Keep phase in [0, 2*PI) to prevent floating point drift
                        if self.phases[i] >= 2.0 * PI {
                            self.phases[i] -= 2.0 * PI;
                        }
                    }

                    // Scale and clamp
                    let scaled = (sample * self.peak_amplitude).round().clamp(-32768.0, 32767.0);
                    output.push(scaled as i16);
                }
                self.sample_position += to_generate;

                if self.sample_position >= tone_samples {
                    if segment.silence_ms > 0 {
                        self.in_silence = true;
                        self.sample_position = 0;
                    } else {
                        self.segment_index += 1;
                        self.sample_position = 0;
                    }
                }
            }
        }

        output
    }

    /// Generate a fixed-duration tone (one-shot, no state).
    ///
    /// Useful for generating test tones without maintaining generator state.
    pub fn generate_tone(
        sample_rate: u32,
        frequencies: &[(f64, f64)],
        duration_ms: u32,
    ) -> Vec<i16> {
        let num_samples = (sample_rate * duration_ms / 1000) as usize;
        let mut output = Vec::with_capacity(num_samples);
        let amplitude = 8000.0;

        for i in 0..num_samples {
            let mut sample = 0.0f64;
            for &(freq, amp) in frequencies {
                let t = i as f64 / sample_rate as f64;
                sample += amp * (2.0 * PI * freq * t).sin();
            }
            let scaled = (sample * amplitude).round().clamp(-32768.0, 32767.0);
            output.push(scaled as i16);
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tone_generator_creation() {
        let tg = ToneGenerator::new(8000);
        assert_eq!(tg.sample_rate, 8000);
        assert!(!tg.is_active());
    }

    #[test]
    fn test_dial_tone_generation() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::DialTone);
        assert!(tg.is_active());

        let samples = tg.generate(800); // 100ms
        assert_eq!(samples.len(), 800);

        // Should not be silence
        let max = samples.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max > 100, "Dial tone should not be silence");
    }

    #[test]
    fn test_busy_tone_has_gaps() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::BusyTone);

        // Generate 1.5 seconds (should include tone + silence + tone)
        let samples = tg.generate(12000);

        // First 500ms should have tone (4000 samples)
        let first_half_energy: f64 = samples[..4000]
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum();

        // 500-1000ms should be silence (samples 4000-8000)
        let silence_energy: f64 = samples[4000..8000]
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum();

        assert!(
            first_half_energy > silence_energy * 100.0,
            "Tone portion should have much more energy than silence"
        );
    }

    #[test]
    fn test_tone_frequency_content() {
        // Generate a 440 Hz tone and verify with Goertzel
        let samples = ToneGenerator::generate_tone(8000, &[(440.0, 1.0)], 100);

        // Use Goertzel to check 440 Hz energy vs 1000 Hz energy
        let energy_440 = goertzel_energy(&samples, 440.0, 8000);
        let energy_1000 = goertzel_energy(&samples, 1000.0, 8000);

        assert!(
            energy_440 > energy_1000 * 10.0,
            "440 Hz energy ({}) should dominate over 1000 Hz ({})",
            energy_440,
            energy_1000
        );
    }

    #[test]
    fn test_dial_tone_frequencies() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::DialTone);
        let samples = tg.generate(800); // 100ms

        // Check both 350 Hz and 440 Hz are present
        let energy_350 = goertzel_energy(&samples, 350.0, 8000);
        let energy_440 = goertzel_energy(&samples, 440.0, 8000);
        let energy_600 = goertzel_energy(&samples, 600.0, 8000);

        assert!(energy_350 > energy_600 * 5.0, "350 Hz should be present");
        assert!(energy_440 > energy_600 * 5.0, "440 Hz should be present");
    }

    #[test]
    fn test_milliwatt_tone() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::Milliwatt);
        let samples = tg.generate(800);

        let energy_1004 = goertzel_energy(&samples, 1004.0, 8000);
        let energy_500 = goertzel_energy(&samples, 500.0, 8000);

        assert!(
            energy_1004 > energy_500 * 10.0,
            "Milliwatt tone should be dominated by 1004 Hz"
        );
    }

    #[test]
    fn test_stop_tone() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::DialTone);
        assert!(tg.is_active());
        tg.stop();
        assert!(!tg.is_active());
    }

    #[test]
    fn test_custom_tone() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::Custom(vec![ToneSegment {
            frequencies: vec![(1000.0, 0.8)],
            duration_ms: 200,
            silence_ms: 100,
        }]));

        let samples = tg.generate(1600); // 200ms
        assert_eq!(samples.len(), 1600);

        let energy_1000 = goertzel_energy(&samples[..1600], 1000.0, 8000);
        assert!(energy_1000 > 1e6, "Custom 1000 Hz tone should have energy");
    }

    #[test]
    fn test_sit_tones() {
        let mut tg = ToneGenerator::new(8000);
        tg.start(TonePattern::Sit(SitType::Intercept));
        let samples = tg.generate(8000); // 1 second
        assert_eq!(samples.len(), 8000);
        // Should not be all zeros
        let max = samples.iter().map(|s| s.abs()).max().unwrap_or(0);
        assert!(max > 100, "SIT tone should produce audio");
    }

    /// Helper: compute Goertzel energy at a specific frequency.
    fn goertzel_energy(samples: &[i16], freq: f32, sample_rate: u32) -> f64 {
        let k = (2.0 * std::f32::consts::PI * freq) / sample_rate as f32;
        let coeff = 2.0 * k.cos();
        let mut s1 = 0.0f64;
        let mut s2 = 0.0f64;

        for &sample in samples {
            let s0 = sample as f64 + coeff as f64 * s1 - s2;
            s2 = s1;
            s1 = s0;
        }

        s1 * s1 + s2 * s2 - coeff as f64 * s1 * s2
    }
}
