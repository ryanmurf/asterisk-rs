//! OGG/Speex file format handler.
//!
//! Port of asterisk/formats/format_ogg_speex.c.
//!
//! This format reads and writes Speex audio data inside an OGG container.
//! The file extension is .spx.
//!
//! OGG structure:
//! - Page 0: OGG header page containing Speex header
//! - Page 1: OGG comment page
//! - Pages 2+: Audio data pages containing Speex frames
//!
//! This is a stub implementation that defines the interface. Full OGG
//! container parsing requires libogg, and Speex header parsing requires
//! libspeex. The file format handler is registered but read/write
//! operations return errors until the dependencies are linked.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_SPEEX8, ID_SPEEX8};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::Arc;

/// Block size for feeding OGG routines.
#[allow(dead_code)]
const BLOCK_SIZE: usize = 4096;

/// OGG/Speex file format handler (stub).
pub struct OggSpeexFormat {
    format: Arc<Format>,
}

impl OggSpeexFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_SPEEX8.clone());
        Self {
            format: Arc::new(Format::new_named("ogg_speex", codec)),
        }
    }
}

impl Default for OggSpeexFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for OggSpeexFormat {
    fn name(&self) -> &str { "ogg_speex" }
    fn extensions(&self) -> &[&str] { &["spx"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, _path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        Err(FormatError::Unsupported(
            "OGG/Speex reading requires libogg and libspeex (not linked)".into(),
        ))
    }

    fn create(&self, _path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        Err(FormatError::Unsupported(
            "OGG/Speex writing requires libogg and libspeex (not linked)".into(),
        ))
    }
}

/// OGG page header structure (for reference / future implementation).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OggPageHeader {
    /// Capture pattern: "OggS"
    pub capture_pattern: [u8; 4],
    /// Stream structure version (0)
    pub version: u8,
    /// Header type flag
    pub header_type: u8,
    /// Absolute granule position
    pub granule_position: i64,
    /// Stream serial number
    pub serial_number: u32,
    /// Page sequence number
    pub page_sequence: u32,
    /// CRC checksum
    pub checksum: u32,
    /// Number of page segments
    pub num_segments: u8,
}

/// Speex header structure (for reference / future implementation).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpeexHeader {
    /// "Speex   " (8 bytes)
    pub speex_string: [u8; 8],
    /// Speex version string
    pub speex_version: [u8; 20],
    /// Speex version id
    pub speex_version_id: i32,
    /// Size of the header
    pub header_size: i32,
    /// Sample rate
    pub rate: i32,
    /// Narrowband/wideband/ultra-wideband mode
    pub mode: i32,
    /// Bit-stream version number
    pub mode_bitstream_version: i32,
    /// Number of channels
    pub nb_channels: i32,
    /// Bitrate
    pub bitrate: i32,
    /// Speex frame size (samples per frame)
    pub frame_size: i32,
    /// VBR flag
    pub vbr: i32,
    /// Number of frames per OGG packet
    pub frames_per_packet: i32,
    /// Number of extra headers
    pub extra_headers: i32,
}

impl SpeexHeader {
    /// Magic string identifying a Speex header.
    pub const MAGIC: &'static [u8; 8] = b"Speex   ";
}
