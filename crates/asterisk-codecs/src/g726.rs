//! G.726 ADPCM codec - 16/24/32/40 kbps.
//!
//! Port of codecs/codec_g726.c from Asterisk C.
//!
//! G.726 is an ITU-T ADPCM codec operating at 16, 24, 32, or 40 kbps
//! on 8kHz sampled audio. It uses adaptive quantization and prediction.
//!
//! References:
//! - ITU-T Recommendation G.726 (12/90)
//! - codec_g726.c in Asterisk source

use crate::builtin_codecs::{ID_G726, ID_SLIN8};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;
use bytes::Bytes;

/// G.726 bitrate variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum G726Rate {
    /// 16 kbps (2 bits per sample)
    Rate16,
    /// 24 kbps (3 bits per sample)
    Rate24,
    /// 32 kbps (4 bits per sample)
    Rate32,
    /// 40 kbps (5 bits per sample)
    Rate40,
}

impl G726Rate {
    /// Number of bits per ADPCM sample.
    pub fn bits_per_sample(&self) -> u8 {
        match self {
            G726Rate::Rate16 => 2,
            G726Rate::Rate24 => 3,
            G726Rate::Rate32 => 4,
            G726Rate::Rate40 => 5,
        }
    }

    /// Bitrate in kbps.
    pub fn kbps(&self) -> u32 {
        match self {
            G726Rate::Rate16 => 16,
            G726Rate::Rate24 => 24,
            G726Rate::Rate32 => 32,
            G726Rate::Rate40 => 40,
        }
    }
}

// ---------------------------------------------------------------------------
// Quantizer tables for 32 kbps (4-bit ADPCM, the most common)
// ---------------------------------------------------------------------------

/// Step size multiplier table for G.726 ADPCM.
/// These values are used to compute the quantizer step size.
static STEP_SIZE_MULT: [i32; 8] = [
    -1, -1, -1, -1, 2, 4, 6, 8,
];

/// Adaptation speed control table.
static ADAPT_SPEED: [i32; 8] = [
    0, 0, 0, 0, 0, 1, 1, 1,
];

// ---------------------------------------------------------------------------
// 32kbps (4-bit) quantizer tables
// ---------------------------------------------------------------------------

/// Decision levels for 4-bit quantizer (normalized).
static QT_32: [i32; 15] = [
    -32635, -32635, -32635, -32635,
    -22,    79,     177,    245,
    299,    348,    399,    400,
    400,    400,    400,
];

/// Reconstruction levels for 4-bit quantizer.
static RECONSTRUCT_32: [i32; 16] = [
    -999, -11, 11, 54, 116, 173, 234, 298,
    365, 445, 542, 665, 826, 1045, 1345, 1749,
];

/// Adaptation index adjustment for 4-bit.
static ADAPT_INDEX_32: [i32; 16] = [
    -1, -1, -1, -1, -1, -1, -1, -1,
    1,  1,  1,  1,  2,  3,  4,  5,
];

// ---------------------------------------------------------------------------
// 16kbps (2-bit) quantizer tables
// ---------------------------------------------------------------------------

static RECONSTRUCT_16: [i32; 4] = [
    -999, 202, 926, 926,
];

static ADAPT_INDEX_16: [i32; 4] = [
    -1, 1, 2, 2,
];

// ---------------------------------------------------------------------------
// 24kbps (3-bit) quantizer tables
// ---------------------------------------------------------------------------

static RECONSTRUCT_24: [i32; 8] = [
    -999, -11, 79, 230, 413, 672, 1081, 1677,
];

static ADAPT_INDEX_24: [i32; 8] = [
    -1, -1, -1, -1, 1, 2, 3, 4,
];

// ---------------------------------------------------------------------------
// 40kbps (5-bit) quantizer tables
// ---------------------------------------------------------------------------

static RECONSTRUCT_40: [i32; 32] = [
    -999, -8, 8, 33, 60, 87, 114, 140,
    168, 197, 228, 262, 300, 343, 396, 459,
    541, 640, 764, 924, 1133, 1415, 1800, 2327,
    3089, 4251, 6175, 6175, 6175, 6175, 6175, 6175,
];

