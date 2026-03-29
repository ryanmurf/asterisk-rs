//! Built-in translator implementations.
//!
//! Ports of codec_ulaw.c, codec_alaw.c from Asterisk.
//! These implement the actual mu-law <-> slin and A-law <-> slin conversions
//! using the lookup tables defined in ulaw_table.rs and alaw_table.rs.

use crate::builtin_codecs::{ID_ALAW, ID_G722, ID_SLIN16, ID_SLIN8, ID_ULAW};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use crate::ulaw_table::{linear_to_mulaw_fast, mulaw_to_linear};
use crate::alaw_table::{alaw_to_linear, linear_to_alaw_fast};
use asterisk_types::Frame;
use bytes::Bytes;
use std::sync::Arc;

/// Helper: extract raw audio data from a voice frame.
fn voice_data(frame: &Frame) -> Result<&Bytes, TranslateError> {
    match frame {
        Frame::Voice { data, .. } => Ok(data),
        _ => Err(TranslateError::Failed(
            "expected a voice frame".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// mu-law <-> Signed Linear
// ---------------------------------------------------------------------------

/// Translator descriptor: ulaw -> slin (8kHz).
pub struct UlawToSlinTranslator;

impl Translator for UlawToSlinTranslator {
    fn name(&self) -> &str { "ulawtolin" }
    fn src_codec_id(&self) -> CodecId { ID_ULAW }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(UlawToSlinInstance {
            output_buf: Vec::with_capacity(8096 * 2),
            samples: 0,
        })
    }
}

struct UlawToSlinInstance {
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for UlawToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = voice_data(frame)?;
        for &byte in data.iter() {
            let sample = mulaw_to_linear(byte);
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

/// Translator descriptor: slin -> ulaw (8kHz).
pub struct SlinToUlawTranslator;

impl Translator for SlinToUlawTranslator {
    fn name(&self) -> &str { "lintoulaw" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_ULAW }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToUlawInstance {
            output_buf: Vec::with_capacity(8096),
            samples: 0,
        })
    }
}

struct SlinToUlawInstance {
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for SlinToUlawInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = voice_data(frame)?;
        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed(
                "slin data length must be even".into(),
            ));
        }
        let mut i = 0;
        while i + 1 < data.len() {
            let sample = i16::from_le_bytes([data[i], data[i + 1]]);
            self.output_buf.push(linear_to_mulaw_fast(sample));
            self.samples += 1;
            i += 2;
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
        Some(Frame::voice(ID_ULAW, samples, data))
    }
}

// ---------------------------------------------------------------------------
// A-law <-> Signed Linear
// ---------------------------------------------------------------------------

/// Translator descriptor: alaw -> slin (8kHz).
pub struct AlawToSlinTranslator;

impl Translator for AlawToSlinTranslator {
    fn name(&self) -> &str { "alawtolin" }
    fn src_codec_id(&self) -> CodecId { ID_ALAW }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(AlawToSlinInstance {
            output_buf: Vec::with_capacity(8096 * 2),
            samples: 0,
        })
    }
}

struct AlawToSlinInstance {
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for AlawToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = voice_data(frame)?;
        for &byte in data.iter() {
            let sample = alaw_to_linear(byte);
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

/// Translator descriptor: slin -> alaw (8kHz).
pub struct SlinToAlawTranslator;

impl Translator for SlinToAlawTranslator {
    fn name(&self) -> &str { "lintoalaw" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_ALAW }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToAlawInstance {
            output_buf: Vec::with_capacity(8096),
            samples: 0,
        })
    }
}

struct SlinToAlawInstance {
    output_buf: Vec<u8>,
    samples: u32,
}

impl TranslatorInstance for SlinToAlawInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = voice_data(frame)?;
        if data.len() % 2 != 0 {
            return Err(TranslateError::Failed(
                "slin data length must be even".into(),
            ));
        }
        let mut i = 0;
        while i + 1 < data.len() {
            let sample = i16::from_le_bytes([data[i], data[i + 1]]);
            self.output_buf.push(linear_to_alaw_fast(sample));
            self.samples += 1;
            i += 2;
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
        Some(Frame::voice(ID_ALAW, samples, data))
    }
}

// ---------------------------------------------------------------------------
// G.722 <-> Signed Linear (stub - complex codec)
// ---------------------------------------------------------------------------

/// Translator descriptor: g722 -> slin16.
/// Stub - G.722 requires a complex state machine for sub-band ADPCM.
pub struct G722ToSlin16Translator;

impl Translator for G722ToSlin16Translator {
    fn name(&self) -> &str { "g722tolin16" }
    fn src_codec_id(&self) -> CodecId { ID_G722 }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN16 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(G722ToSlin16Instance)
    }
}

struct G722ToSlin16Instance;

impl TranslatorInstance for G722ToSlin16Instance {
    fn frame_in(&mut self, _frame: &Frame) -> Result<(), TranslateError> {
        // TODO: Implement G.722 sub-band ADPCM decoding
        Err(TranslateError::Failed("G.722 decoding not yet implemented".into()))
    }
    fn frame_out(&mut self) -> Option<Frame> { None }
}

/// Translator descriptor: slin16 -> g722.
pub struct Slin16ToG722Translator;

impl Translator for Slin16ToG722Translator {
    fn name(&self) -> &str { "lin16tog722" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN16 }
    fn dst_codec_id(&self) -> CodecId { ID_G722 }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(Slin16ToG722Instance)
    }
}

struct Slin16ToG722Instance;

impl TranslatorInstance for Slin16ToG722Instance {
    fn frame_in(&mut self, _frame: &Frame) -> Result<(), TranslateError> {
        // TODO: Implement G.722 sub-band ADPCM encoding
        Err(TranslateError::Failed("G.722 encoding not yet implemented".into()))
    }
    fn frame_out(&mut self) -> Option<Frame> { None }
}

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Register all built-in translators into a TranslationMatrix.
pub fn register_builtin_translators(matrix: &mut crate::translate::TranslationMatrix) {
    matrix.register(Arc::new(UlawToSlinTranslator));
    matrix.register(Arc::new(SlinToUlawTranslator));
    matrix.register(Arc::new(AlawToSlinTranslator));
    matrix.register(Arc::new(SlinToAlawTranslator));
    matrix.register(Arc::new(G722ToSlin16Translator));
    matrix.register(Arc::new(Slin16ToG722Translator));

    // ADPCM (real encode/decode)
    matrix.register(Arc::new(crate::adpcm::AdpcmToSlinTranslator));
    matrix.register(Arc::new(crate::adpcm::SlinToAdpcmTranslator));

    // LPC-10 (stub)
    matrix.register(Arc::new(crate::lpc10::Lpc10ToSlinTranslator));
    matrix.register(Arc::new(crate::lpc10::SlinToLpc10Translator));

    // Codec2 (stub)
    matrix.register(Arc::new(crate::codec2::Codec2ToSlinTranslator));
    matrix.register(Arc::new(crate::codec2::SlinToCodec2Translator));

    // Resamplers (all SLIN rate combinations)
    for translator in crate::resample::all_resample_translators() {
        matrix.register(translator);
    }
}
