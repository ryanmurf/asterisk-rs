//! Speex codec integration layer.
//!
//! Port of codecs/codec_speex.c from Asterisk C.
//!
//! Speex supports three modes:
//! - Narrowband (8kHz)
//! - Wideband (16kHz)
//! - Ultra-wideband (32kHz)
//!
//! This module provides the configuration, trait implementations, and
//! translator registrations. The actual DSP (encode/decode) is stubbed
//! since it requires the external libspeex library.

use crate::builtin_codecs::{ID_SLIN8, ID_SLIN16, ID_SLIN32, ID_SPEEX8, ID_SPEEX16, ID_SPEEX32};
use crate::codec::CodecId;
use crate::translate::{TransCost, TranslateError, Translator, TranslatorInstance};
use asterisk_types::Frame;

/// Speex encoder configuration.
///
/// Maps to the configuration options available in codecs.conf for Speex.
#[derive(Debug, Clone)]
pub struct SpeexEncoderConfig {
    /// Encoding quality (0-10). Higher = better quality, more CPU.
    pub quality: u32,
    /// Encoding complexity (1-10). Higher = more CPU for marginal improvement.
    pub complexity: u32,
    /// Variable bitrate encoding.
    pub vbr: bool,
    /// Voice Activity Detection.
    pub vad: bool,
    /// Discontinuous Transmission (silence suppression).
    pub dtx: bool,
    /// Average Bit Rate target (bits/sec, 0 = disabled).
    pub abr: u32,
    /// Enable perceptual enhancement.
    pub enhancement: bool,
    /// Preprocessing: Automatic Gain Control.
    pub preprocess_agc: bool,
    /// AGC level target.
    pub agc_level: f32,
    /// Preprocessing: noise suppression.
    pub preprocess_denoise: bool,
    /// Noise suppression level in dB (negative value).
    pub noise_suppress: i32,
    /// Maximum bitrate for VBR (bits/sec).
    pub vbr_max_bitrate: u32,
}

impl Default for SpeexEncoderConfig {
    fn default() -> Self {
        Self {
            quality: 3,
            complexity: 2,
            vbr: false,
            vad: false,
            dtx: false,
            abr: 0,
            enhancement: true,
            preprocess_agc: false,
            agc_level: 8000.0,
            preprocess_denoise: false,
            noise_suppress: -30,
            vbr_max_bitrate: 0,
        }
    }
}

/// Speex decoder configuration.
#[derive(Debug, Clone)]
pub struct SpeexDecoderConfig {
    /// Enable perceptual enhancement during decode.
    pub enhancement: bool,
}

impl Default for SpeexDecoderConfig {
    fn default() -> Self {
        Self {
            enhancement: true,
        }
    }
}

/// Speex mode (sample rate variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeexMode {
    /// Narrowband: 8000 Hz
    Narrowband,
    /// Wideband: 16000 Hz
    Wideband,
    /// Ultra-wideband: 32000 Hz
    UltraWideband,
}

impl SpeexMode {
    /// Sample rate for this mode.
    pub fn sample_rate(&self) -> u32 {
        match self {
            SpeexMode::Narrowband => 8000,
            SpeexMode::Wideband => 16000,
            SpeexMode::UltraWideband => 32000,
        }
    }

    /// Frame size in samples (20ms frame).
    pub fn frame_size(&self) -> usize {
        match self {
            SpeexMode::Narrowband => 160,     // 8000 * 0.02
            SpeexMode::Wideband => 320,       // 16000 * 0.02
            SpeexMode::UltraWideband => 640,  // 32000 * 0.02
        }
    }

    /// Codec ID for this mode.
    pub fn codec_id(&self) -> CodecId {
        match self {
            SpeexMode::Narrowband => ID_SPEEX8,
            SpeexMode::Wideband => ID_SPEEX16,
            SpeexMode::UltraWideband => ID_SPEEX32,
        }
    }

    /// Signed linear codec ID for this mode.
    pub fn slin_id(&self) -> CodecId {
        match self {
            SpeexMode::Narrowband => ID_SLIN8,
            SpeexMode::Wideband => ID_SLIN16,
            SpeexMode::UltraWideband => ID_SLIN32,
        }
    }
}

/// Speex encoder (stub).
///
/// In a real implementation, this would hold a `SpeexBits` structure
/// and the encoder state from libspeex.
pub struct SpeexEncoder {
    pub config: SpeexEncoderConfig,
    pub mode: SpeexMode,
}

impl SpeexEncoder {
    pub fn new(mode: SpeexMode) -> Self {
        Self {
            config: SpeexEncoderConfig::default(),
            mode,
        }
    }

    pub fn with_config(mode: SpeexMode, config: SpeexEncoderConfig) -> Self {
        Self { config, mode }
    }

    /// Encode PCM samples to Speex data.
    ///
    /// STUB: Returns an empty frame. Real implementation requires libspeex.
    pub fn encode(&mut self, _samples: &[i16]) -> Result<Vec<u8>, TranslateError> {
        Err(TranslateError::Failed(
            "Speex encoding requires libspeex (not linked)".into(),
        ))
    }
}

/// Speex decoder (stub).
pub struct SpeexDecoder {
    pub config: SpeexDecoderConfig,
    pub mode: SpeexMode,
}

impl SpeexDecoder {
    pub fn new(mode: SpeexMode) -> Self {
        Self {
            config: SpeexDecoderConfig::default(),
            mode,
        }
    }

    /// Decode Speex data to PCM samples.
    ///
    /// STUB: Returns error. Real implementation requires libspeex.
    pub fn decode(&mut self, _data: &[u8]) -> Result<Vec<i16>, TranslateError> {
        Err(TranslateError::Failed(
            "Speex decoding requires libspeex (not linked)".into(),
        ))
    }
}

