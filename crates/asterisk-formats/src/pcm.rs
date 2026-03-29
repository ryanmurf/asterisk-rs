//! PCM (raw mu-law / A-law) file format handler.
//!
//! Port of asterisk/formats/format_pcm.c.
//! Raw 8-bit companded audio, no headers.
//! - .pcm, .ulaw, .ul, .mu, .ulw = mu-law
//! - .alaw, .al, .alw = A-law
//! - .au = Sun/NeXT au format (mu-law, simple header skipped)

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_ULAW, CODEC_ALAW, ID_ULAW, ID_ALAW};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Read buffer size: 160 bytes = 160 samples = 20ms at 8kHz
const BUF_SIZE: usize = 160;

/// mu-law PCM file format handler.
pub struct PcmUlawFormat {
    format: Arc<Format>,
}

impl PcmUlawFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_ULAW.clone());
        Self {
            format: Arc::new(Format::new_named("ulaw", codec)),
        }
    }
}

impl Default for PcmUlawFormat {
    fn default() -> Self { Self::new() }
}

impl FileFormat for PcmUlawFormat {
    fn name(&self) -> &str { "pcm" }
    fn extensions(&self) -> &[&str] { &["pcm", "ulaw", "ul", "mu", "ulw"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Box::new(PcmFileStream {
            reader: io::BufReader::new(file),
            codec_id: ID_ULAW,
            position_samples: 0,
            file_size: metadata.len() as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(PcmFileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

/// A-law PCM file format handler.
pub struct PcmAlawFormat {
    format: Arc<Format>,
}

impl PcmAlawFormat {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_ALAW.clone());
        Self {
            format: Arc::new(Format::new_named("alaw", codec)),
        }
    }
}

impl Default for PcmAlawFormat {
    fn default() -> Self { Self::new() }
}

impl FileFormat for PcmAlawFormat {
    fn name(&self) -> &str { "alaw" }
    fn extensions(&self) -> &[&str] { &["alaw", "al", "alw"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        Ok(Box::new(PcmFileStream {
            reader: io::BufReader::new(file),
            codec_id: ID_ALAW,
            position_samples: 0,
            file_size: metadata.len() as i64,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(PcmFileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

// ---------------------------------------------------------------------------
// PCM file stream (reader) - works for both ulaw and alaw
// ---------------------------------------------------------------------------

struct PcmFileStream {
    reader: io::BufReader<std::fs::File>,
    codec_id: u32,
    position_samples: i64,
    file_size: i64,
}

impl FileStream for PcmFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = vec![0u8; BUF_SIZE];
        let bytes_read = self.reader.read(&mut buf)?;

        if bytes_read == 0 {
            return Ok(None);
        }

        buf.truncate(bytes_read);
        let samples = bytes_read as u32; // 1 byte per sample for ulaw/alaw
        self.position_samples += samples as i64;

        Ok(Some(Frame::voice(
            self.codec_id,
            samples,
            Bytes::from(buf),
        )))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        // For PCM, 1 byte = 1 sample, so byte offset = sample offset
        let new_pos = self.reader.seek(pos)?;
        self.position_samples = new_pos as i64;
        Ok(new_pos)
    }

    fn tell(&self) -> i64 {
        self.position_samples
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.file_size = pos as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

// ---------------------------------------------------------------------------
// PCM file writer
// ---------------------------------------------------------------------------

struct PcmFileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for PcmFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "PCM writer expects voice frames".into(),
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
