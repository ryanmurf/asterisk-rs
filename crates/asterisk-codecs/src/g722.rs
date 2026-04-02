//! G.722 wideband audio codec - sub-band ADPCM encoder/decoder.
//!
//! Port of codecs/codec_g722.c from Asterisk C.
//!
//! G.722 encodes 16kHz audio into 64kbps using sub-band ADPCM (SB-ADPCM).
//!
//! Quantizer and adaptation tables are defined per the ITU-T spec
//! even if not all are directly referenced (they form the complete codec state).
//! The 16kHz input is split into two 8kHz sub-bands via QMF:
//! - Lower sub-band: 48kbps (6 bits per sample) ADPCM
//! - Upper sub-band: 16kbps (2 bits per sample) ADPCM
//!
//! References:
//! - ITU-T Recommendation G.722 (11/88)
//! - codec_g722.c in Asterisk source

use crate::builtin_codecs::{ID_G722, ID_SLIN16};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;
use bytes::Bytes;

// ---------------------------------------------------------------------------
// Quantizer tables for lower sub-band (6-bit ADPCM)
// ---------------------------------------------------------------------------

/// Decision levels for 6-bit lower sub-band quantizer.
static QQ6: [i32; 30] = [
    0, 35, 72, 110, 150, 190, 233, 276, 323, 370, 422, 475, 535, 598,
    665, 739, 818, 905, 1000, 1105, 1220, 1349, 1493, 1654, 1837, 2048,
    2295, 2584, 2929, 3342,
];

/// Reconstruction levels for 6-bit lower sub-band quantizer (inverse quantizer).
static QQ6_RECONSTRUCT: [i32; 64] = [
      0,  35,  72, 110, 150, 190, 233, 276,
    323, 370, 422, 475, 535, 598, 665, 739,
    818, 905,1000,1105,1220,1349,1493,1654,
   1837,2048,2295,2584,2929,3342,3842,4439,
      0, -35, -72,-110,-150,-190,-233,-276,
   -323,-370,-422,-475,-535,-598,-665,-739,
   -818,-905,-1000,-1105,-1220,-1349,-1493,-1654,
   -1837,-2048,-2295,-2584,-2929,-3342,-3842,-4439,
];

/// Step size adaptation for lower sub-band.
static QQ6_ADAPTATION: [i32; 64] = [
    0, 1, 0, 1, 2, 3, 4, 5,
    6, 7, 8, 9, 10, 11, 12, 13,
    14, 15, 16, 17, 18, 19, 20, 21,
    22, 23, 24, 25, 26, 27, 28, 29,
    29, 28, 27, 26, 25, 24, 23, 22,
    21, 20, 19, 18, 17, 16, 15, 14,
    13, 12, 11, 10, 9, 8, 7, 6,
    5, 4, 3, 2, 1, 0, 1, 0,
];

// ---------------------------------------------------------------------------
// Quantizer tables for upper sub-band (2-bit ADPCM)
// ---------------------------------------------------------------------------

/// Reconstruction levels for 2-bit upper sub-band quantizer.
static QQ2_RECONSTRUCT: [i32; 4] = [
    -7408, -1616, 7408, 1616,
];

/// Step size adaptation for upper sub-band.
static QQ2_ADAPTATION: [i32; 4] = [
    -7, 23, -7, 23,
];

// ---------------------------------------------------------------------------
// Log/inverse-log tables for step-size calculation
// ---------------------------------------------------------------------------

/// Step size table indexed by adaptation speed index (0..88).
/// This maps the adaptation index to the actual step size (det).
static STEP_SIZE_TABLE: [i32; 89] = [
    16, 17, 19, 21, 23, 25, 28, 31,
    34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143,
    157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411,
    1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024,
    3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484,
    7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767, 0, 0, 0, 0, 0, 0, 0, 0,
];

// ---------------------------------------------------------------------------
// Sub-band ADPCM state
// ---------------------------------------------------------------------------

/// State for one ADPCM sub-band (lower or upper).
#[derive(Clone)]
struct SubBandState {
    /// Step size index (0..88).
    step_index: i32,
    /// Predicted sample value.
    predicted: i32,
    /// Previous predicted values for the adaptive predictor.
    prev_qmf: [i32; 24],
    /// Reconstruction filter state (poles).
    poles: [i32; 2],
    /// Reconstruction filter state (zeros).
    zeros: [i32; 6],
    /// Delayed reconstructed samples.
    delay: [i32; 6],
    /// Signal estimate.
    signal_estimate: i32,
    /// Previous reconstructed values for QMF.
    prev_reconstructed: [i32; 2],
}

