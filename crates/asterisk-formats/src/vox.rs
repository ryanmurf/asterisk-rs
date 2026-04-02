//! VOX (Dialogic ADPCM) file format handler.
//!
//! Port of asterisk/formats/format_vox.c.
//!
//! Flat, binary, headerless 4-bit ADPCM data. Two samples per byte,
//! so 80 bytes = 160 samples = 20ms at 8kHz.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_ADPCM, ID_ADPCM};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Buffer size in bytes (80 bytes = 160 samples).
const VOX_BUF_SIZE: usize = 80;
/// Samples per buffer (2 samples per byte).
#[allow(dead_code)]
const VOX_SAMPLES: u32 = 160;

/// VOX (Dialogic ADPCM) file format handler.
pub struct VoxFormat {
    format: Arc<Format>,
}

impl VoxFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_ADPCM.clone());
        Self {
            format: Arc::new(Format::new_named("vox", codec)),
        }
    }
}

impl Default for VoxFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for VoxFormat {
    fn name(&self) -> &str { "vox" }
    fn extensions(&self) -> &[&str] { &["vox"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(VoxFileStream {
            reader: io::BufReader::new(file),
            file_size,
            byte_position: 0,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(VoxFileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct VoxFileStream {
    reader: io::BufReader<std::fs::File>,
    file_size: u64,
    /// Current byte position in the file, tracked manually so tell() can be &self.
    byte_position: u64,
}

impl FileStream for VoxFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; VOX_BUF_SIZE];
        match self.reader.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => {
                self.byte_position += n as u64;
                // Each byte contains 2 samples
                let samples = (n as u32) * 2;
                Ok(Some(Frame::voice(
                    ID_ADPCM,
                    samples,
                    Bytes::copy_from_slice(&buf[..n]),
                )))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(FormatError::Io(e)),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        // Samples to bytes: 2 samples per byte
        let byte_pos = match pos {
            SeekFrom::Start(samples) => SeekFrom::Start(samples / 2),
            SeekFrom::Current(samples) => SeekFrom::Current(samples / 2),
            SeekFrom::End(samples) => SeekFrom::End(samples / 2),
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        // Clamp to file boundaries
        let clamped = new_byte_pos.min(self.file_size);
        if clamped != new_byte_pos {
            self.reader.seek(SeekFrom::Start(clamped))?;
        }
        self.byte_position = clamped;
        Ok(clamped * 2) // Return position in samples
    }

    fn tell(&self) -> i64 {
        self.byte_position as i64 * 2
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.byte_position;
        self.reader.get_ref().set_len(pos)?;
        self.file_size = pos;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct VoxFileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for VoxFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "VOX writer expects voice frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}
