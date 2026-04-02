//! Format attribute handlers for codec SDP fmtp negotiation.
//!
//! Port of `res/res_format_attr_opus.c`, `res_format_attr_silk.c`,
//! `res_format_attr_vp8.c`, `res_format_attr_h264.c`,
//! `res_format_attr_h263.c`. Provides per-codec SDP `fmtp` line parsing
//! and generation for media negotiation.

use std::collections::HashMap;
use std::fmt;


// ---------------------------------------------------------------------------
// Opus attributes (RFC 7587)
// ---------------------------------------------------------------------------

/// Opus codec attributes.
///
/// Mirrors `struct opus_attr` from `res_format_attr_opus.c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusAttr {
    /// Maximum bitrate in bits/second.
    pub maxbitrate: u32,
    /// Maximum playback sampling rate.
    pub maxplayrate: u32,
    /// Packet time in milliseconds.
    pub ptime: u32,
    /// Stereo encoding (0 or 1).
    pub stereo: u8,
    /// Constant bitrate mode (0 or 1).
    pub cbr: u8,
    /// Forward error correction (0 or 1).
    pub fec: u8,
    /// Discontinuous transmission (0 or 1).
    pub dtx: u8,
    /// Sender's max capture rate.
    pub sprop_maxcapturerate: u32,
    /// Sender's stereo preference.
    pub sprop_stereo: u8,
    /// Maximum packet time.
    pub maxptime: u32,
}

impl Default for OpusAttr {
    fn default() -> Self {
        Self {
            maxbitrate: 510_000,
            maxplayrate: 48_000,
            ptime: 20,
            stereo: 0,
            cbr: 0,
            fec: 0,
            dtx: 0,
            sprop_maxcapturerate: 48_000,
            sprop_stereo: 0,
            maxptime: 120,
        }
    }
}

impl OpusAttr {
    /// Parse from an SDP fmtp line value (key=value pairs separated by `;`).
    pub fn from_fmtp(fmtp: &str) -> Self {
        let mut attr = Self::default();
        for param in fmtp.split(';') {
            let param = param.trim();
            if let Some((key, value)) = param.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "maxaveragebitrate" => {
                        if let Ok(v) = value.parse() { attr.maxbitrate = v; }
                    }
                    "maxplaybackrate" => {
                        if let Ok(v) = value.parse() { attr.maxplayrate = v; }
                    }
                    "ptime" => {
                        if let Ok(v) = value.parse() { attr.ptime = v; }
                    }
                    "stereo" => {
                        if let Ok(v) = value.parse() { attr.stereo = v; }
                    }
                    "cbr" => {
                        if let Ok(v) = value.parse() { attr.cbr = v; }
                    }
                    "useinbandfec" => {
                        if let Ok(v) = value.parse() { attr.fec = v; }
                    }
                    "usedtx" => {
                        if let Ok(v) = value.parse() { attr.dtx = v; }
                    }
                    "sprop-maxcapturerate" => {
                        if let Ok(v) = value.parse() { attr.sprop_maxcapturerate = v; }
                    }
                    "sprop-stereo" => {
                        if let Ok(v) = value.parse() { attr.sprop_stereo = v; }
                    }
                    "maxptime" => {
                        if let Ok(v) = value.parse() { attr.maxptime = v; }
                    }
                    _ => {}
                }
            }
        }
        attr
    }

    /// Generate an SDP fmtp value string.
    pub fn to_fmtp(&self) -> String {
        let mut parts = Vec::new();
        if self.maxbitrate != 510_000 {
            parts.push(format!("maxaveragebitrate={}", self.maxbitrate));
        }
        if self.stereo != 0 {
            parts.push(format!("stereo={}", self.stereo));
        }
        if self.fec != 0 {
            parts.push(format!("useinbandfec={}", self.fec));
        }
        if self.dtx != 0 {
            parts.push(format!("usedtx={}", self.dtx));
        }
        if self.cbr != 0 {
            parts.push(format!("cbr={}", self.cbr));
        }
        if self.sprop_maxcapturerate != 48_000 {
            parts.push(format!("sprop-maxcapturerate={}", self.sprop_maxcapturerate));
        }
        if self.sprop_stereo != 0 {
            parts.push(format!("sprop-stereo={}", self.sprop_stereo));
        }
        parts.join(";")
    }
}

