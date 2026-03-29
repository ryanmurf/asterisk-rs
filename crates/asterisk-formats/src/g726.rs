//! G.726 file format handler.
//!
//! Port of asterisk/formats/format_g726.c.
//!
//! G.726 is stored as raw ADPCM data. The frame size depends on the bitrate:
//! - 16 kbps: 2 bits/sample, 20 bytes per 20ms (80 samples)
//! - 24 kbps: 3 bits/sample, 30 bytes per 20ms
//! - 32 kbps: 4 bits/sample, 40 bytes per 20ms
//! - 40 kbps: 5 bits/sample, 50 bytes per 20ms
//!
//! The default (and most common) rate is 32 kbps.
//! All rates operate at 8000 Hz sample rate.

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_G726, ID_G726};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Samples per frame (20ms at 8kHz).
const G726_SAMPLES_PER_FRAME: u32 = 160;

/// G.726 bitrate variant for file format purposes.
#[derive(Debug, Clone, Copy)]
pub enum G726FileRate {
    /// 16 kbps
    Rate16,
    /// 24 kbps
    Rate24,
    /// 32 kbps (default)
    Rate32,
    /// 40 kbps
    Rate40,
}

impl G726FileRate {
    /// Frame size in bytes for a 20ms frame.
    pub fn frame_bytes(&self) -> usize {
        match self {
            G726FileRate::Rate16 => 40,   // 160 samples * 2 bits / 8
            G726FileRate::Rate24 => 60,   // 160 samples * 3 bits / 8
            G726FileRate::Rate32 => 80,   // 160 samples * 4 bits / 8
            G726FileRate::Rate40 => 100,  // 160 samples * 5 bits / 8
        }
    }

    /// File extension suffix.
    pub fn extension(&self) -> &str {
        match self {
            G726FileRate::Rate16 => "g726-16",
            G726FileRate::Rate24 => "g726-24",
            G726FileRate::Rate32 => "g726-32",
            G726FileRate::Rate40 => "g726-40",
        }
    }
}

/// G.726 file format handler (default 32kbps).
pub struct G726Format {
    format: Arc<Format>,
    rate: G726FileRate,
}

impl G726Format {
    /// Create a G.726 format handler at the default 32kbps rate.
    pub fn new() -> Self {
        Self::with_rate(G726FileRate::Rate32)
    }

    /// Create a G.726 format handler for a specific bitrate.
    pub fn with_rate(rate: G726FileRate) -> Self {
        let codec = Arc::new(CODEC_G726.clone());
        Self {
            format: Arc::new(Format::new_named("g726", codec)),
            rate,
        }
    }
}

impl Default for G726Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for G726Format {
    fn name(&self) -> &str { "g726" }
    fn extensions(&self) -> &[&str] {
        &["g726", "g726-16", "g726-24", "g726-32", "g726-40"]
    }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        // Determine rate from file extension
        let rate = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| match ext {
                "g726-16" => Some(G726FileRate::Rate16),
                "g726-24" => Some(G726FileRate::Rate24),
                "g726-32" => Some(G726FileRate::Rate32),
                "g726-40" => Some(G726FileRate::Rate40),
                "g726" => Some(self.rate),
                _ => None,
            })
            .unwrap_or(self.rate);

        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        let frame_bytes = rate.frame_bytes();

        Ok(Box::new(G726FileStream {
            reader: io::BufReader::new(file),
            position_frames: 0,
            total_frames: (file_size / frame_bytes as u64) as i64,
            frame_bytes,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(G726FileWriter {
            writer: io::BufWriter::new(file),
            frame_bytes: self.rate.frame_bytes(),
        }))
    }
}

struct G726FileStream {
    reader: io::BufReader<std::fs::File>,
    position_frames: i64,
    total_frames: i64,
    frame_bytes: usize,
}

impl FileStream for G726FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        let mut buf = vec![0u8; self.frame_bytes];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.position_frames += 1;
                Ok(Some(Frame::voice(
                    ID_G726,
                    G726_SAMPLES_PER_FRAME,
                    Bytes::from(buf),
                )))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(FormatError::Io(e)),
        }
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        let byte_pos = match pos {
            SeekFrom::Start(samples) => {
                let frames = samples / G726_SAMPLES_PER_FRAME as u64;
                SeekFrom::Start(frames * self.frame_bytes as u64)
            }
            SeekFrom::Current(samples) => {
                let frames = samples / G726_SAMPLES_PER_FRAME as i64;
                SeekFrom::Current(frames * self.frame_bytes as i64)
            }
            SeekFrom::End(samples) => {
                let frames = samples / G726_SAMPLES_PER_FRAME as i64;
                SeekFrom::End(frames * self.frame_bytes as i64)
            }
        };
        let new_byte_pos = self.reader.seek(byte_pos)?;
        let frame_pos = new_byte_pos / self.frame_bytes as u64;
        self.position_frames = frame_pos as i64;
        Ok(frame_pos * G726_SAMPLES_PER_FRAME as u64)
    }

    fn tell(&self) -> i64 {
        self.position_frames * G726_SAMPLES_PER_FRAME as i64
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        self.total_frames = (pos / self.frame_bytes as u64) as i64;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct G726FileWriter {
    writer: io::BufWriter<std::fs::File>,
    frame_bytes: usize,
}

impl FileWriter for G726FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.len() % self.frame_bytes != 0 {
                    return Err(FormatError::InvalidFormat(format!(
                        "G.726 frame data must be a multiple of {} bytes, got {}",
                        self.frame_bytes,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "G.726 writer expects voice frames".into(),
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

/// Create format handlers for all G.726 rates.
pub fn all_g726_formats() -> Vec<G726Format> {
    vec![
        G726Format::with_rate(G726FileRate::Rate16),
        G726Format::with_rate(G726FileRate::Rate24),
        G726Format::with_rate(G726FileRate::Rate32),
        G726Format::with_rate(G726FileRate::Rate40),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g726_frame_sizes() {
        assert_eq!(G726FileRate::Rate16.frame_bytes(), 40);
        assert_eq!(G726FileRate::Rate24.frame_bytes(), 60);
        assert_eq!(G726FileRate::Rate32.frame_bytes(), 80);
        assert_eq!(G726FileRate::Rate40.frame_bytes(), 100);
    }
}
