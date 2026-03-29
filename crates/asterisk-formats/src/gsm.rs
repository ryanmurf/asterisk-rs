//! GSM 06.10 file format handler.
//!
//! Port of asterisk/formats/format_gsm.c.
//! GSM 06.10 (Full Rate) frame container.
//! Each frame is exactly 33 bytes and represents 20ms (160 samples at 8kHz).

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_GSM, ID_GSM};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// GSM frame size in bytes.
const GSM_FRAME_SIZE: usize = 33;
/// GSM frame duration in samples (20ms at 8kHz).
const GSM_SAMPLES_PER_FRAME: u32 = 160;

/// GSM file format handler.
pub struct GsmFormat {
    format: Arc<Format>,
}

impl GsmFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_GSM.clone());
        Self {
            format: Arc::new(Format::new_named("gsm", codec)),
        }
    }
}

impl Default for GsmFormat {
    fn default() -> Self { Self::new() }
}

impl FileFormat for GsmFormat {
    fn name(&self) -> &str { "gsm" }
    fn extensions(&self) -> &[&str] { &["gsm"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(GsmFileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / GSM_FRAME_SIZE as u64) as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(GsmFileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct GsmFileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
}

impl FileStream for GsmFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; GSM_FRAME_SIZE];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_GSM,
                    GSM_SAMPLES_PER_FRAME,
                    Bytes::copy_from_slice(&buf),
                )))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(FormatError::Io(e)),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        // Convert sample offset to frame offset (160 samples per frame)
        let byte_pos = match pos {
            SeekFrom::Start(samples) => {
                let frames = samples / GSM_SAMPLES_PER_FRAME as u64;
                SeekFrom::Start(frames * GSM_FRAME_SIZE as u64)
            }
            SeekFrom::Current(samples) => {
                let frames = samples / GSM_SAMPLES_PER_FRAME as i64;
                SeekFrom::Current(frames * GSM_FRAME_SIZE as i64)
            }
            SeekFrom::End(samples) => {
                let frames = samples / GSM_SAMPLES_PER_FRAME as i64;
                SeekFrom::End(frames * GSM_FRAME_SIZE as i64)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / GSM_FRAME_SIZE as u64;
        self.position_frames = frame_pos as i64;
        Ok(frame_pos * GSM_SAMPLES_PER_FRAME as u64)
    }

    fn tell(&self) -> i64 {
        self.position_frames * GSM_SAMPLES_PER_FRAME as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / GSM_FRAME_SIZE as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct GsmFileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for GsmFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() % GSM_FRAME_SIZE != 0 {
                    return Err(FormatError::InvalidFormat(format!(
                        "GSM frame data must be a multiple of {} bytes, got {}",
                        GSM_FRAME_SIZE,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "GSM writer expects voice frames".into(),
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
