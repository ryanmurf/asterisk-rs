//! Built-in codec definitions ported from asterisk/main/codec_builtin.c.
//!
//! All codec parameters (sample rates, frame sizes, quality values) are taken
//! directly from the original Asterisk C source.

use crate::codec::Codec;
use asterisk_types::MediaType;

// Codec IDs - assigned sequentially starting at 1 like Asterisk
pub const ID_NONE: u32    = 1;
pub const ID_ULAW: u32   = 2;
pub const ID_ALAW: u32   = 3;
pub const ID_GSM: u32    = 4;
pub const ID_G726: u32   = 5;
pub const ID_G726AAL2: u32 = 6;
pub const ID_ADPCM: u32  = 7;
pub const ID_SLIN8: u32  = 8;
pub const ID_SLIN12: u32 = 9;
pub const ID_SLIN16: u32 = 10;
pub const ID_SLIN24: u32 = 11;
pub const ID_SLIN32: u32 = 12;
pub const ID_SLIN44: u32 = 13;
pub const ID_SLIN48: u32 = 14;
pub const ID_SLIN96: u32 = 15;
pub const ID_SLIN192: u32 = 16;
pub const ID_LPC10: u32  = 17;
pub const ID_G729: u32   = 18;
pub const ID_SPEEX8: u32 = 19;
pub const ID_SPEEX16: u32 = 20;
pub const ID_SPEEX32: u32 = 21;
pub const ID_ILBC: u32   = 22;
pub const ID_G722: u32   = 23;
pub const ID_OPUS: u32   = 24;
pub const ID_G723: u32   = 25;
pub const ID_CODEC2: u32 = 26;
pub const ID_SIREN7: u32 = 27;
pub const ID_SIREN14: u32 = 28;
// Video codecs
pub const ID_H261: u32   = 30;
pub const ID_H263: u32   = 31;
pub const ID_H263P: u32  = 32;
pub const ID_H264: u32   = 33;
pub const ID_H265: u32   = 34;
pub const ID_VP8: u32    = 35;
pub const ID_VP9: u32    = 36;
pub const ID_MPEG4: u32  = 37;
// Image / text
pub const ID_JPEG: u32   = 40;
pub const ID_PNG: u32    = 41;
pub const ID_T140: u32   = 42;
pub const ID_T140RED: u32 = 43;
pub const ID_T38: u32    = 44;

pub static CODEC_NONE: Codec = Codec {
    id: ID_NONE,
    name: "none",
    description: "<Null> codec",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 140,
    default_ms: 20,
    minimum_bytes: 20,
    smooth: false,
    quality: 0,
};

pub static CODEC_ULAW: Codec = Codec {
    id: ID_ULAW,
    name: "ulaw",
    description: "G.711 u-law",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 140,
    default_ms: 20,
    minimum_bytes: 80,
    smooth: true,
    quality: 100,
};

pub static CODEC_ALAW: Codec = Codec {
    id: ID_ALAW,
    name: "alaw",
    description: "G.711 a-law",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 140,
    default_ms: 20,
    minimum_bytes: 80,
    smooth: true,
    quality: 100,
};

pub static CODEC_GSM: Codec = Codec {
    id: ID_GSM,
    name: "gsm",
    description: "GSM",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 20,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 33,
    smooth: true,
    quality: 40,
};

pub static CODEC_G726: Codec = Codec {
    id: ID_G726,
    name: "g726",
    description: "G.726 RFC3551",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 40,
    smooth: true,
    quality: 85,
};

pub static CODEC_G726AAL2: Codec = Codec {
    id: ID_G726AAL2,
    name: "g726aal2",
    description: "G.726 AAL2",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 40,
    smooth: true,
    quality: 85,
};

pub static CODEC_ADPCM: Codec = Codec {
    id: ID_ADPCM,
    name: "adpcm",
    description: "Dialogic ADPCM",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 40,
    smooth: true,
    quality: 80,
};

pub static CODEC_SLIN8: Codec = Codec {
    id: ID_SLIN8,
    name: "slin",
    description: "16 bit Signed Linear PCM",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 160,
    smooth: true,
    quality: 115,
};

pub static CODEC_SLIN12: Codec = Codec {
    id: ID_SLIN12,
    name: "slin",
    description: "16 bit Signed Linear PCM (12kHz)",
    media_type: MediaType::Audio,
    sample_rate: 12000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 240,
    smooth: true,
    quality: 116,
};