// ---------------------------------------------------------------------------
// SILK attributes
// ---------------------------------------------------------------------------

/// SILK codec attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilkAttr {
    /// Maximum bitrate.
    pub maxbitrate: u32,
    /// Use DTX.
    pub dtx: bool,
    /// Use FEC.
    pub fec: bool,
    /// Packet loss percentage hint.
    pub packet_loss_pct: u32,
}

impl Default for SilkAttr {
    fn default() -> Self {
        Self {
            maxbitrate: 0,
            dtx: false,
            fec: true,
            packet_loss_pct: 0,
        }
    }
}

impl SilkAttr {
    pub fn from_fmtp(fmtp: &str) -> Self {
        let mut attr = Self::default();
        for param in fmtp.split(';') {
            let param = param.trim();
            if let Some((key, value)) = param.split_once('=') {
                match key.trim() {
                    "maxaveragebitrate" => {
                        if let Ok(v) = value.trim().parse() { attr.maxbitrate = v; }
                    }
                    "usedtx" => attr.dtx = value.trim() == "1",
                    "useinbandfec" => attr.fec = value.trim() == "1",
                    _ => {}
                }
            }
        }
        attr
    }
}

// ---------------------------------------------------------------------------
// VP8 attributes
// ---------------------------------------------------------------------------

/// VP8 video codec attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Default)]
pub struct Vp8Attr {
    /// Maximum frame rate.
    pub max_fr: Option<u32>,
    /// Maximum frame size (in macroblocks).
    pub max_fs: Option<u32>,
}


impl Vp8Attr {
    pub fn from_fmtp(fmtp: &str) -> Self {
        let mut attr = Self::default();
        for param in fmtp.split(';') {
            let param = param.trim();
            if let Some((key, value)) = param.split_once('=') {
                match key.trim() {
                    "max-fr" => attr.max_fr = value.trim().parse().ok(),
                    "max-fs" => attr.max_fs = value.trim().parse().ok(),
                    _ => {}
                }
            }
        }
        attr
    }
}

// ---------------------------------------------------------------------------
// H.264 attributes (RFC 6184)
// ---------------------------------------------------------------------------

/// H.264 video codec attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Default)]
pub struct H264Attr {
    /// Profile-level-id (3 bytes hex-encoded).
    pub profile_level_id: Option<String>,
    /// Max-mbps (macroblocks per second).
    pub max_mbps: Option<u32>,
    /// Max-fs (max frame size in macroblocks).
    pub max_fs: Option<u32>,
    /// Max-br (max bitrate in units of 1000 bps).
    pub max_br: Option<u32>,
    /// Packetization mode.
    pub packetization_mode: u8,
    /// Level-asymmetry-allowed.
    pub level_asymmetry_allowed: bool,
}


impl H264Attr {
    pub fn from_fmtp(fmtp: &str) -> Self {
        let mut attr = Self::default();
        for param in fmtp.split(';') {
            let param = param.trim();
            if let Some((key, value)) = param.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "profile-level-id" => attr.profile_level_id = Some(value.to_string()),
                    "max-mbps" => attr.max_mbps = value.parse().ok(),
                    "max-fs" => attr.max_fs = value.parse().ok(),
                    "max-br" => attr.max_br = value.parse().ok(),
                    "packetization-mode" => {
                        if let Ok(v) = value.parse() { attr.packetization_mode = v; }
                    }
                    "level-asymmetry-allowed" => {
                        attr.level_asymmetry_allowed = value == "1";
                    }
                    _ => {}
                }
            }
        }
        attr
    }

    pub fn to_fmtp(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ref plid) = self.profile_level_id {
            parts.push(format!("profile-level-id={}", plid));
        }
        if self.packetization_mode != 0 {
            parts.push(format!("packetization-mode={}", self.packetization_mode));
        }
        if self.level_asymmetry_allowed {
            parts.push("level-asymmetry-allowed=1".to_string());
        }
        if let Some(v) = self.max_mbps {
            parts.push(format!("max-mbps={}", v));
        }
        if let Some(v) = self.max_fs {
            parts.push(format!("max-fs={}", v));
        }
        if let Some(v) = self.max_br {
            parts.push(format!("max-br={}", v));
        }
        parts.join(";")
    }
}

// ---------------------------------------------------------------------------
// H.263 attributes
// ---------------------------------------------------------------------------

