//! Sample rate converter - linear interpolation resampler.
//!
//! Port of codecs/codec_resample.c from Asterisk C.
//!
//! Converts between signed linear (SLIN) audio at different sample rates
//! (8kHz, 12kHz, 16kHz, 24kHz, 32kHz, 44.1kHz, 48kHz, 96kHz, 192kHz).
//!
//! The original Asterisk C code uses the Speex resampler library. This
//! Rust port uses basic linear interpolation, which is sufficient for
//! telephony audio and avoids the external dependency.

use crate::builtin_codecs::{
    ID_SLIN8, ID_SLIN12, ID_SLIN16, ID_SLIN24, ID_SLIN32,
    ID_SLIN44, ID_SLIN48, ID_SLIN96, ID_SLIN192,
};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;
use bytes::Bytes;

/// All supported SLIN sample rates with their codec IDs.
pub const SLIN_RATES: &[(u32, CodecId)] = &[
    (8000, ID_SLIN8),
    (12000, ID_SLIN12),
    (16000, ID_SLIN16),
    (24000, ID_SLIN24),
    (32000, ID_SLIN32),
    (44100, ID_SLIN44),
    (48000, ID_SLIN48),
    (96000, ID_SLIN96),
    (192000, ID_SLIN192),
];

/// Output buffer size limit in samples.
const OUTBUF_SAMPLES: usize = 11520;

/// Resample signed-linear 16-bit audio using linear interpolation.
///
/// Input and output are slices of i16 samples. The function computes
/// the ratio `src_rate / dst_rate` and interpolates between adjacent
/// source samples to produce the output.
pub fn resample_linear(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let out_len = out_len.min(OUTBUF_SAMPLES);
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        if idx + 1 < input.len() {
            // Linear interpolation between two adjacent samples
            let s0 = input[idx] as f64;
            let s1 = input[idx + 1] as f64;
            let interpolated = s0 + frac * (s1 - s0);
            output.push(interpolated.round().clamp(-32768.0, 32767.0) as i16);
        } else if idx < input.len() {
            output.push(input[idx]);
        } else {
            break;
        }
    }

    output
}

/// A resampling translator between two SLIN sample rates.
pub struct ResampleTranslator {
    name: String,
    src_rate: u32,
    dst_rate: u32,
    src_codec_id: CodecId,
    dst_codec_id: CodecId,
}

impl ResampleTranslator {
    pub fn new(src_rate: u32, src_id: CodecId, dst_rate: u32, dst_id: CodecId) -> Self {
        Self {
            name: format!("slin {}khz -> {}khz", src_rate / 1000, dst_rate / 1000),
            src_rate,
            dst_rate,
            src_codec_id: src_id,
            dst_codec_id: dst_id,
        }
    }
}

impl Translator for ResampleTranslator {
    fn name(&self) -> &str { &self.name }
    fn src_codec_id(&self) -> CodecId { self.src_codec_id }
    fn dst_codec_id(&self) -> CodecId { self.dst_codec_id }
    fn table_cost(&self) -> u32 {
        if self.src_rate < self.dst_rate {
            TransCost::LL_LL_UPSAMP
        } else {
            TransCost::LL_LL_DOWNSAMP
        }
    }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(ResampleInstance {
            src_rate: self.src_rate,
            dst_rate: self.dst_rate,
            dst_codec_id: self.dst_codec_id,
            output_buf: Vec::with_capacity(OUTBUF_SAMPLES * 2),
            samples: 0,
        })
    }
}

struct ResampleInstance {
    src_rate: u32,
    dst_rate: u32,
    dst_codec_id: CodecId,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for ResampleInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };

        if data.is_empty() {
            return Err(TranslateError::Failed("empty frame data".into()));
        }

        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed("slin data must have even length".into()));
        }

        // Convert bytes to i16 samples
        let mut input_samples: Vec<i16> = Vec::with_capacity(data.len() / 2);
        let mut i = 0;
        while i + 1 < data.len() {
            input_samples.push(i16::from_le_bytes([data[i], data[i + 1]]));
            i += 2;
        }

        // Resample
        let resampled = resample_linear(&input_samples, self.src_rate, self.dst_rate);

        // Append to output buffer
        for &sample in &resampled {
            self.output_buf.extend_from_slice(&sample.to_le_bytes());
            self.samples += 1;
        }

        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        if self.output_buf.is_empty() {
            return None;
        }
        let data = Bytes::from(std::mem::take(&mut self.output_buf));
        let samples = self.samples;
        self.samples = 0;
        Some(Frame::voice(self.dst_codec_id, samples, data))
    }
}

/// Generate all resample translator pairs (N*(N-1) combinations).
///
/// Returns a Vec of Arc<dyn Translator> for registration into a TranslationMatrix.
pub fn all_resample_translators() -> Vec<std::sync::Arc<dyn Translator>> {
    let mut translators: Vec<std::sync::Arc<dyn Translator>> = Vec::new();
    for &(src_rate, src_id) in SLIN_RATES {
        for &(dst_rate, dst_id) in SLIN_RATES {
            if src_rate != dst_rate {
                translators.push(std::sync::Arc::new(
                    ResampleTranslator::new(src_rate, src_id, dst_rate, dst_id),
                ));
            }
        }
    }
    translators
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_same_rate() {
        let input: Vec<i16> = vec![100, 200, 300, 400];
        let output = resample_linear(&input, 8000, 8000);
        assert_eq!(output, input);
    }

    #[test]
    fn test_resample_upsample_2x() {
        let input: Vec<i16> = vec![0, 1000, 2000, 3000];
        let output = resample_linear(&input, 8000, 16000);
        // Should produce approximately twice as many samples
        assert!(output.len() >= input.len());
        assert!(output.len() <= input.len() * 2 + 1);
    }

    #[test]
    fn test_resample_downsample_2x() {
        let input: Vec<i16> = vec![0, 500, 1000, 1500, 2000, 2500, 3000, 3500];
        let output = resample_linear(&input, 16000, 8000);
        // Should produce approximately half as many samples
        assert!(output.len() <= input.len());
        assert!(output.len() >= input.len() / 2 - 1);
    }

    #[test]
    fn test_resample_empty() {
        let input: Vec<i16> = vec![];
        let output = resample_linear(&input, 8000, 16000);
        assert!(output.is_empty());
    }

    #[test]
    fn test_resample_preserves_dc() {
        // A constant signal should remain constant after resampling
        let input: Vec<i16> = vec![1000; 100];
        let output = resample_linear(&input, 8000, 16000);
        for &s in &output {
            assert_eq!(s, 1000);
        }
    }

    #[test]
    fn test_all_translator_count() {
        let translators = all_resample_translators();
        // 9 rates, so 9 * 8 = 72 translators
        assert_eq!(translators.len(), 9 * 8);
    }
}
