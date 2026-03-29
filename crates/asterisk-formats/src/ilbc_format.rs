//! iLBC file format handler.
//!
//! Port of asterisk/formats/format_ilbc.c.
//!
//! Raw, headerless iLBC data files. The default mode is 30ms (50 bytes
//! per frame, 240 samples at 8kHz). The 20ms mode uses 38 bytes per
//! frame and 160 samples.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_ILBC, ID_ILBC};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// iLBC frame size in bytes (30ms mode).
const ILBC_BUF_SIZE: usize = 50;
/// Samples per frame (30ms at 8kHz).
const ILBC_SAMPLES: u32 = 240;

/// iLBC file format handler (30ms mode).
pub struct IlbcFormat {
    format: Arc<Format>,
}

impl IlbcFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_ILBC.clone());
        Self {
            format: Arc::new(Format::new_named("ilbc", codec)),
        }
    }
}

impl Default for IlbcFormat {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for IlbcFormat {
    fn name(&self) -> &str { "ilbc" }
    fn extensions(&self) -> &[&str] { &["ilbc"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        Ok(Box::new(IlbcFileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / ILBC_BUF_SIZE as u64) as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(IlbcFileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct IlbcFileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
}

impl FileStream for IlbcFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = [0u8; ILBC_BUF_SIZE];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_ILBC,
                    ILBC_SAMPLES,
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
                let frames = samples / ILBC_SAMPLES as u64;
                SeekFrom::Start(frames * ILBC_BUF_SIZE as u64)
            }
            SeekFrom::Current(samples) => {
                let frames = samples / ILBC_SAMPLES as i64;
                SeekFrom::Current(frames * ILBC_BUF_SIZE as i64)
            }
            SeekFrom::End(samples) => {
                let frames = samples / ILBC_SAMPLES as i64;
                SeekFrom::End(frames * ILBC_BUF_SIZE as i64)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / ILBC_BUF_SIZE as u64;
        self.position_frames = frame_pos as i64;
        Ok(frame_pos * ILBC_SAMPLES as u64)
    }

    fn tell(&self) -> i64 {
        self.position_frames * ILBC_SAMPLES as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / ILBC_BUF_SIZE as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct IlbcFileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for IlbcFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() != ILBC_BUF_SIZE {
                    return Err(FormatError::InvalidFormat(format!(
                        "iLBC frame data must be {} bytes, got {}",
                        ILBC_BUF_SIZE,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "iLBC writer expects voice frames".into(),
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