pub static CODEC_SLIN16: Codec = Codec {
    id: ID_SLIN16,
    name: "slin",
    description: "16 bit Signed Linear PCM (16kHz)",
    media_type: MediaType::Audio,
    sample_rate: 16000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 320,
    smooth: true,
    quality: 117,
};

pub static CODEC_SLIN24: Codec = Codec {
    id: ID_SLIN24,
    name: "slin",
    description: "16 bit Signed Linear PCM (24kHz)",
    media_type: MediaType::Audio,
    sample_rate: 24000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 480,
    smooth: true,
    quality: 118,
};

pub static CODEC_SLIN32: Codec = Codec {
    id: ID_SLIN32,
    name: "slin",
    description: "16 bit Signed Linear PCM (32kHz)",
    media_type: MediaType::Audio,
    sample_rate: 32000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 640,
    smooth: true,
    quality: 119,
};

pub static CODEC_SLIN44: Codec = Codec {
    id: ID_SLIN44,
    name: "slin",
    description: "16 bit Signed Linear PCM (44kHz)",
    media_type: MediaType::Audio,
    sample_rate: 44100,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 882,
    smooth: true,
    quality: 120,
};

pub static CODEC_SLIN48: Codec = Codec {
    id: ID_SLIN48,
    name: "slin",
    description: "16 bit Signed Linear PCM (48kHz)",
    media_type: MediaType::Audio,
    sample_rate: 48000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 960,
    smooth: true,
    quality: 121,
};

pub static CODEC_SLIN96: Codec = Codec {
    id: ID_SLIN96,
    name: "slin",
    description: "16 bit Signed Linear PCM (96kHz)",
    media_type: MediaType::Audio,
    sample_rate: 96000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 1920,
    smooth: true,
    quality: 122,
};

pub static CODEC_SLIN192: Codec = Codec {
    id: ID_SLIN192,
    name: "slin",
    description: "16 bit Signed Linear PCM (192kHz)",
    media_type: MediaType::Audio,
    sample_rate: 192000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 3840,
    smooth: true,
    quality: 123,
};

pub static CODEC_LPC10: Codec = Codec {
    id: ID_LPC10,
    name: "lpc10",
    description: "LPC10",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 20,
    maximum_ms: 20,
    default_ms: 20,
    minimum_bytes: 7,
    smooth: true,
    quality: 8,
};

pub static CODEC_G729: Codec = Codec {
    id: ID_G729,
    name: "g729",
    description: "G.729A",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 220,
    default_ms: 20,
    minimum_bytes: 10,
    smooth: true,
    quality: 45,
};

pub static CODEC_SPEEX8: Codec = Codec {
    id: ID_SPEEX8,
    name: "speex",
    description: "SpeeX",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 10,
    smooth: false,
    quality: 40,
};

pub static CODEC_SPEEX16: Codec = Codec {
    id: ID_SPEEX16,
    name: "speex",
    description: "SpeeX 16khz",
    media_type: MediaType::Audio,
    sample_rate: 16000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 10,
    smooth: false,
    quality: 40,
};

pub static CODEC_SPEEX32: Codec = Codec {
    id: ID_SPEEX32,
    name: "speex",
    description: "SpeeX 32khz",
    media_type: MediaType::Audio,
    sample_rate: 32000,
    minimum_ms: 10,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 10,
    smooth: false,
    quality: 40,
};

pub static CODEC_ILBC: Codec = Codec {
    id: ID_ILBC,
    name: "ilbc",
    description: "iLBC",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 20,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 38,
    smooth: false,
    quality: 35,
};

pub static CODEC_G722: Codec = Codec {
    id: ID_G722,
    name: "g722",
    description: "G722",
    media_type: MediaType::Audio,
    sample_rate: 16000,
    minimum_ms: 10,
    maximum_ms: 140,
    default_ms: 20,
    minimum_bytes: 80,
    smooth: true,
    quality: 110,
};

pub static CODEC_OPUS: Codec = Codec {
    id: ID_OPUS,
    name: "opus",
    description: "Opus Codec",
    media_type: MediaType::Audio,
    sample_rate: 48000,
    minimum_ms: 20,
    maximum_ms: 60,
    default_ms: 20,
    minimum_bytes: 10,
    smooth: false,
    quality: 75,
};

pub static CODEC_G723: Codec = Codec {
    id: ID_G723,
    name: "g723",
    description: "G.723.1",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 30,
    maximum_ms: 300,
    default_ms: 30,
    minimum_bytes: 20,
    smooth: false,
    quality: 20,
};

