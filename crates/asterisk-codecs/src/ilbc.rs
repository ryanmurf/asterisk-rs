//! iLBC (Internet Low Bitrate Codec) integration layer.
//!
//! Port of codecs/codec_ilbc.c from Asterisk C.
//!
//! iLBC supports two modes:
//! - 20ms frame: 38 bytes, 15.20 kbps
//! - 30ms frame: 50 bytes, 13.33 kbps
//!
//! This module provides the configuration, trait implementations, and
//! translator registrations. The actual DSP is stubbed since it requires
//! the external libilbc library.
//!
//! References:
//! - RFC 3951: Internet Low Bit Rate Codec (iLBC)
//! - RFC 3952: RTP Payload Format for iLBC Speech

use crate::builtin_codecs::{ID_ILBC, ID_SLIN8};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;

/// iLBC frame mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IlbcMode {
    /// 20ms mode: 38 bytes per frame, 15.20 kbps
    Mode20ms,
    /// 30ms mode: 50 bytes per frame, 13.33 kbps
    Mode30ms,
}

impl IlbcMode {
    /// Frame duration in milliseconds.
    pub fn frame_duration_ms(&self) -> u32 {
        match self {
            IlbcMode::Mode20ms => 20,
            IlbcMode::Mode30ms => 30,
        }
    }

    /// Frame size in bytes.
    pub fn frame_bytes(&self) -> usize {
        match self {
            IlbcMode::Mode20ms => 38,
            IlbcMode::Mode30ms => 50,
        }
    }

    /// Bitrate in bits per second.
    pub fn bitrate(&self) -> u32 {
        match self {
            IlbcMode::Mode20ms => 15200,
            IlbcMode::Mode30ms => 13330,
        }
    }

    /// Number of PCM samples per frame (at 8000 Hz).
    pub fn samples_per_frame(&self) -> u32 {
        match self {
            IlbcMode::Mode20ms => 160,  // 8000 * 0.020
            IlbcMode::Mode30ms => 240,  // 8000 * 0.030
        }
    }

    /// Number of sub-frames.
    pub fn num_sub_frames(&self) -> u32 {
        match self {
            IlbcMode::Mode20ms => 4,
            IlbcMode::Mode30ms => 6,
        }
    }

    /// Parse mode from SDP fmtp "mode" attribute.
    pub fn from_fmtp(mode_str: &str) -> Option<Self> {
        match mode_str.trim() {
            "20" => Some(IlbcMode::Mode20ms),
            "30" => Some(IlbcMode::Mode30ms),
            _ => None,
        }
    }
}

impl Default for IlbcMode {
    fn default() -> Self {
        IlbcMode::Mode30ms // Asterisk default
    }
}

/// iLBC encoder configuration.
#[derive(Debug, Clone)]
pub struct IlbcEncoderConfig {
    /// Frame mode (20ms or 30ms).
    pub mode: IlbcMode,
}

impl Default for IlbcEncoderConfig {
    fn default() -> Self {
        Self {
            mode: IlbcMode::default(),
        }
    }
}

/// iLBC decoder configuration.
#[derive(Debug, Clone)]
pub struct IlbcDecoderConfig {
    /// Frame mode (20ms or 30ms).
    pub mode: IlbcMode,
    /// Enable perceptual enhancement.
    pub enhancement: bool,
}

impl Default for IlbcDecoderConfig {
    fn default() -> Self {
        Self {
            mode: IlbcMode::default(),
            enhancement: true,
        }
    }
}

/// iLBC encoder (stub).
///
/// Real implementation requires libilbc.
pub struct IlbcEncoder {
    pub config: IlbcEncoderConfig,
}

impl IlbcEncoder {
    pub fn new(mode: IlbcMode) -> Self {
        Self {
            config: IlbcEncoderConfig { mode },
        }
    }

    /// Encode PCM samples to iLBC data.
    ///
    /// STUB: Real implementation requires libilbc.
    pub fn encode(&mut self, _samples: &[i16]) -> Result<Vec<u8>, TranslateError> {
        Err(TranslateError::Failed(
            "iLBC encoding requires libilbc (not linked)".into(),
        ))
    }
}

/// iLBC decoder (stub).
pub struct IlbcDecoder {
    pub config: IlbcDecoderConfig,
}

impl IlbcDecoder {
    pub fn new(mode: IlbcMode) -> Self {
        Self {
            config: IlbcDecoderConfig {
                mode,
                enhancement: true,
            },
        }
    }

    /// Decode iLBC data to PCM samples.
    ///
    /// STUB: Real implementation requires libilbc.
    pub fn decode(&mut self, _data: &[u8]) -> Result<Vec<i16>, TranslateError> {
        Err(TranslateError::Failed(
            "iLBC decoding requires libilbc (not linked)".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Translator: iLBC -> Signed Linear 8kHz.
pub struct IlbcToSlinTranslator;

impl Translator for IlbcToSlinTranslator {
    fn name(&self) -> &str { "ilbctolin" }
    fn src_codec_id(&self) -> CodecId { ID_ILBC }
    fn dst_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(IlbcToSlinInstance {
            decoder: IlbcDecoder::new(IlbcMode::default()),
        })
    }
}

struct IlbcToSlinInstance {
    decoder: IlbcDecoder,
}

impl TranslatorInstance for IlbcToSlinInstance {
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

/// Translator: Signed Linear 8kHz -> iLBC.
pub struct SlinToIlbcTranslator;

impl Translator for SlinToIlbcTranslator {
    fn name(&self) -> &str { "lintoilbc" }
    fn src_codec_id(&self) -> CodecId { ID_SLIN8 }
    fn dst_codec_id(&self) -> CodecId { ID_ILBC }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToIlbcInstance {
            encoder: IlbcEncoder::new(IlbcMode::default()),
        })
    }
}

struct SlinToIlbcInstance {
    encoder: IlbcEncoder,
}

impl TranslatorInstance for SlinToIlbcInstance {
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
    fn test_ilbc_modes() {
        let m20 = IlbcMode::Mode20ms;
        assert_eq!(m20.frame_duration_ms(), 20);
        assert_eq!(m20.frame_bytes(), 38);
        assert_eq!(m20.bitrate(), 15200);
        assert_eq!(m20.samples_per_frame(), 160);

        let m30 = IlbcMode::Mode30ms;
        assert_eq!(m30.frame_duration_ms(), 30);
        assert_eq!(m30.frame_bytes(), 50);
        assert_eq!(m30.bitrate(), 13330);
        assert_eq!(m30.samples_per_frame(), 240);
    }

    #[test]
    fn test_ilbc_fmtp_parse() {
        assert_eq!(IlbcMode::from_fmtp("20"), Some(IlbcMode::Mode20ms));
        assert_eq!(IlbcMode::from_fmtp("30"), Some(IlbcMode::Mode30ms));
        assert_eq!(IlbcMode::from_fmtp("25"), None);
    }

    #[test]
    fn test_encoder_stub() {
        let mut enc = IlbcEncoder::new(IlbcMode::Mode20ms);
        let samples = vec![0i16; 160];
        assert!(enc.encode(&samples).is_err());
    }

    #[test]
    fn test_decoder_stub() {
        let mut dec = IlbcDecoder::new(IlbcMode::Mode30ms);
        let data = vec![0u8; 50];
        assert!(dec.decode(&data).is_err());
    }
}
