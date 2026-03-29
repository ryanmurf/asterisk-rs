//! Siren14 (G.722.1 Annex C) file format handler.
//!
//! Port of asterisk/formats/format_siren14.c.
//!
//! ITU G.722.1 Annex C (Siren14, licensed from Polycom), 48kbps bitrate only.
//! 120 bytes per frame, 640 samples per frame (20ms at 32kHz).
//! Raw headerless format.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_SIREN14, ID_SIREN14};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Frame size in bytes (20ms at 48kbps).
const SIREN14_FRAME_SIZE: usize = 120;
/// Samples per frame (20ms at 32kHz).
const SIREN14_SAMPLES_PER_FRAME: u32 = 640;

/// Bytes-to-samples ratio: 640 / 120 = 16/3.
fn bytes_to_samples(bytes: u64) -> u64 {
    bytes * 640 / 120
}

fn samples_to_bytes(samples: i64) -> i64 {
    samples * 120 / 640
}

/// Siren14 file format handler.
pub struct Siren14Format {
    format: Arc<Format>,
}

impl Siren14Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_SIREN14.clone());
        Self {
            format: Arc::new(Format::new_named("siren14", codec)),
        }
    }
}

impl Default for Siren14Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for Siren14Format {
    fn name(&self) -> &str { "siren14" }
    fn extensions(&self) -> &[&str] { &["siren14"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(Siren14FileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / SIREN14_FRAME_SIZE as u64) as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(Siren14FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct Siren14FileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
}

impl FileStream for Siren14FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; SIREN14_FRAME_SIZE];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_SIREN14,
                    SIREN14_SAMPLES_PER_FRAME,
                    Bytes::copy_from_slice(&buf),
                )))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(FormatError::Io(e)),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        let byte_pos = match pos {
            SeekFrom::Start(samples) => {
                SeekFrom::Start(samples_to_bytes(samples as i64) as u64)
            }
            SeekFrom::Current(samples) => {
                SeekFrom::Current(samples_to_bytes(samples))
            }
            SeekFrom::End(samples) => {
                SeekFrom::End(samples_to_bytes(samples))
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / SIREN14_FRAME_SIZE as u64;
        self.position_frames = frame_pos as i64;
        Ok(bytes_to_samples(new_byte_pos))
    }

    fn tell(&self) -> i64 {
        self.position_frames * SIREN14_SAMPLES_PER_FRAME as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / SIREN14_FRAME_SIZE as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        32000
    }
}

struct Siren14FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for Siren14FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() % SIREN14_FRAME_SIZE != 0 {
                    return Err(FormatError::InvalidFormat(format!(
                        "Siren14 frame data must be a multiple of {} bytes, got {}",
                        SIREN14_FRAME_SIZE,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "Siren14 writer expects voice frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        32000
    }
}