/// Speex preprocessor configuration.
///
/// Mirrors the speex preprocessor from libspeex.
#[derive(Debug, Clone)]
pub struct SpeexPreprocessor {
    /// Noise suppression level in dB.
    pub noise_suppress: i32,
    /// Enable AGC.
    pub agc: bool,
    /// AGC target level.
    pub agc_level: f32,
    /// Enable voice activity detection.
    pub vad: bool,
    /// Enable noise suppression.
    pub denoise: bool,
    /// Enable de-reverberation.
    pub dereverb: bool,
    /// De-reverberation level.
    pub dereverb_level: f32,
    /// De-reverberation decay factor.
    pub dereverb_decay: f32,
}

impl Default for SpeexPreprocessor {
    fn default() -> Self {
        Self {
            noise_suppress: -30,
            agc: false,
            agc_level: 8000.0,
            vad: false,
            denoise: true,
            dereverb: false,
            dereverb_level: 0.4,
            dereverb_decay: 0.3,
        }
    }
}

// ---------------------------------------------------------------------------
// Translator implementations
// ---------------------------------------------------------------------------

/// Generic Speex-to-Slin translator.
pub struct SpeexToSlinTranslator {
    mode: SpeexMode,
}

impl SpeexToSlinTranslator {
    pub fn narrowband() -> Self {
        Self { mode: SpeexMode::Narrowband }
    }
    pub fn wideband() -> Self {
        Self { mode: SpeexMode::Wideband }
    }
    pub fn ultra_wideband() -> Self {
        Self { mode: SpeexMode::UltraWideband }
    }
}

impl Translator for SpeexToSlinTranslator {
    fn name(&self) -> &str {
        match self.mode {
            SpeexMode::Narrowband => "speextolin",
            SpeexMode::Wideband => "speex16tolin16",
            SpeexMode::UltraWideband => "speex32tolin32",
        }
    }
    fn src_codec_id(&self) -> CodecId { self.mode.codec_id() }
    fn dst_codec_id(&self) -> CodecId { self.mode.slin_id() }
    fn table_cost(&self) -> u32 { TransCost::LY_LL_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SpeexToSlinInstance {
            decoder: SpeexDecoder::new(self.mode),
        })
    }
}

struct SpeexToSlinInstance {
    decoder: SpeexDecoder,
}

impl TranslatorInstance for SpeexToSlinInstance {
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError> {
        let data = match frame {
            Frame::Voice { data, .. } => data,
            _ => return Err(TranslateError::Failed("expected voice frame".into())),
        };
        // Attempt decode (will fail with stub message)
        let _samples = self.decoder.decode(data)?;
        Ok(())
    }

    fn frame_out(&mut self) -> Option<Frame> {
        None
    }
}

/// Generic Slin-to-Speex translator.
pub struct SlinToSpeexTranslator {
    mode: SpeexMode,
}

impl SlinToSpeexTranslator {
    pub fn narrowband() -> Self {
        Self { mode: SpeexMode::Narrowband }
    }
    pub fn wideband() -> Self {
        Self { mode: SpeexMode::Wideband }
    }
    pub fn ultra_wideband() -> Self {
        Self { mode: SpeexMode::UltraWideband }
    }
}

impl Translator for SlinToSpeexTranslator {
    fn name(&self) -> &str {
        match self.mode {
            SpeexMode::Narrowband => "lintospeex",
            SpeexMode::Wideband => "lin16tospeex16",
            SpeexMode::UltraWideband => "lin32tospeex32",
        }
    }
    fn src_codec_id(&self) -> CodecId { self.mode.slin_id() }
    fn dst_codec_id(&self) -> CodecId { self.mode.codec_id() }
    fn table_cost(&self) -> u32 { TransCost::LL_LY_ORIGSAMP }
    fn new_instance(&self) -> Box<dyn TranslatorInstance> {
        Box::new(SlinToSpeexInstance {
            encoder: SpeexEncoder::new(self.mode),
        })
    }
}

struct SlinToSpeexInstance {
    encoder: SpeexEncoder,
}

impl TranslatorInstance for SlinToSpeexInstance {
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
    fn test_speex_modes() {
        assert_eq!(SpeexMode::Narrowband.sample_rate(), 8000);
        assert_eq!(SpeexMode::Wideband.sample_rate(), 16000);
        assert_eq!(SpeexMode::UltraWideband.sample_rate(), 32000);
        assert_eq!(SpeexMode::Narrowband.frame_size(), 160);
        assert_eq!(SpeexMode::Wideband.frame_size(), 320);
        assert_eq!(SpeexMode::UltraWideband.frame_size(), 640);
    }

    #[test]
    fn test_encoder_config_defaults() {
        let config = SpeexEncoderConfig::default();
        assert_eq!(config.quality, 3);
        assert_eq!(config.complexity, 2);
        assert!(!config.vbr);
        assert!(!config.vad);
        assert!(!config.dtx);
    }

    #[test]
    fn test_preprocessor_defaults() {
        let pp = SpeexPreprocessor::default();
        assert_eq!(pp.noise_suppress, -30);
        assert!(pp.denoise);
        assert!(!pp.agc);
        assert!(!pp.vad);
    }

    #[test]
    fn test_encode_returns_stub_error() {
        let mut enc = SpeexEncoder::new(SpeexMode::Narrowband);
        let samples = vec![0i16; 160];
        assert!(enc.encode(&samples).is_err());
    }

    #[test]
    fn test_decode_returns_stub_error() {
        let mut dec = SpeexDecoder::new(SpeexMode::Narrowband);
        let data = vec![0u8; 20];
        assert!(dec.decode(&data).is_err());
    }
}