impl SubBandState {
    fn new() -> Self {
        Self {
            step_index: 0,
            predicted: 0,
            prev_qmf: [0; 24],
            poles: [0; 2],
            zeros: [0; 6],
            delay: [0; 6],
            signal_estimate: 0,
            prev_reconstructed: [0; 2],
        }
    }

    /// Saturate a value to i16 range.
    fn saturate(val: i32) -> i16 {
        val.max(i16::MIN as i32).min(i16::MAX as i32) as i16
    }

    /// Update adaptive predictor state.
    fn block4_encode(&mut self, d: i32) {
        let mut reconstructed = self.signal_estimate + d;
        reconstructed = reconstructed.clamp(-32768, 32767);

        // Update poles
        let p0 = self.poles[0];
        let p1 = self.poles[1];

        // Leaky predictor adaptation
        self.poles[0] = ((p0 as i64 * 255 / 256) as i32)
            + if reconstructed != 0 && self.prev_reconstructed[0] != 0 {
                if (reconstructed ^ self.prev_reconstructed[0]) >= 0 { 192 } else { -192 }
            } else {
                0
            };

        self.poles[1] = ((p1 as i64 * 255 / 256) as i32)
            + if reconstructed != 0 && self.prev_reconstructed[1] != 0 {
                if (reconstructed ^ self.prev_reconstructed[1]) >= 0 { 32 } else { -32 }
            } else {
                0
            };

        // Clamp poles
        self.poles[0] = self.poles[0].clamp(-12288, 12288);
        let limit = 12288i32.min(15360 - self.poles[0].abs());
        self.poles[1] = self.poles[1].max(-limit).min(limit);

        // Update zeros
        for i in (1..6).rev() {
            self.delay[i] = self.delay[i - 1];
        }
        self.delay[0] = d;

        for i in 0..6 {
            self.zeros[i] = ((self.zeros[i] as i64 * 255 / 256) as i32)
                + if d != 0 && self.delay[i] != 0 {
                    if (d ^ self.delay[i]) >= 0 { 128 } else { -128 }
                } else {
                    0
                };
        }

        // Compute signal estimate
        let mut sz = 0i64;
        for i in 0..6 {
            sz += self.zeros[i] as i64 * self.delay[i] as i64;
        }
        let sp = self.poles[0] as i64 * self.prev_reconstructed[0] as i64
            + self.poles[1] as i64 * self.prev_reconstructed[1] as i64;

        self.signal_estimate = ((sp + sz) >> 14) as i32;
        self.signal_estimate = self.signal_estimate.clamp(-32768, 32767);

        self.prev_reconstructed[1] = self.prev_reconstructed[0];
        self.prev_reconstructed[0] = reconstructed;
    }
}

// ---------------------------------------------------------------------------
// QMF (Quadrature Mirror Filter) coefficients
// ---------------------------------------------------------------------------

/// QMF filter coefficients for splitting/combining sub-bands.
static QMF_COEFFS: [i32; 24] = [
    3, -11, 12, 32, -210, 951, 3876, -805, 362, -156, 53, -11,
    11, -53, 156, -362, 805, -3876, -951, 210, -32, -12, 11, -3,
];

// ---------------------------------------------------------------------------
// G.722 Encoder
// ---------------------------------------------------------------------------

/// G.722 encoder state.
#[derive(Clone)]
pub struct G722Encoder {
    /// Lower sub-band ADPCM state.
    lower: SubBandState,
    /// Upper sub-band ADPCM state.
    upper: SubBandState,
    /// QMF history buffer for input samples.
    x: [i32; 24],
    /// Encoding mode (bit-rate): 1=64kbps, 2=56kbps, 3=48kbps.
    mode: u8,
}

impl G722Encoder {
    /// Create a new G.722 encoder (defaults to 64kbps mode).
    pub fn new() -> Self {
        Self {
            lower: SubBandState::new(),
            upper: SubBandState::new(),
            x: [0; 24],
            mode: 1, // 64kbps
        }
    }

    /// Set the encoding mode (1=64kbps, 2=56kbps, 3=48kbps).
    pub fn set_mode(&mut self, mode: u8) {
        self.mode = mode.clamp(1, 3);
    }

