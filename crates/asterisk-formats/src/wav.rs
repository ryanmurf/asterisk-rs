//! WAV file format handler.
//!
//! Port of asterisk/formats/format_wav.c.
//! Supports reading and writing Microsoft WAV files:
//! - PCM encoded (format tag 1)
//! - 16-bit samples
//! - Mono
//! - 8000Hz or 16000Hz sample rate

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_SLIN8, CODEC_SLIN16, ID_SLIN8, ID_SLIN16};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// WAV file header size (RIFF + fmt + data header = 44 bytes).
const WAV_HEADER_SIZE: u64 = 44;
/// Number of samples to read per frame (20ms at 8000Hz = 160 samples).
const WAV_BUF_SAMPLES: usize = 160;

/// WAV file format handler for 8kHz signed linear.
pub struct WavFormat8k {
    format: Arc<Format>,
}

impl WavFormat8k {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_SLIN8.clone());
        Self {
            format: Arc::new(Format::new_named("slin", codec)),
        }
    }
}

impl Default for WavFormat8k {
    fn default() -> Self { Self::new() }
}

impl FileFormat for WavFormat8k {
    fn name(&self) -> &str { "wav" }
    fn extensions(&self) -> &[&str] { &["wav"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let mut reader = io::BufReader::new(file);
        let (data_size, sample_rate) = parse_wav_header(&mut reader)?;
        Ok(Box::new(WavFileStream {
            reader,
            data_remaining: data_size as i64,
            sample_rate,
            position_samples: 0,
            codec_id: if sample_rate == 16000 { ID_SLIN16 } else { ID_SLIN8 },
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        let mut writer = io::BufWriter::new(file);
        write_wav_header(&mut writer, 8000)?;
        Ok(Box::new(WavFileWriter {
            writer,
            sample_rate: 8000,
        }))
    }
}

/// WAV file format handler for 16kHz signed linear.
pub struct WavFormat16k {
    format: Arc<Format>,
}

impl WavFormat16k {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_SLIN16.clone());
        Self {
            format: Arc::new(Format::new_named("slin16", codec)),
        }
    }
}

impl Default for WavFormat16k {
    fn default() -> Self { Self::new() }
}

impl FileFormat for WavFormat16k {
    fn name(&self) -> &str { "wav16" }
    fn extensions(&self) -> &[&str] { &["wav16"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        let mut reader = io::BufReader::new(file);
        let (data_size, sample_rate) = parse_wav_header(&mut reader)?;
        if sample_rate != 16000 {
            return Err(FormatError::InvalidFormat(format!(
                "expected 16000Hz WAV, got {}Hz",
                sample_rate
            )));
        }
        Ok(Box::new(WavFileStream {
            reader,
            data_remaining: data_size as i64,
            sample_rate,
            position_samples: 0,
            codec_id: ID_SLIN16,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        let mut writer = io::BufWriter::new(file);
        write_wav_header(&mut writer, 16000)?;
        Ok(Box::new(WavFileWriter {
            writer,
            sample_rate: 16000,
        }))
    }
}

// ---------------------------------------------------------------------------
// WAV header parsing
// ---------------------------------------------------------------------------

/// Parse a WAV file header, returning (data_size_bytes, sample_rate).
fn parse_wav_header<R: Read + Seek>(reader: &mut R) -> Result<(u32, u32), FormatError> {
    // RIFF header
    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4)?;
    if &buf4 != b"RIFF" {
        return Err(FormatError::InvalidFormat("not a RIFF file".into()));
    }

    // File size (minus 8 bytes for RIFF + size)
    reader.read_exact(&mut buf4)?;
    // We don't validate the total size, just read it

    // WAVE format
    reader.read_exact(&mut buf4)?;
    if &buf4 != b"WAVE" {
        return Err(FormatError::InvalidFormat("not a WAVE file".into()));
    }

    // Read chunks until we find "data"
    let mut sample_rate = 0u32;
    let mut found_fmt = false;

    loop {
        // Chunk ID
        let mut chunk_id = [0u8; 4];
        if reader.read_exact(&mut chunk_id).is_err() {
            return Err(FormatError::Corrupt("unexpected end of file".into()));
        }

        // Chunk size
        reader.read_exact(&mut buf4)?;
        let chunk_size = u32::from_le_bytes(buf4);

        if &chunk_id == b"fmt " {
            // Format chunk
            if chunk_size < 16 {
                return Err(FormatError::InvalidFormat(
                    "fmt chunk too small".into(),
                ));
            }

            let mut fmt_buf = [0u8; 16];
            reader.read_exact(&mut fmt_buf)?;

            let audio_format = u16::from_le_bytes([fmt_buf[0], fmt_buf[1]]);
            if audio_format != 1 {
                return Err(FormatError::Unsupported(format!(
                    "only PCM format (1) is supported, got {}",
                    audio_format
                )));
            }

            let channels = u16::from_le_bytes([fmt_buf[2], fmt_buf[3]]);
            if channels != 1 {
                return Err(FormatError::Unsupported(format!(
                    "only mono is supported, got {} channels",
                    channels
                )));
            }

            sample_rate = u32::from_le_bytes([fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7]]);
            if sample_rate != 8000 && sample_rate != 16000 {
                return Err(FormatError::Unsupported(format!(
                    "only 8000Hz and 16000Hz supported, got {}Hz",
                    sample_rate
                )));
            }

            // bytes_per_second at offset 8..12 (skip)
            // block_align at offset 12..14
            let block_align = u16::from_le_bytes([fmt_buf[12], fmt_buf[13]]);
            if block_align != 2 {
                return Err(FormatError::Unsupported(format!(
                    "only 16-bit samples (block_align=2) supported, got {}",
                    block_align
                )));
            }

            let bits_per_sample = u16::from_le_bytes([fmt_buf[14], fmt_buf[15]]);
            if bits_per_sample != 16 {
                return Err(FormatError::Unsupported(format!(
                    "only 16-bit samples supported, got {}",
                    bits_per_sample
                )));
            }

            // Skip any extra fmt bytes
            if chunk_size > 16 {
                reader.seek(SeekFrom::Current((chunk_size - 16) as i64))?;
            }

            found_fmt = true;
        } else if &chunk_id == b"data" {
            if !found_fmt {
                return Err(FormatError::Corrupt(
                    "data chunk before fmt chunk".into(),
                ));
            }
            return Ok((chunk_size, sample_rate));
        } else {
            // Skip unknown chunk
            reader.seek(SeekFrom::Current(chunk_size as i64))?;
        }
    }
}

/// Write a WAV file header (will need updating when file is closed).
fn write_wav_header<W: Write + Seek>(writer: &mut W, hz: u32) -> Result<(), FormatError> {
    let bytes_per_second = hz * 2; // 16-bit mono

    writer.seek(SeekFrom::Start(0))?;

    // RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&0u32.to_le_bytes())?; // placeholder for file size
    writer.write_all(b"WAVE")?;

    // fmt chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?; // chunk size
    writer.write_all(&1u16.to_le_bytes())?; // PCM format
    writer.write_all(&1u16.to_le_bytes())?; // mono
    writer.write_all(&hz.to_le_bytes())?; // sample rate
    writer.write_all(&bytes_per_second.to_le_bytes())?; // bytes per second
    writer.write_all(&2u16.to_le_bytes())?; // block align (2 bytes per sample)
    writer.write_all(&16u16.to_le_bytes())?; // bits per sample

    // data chunk
    writer.write_all(b"data")?;
    writer.write_all(&0u32.to_le_bytes())?; // placeholder for data size

    Ok(())
}

/// Update the WAV header with final sizes.
fn update_wav_header<W: Write + Seek>(writer: &mut W) -> Result<(), FormatError> {
    let end = writer.seek(SeekFrom::End(0))?;
    let data_bytes = end - WAV_HEADER_SIZE;

    // Update RIFF chunk size (file_size - 8)
    let file_size = (end - 8) as u32;
    writer.seek(SeekFrom::Start(4))?;
    writer.write_all(&file_size.to_le_bytes())?;

    // Update data chunk size
    writer.seek(SeekFrom::Start(40))?;
    writer.write_all(&(data_bytes as u32).to_le_bytes())?;

    // Seek back to end
    writer.seek(SeekFrom::End(0))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// WAV file stream (reader)
// ---------------------------------------------------------------------------

struct WavFileStream {
    reader: io::BufReader<std::fs::File>,
    data_remaining: i64,
    sample_rate: u32,
    position_samples: i64,
    codec_id: u32,
}

impl FileStream for WavFileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        if self.data_remaining <= 0 {
            return Ok(None);
        }

        // Read up to WAV_BUF_SAMPLES samples (20ms at 8kHz)
        let samples_to_read = WAV_BUF_SAMPLES.min((self.data_remaining / 2) as usize);
        if samples_to_read == 0 {
            return Ok(None);
        }

        let bytes_to_read = samples_to_read * 2;
        let mut buf = vec![0u8; bytes_to_read];
        let bytes_read = self.reader.read(&mut buf)?;

        if bytes_read == 0 {
            return Ok(None);
        }

        buf.truncate(bytes_read);
        let samples = (bytes_read / 2) as u32;
        self.data_remaining -= bytes_read as i64;
        self.position_samples += samples as i64;

        Ok(Some(Frame::voice(
            self.codec_id,
            samples,
            Bytes::from(buf),
        )))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        let byte_pos = match pos {
            SeekFrom::Start(samples) => {
                SeekFrom::Start(WAV_HEADER_SIZE + samples * 2)
            }
            SeekFrom::Current(samples) => {
                SeekFrom::Current(samples * 2)
            }
            SeekFrom::End(samples) => {
                SeekFrom::End(samples * 2)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let sample_pos = if new_byte_pos >= WAV_HEADER_SIZE {
            (new_byte_pos - WAV_HEADER_SIZE) / 2
        } else {
            0
        };
        self.position_samples = sample_pos as i64;
        Ok(sample_pos)
    }

    fn tell(&self) -> i64 {
        self.position_samples
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.data_remaining = 0;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

// ---------------------------------------------------------------------------
// WAV file writer
// ---------------------------------------------------------------------------

struct WavFileWriter {
    writer: io::BufWriter<std::fs::File>,
    sample_rate: u32,
}

impl FileWriter for WavFileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "WAV writer expects voice frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        update_wav_header(&mut self.writer)?;
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
