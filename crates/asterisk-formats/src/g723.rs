//! G.723.1 file format handler.
//!
//! Port of asterisk/formats/format_g723.c.
//!
//! G.723.1 has variable frame sizes determined by the first 2 bits of the frame:
//! - Type 0: 24 bytes (6.3 kbps)
//! - Type 1: 20 bytes (5.3 kbps)
//! - Type 2:  4 bytes (SID/comfort noise)
//! - Type 3:  1 byte  (untransmitted/erasure)
//!
//! Each frame represents 30ms of audio (240 samples at 8000 Hz).

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_G723, ID_G723};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Samples per G.723.1 frame (30ms at 8kHz).
const G723_SAMPLES_PER_FRAME: u32 = 240;

/// Determine the frame size from the first byte of a G.723.1 frame.
///
/// The frame type is encoded in bits 0-1 of the first byte:
/// - 0: 24 bytes (6.3 kbps rate)
/// - 1: 20 bytes (5.3 kbps rate)
/// - 2:  4 bytes (SID frame)
/// - 3:  1 byte  (untransmitted)
fn g723_frame_size(first_byte: u8) -> usize {
    match first_byte & 0x03 {
        0 => 24,  // 6.3 kbps
        1 => 20,  // 5.3 kbps
        2 => 4,   // SID (silence insertion descriptor)
        3 => 1,   // Untransmitted/erasure
        _ => unreachable!(),
    }
}

/// G.723.1 file format handler.
pub struct G723Format {
    format: Arc<Format>,
}

impl G723Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_G723.clone());
        Self {
            format: Arc::new(Format::new_named("g723", codec)),
        }
    }
}

impl Default for G723Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for G723Format {
    fn name(&self) -> &str { "g723sf" }
    fn extensions(&self) -> &[&str] { &["g723", "g723sf"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        Ok(Box::new(G723FileStream {
            reader: io::BufReader::new(file),
            position_samples: 0,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(G723FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct G723FileStream {
    reader: io::BufReader<std::fs::File>,
    position_samples: i64,
}

impl FileStream for G723FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        // Read the first byte to determine frame type and size
        let mut first = [0u8; 1];
        match self.reader.read_exact(&mut first) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(FormatError::Io(e)),
        }

        let frame_size = g723_frame_size(first[0]);

        // Build the complete frame
        let mut buf = vec![0u8; frame_size];
        buf[0] = first[0];

        if frame_size > 1 {
            match self.reader.read_exact(&mut buf[1..]) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(FormatError::Corrupt(
                        "G.723.1: truncated frame".into(),
                    ));
                }
                Err(e) => return Err(FormatError::Io(e)),
            }
        }

        self.position_samples += G723_SAMPLES_PER_FRAME as i64;

        Ok(Some(Frame::voice(
            ID_G723,
            G723_SAMPLES_PER_FRAME,
            Bytes::from(buf),
        )))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        // G.723.1 has variable frame sizes, so seeking requires scanning.
        // For SeekFrom::Start, we rewind and skip forward.
        match pos {
            SeekFrom::Start(target_samples) => {
                self.reader.seek(SeekFrom::Start(0))?;
                self.position_samples = 0;

                while (self.position_samples as u64) < target_samples {
                    let mut first = [0u8; 1];
                    match self.reader.read_exact(&mut first) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(FormatError::Io(e)),
                    }
                    let frame_size = g723_frame_size(first[0]);
                    if frame_size > 1 {
                        self.reader.seek(SeekFrom::Current(frame_size as i64 - 1))?;
                    }
                    self.position_samples += G723_SAMPLES_PER_FRAME as i64;
                }

                Ok(self.position_samples as u64)
            }
            SeekFrom::Current(offset_samples) => {
                if offset_samples >= 0 {
                    let target = self.position_samples as u64 + offset_samples as u64;
                    self.seek(SeekFrom::Start(target))
                } else {
                    let target = (self.position_samples + offset_samples).max(0) as u64;
                    self.seek(SeekFrom::Start(target))
                }
            }
            SeekFrom::End(_) => {
                // Seeking from end not efficiently supported for VBR
                Err(FormatError::Unsupported(
                    "G.723.1: SeekFrom::End not supported (variable frame sizes)".into(),
                ))
            }
        }
    }

    fn tell(&self) -> i64 {
        self.position_samples
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }
}

struct G723FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for G723FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Voice { data, .. } => {
                if data.is_empty() {
                    return Err(FormatError::InvalidFormat(
                        "G.723.1: empty frame data".into(),
                    ));
                }
                // Validate frame size against type bits
                let expected_size = g723_frame_size(data[0]);
                if data.len() != expected_size {
                    return Err(FormatError::InvalidFormat(format!(
                        "G.723.1: frame type {} expects {} bytes, got {}",
                        data[0] & 0x03,
                        expected_size,
                        data.len()
                    )));
                }
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "G.723.1 writer expects voice frames".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g723_frame_sizes() {
        // Type 0: 6.3 kbps -> 24 bytes
        assert_eq!(g723_frame_size(0x00), 24);
        assert_eq!(g723_frame_size(0xFC), 24); // high bits irrelevant

        // Type 1: 5.3 kbps -> 20 bytes
        assert_eq!(g723_frame_size(0x01), 20);
        assert_eq!(g723_frame_size(0xFD), 20);

        // Type 2: SID -> 4 bytes
        assert_eq!(g723_frame_size(0x02), 4);

        // Type 3: untransmitted -> 1 byte
        assert_eq!(g723_frame_size(0x03), 1);
    }
}