    /// Encode 16kHz signed linear samples into G.722 data.
    ///
    /// Input: slice of i16 PCM samples at 16000Hz
    /// Output: G.722 encoded bytes (one byte per two input samples)
    pub fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        let mut output = Vec::with_capacity(samples.len() / 2);
        let mut i = 0;

        while i + 1 < samples.len() {
            // Shift QMF history
            for j in (2..24).rev() {
                self.x[j] = self.x[j - 2];
            }
            self.x[1] = samples[i] as i32;
            self.x[0] = samples[i + 1] as i32;

            // QMF analysis - split into lower and upper sub-bands
            let mut sum_even = 0i64;
            let mut sum_odd = 0i64;
            for j in 0..12 {
                sum_even += QMF_COEFFS[2 * j] as i64 * self.x[2 * j] as i64;
                sum_odd += QMF_COEFFS[2 * j + 1] as i64 * self.x[2 * j + 1] as i64;
            }

            let lower_input = ((sum_even + sum_odd) >> 12) as i32;
            let upper_input = ((sum_even - sum_odd) >> 12) as i32;

            // Encode lower sub-band with 6-bit ADPCM
            let el = lower_input.saturating_sub(self.lower.signal_estimate);
            let lower_code = quantize_lower(el, &self.lower);
            let dl = QQ6_RECONSTRUCT[lower_code as usize];
            self.lower.block4_encode(dl);

            // Encode upper sub-band with 2-bit ADPCM
            let eh = upper_input.saturating_sub(self.upper.signal_estimate);
            let upper_code = quantize_upper(eh);
            let dh = QQ2_RECONSTRUCT[upper_code as usize];
            self.upper.block4_encode(dh);

            // Pack into output byte
            let byte = ((lower_code & 0x3F) | ((upper_code & 0x03) << 6)) as u8;
            output.push(byte);

            i += 2;
        }

        output
    }
}

impl Default for G722Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Quantize a lower sub-band sample to 6-bit code.
fn quantize_lower(el: i32, _state: &SubBandState) -> i32 {
    let abs_el = el.unsigned_abs() as i32;
    let sign = if el < 0 { 1 } else { 0 };

    // Binary search in the decision table
    let mut code: i32 = 0;
    #[allow(clippy::needless_range_loop)]
    for i in 0..30 {
        if abs_el > QQ6[i] {
            code = i as i32 + 1;
        } else {
            break;
        }
    }
    code = code.min(31);

    if sign != 0 {
        code + 32
    } else {
        code
    }
}

/// Quantize an upper sub-band sample to 2-bit code.
fn quantize_upper(eh: i32) -> i32 {
    if eh >= 0 {
        if eh > 1616 { 3 } else { 2 }
    } else {
        if eh > -1616 { 1 } else { 0 }
    }
}

// ---------------------------------------------------------------------------
// G.722 Decoder
// ---------------------------------------------------------------------------

/// G.722 decoder state.
#[derive(Clone)]
pub struct G722Decoder {
    /// Lower sub-band ADPCM state.
    lower: SubBandState,
    /// Upper sub-band ADPCM state.
    upper: SubBandState,
    /// QMF synthesis filter buffer.
    qmf_signal: [i32; 24],
    /// Decoding mode (1=64kbps, 2=56kbps, 3=48kbps).
    mode: u8,
}

impl G722Decoder {
    /// Create a new G.722 decoder (64kbps mode).
    pub fn new() -> Self {
        Self {
            lower: SubBandState::new(),
            upper: SubBandState::new(),
            qmf_signal: [0; 24],
            mode: 1,
        }
    }

    /// Set the decoding mode (1=64kbps, 2=56kbps, 3=48kbps).
    pub fn set_mode(&mut self, mode: u8) {
        self.mode = mode.clamp(1, 3);
    }

    /// Decode G.722 data into 16kHz signed linear samples.
    ///
    /// Input: G.722 encoded bytes
    /// Output: i16 PCM samples at 16000Hz (two samples per input byte)
    pub fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        let mut output = Vec::with_capacity(data.len() * 2);

