//! LPC-10 (Linear Predictor Code) codec integration layer.
//!
//! Port of codecs/codec_lpc10.c from Asterisk C.
//!
//! LPC-10 is a 2400bps vocoder. Frames are 180 samples (22.5ms at 8kHz),
//! compressed to 7 bytes (54 bits + 2 padding bits).
//!
//! This module provides the configuration, trait implementations, and
//! translator registrations. The actual DSP is stubbed since it requires
//! the external LPC-10 library.
//!
//! References:
//! - Federal Standard 1015 / NATO STANAG 4198

use crate::builtin_codecs::{ID_LPC10, ID_SLIN8};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;

/// Number of samples per LPC-10 frame (22.5ms at 8000 Hz).
pub const LPC10_SAMPLES_PER_FRAME: u32 = 180;

/// Bits in a compressed LPC-10 frame.
pub const LPC10_BITS_IN_COMPRESSED_FRAME: usize = 54;

/// Bytes in a compressed LPC-10 frame (54 bits padded to byte boundary).
pub const LPC10_BYTES_IN_COMPRESSED_FRAME: usize =
    (LPC10_BITS_IN_COMPRESSED_FRAME + 7) / 8; // == 7

/// LPC-10 encoder (stub).
///
/// In a real implementation, this would hold lpc10_encoder_state from liblpc10.
pub struct Lpc10Encoder {
    _private: (),
}

impl Lpc10Encoder {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Encode PCM samples to LPC-10 data.
    ///
    /// STUB: Real implementation requires liblpc10.
    pub fn encode(&mut self, _samples: &[i16]) -> Result<Vec<u8>, TranslateError> {
        Err(TranslateError::Failed(
            "LPC-10 encoding requires liblpc10 (not linked)".into(),
        ))
    }
}

impl Default for Lpc10Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// LPC-10 decoder (stub).
pub struct Lpc10Decoder {
    _private: (),
}

impl Lpc10Decoder {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Decode LPC-10 data to PCM samples.
    ///
    /// STUB: Real implementation requires liblpc10.
    pub fn decode(&mut self, _data: &[u8]) -> Result<Vec<i16>, TranslateError> {
        Err(TranslateError::Failed(
            "LPC-10 decoding requires liblpc10 (not linked)".into(),
        ))
    }
}

impl Default for Lpc10Decoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Translator: LPC-10 -> Signed Linear 8kHz.
pub struct Lpc10ToSlinTranslator;

impl Translator for Lpc10ToSlinTranslator {
    fn name(&self) -> &str { "lpc10tolin" }
    fn src_codec_id(&self) -> CodecId { ID_LPC10 }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(Lpc10ToSlinInstance {
            decoder: Lpc10Decoder::new(),
        })
    }
}

struct Lpc10ToSlinInstance {
    decoder: Lpc10Decoder,
}

impl TranslatorInstance for Lpc10ToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };
        let _samples = self.decoder.decode(data)?;
        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        None
    }
}

/// Translator: Signed Linear 8kHz -> LPC-10.
pub struct SlinToLpc10Translator;

impl Translator for SlinToLpc10Translator {
    fn name(&self) -> &str { "lintolpc10" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_LPC10 }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToLpc10Instance {
            encoder: Lpc10Encoder::new(),
        })
    }
}

struct SlinToLpc10Instance {
    encoder: Lpc10Encoder,
}

impl TranslatorInstance for SlinToLpc10Instance {
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

        let _encoded = self.encoder.encode(&samples)?;
        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lpc10_constants() {
        assert_eq!(LPC10_SAMPLES_PER_FRAME, 180);
        assert_eq!(LPC10_BYTES_IN_COMPRESSED_FRAME, 7);
    }

    #[test]
    fn test_encoder_stub() {
        let mut enc = Lpc10Encoder::new();
        let samples = vec![0i16; 180];
        assert!(enc.encode(&samples).is_err());
    }

    #[test]
    fn test_decoder_stub() {
        let mut dec = Lpc10Decoder::new();
        let data = vec![0u8; 7];
        assert!(dec.decode(&data).is_err());
    }
}
