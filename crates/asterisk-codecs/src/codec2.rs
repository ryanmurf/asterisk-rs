//! Codec2 - open source low bitrate speech codec.
//!
//! Port of codecs/codec_codec2.c from Asterisk C.
//!
//! Codec2 supports multiple modes with varying bitrates:
//! - 3200 bps (160 samples, 8 bytes/frame)
//! - 2400 bps (160 samples, 6 bytes/frame)
//! - 1600 bps (320 samples, 8 bytes/frame)
//! - 1400 bps (320 samples, 7 bytes/frame)
//! - 1200 bps (320 samples, 6 bytes/frame)
//! - 700  bps (320 samples, 4 bytes/frame) [700C mode]
//!
//! The default Asterisk mode is 2400 bps with 160 samples per frame
//! and 6 bytes per compressed frame.
//!
//! This module provides the configuration, trait implementations, and
//! translator registrations. The actual DSP is stubbed since it requires
//! the external libcodec2 library.
//!
//! References:
//! - http://www.rowetel.com/codec2.html

use crate::builtin_codecs::{ID_CODEC2, ID_SLIN8};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;

/// Default samples per Codec2 frame (at 8000 Hz, 2400bps mode).
pub const CODEC2_SAMPLES: u32 = 160;

/// Default compressed frame length in bytes (2400bps mode).
pub const CODEC2_FRAME_LEN: usize = 6;

/// Codec2 operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum Codec2Mode {
    /// 3200 bps: 160 samples, 8 bytes
    Mode3200,
    /// 2400 bps: 160 samples, 6 bytes (default)
    #[default]
    Mode2400,
    /// 1600 bps: 320 samples, 8 bytes
    Mode1600,
    /// 1400 bps: 320 samples, 7 bytes
    Mode1400,
    /// 1200 bps: 320 samples, 6 bytes
    Mode1200,
    /// 700C bps: 320 samples, 4 bytes
    Mode700C,
}

impl Codec2Mode {
    /// Bitrate in bits per second.
    pub fn bitrate(&self) -> u32 {
        match self {
            Codec2Mode::Mode3200 => 3200,
            Codec2Mode::Mode2400 => 2400,
            Codec2Mode::Mode1600 => 1600,
            Codec2Mode::Mode1400 => 1400,
            Codec2Mode::Mode1200 => 1200,
            Codec2Mode::Mode700C => 700,
        }
    }

    /// Samples per frame.
    pub fn samples_per_frame(&self) -> u32 {
        match self {
            Codec2Mode::Mode3200 | Codec2Mode::Mode2400 => 160,
            _ => 320,
        }
    }

    /// Bytes per compressed frame.
    pub fn bytes_per_frame(&self) -> usize {
        match self {
            Codec2Mode::Mode3200 => 8,
            Codec2Mode::Mode2400 => 6,
            Codec2Mode::Mode1600 => 8,
            Codec2Mode::Mode1400 => 7,
            Codec2Mode::Mode1200 => 6,
            Codec2Mode::Mode700C => 4,
        }
    }
}


/// Codec2 encoder (stub).
///
/// Real implementation requires libcodec2.
pub struct Codec2Encoder {
    pub mode: Codec2Mode,
}

impl Codec2Encoder {
    pub fn new(mode: Codec2Mode) -> Self {
        Self { mode }
    }

    /// Encode PCM samples to Codec2 data.
    ///
    /// STUB: Real implementation requires libcodec2.
    pub fn encode(&mut self, _samples: &[i16]) -> Result<Vec<u8>, TranslateError> {
        Err(TranslateError::Failed(
            "Codec2 encoding requires libcodec2 (not linked)".into(),
        ))
    }
}

/// Codec2 decoder (stub).
pub struct Codec2Decoder {
    pub mode: Codec2Mode,
}

impl Codec2Decoder {
    pub fn new(mode: Codec2Mode) -> Self {
        Self { mode }
    }

    /// Decode Codec2 data to PCM samples.
    ///
    /// STUB: Real implementation requires libcodec2.
    pub fn decode(&mut self, _data: &[u8]) -> Result<Vec<i16>, TranslateError> {
        Err(TranslateError::Failed(
            "Codec2 decoding requires libcodec2 (not linked)".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Translator: Codec2 -> Signed Linear 8kHz.
pub struct Codec2ToSlinTranslator;

impl Translator for Codec2ToSlinTranslator {
    fn name(&self) -> &str { "codec2tolin" }
    fn src_codec_id(&self) -> CodecId { ID_CODEC2 }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(Codec2ToSlinInstance {
            decoder: Codec2Decoder::new(Codec2Mode::default()),
        })
    }
}

struct Codec2ToSlinInstance {
    decoder: Codec2Decoder,
}

impl TranslatorInstance for Codec2ToSlinInstance {
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

/// Translator: Signed Linear 8kHz -> Codec2.
pub struct SlinToCodec2Translator;

impl Translator for SlinToCodec2Translator {
    fn name(&self) -> &str { "lintocodec2" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_CODEC2 }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToCodec2Instance {
            encoder: Codec2Encoder::new(Codec2Mode::default()),
        })
    }
}

struct SlinToCodec2Instance {
    encoder: Codec2Encoder,
}

impl TranslatorInstance for SlinToCodec2Instance {
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
    fn test_codec2_modes() {
        let m = Codec2Mode::Mode2400;
        assert_eq!(m.bitrate(), 2400);
        assert_eq!(m.samples_per_frame(), 160);
        assert_eq!(m.bytes_per_frame(), 6);

        let m = Codec2Mode::Mode700C;
        assert_eq!(m.bitrate(), 700);
        assert_eq!(m.samples_per_frame(), 320);
        assert_eq!(m.bytes_per_frame(), 4);
    }

    #[test]
    fn test_codec2_default_mode() {
        assert_eq!(Codec2Mode::default(), Codec2Mode::Mode2400);
    }

    #[test]
    fn test_encoder_stub() {
        let mut enc = Codec2Encoder::new(Codec2Mode::Mode2400);
        let samples = vec![0i16; 160];
        assert!(enc.encode(&samples).is_err());
    }

    #[test]
    fn test_decoder_stub() {
        let mut dec = Codec2Decoder::new(Codec2Mode::Mode2400);
        let data = vec![0u8; 6];
        assert!(dec.decode(&data).is_err());
    }
}