pub static CODEC_CODEC2: Codec = Codec {
    id: ID_CODEC2,
    name: "codec2",
    description: "Codec 2",
    media_type: MediaType::Audio,
    sample_rate: 8000,
    minimum_ms: 20,
    maximum_ms: 300,
    default_ms: 20,
    minimum_bytes: 6,
    smooth: true,
    quality: 0,
};

pub static CODEC_SIREN7: Codec = Codec {
    id: ID_SIREN7,
    name: "siren7",
    description: "ITU G.722.1 (Siren7, licensed from Polycom)",
    media_type: MediaType::Audio,
    sample_rate: 16000,
    minimum_ms: 20,
    maximum_ms: 80,
    default_ms: 20,
    minimum_bytes: 80,
    smooth: false,
    quality: 85,
};

pub static CODEC_SIREN14: Codec = Codec {
    id: ID_SIREN14,
    name: "siren14",
    description: "ITU G.722.1 Annex C (Siren14, licensed from Polycom)",
    media_type: MediaType::Audio,
    sample_rate: 32000,
    minimum_ms: 20,
    maximum_ms: 80,
    default_ms: 20,
    minimum_bytes: 120,
    smooth: false,
    quality: 90,
};

// Video codecs

pub static CODEC_H261: Codec = Codec {
    id: ID_H261,
    name: "h261",
    description: "H.261 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_H263: Codec = Codec {
    id: ID_H263,
    name: "h263",
    description: "H.263 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_H263P: Codec = Codec {
    id: ID_H263P,
    name: "h263p",
    description: "H.263+ video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_H264: Codec = Codec {
    id: ID_H264,
    name: "h264",
    description: "H.264 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_H265: Codec = Codec {
    id: ID_H265,
    name: "h265",
    description: "H.265 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_VP8: Codec = Codec {
    id: ID_VP8,
    name: "vp8",
    description: "VP8 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_VP9: Codec = Codec {
    id: ID_VP9,
    name: "vp9",
    description: "VP9 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_MPEG4: Codec = Codec {
    id: ID_MPEG4,
    name: "mpeg4",
    description: "MPEG4 video",
    media_type: MediaType::Video,
    sample_rate: 1000,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

// Image / text codecs

pub static CODEC_JPEG: Codec = Codec {
    id: ID_JPEG,
    name: "jpeg",
    description: "JPEG image",
    media_type: MediaType::Image,
    sample_rate: 0,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_PNG: Codec = Codec {
    id: ID_PNG,
    name: "png",
    description: "PNG Image",
    media_type: MediaType::Image,
    sample_rate: 0,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_T140: Codec = Codec {
    id: ID_T140,
    name: "t140",
    description: "Passthrough T.140 Realtime Text",
    media_type: MediaType::Text,
    sample_rate: 0,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_T140RED: Codec = Codec {
    id: ID_T140RED,
    name: "red",
    description: "T.140 Realtime Text with redundancy",
    media_type: MediaType::Text,
    sample_rate: 0,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

pub static CODEC_T38: Codec = Codec {
    id: ID_T38,
    name: "t38",
    description: "T.38 UDPTL Fax",
    media_type: MediaType::Image,
    sample_rate: 0,
    minimum_ms: 0,
    maximum_ms: 0,
    default_ms: 0,
    minimum_bytes: 0,
    smooth: false,
    quality: 0,
};

/// Return a list of all built-in codecs.
pub fn all_builtin_codecs() -> Vec<&'static Codec> {
    vec![
        &CODEC_NONE,
        &CODEC_ULAW, &CODEC_ALAW, &CODEC_GSM,
        &CODEC_G726, &CODEC_G726AAL2, &CODEC_ADPCM,
        &CODEC_SLIN8, &CODEC_SLIN12, &CODEC_SLIN16,
        &CODEC_SLIN24, &CODEC_SLIN32, &CODEC_SLIN44,
        &CODEC_SLIN48, &CODEC_SLIN96, &CODEC_SLIN192,
        &CODEC_LPC10, &CODEC_G729,
        &CODEC_SPEEX8, &CODEC_SPEEX16, &CODEC_SPEEX32,
        &CODEC_ILBC, &CODEC_G722, &CODEC_OPUS,
        &CODEC_G723, &CODEC_CODEC2,
        &CODEC_SIREN7, &CODEC_SIREN14,
        &CODEC_H261, &CODEC_H263, &CODEC_H263P,
        &CODEC_H264, &CODEC_H265,
        &CODEC_VP8, &CODEC_VP9, &CODEC_MPEG4,
        &CODEC_JPEG, &CODEC_PNG,
        &CODEC_T140, &CODEC_T140RED, &CODEC_T38,
    ]
}
