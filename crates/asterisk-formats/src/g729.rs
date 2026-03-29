//! G.729 file format handler.
//!
//! Port of asterisk/formats/format_g729.c.
//!
//! G.729 frames are exactly 10 bytes each, representing 10ms of audio
//! (80 samples at 8000 Hz). The file is a simple sequence of frames
//! with no container header.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_G729, ID_G729};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// G.729 frame size in bytes.
const G729_FRAME_SIZE: usize = 10;
/// Samples per frame (10ms at 8kHz).
const G729_SAMPLES_PER_FRAME: u32 = 80;

/// G.729 file format handler.
pub struct G729Format {
    format: Arc<Format>,
}

impl G729Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_G729.clone());
        Self {
            format: Arc::new(Format::new_named("g729", codec)),
        }
    }
}

impl Default for G729Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for G729Format {
    fn name(&self) -> &str { "g729" }
    fn extensions(&self) -> &[&str] { &["g729"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(G729FileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / G729_FRAME_SIZE as u64) as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(G729FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct G729FileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
}

impl FileStream for G729FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; G729_FRAME_SIZE];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_G729,
                    G729_SAMPLES_PER_FRAME,
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
                let frames = samples / G729_SAMPLES_PER_FRAME as u64;
                SeekFrom::Start(frames * G729_FRAME_SIZE as u64)
            }
            SeekFrom::Current(samples) => {
                let frames = samples / G729_SAMPLES_PER_FRAME as i64;
                SeekFrom::Current(frames * G729_FRAME_SIZE as i64)
            }
            SeekFrom::End(samples) => {
                let frames = samples / G729_SAMPLES_PER_FRAME as i64;
                SeekFrom::End(frames * G729_FRAME_SIZE as i64)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / G729_FRAME_SIZE as u64;
        self.position_frames = frame_pos as i64;
        Ok(frame_pos * G729_SAMPLES_PER_FRAME as u64)
    }

    fn tell(&self) -> i64 {
        self.position_frames * G729_SAMPLES_PER_FRAME as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / G729_FRAME_SIZE as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct G729FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for G729FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() % G729_FRAME_SIZE != 0 {
                    return Err(FormatError::InvalidFormat(format!(
                        "G.729 frame data must be a multiple of {} bytes, got {}",
                        G729_FRAME_SIZE,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "G.729 writer expects voice frames".into(),
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
