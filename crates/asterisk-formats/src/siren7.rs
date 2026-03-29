//! Siren7 (G.722.1) file format handler.
//!
//! Port of asterisk/formats/format_siren7.c.
//!
//! ITU G.722.1 (Siren7, licensed from Polycom), 32kbps bitrate only.
//! 80 bytes per frame, 320 samples per frame (20ms at 16kHz).
//! Raw headerless format.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_SIREN7, ID_SIREN7};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Frame size in bytes (20ms at 32kbps).
const SIREN7_FRAME_SIZE: usize = 80;
/// Samples per frame (20ms at 16kHz).
const SIREN7_SAMPLES_PER_FRAME: u32 = 320;

/// Ratio for sample-to-byte conversion: 320 samples / 80 bytes = 4.
const SAMPLES_PER_BYTE: u32 = 4;

/// Siren7 file format handler.
pub struct Siren7Format {
    format: Arc<Format>,
}

impl Siren7Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_SIREN7.clone());
        Self {
            format: Arc::new(Format::new_named("siren7", codec)),
        }
    }
}

impl Default for Siren7Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for Siren7Format {
    fn name(&self) -> &str { "siren7" }
    fn extensions(&self) -> &[&str] { &["siren7"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(Siren7FileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / SIREN7_FRAME_SIZE as u64) as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(Siren7FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct Siren7FileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
}

impl FileStream for Siren7FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; SIREN7_FRAME_SIZE];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_SIREN7,
                    SIREN7_SAMPLES_PER_FRAME,
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
                let bytes = samples / SAMPLES_PER_BYTE as u64;
                SeekFrom::Start(bytes)
            }
            SeekFrom::Current(samples) => {
                let bytes = samples / SAMPLES_PER_BYTE as i64;
                SeekFrom::Current(bytes)
            }
            SeekFrom::End(samples) => {
                let bytes = samples / SAMPLES_PER_BYTE as i64;
                SeekFrom::End(bytes)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / SIREN7_FRAME_SIZE as u64;
        self.position_frames = frame_pos as i64;
        Ok(new_byte_pos * SAMPLES_PER_BYTE as u64)
    }

    fn tell(&self) -> i64 {
        self.position_frames * SIREN7_SAMPLES_PER_FRAME as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / SIREN7_FRAME_SIZE as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        16000
    }
}

struct Siren7FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for Siren7FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() % SIREN7_FRAME_SIZE != 0 {
                    return Err(FormatError::InvalidFormat(format!(
                        "Siren7 frame data must be a multiple of {} bytes, got {}",
                        SIREN7_FRAME_SIZE,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "Siren7 writer expects voice frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        16000
    }
}
