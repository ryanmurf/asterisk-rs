//! SLN (raw signed linear) file format handler.
//!
//! Port of asterisk/formats/format_sln.c.
//! Raw 16-bit signed linear PCM, no headers.
//! Multiple sample rates supported via file extension:
//! - .sln  = 8000Hz
//! - .sln12 = 12000Hz
//! - .sln16 = 16000Hz
//! - .sln24 = 24000Hz
//! - .sln32 = 32000Hz
//! - .sln44 = 44100Hz
//! - .sln48 = 48000Hz
//! - .sln96 = 96000Hz
//! - .sln192 = 192000Hz

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs;
use asterisk_codecs::codec::Codec;
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Create an SLN format handler for the given sample rate and codec.
fn make_sln_format(name: &str, extensions: &'static [&'static str], codec: &Codec) -> SlnFormat {
    let codec_arc = Arc::new(codec.clone());
    SlnFormat {
        name: name.to_string(),
        extensions,
        format: Arc::new(Format::new_named(name, codec_arc)),
        sample_rate: codec.sample_rate,
        codec_id: codec.id,
    }
}

pub fn sln8() -> SlnFormat {
    make_sln_format("sln", &["sln", "raw"], &builtin_codecs::CODEC_SLIN8)
}
pub fn sln12() -> SlnFormat {
    make_sln_format("sln12", &["sln12"], &builtin_codecs::CODEC_SLIN12)
}
pub fn sln16() -> SlnFormat {
    make_sln_format("sln16", &["sln16"], &builtin_codecs::CODEC_SLIN16)
}
pub fn sln24() -> SlnFormat {
    make_sln_format("sln24", &["sln24"], &builtin_codecs::CODEC_SLIN24)
}
pub fn sln32() -> SlnFormat {
    make_sln_format("sln32", &["sln32"], &builtin_codecs::CODEC_SLIN32)
}
pub fn sln44() -> SlnFormat {
    make_sln_format("sln44", &["sln44"], &builtin_codecs::CODEC_SLIN44)
}
pub fn sln48() -> SlnFormat {
    make_sln_format("sln48", &["sln48"], &builtin_codecs::CODEC_SLIN48)
}
pub fn sln96() -> SlnFormat {
    make_sln_format("sln96", &["sln96"], &builtin_codecs::CODEC_SLIN96)
}
pub fn sln192() -> SlnFormat {
    make_sln_format("sln192", &["sln192"], &builtin_codecs::CODEC_SLIN192)
}

/// Return all SLN format variants.
pub fn all_sln_formats() -> Vec<SlnFormat> {
    vec![sln8(), sln12(), sln16(), sln24(), sln32(), sln44(), sln48(), sln96(), sln192()]
}

/// SLN file format handler.
pub struct SlnFormat {
    name: String,
    extensions: &'static [&'static str],
    format: Arc<Format>,
    sample_rate: u32,
    codec_id: u32,
}

impl FileFormat for SlnFormat {
    fn name(&self) -> &str { &self.name }
    fn extensions(&self) -> &[&str] { self.extensions }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len() as i64;
        Ok(Box::new(SlnFileStream {
            reader: io::BufReader::new(file),
            sample_rate: self.sample_rate,
            codec_id: self.codec_id,
            position_samples: 0,
            file_size_bytes: file_size,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(SlnFileWriter {
            writer: io::BufWriter::new(file),
            sample_rate: self.sample_rate,
        }))
    }
}

struct SlnFileStream {
    reader: io::BufReader<std::fs::File>,
    sample_rate: u32,
    codec_id: u32,
    position_samples: i64,
    file_size_bytes: i64,
}

impl FileStream for SlnFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        // Read 20ms of samples
        let samples = (self.sample_rate as usize) / 50; // 20ms
        let bytes_to_read = samples * 2;

        let mut buf = vec![0u8; bytes_to_read];
        let bytes_read = self.reader.read(&mut buf)?;

        if bytes_read == 0 {
            return Ok(None);
        }

        buf.truncate(bytes_read);
        let actual_samples = (bytes_read / 2) as u32;
        self.position_samples += actual_samples as i64;

        Ok(Some(Frame::voice(
            self.codec_id,
            actual_samples,
            Bytes::from(buf),
        )))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        let byte_pos = match pos {
            SeekFrom::Start(samples) => SeekFrom::Start(samples * 2),
            SeekFrom::Current(samples) => SeekFrom::Current(samples * 2),
            SeekFrom::End(samples) => SeekFrom::End(samples * 2),
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let sample_pos = new_byte_pos / 2;
        self.position_samples = sample_pos as i64;
        Ok(sample_pos)
    }

    fn tell(&self) -> i64 {
        self.position_samples
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.file_size_bytes = pos as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

struct SlnFileWriter {
    writer: io::BufWriter<std::fs::File>,
    sample_rate: u32,
}

impl FileWriter for SlnFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "SLN writer expects voice frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