/// H.263 video codec attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Default)]
pub struct H263Attr {
    /// SQCIF MPI (Minimum Picture Interval).
    pub sqcif: Option<u32>,
    /// QCIF MPI.
    pub qcif: Option<u32>,
    /// CIF MPI.
    pub cif: Option<u32>,
    /// CIF4 MPI.
    pub cif4: Option<u32>,
    /// Maximum bitrate (in units of 100 bps).
    pub max_br: Option<u32>,
}


impl H263Attr {
    pub fn from_fmtp(fmtp: &str) -> Self {
        let mut attr = Self::default();
        for param in fmtp.split(';') {
            let param = param.trim();
            if let Some((key, value)) = param.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "SQCIF" => attr.sqcif = value.parse().ok(),
                    "QCIF" => attr.qcif = value.parse().ok(),
                    "CIF" => attr.cif = value.parse().ok(),
                    "CIF4" => attr.cif4 = value.parse().ok(),
                    "MaxBR" => attr.max_br = value.parse().ok(),
                    _ => {}
                }
            }
        }
        attr
    }
}

// ---------------------------------------------------------------------------
// Generic format attribute handler trait
// ---------------------------------------------------------------------------

/// Generic trait for format attribute handlers.
pub trait FormatAttributeHandler: Send + Sync + fmt::Debug {
    /// Codec name.
    fn codec_name(&self) -> &str;

    /// Parse fmtp parameters from SDP.
    fn parse_sdp_fmtp(&self, fmtp: &str) -> HashMap<String, String>;

    /// Generate fmtp string for SDP.
    fn generate_sdp_fmtp(&self, attrs: &HashMap<String, String>) -> String;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_parse_fmtp() {
        let attr = OpusAttr::from_fmtp(
            "maxaveragebitrate=128000;stereo=1;useinbandfec=1;usedtx=0"
        );
        assert_eq!(attr.maxbitrate, 128_000);
        assert_eq!(attr.stereo, 1);
        assert_eq!(attr.fec, 1);
        assert_eq!(attr.dtx, 0);
    }

    #[test]
    fn test_opus_to_fmtp() {
        let mut attr = OpusAttr::default();
        attr.stereo = 1;
        attr.fec = 1;
        let fmtp = attr.to_fmtp();
        assert!(fmtp.contains("stereo=1"));
        assert!(fmtp.contains("useinbandfec=1"));
    }

    #[test]
    fn test_opus_default() {
        let attr = OpusAttr::default();
        assert_eq!(attr.maxbitrate, 510_000);
        assert_eq!(attr.maxplayrate, 48_000);
        assert_eq!(attr.ptime, 20);
    }

    #[test]
    fn test_h264_parse_fmtp() {
        let attr = H264Attr::from_fmtp(
            "profile-level-id=42801e;packetization-mode=1;level-asymmetry-allowed=1"
        );
        assert_eq!(attr.profile_level_id.as_deref(), Some("42801e"));
        assert_eq!(attr.packetization_mode, 1);
        assert!(attr.level_asymmetry_allowed);
    }

    #[test]
    fn test_h264_roundtrip() {
        let mut attr = H264Attr::default();
        attr.profile_level_id = Some("42801e".to_string());
        attr.packetization_mode = 1;
        let fmtp = attr.to_fmtp();
        let parsed = H264Attr::from_fmtp(&fmtp);
        assert_eq!(parsed.profile_level_id, attr.profile_level_id);
        assert_eq!(parsed.packetization_mode, attr.packetization_mode);
    }

    #[test]
    fn test_vp8_parse() {
        let attr = Vp8Attr::from_fmtp("max-fr=30;max-fs=3600");
        assert_eq!(attr.max_fr, Some(30));
        assert_eq!(attr.max_fs, Some(3600));
    }

    #[test]
    fn test_silk_parse() {
        let attr = SilkAttr::from_fmtp("usedtx=1;useinbandfec=0");
        assert!(attr.dtx);
        assert!(!attr.fec);
    }

    #[test]
    fn test_h263_parse() {
        let attr = H263Attr::from_fmtp("CIF=1;QCIF=2;MaxBR=2560");
        assert_eq!(attr.cif, Some(1));
        assert_eq!(attr.qcif, Some(2));
        assert_eq!(attr.max_br, Some(2560));
    }
}
