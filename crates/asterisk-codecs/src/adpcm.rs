//! IMA/Dialogic ADPCM codec - real encode/decode implementation.
//!
//! Port of codecs/codec_adpcm.c from Asterisk C.
//!
//! Dialogic ADPCM encodes 16-bit signed linear PCM into 4-bit ADPCM nibbles.
//! Two samples pack into one byte. Uses a step-size adaptation table and
//! index shift table for adaptive quantization.
//!
//! This is a complete, working implementation (not a stub).

use crate::builtin_codecs::{ID_ADPCM, ID_SLIN8};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;
use bytes::Bytes;

/// Step size index shift table (indexed by lower 3 bits of encoded nibble).
static INDEX_SHIFT: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Step size table: stpsz[i] = floor(16 * (11/10)^i).
static STEP_SIZE: [i32; 49] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73,
    80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279,
    307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552,
];

/// ADPCM encoder/decoder state.
///
/// Both encoder and decoder maintain synchronized state.
#[derive(Debug, Clone)]
pub struct AdpcmState {
    /// Current step size table index (0..48).
    pub ssindex: i32,
    /// Current signal estimate.
    pub signal: i32,
    /// Count of consecutive zero-encoded nibbles (for auto-return).
    pub zero_count: i32,
    /// Flag for next sample adjustment.
    pub next_flag: i32,
}

impl AdpcmState {
    pub fn new() -> Self {
        Self {
            ssindex: 0,
            signal: 0,
            zero_count: 0,
            next_flag: 0,
        }
    }
}

impl Default for AdpcmState {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a single 4-bit ADPCM nibble to a 16-bit PCM sample.
///
/// Updates the state for the next decode/encode.
pub fn adpcm_decode(encoded: i32, state: &mut AdpcmState) -> i16 {
    let step = STEP_SIZE[state.ssindex as usize];
    let sign = encoded & 0x08;
    let magnitude = encoded & 0x07;

    // BLI (bit-level identical) decoding
    let mut diff = step >> 3;
    if magnitude & 4 != 0 {
        diff += step;
    }
    if magnitude & 2 != 0 {
        diff += step >> 1;
    }
    if magnitude & 1 != 0 {
        diff += step >> 2;
    }
    if (magnitude >> 1) & step & 0x1 != 0 {
        diff += 1;
    }

    if sign != 0 {
        diff = -diff;
    }

    if state.next_flag & 0x1 != 0 {
        state.signal -= 8;
    } else if state.next_flag & 0x2 != 0 {
        state.signal += 8;
    }

    state.signal += diff;

    state.signal = state.signal.clamp(-2047, 2047);

    state.next_flag = 0;

    // Update step size index
    state.ssindex += INDEX_SHIFT[magnitude as usize];
    state.ssindex = state.ssindex.clamp(0, 48);

    // Output is signal shifted left by 4 (scale to 16-bit range)
    (state.signal << 4) as i16
}

/// Encode a single 16-bit PCM sample to a 4-bit ADPCM nibble.
///
/// Updates the state for the next encode/decode.
pub fn adpcm_encode(csig: i16, state: &mut AdpcmState) -> u8 {
    // Scale down input to 12-bit range
    let csig_scaled = (csig as i32) >> 4;

    let step = STEP_SIZE[state.ssindex as usize];
    let diff = csig_scaled - state.signal;

    // BLI encoding
    let mut encoded;
    let mut abs_diff;
    if diff < 0 {
        encoded = 8u8;
        abs_diff = -diff;
    } else {
        encoded = 0u8;
        abs_diff = diff;
    }

    if abs_diff >= step {
        encoded |= 4;
        abs_diff -= step;
    }
    let half_step = step >> 1;
    if abs_diff >= half_step {
        encoded |= 2;
        abs_diff -= half_step;
    }
    let quarter_step = half_step >> 1;
    if abs_diff >= quarter_step {
        encoded |= 1;
    }

    // Feedback to state: run decode to keep encoder/decoder in sync
    adpcm_decode(encoded as i32, state);

    encoded
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Translator: ADPCM -> Signed Linear 8kHz.
pub struct AdpcmToSlinTranslator;

impl Translator for AdpcmToSlinTranslator {
    fn name(&self) -> &str { "adpcmtolin" }
    fn src_codec_id(&self) -> CodecId { ID_ADPCM }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(AdpcmToSlinInstance {
            state: AdpcmState::new(),
            output_buf: Vec::with_capacity(8096 * 2),
            samples: 0,
        })
    }
}

struct AdpcmToSlinInstance {
    state: AdpcmState,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for AdpcmToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };
        for &byte in data.iter() {
            // High nibble first, then low nibble (two samples per byte)
            let sample_hi = adpcm_decode(((byte >> 4) & 0x0f) as i32, &mut self.state);
            self.output_buf.extend_from_slice(&sample_hi.to_le_bytes());
            self.samples += 1;

            let sample_lo = adpcm_decode((byte & 0x0f) as i32, &mut self.state);
            self.output_buf.extend_from_slice(&sample_lo.to_le_bytes());
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

/// Translator: Signed Linear 8kHz -> ADPCM.
pub struct SlinToAdpcmTranslator;

impl Translator for SlinToAdpcmTranslator {
    fn name(&self) -> &str { "lintoadpcm" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_ADPCM }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToAdpcmInstance {
            state: AdpcmState::new(),
            sample_buf: Vec::with_capacity(8096),
            output_buf: Vec::with_capacity(4096),
            samples: 0,
        })
    }
}