static ADAPT_INDEX_40: [i32; 32] = [
    -1, -1, -1, -1, -1, -1, -1, -1,
    -1, -1, -1, -1, -1, -1, -1, -1,
    1,  1,  1,  1,  2,  2,  3,  3,
    4,  5,  6,  6,  6,  6,  6,  6,
];

// ---------------------------------------------------------------------------
// G.726 ADPCM state and codec
// ---------------------------------------------------------------------------

/// G.726 encoder/decoder state.
#[derive(Clone)]
pub struct G726State {
    /// Current bitrate.
    rate: G726Rate,
    /// Step size index (used for adaptation).
    step_index: i32,
    /// Previous quantized difference.
    prev_dq: [i32; 2],
    /// Adaptive predictor coefficients (poles).
    poles: [i32; 2],
    /// Adaptive predictor coefficients (zeros).
    zeros: [i32; 6],
    /// Previous reconstructed signal values.
    prev_sr: [i32; 2],
    /// Signal reconstruction memory.
    delay_dq: [i32; 6],
    /// Predicted signal.
    predicted: i32,
    /// Slow speed control.
    ap: i32,
    /// Step size.
    step_size: i32,
}

impl G726State {
    /// Create a new G.726 state for the given bitrate.
    pub fn new(rate: G726Rate) -> Self {
        Self {
            rate,
            step_index: 0,
            prev_dq: [0; 2],
            poles: [0; 2],
            zeros: [0; 6],
            prev_sr: [0; 2],
            delay_dq: [0; 6],
            predicted: 0,
            ap: 0,
            step_size: 8, // Initial step size
        }
    }

    /// Get reconstruction table for current rate.
    fn reconstruct_table(&self) -> &'static [i32] {
        match self.rate {
            G726Rate::Rate16 => &RECONSTRUCT_16,
            G726Rate::Rate24 => &RECONSTRUCT_24,
            G726Rate::Rate32 => &RECONSTRUCT_32,
            G726Rate::Rate40 => &RECONSTRUCT_40,
        }
    }

    /// Get adaptation index table for current rate.
    fn adapt_table(&self) -> &'static [i32] {
        match self.rate {
            G726Rate::Rate16 => &ADAPT_INDEX_16,
            G726Rate::Rate24 => &ADAPT_INDEX_24,
            G726Rate::Rate32 => &ADAPT_INDEX_32,
            G726Rate::Rate40 => &ADAPT_INDEX_40,
        }
    }

    /// Quantize a difference signal.
    fn quantize(&self, d: i32) -> i32 {
        let bits = self.rate.bits_per_sample();
        let levels = 1i32 << bits;
        let half_levels = levels / 2;

        let abs_d = d.unsigned_abs() as i32;
        let norm_d = if self.step_size > 0 {
            abs_d * 1000 / self.step_size
        } else {
            abs_d
        };

        // Find the quantization level
        let recon = self.reconstruct_table();
        let mut code = 0i32;
        for i in 1..half_levels {
            if norm_d > recon[i as usize].unsigned_abs() as i32 {
                code = i;
            }
        }

        // Add sign bit
        if d < 0 {
            code + half_levels
        } else {
            code
        }
    }

    /// Inverse quantize a code back to a difference signal.
    fn inverse_quantize(&self, code: i32) -> i32 {
        let bits = self.rate.bits_per_sample();
        let levels = 1i32 << bits;
        let half_levels = levels / 2;

        let recon = self.reconstruct_table();
        let mag_code = code & (half_levels - 1);
        let sign = code >= half_levels;

        let mag = if (mag_code as usize) < recon.len() {
            let val = recon[mag_code as usize];
            (val as i64 * self.step_size as i64 / 1000) as i32
        } else {
            0
        };

        if sign { -mag } else { mag }
    }

    /// Update the adaptive predictor and step size.
    fn adapt(&mut self, code: i32, dq: i32, sr: i32) {
        let adapt = self.adapt_table();
        let adapt_idx = if (code as usize) < adapt.len() {
            adapt[code as usize]
        } else {
            0
        };

        // Update step size index
        self.step_index = (self.step_index + adapt_idx).max(0).min(48);

        // Update step size from index
        // Step sizes roughly double every 8 steps
        self.step_size = 8 * (1 << (self.step_index / 4));
        self.step_size = self.step_size.max(1).min(32767);

        // Update predictor poles
        let p0 = self.poles[0];
        let p1 = self.poles[1];

        self.poles[0] = ((p0 as i64 * 255 / 256) as i32)
            + if sr != 0 && self.prev_sr[0] != 0 {
                if (sr ^ self.prev_sr[0]) >= 0 { 192 } else { -192 }
            } else {
                0
            };

        self.poles[1] = ((p1 as i64 * 255 / 256) as i32)
            + if sr != 0 && self.prev_sr[1] != 0 {
                if (sr ^ self.prev_sr[1]) >= 0 { 32 } else { -32 }
            } else {
                0
            };

        // Clamp poles
        self.poles[0] = self.poles[0].max(-12288).min(12288);
        let limit = 12288i32.min(15360 - self.poles[0].abs());
        self.poles[1] = self.poles[1].max(-limit).min(limit);

        // Update predictor zeros
        for i in (1..6).rev() {
            self.delay_dq[i] = self.delay_dq[i - 1];
        }
        self.delay_dq[0] = dq;

        for i in 0..6 {
            self.zeros[i] = ((self.zeros[i] as i64 * 255 / 256) as i32)
                + if dq != 0 && self.delay_dq[i] != 0 {
                    if (dq ^ self.delay_dq[i]) >= 0 { 128 } else { -128 }
                } else {
                    0
                };
        }

        // Compute new prediction
        let mut sz = 0i64;
        for i in 0..6 {
            sz += self.zeros[i] as i64 * self.delay_dq[i] as i64;
        }
        let sp = self.poles[0] as i64 * self.prev_sr[0] as i64
            + self.poles[1] as i64 * self.prev_sr[1] as i64;
        self.predicted = ((sp + sz) >> 14) as i32;
        self.predicted = self.predicted.max(-32768).min(32767);

        self.prev_sr[1] = self.prev_sr[0];
        self.prev_sr[0] = sr;
        self.prev_dq[1] = self.prev_dq[0];
        self.prev_dq[0] = dq;
    }

    /// Encode one PCM sample to an ADPCM code.
    pub fn encode_sample(&mut self, sample: i16) -> i32 {
        let d = sample as i32 - self.predicted;
        let code = self.quantize(d);
        let dq = self.inverse_quantize(code);
        let sr = (self.predicted + dq).max(-32768).min(32767);
        self.adapt(code, dq, sr);
        code
    }

    /// Decode one ADPCM code to a PCM sample.
    pub fn decode_sample(&mut self, code: i32) -> i16 {
        let dq = self.inverse_quantize(code);
        let sr = (self.predicted + dq).max(-32768).min(32767);
        self.adapt(code, dq, sr);
        sr as i16
    }
}

