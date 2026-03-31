//! Asterisk Codecs Crate
//!
//! Provides codec definitions, format capabilities, and the translation framework
//! for converting between audio/video formats. This is a port of Asterisk's
//! codec.h, format.h, format_cap.h, translate.h, and codec_builtin.c.

pub mod codec;
pub mod format;
pub mod format_cap;
pub mod translate;
pub mod builtin_codecs;
pub mod builtin_translators;
pub mod registry;
pub mod ulaw_table;
pub mod alaw_table;
#[allow(dead_code)]
pub mod g722;
#[allow(dead_code)]
pub mod g726;
pub mod speex;
pub mod ilbc;
pub mod opus;
pub mod lpc10;
pub mod adpcm;
pub mod codec2;
pub mod resample;
pub mod echo_cancel;
pub mod noise_suppress;
pub mod agc;
pub mod plc;
pub mod dtmf_detect;
pub mod tone_gen;

// Feature-gated FFI codec bridges
pub mod opus_ffi;
pub mod gsm_ffi;
pub mod speex_ffi;

// Re-export primary types
pub use codec::{Codec as InternalCodec, CodecId};
pub use format::{Format, FormatCmp};
pub use format_cap::FormatCap;
pub use translate::{Translator, TranslatorInstance, TranslationMatrix, TranslationPath};
pub use registry::CodecRegistry;

// ---------------------------------------------------------------------------
// Backward-compatible simple Codec / FormatCapabilities API
// This was defined by the SIP agent and other crates depend on it.
// We keep it here as a lightweight SDP-level codec descriptor separate from
// the full internal Codec used by the translation engine.
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

/// A lightweight codec identifier with RTP parameters, used primarily for SDP
/// negotiation and channel capability advertisement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Codec {
    /// RTP payload type number
    pub payload_type: u8,
    /// Codec name (e.g., "PCMU", "PCMA", "opus")
    pub name: String,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u8,
}

impl Codec {
    pub fn new(name: &str, payload_type: u8, sample_rate: u32) -> Self {
        Self {
            payload_type,
            name: name.to_string(),
            sample_rate,
            channels: 1,
        }
    }
}

/// Well-known codec definitions for SDP / RTP usage.
pub mod codecs {
    use super::Codec;

    pub fn pcmu() -> Codec { Codec::new("PCMU", 0, 8000) }
    pub fn pcma() -> Codec { Codec::new("PCMA", 8, 8000) }
    pub fn g722() -> Codec { Codec::new("G722", 9, 8000) }
    pub fn g729() -> Codec { Codec::new("G729", 18, 8000) }
    pub fn slin() -> Codec { Codec::new("L16", 10, 8000) }
    pub fn slin16() -> Codec { Codec::new("L16", 96, 16000) }
    pub fn opus() -> Codec {
        Codec { payload_type: 111, name: "opus".to_string(), sample_rate: 48000, channels: 2 }
    }
    pub fn telephone_event() -> Codec { Codec::new("telephone-event", 101, 8000) }

    // Video codecs
    pub fn h264() -> Codec { Codec::new("H264", 99, 90000) }
    pub fn vp8() -> Codec { Codec::new("VP8", 96, 90000) }
    pub fn vp9() -> Codec { Codec::new("VP9", 98, 90000) }
    pub fn h265() -> Codec { Codec::new("H265", 100, 90000) }
}

/// Format capabilities - a set of codecs a channel supports.
#[derive(Debug, Clone, Default)]
pub struct FormatCapabilities {
    pub codecs: Vec<Codec>,
}

impl FormatCapabilities {
    pub fn new() -> Self { Self { codecs: Vec::new() } }

    pub fn add(&mut self, codec: Codec) {
        if !self.codecs.contains(&codec) {
            self.codecs.push(codec);
        }
    }

    pub fn contains(&self, codec: &Codec) -> bool {
        self.codecs.contains(codec)
    }

    pub fn is_empty(&self) -> bool {
        self.codecs.is_empty()
    }

    /// Find common codecs between two capability sets.
    pub fn intersect(&self, other: &FormatCapabilities) -> FormatCapabilities {
        let mut result = FormatCapabilities::new();
        for c in &self.codecs {
            if other.codecs.iter().any(|o| o.name == c.name && o.sample_rate == c.sample_rate) {
                result.add(c.clone());
            }
        }
        result
    }
}