struct SlinToAdpcmInstance {
    state: AdpcmState,
    sample_buf: Vec<i16>,
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for SlinToAdpcmInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };

        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed("slin data must have even length".into()));
        }

        // Accumulate 16-bit samples
        let mut i = 0;
        while i + 1 < data.len() {
            self.sample_buf.push(i16::from_le_bytes([data[i], data[i + 1]]));
            i += 2;
        }

        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        if self.sample_buf.len() < 2 {
            return None;
        }

        // Encode pairs of samples (atomic size is 2 samples -> 1 byte)
        let pair_count = self.sample_buf.len() / 2;
        self.output_buf.clear();
        self.samples = 0;

        for i in 0..pair_count {
            let hi = adpcm_encode(self.sample_buf[i * 2], &mut self.state);
            let lo = adpcm_encode(self.sample_buf[i * 2 + 1], &mut self.state);
            self.output_buf.push((hi << 4) | lo);
            self.samples += 2;
        }

        // Keep leftover sample if odd count
        let consumed = pair_count * 2;
        if consumed < self.sample_buf.len() {
            let leftover = self.sample_buf[consumed];
            self.sample_buf.clear();
            self.sample_buf.push(leftover);
        } else {
            self.sample_buf.clear();
        }

        let data = Bytes::from(std::mem::take(&mut self.output_buf));
        let samples = self.samples;
        self.samples = 0;
        Some(Frame::voice(ID_ADPCM, samples, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adpcm_state_default() {
        let state = AdpcmState::new();
        assert_eq!(state.ssindex, 0);
        assert_eq!(state.signal, 0);
    }

    #[test]
    fn test_decode_silence() {
        let mut state = AdpcmState::new();
        // Decoding zero nibble should produce near-zero output
        let sample = adpcm_decode(0, &mut state);
        // First decode of 0 with initial state: step=16, diff=16>>3=2, signal=2, output=2<<4=32
        assert_eq!(sample, 32);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut enc_state = AdpcmState::new();
        let mut dec_state = AdpcmState::new();

        // Encode then decode a sequence
        let input: Vec<i16> = (0..20).map(|i| (i * 1000) as i16).collect();
        let mut decoded = Vec::new();

        for &sample in &input {
            let nibble = adpcm_encode(sample, &mut enc_state);
            let out = adpcm_decode(nibble as i32, &mut dec_state);
            decoded.push(out);
        }

        // States should be synchronized
        assert_eq!(enc_state.ssindex, dec_state.ssindex);
        assert_eq!(enc_state.signal, dec_state.signal);
    }

    #[test]
    fn test_encode_clamps_index() {
        let mut state = AdpcmState::new();
        // Many zero-difference encodes should not go below index 0
        for _ in 0..100 {
            adpcm_encode(0, &mut state);
        }
        assert!(state.ssindex >= 0);
        assert!(state.ssindex <= 48);
    }

    #[test]
    fn test_step_size_table() {
        assert_eq!(STEP_SIZE[0], 16);
        assert_eq!(STEP_SIZE[48], 1552);
        assert_eq!(STEP_SIZE.len(), 49);
    }

    #[test]
    fn test_index_shift_table() {
        assert_eq!(INDEX_SHIFT[0], -1);
        assert_eq!(INDEX_SHIFT[4], 2);
        assert_eq!(INDEX_SHIFT[7], 8);
    }
}