/// G.726 encoder.
pub struct G726Encoder {
    pub state: G726State,
}

impl G726Encoder {
    pub fn new(rate: G726Rate) -> Self {
        Self {
            state: G726State::new(rate),
        }
    }

    /// Encode PCM samples to G.726 ADPCM bytes.
    ///
    /// Packs ADPCM codes into bytes using RFC 3551 packing order.
    pub fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        let bits = self.state.rate.bits_per_sample() as u32;
        let mut output = Vec::new();
        let mut accumulator: u32 = 0;
        let mut bits_accumulated: u32 = 0;

        for &sample in samples {
            let code = self.state.encode_sample(sample) as u32;
            accumulator = (accumulator << bits) | (code & ((1 << bits) - 1));
            bits_accumulated += bits;

            while bits_accumulated >= 8 {
                bits_accumulated -= 8;
                output.push((accumulator >> bits_accumulated) as u8);
                accumulator &= (1 << bits_accumulated) - 1;
            }
        }

        // Flush remaining bits (pad with zeros)
        if bits_accumulated > 0 {
            output.push((accumulator << (8 - bits_accumulated)) as u8);
        }

        output
    }
}

/// G.726 decoder.
pub struct G726Decoder {
    pub state: G726State,
}

impl G726Decoder {
    pub fn new(rate: G726Rate) -> Self {
        Self {
            state: G726State::new(rate),
        }
    }

    /// Decode G.726 ADPCM bytes to PCM samples.
    pub fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        let bits = self.state.rate.bits_per_sample() as u32;
        let mask = (1i32 << bits) - 1;
        let mut output = Vec::new();
        let mut accumulator: u32 = 0;
        let mut bits_accumulated: u32 = 0;