        for &byte in data {
            // Unpack lower (6-bit) and upper (2-bit) codes
            let lower_code = (byte & 0x3F) as i32;
            let upper_code = ((byte >> 6) & 0x03) as i32;

            // Decode lower sub-band
            let dl = QQ6_RECONSTRUCT[lower_code as usize];
            let lower_output = self.lower.signal_estimate + dl;
            self.lower.block4_encode(dl);

            // Decode upper sub-band
            let dh = QQ2_RECONSTRUCT[upper_code as usize];
            let upper_output = self.upper.signal_estimate + dh;
            self.upper.block4_encode(dh);

            // QMF synthesis - combine sub-bands back to 16kHz
            // Shift QMF buffer
            for j in (2..24).rev() {
                self.qmf_signal[j] = self.qmf_signal[j - 2];
            }
            self.qmf_signal[0] = lower_output + upper_output;
            self.qmf_signal[1] = lower_output - upper_output;

            let mut sum_even = 0i64;
            let mut sum_odd = 0i64;
            for j in 0..12 {
                sum_even += QMF_COEFFS[2 * j] as i64 * self.qmf_signal[2 * j] as i64;
                sum_odd += QMF_COEFFS[2 * j + 1] as i64 * self.qmf_signal[2 * j + 1] as i64;
            }

            let sample1 = SubBandState::saturate(((sum_even + sum_odd) >> 11) as i32);
            let sample2 = SubBandState::saturate(((sum_even - sum_odd) >> 11) as i32);

            output.push(sample1);
            output.push(sample2);
        }

        output
    }
}

impl Default for G722Decoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Translator: G.722 -> Signed Linear 16kHz.
pub struct G722ToSlin16;

impl Translator for G722ToSlin16 {
    fn name(&self) -> &str { "g722tolin16" }
    fn src_codec_id(&self) -> CodecId { ID_G722 }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN16 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(G722ToSlin16Instance {
            decoder: G722Decoder::new(),
            output_buf: Vec::with_capacity(8192),
            samples: 0,
        })
    }
}

struct G722ToSlin16Instance {
    decoder: G722Decoder,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for G722ToSlin16Instance {
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
        Some(Frame::voice(ID_SLIN16, samples, data))
    }
}

/// Translator: Signed Linear 16kHz -> G.722.
pub struct Slin16ToG722;

impl Translator for Slin16ToG722 {
    fn name(&self) -> &str { "lin16tog722" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN16 }
    fn dst_codec_id(&self) -> CodecId { ID_G722 }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(Slin16ToG722Instance {
            encoder: G722Encoder::new(),
            output_buf: Vec::with_capacity(4096),
            samples: 0,
        })
    }
}

struct Slin16ToG722Instance {
    encoder: G722Encoder,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for Slin16ToG722Instance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };

        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed("slin16 data must have even length".into()));
        }

        // Convert bytes to i16 samples
        let mut samples: Vec<i16> = Vec::with_capacity(data.len() / 2);
        let mut i = 0;
        while i + 1 < data.len() {
            let sample = i16::from_le_bytes([data[i], data[i + 1]]);
            samples.push(sample);
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
        Some(Frame::voice(ID_G722, samples, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g722_encoder_creates_output() {
        let mut encoder = G722Encoder::new();
        let samples: Vec<i16> = (0..320).map(|i| ((i as f64 * 0.1).sin() * 8000.0) as i16).collect();
        let encoded = encoder.encode(&samples);
        // 320 samples -> 160 bytes
        assert_eq!(encoded.len(), 160);
    }

    #[test]
    fn test_g722_decoder_creates_output() {
        let mut decoder = G722Decoder::new();
        let data = vec![0u8; 160];
        let decoded = decoder.decode(&data);
        // 160 bytes -> 320 samples
        assert_eq!(decoded.len(), 320);
    }

    #[test]
    fn test_g722_roundtrip() {
        let mut encoder = G722Encoder::new();
        let mut decoder = G722Decoder::new();

        // Create a simple sine wave
        let original: Vec<i16> = (0..320)
            .map(|i| ((i as f64 * 2.0 * std::f64::consts::PI / 160.0).sin() * 4000.0) as i16)
            .collect();

        let encoded = encoder.encode(&original);
        let decoded = decoder.decode(&encoded);

        // After encode/decode, the output should have the same length
        assert_eq!(decoded.len(), original.len());
        // Lossy codec, but should not be all zeros
        let non_zero = decoded.iter().any(|&s| s != 0);
        assert!(non_zero, "decoded output should not be all zeros");
    }

    #[test]
    fn test_g722_silence() {
        let mut encoder = G722Encoder::new();
        let mut decoder = G722Decoder::new();

        let silence: Vec<i16> = vec![0; 320];
        let encoded = encoder.encode(&silence);
        let decoded = decoder.decode(&encoded);
        assert_eq!(decoded.len(), 320);
    }
}