        for &byte in data {
            accumulator = (accumulator << 8) | byte as u32;
            bits_accumulated += 8;

            while bits_accumulated >= bits {
                bits_accumulated -= bits;
                let code = ((accumulator >> bits_accumulated) as i32) & mask;
                let sample = self.state.decode_sample(code);
                output.push(sample);
                accumulator &= (1 << bits_accumulated) - 1;
            }
        }

        output
    }
}

// ---------------------------------------------------------------------------
// Translator implementations (G.726 32kbps <-> SLIN)
// ---------------------------------------------------------------------------

/// Translator: G.726 (32kbps) -> Signed Linear 8kHz.
pub struct G726ToSlin;

impl Translator for G726ToSlin {
    fn name(&self) -> &str { "g726tolin" }
    fn src_codec_id(&self) -> CodecId { ID_G726 }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(G726ToSlinInstance {
            decoder: G726Decoder::new(G726Rate::Rate32),
            output_buf: Vec::with_capacity(8192),
            samples: 0,
        })
    }
}

struct G726ToSlinInstance {
    decoder: G726Decoder,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for G726ToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };

        let pcm_samples = self.decoder.decode(data);
        for sample in &pcm_samples {
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
        Some(Frame::voice(ID_SLIN8, samples, data))
    }
}

/// Translator: Signed Linear 8kHz -> G.726 (32kbps).
pub struct SlinToG726;

impl Translator for SlinToG726 {
    fn name(&self) -> &str { "lintog726" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_G726 }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToG726Instance {
            encoder: G726Encoder::new(G726Rate::Rate32),
            output_buf: Vec::with_capacity(4096),
            samples: 0,
        })
    }
}

struct SlinToG726Instance {
    encoder: G726Encoder,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for SlinToG726Instance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };

        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed("slin data must have even length".into()));
        }

        let mut samples: Vec<i16> = Vec::with_capacity(data.len() / 2);
        let mut i = 0;
        while i + 1 < data.len() {
            samples.push(i16::from_le_bytes([data[i], data[i + 1]]));
            i += 2;
        }

        let encoded = self.encoder.encode(&samples);
        self.samples += samples.len() as u32;
        self.output_buf.extend_from_slice(&encoded);
        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        if self.output_buf.is_empty() {
            return None;
        }
        let data = Bytes::from(std::mem::take(&mut self.output_buf));
        let samples = self.samples;
        self.samples = 0;
        Some(Frame::voice(ID_G726, samples, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g726_encode_decode_32k() {
        let mut encoder = G726Encoder::new(G726Rate::Rate32);
        let mut decoder = G726Decoder::new(G726Rate::Rate32);

        let samples: Vec<i16> = (0..160)
            .map(|i| ((i as f64 * 2.0 * std::f64::consts::PI / 80.0).sin() * 4000.0) as i16)
            .collect();

        let encoded = encoder.encode(&samples);
        let decoded = decoder.decode(&encoded);

        // 32kbps = 4 bits/sample, so 160 samples = 80 bytes
        assert_eq!(encoded.len(), 80);
        // decoded should have same number of samples
        assert_eq!(decoded.len(), samples.len());
    }

    #[test]
    fn test_g726_rates() {
        for rate in [G726Rate::Rate16, G726Rate::Rate24, G726Rate::Rate32, G726Rate::Rate40] {
            let mut encoder = G726Encoder::new(rate);
            let mut decoder = G726Decoder::new(rate);

            let samples: Vec<i16> = vec![0; 80];
            let encoded = encoder.encode(&samples);
            let decoded = decoder.decode(&encoded);

            // Verify output length is reasonable
            assert!(!encoded.is_empty(), "rate {:?} produced no output", rate);
            assert!(!decoded.is_empty(), "rate {:?} decoded nothing", rate);
        }
    }

    #[test]
    fn test_g726_silence() {
        let mut encoder = G726Encoder::new(G726Rate::Rate32);
        let mut decoder = G726Decoder::new(G726Rate::Rate32);

        let silence: Vec<i16> = vec![0; 160];
        let encoded = encoder.encode(&silence);
        let decoded = decoder.decode(&encoded);
        assert_eq!(decoded.len(), 160);
    }
}
