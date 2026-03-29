//! H.264 video file format handler.
//!
//! Port of asterisk/formats/format_h264.c.
//!
//! H.264 frames are stored with a 32-bit big-endian length prefix followed
//! by the NAL (Network Abstraction Layer) unit data. Each stored unit may
//! be a complete frame or a slice.
//!
//! File layout:
//!   [4-byte length][NAL unit data][4-byte length][NAL unit data]...
//!
//! The NAL unit header byte contains:
//!   - forbidden_zero_bit (1 bit)
//!   - nal_ref_idc (2 bits) - importance indicator
//!   - nal_unit_type (5 bits) - type of NAL unit

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_H264, ID_H264};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Maximum NAL unit size (to prevent OOM on corrupt files).
const H264_MAX_FRAME_SIZE: u32 = 8 * 1024 * 1024; // 8 MB

/// H.264 NAL unit types (subset relevant to file format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalUnitType {
    /// Non-IDR slice
    Slice = 1,
    /// Slice data partition A
    SliceDataA = 2,
    /// Slice data partition B
    SliceDataB = 3,
    /// Slice data partition C
    SliceDataC = 4,
    /// IDR (Instantaneous Decoding Refresh) slice
    SliceIdr = 5,
    /// SEI (Supplemental Enhancement Information)
    Sei = 6,
    /// Sequence Parameter Set
    Sps = 7,
    /// Picture Parameter Set
    Pps = 8,
    /// Access Unit Delimiter
    Aud = 9,
    /// End of Sequence
    EndSeq = 10,
    /// End of Stream
    EndStream = 11,
    /// Filler data
    Filler = 12,
    /// Other/unknown
    Other,
}

impl NalUnitType {
    /// Parse the NAL unit type from the first byte of NAL data.
    pub fn from_byte(byte: u8) -> Self {
        match byte & 0x1F {
            1 => NalUnitType::Slice,
            2 => NalUnitType::SliceDataA,
            3 => NalUnitType::SliceDataB,
            4 => NalUnitType::SliceDataC,
            5 => NalUnitType::SliceIdr,
            6 => NalUnitType::Sei,
            7 => NalUnitType::Sps,
            8 => NalUnitType::Pps,
            9 => NalUnitType::Aud,
            10 => NalUnitType::EndSeq,
            11 => NalUnitType::EndStream,
            12 => NalUnitType::Filler,
            _ => NalUnitType::Other,
        }
    }

    /// Whether this NAL unit type indicates a keyframe.
    pub fn is_keyframe(&self) -> bool {
        matches!(self, NalUnitType::SliceIdr)
    }
}

/// H.264 video file format handler.
pub struct H264Format {
    format: Arc<Format>,
}

impl H264Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_H264.clone());
        Self {
            format: Arc::new(Format::new_named("h264", codec)),
        }
    }
}

impl Default for H264Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for H264Format {
    fn name(&self) -> &str { "h264" }
    fn extensions(&self) -> &[&str] { &["h264"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        Ok(Box::new(H264FileStream {
            reader: io::BufReader::new(file),
            frame_count: 0,
            timestamp_ms: 0,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(H264FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct H264FileStream {
    reader: io::BufReader<std::fs::File>,
    frame_count: i64,
    timestamp_ms: u64,
}

impl FileStream for H264FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        // Read 4-byte big-endian length prefix
        let mut len_buf = [0u8; 4];
        match self.reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(FormatError::Io(e)),
        }

        let frame_len = u32::from_be_bytes(len_buf);

        if frame_len == 0 {
            return Ok(None);
        }

        if frame_len > H264_MAX_FRAME_SIZE {
            return Err(FormatError::Corrupt(format!(
                "H.264: frame size {} exceeds maximum {}",
                frame_len, H264_MAX_FRAME_SIZE
            )));
        }

        let mut buf = vec![0u8; frame_len as usize];
        self.reader.read_exact(&mut buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                FormatError::Corrupt("H.264: truncated NAL unit".into())
            } else {
                FormatError::Io(e)
            }
        })?;

        self.frame_count += 1;
        self.timestamp_ms += 33; // ~30 fps

        // Determine if this is a frame ending by checking NAL type
        let is_frame_ending = if !buf.is_empty() {
            let nal_type = NalUnitType::from_byte(buf[0]);
            matches!(
                nal_type,
                NalUnitType::Slice
                    | NalUnitType::SliceIdr
                    | NalUnitType::Aud
                    | NalUnitType::EndSeq
                    | NalUnitType::EndStream
            )
        } else {
            true
        };

        Ok(Some(Frame::Video {
            codec_id: ID_H264,
            data: Bytes::from(buf),
            timestamp_ms: self.timestamp_ms,
            seqno: self.frame_count as i32,
            frame_ending: is_frame_ending,
            stream_num: 0,
        }))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        match pos {
            SeekFrom::Start(0) => {
                self.reader.seek(SeekFrom::Start(0))?;
                self.frame_count = 0;
                self.timestamp_ms = 0;
                Ok(0)
            }
            SeekFrom::Start(target_frame) => {
                self.reader.seek(SeekFrom::Start(0))?;
                self.frame_count = 0;
                self.timestamp_ms = 0;

                for _ in 0..target_frame {
                    let mut len_buf = [0u8; 4];
                    match self.reader.read_exact(&mut len_buf) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(FormatError::Io(e)),
                    }
                    let frame_len = u32::from_be_bytes(len_buf);
                    if frame_len > H264_MAX_FRAME_SIZE {
                        return Err(FormatError::Corrupt("H.264: corrupt during seek".into()));
                    }
                    self.reader.seek(SeekFrom::Current(frame_len as i64))?;
                    self.frame_count += 1;
                    self.timestamp_ms += 33;
                }

                Ok(self.frame_count as u64)
            }
            _ => Err(FormatError::Unsupported(
                "H.264: only SeekFrom::Start is supported".into(),
            )),
        }
    }

    fn tell(&self) -> i64 {
        self.frame_count
    }

    fn truncate(&mut self) -> Result<(), FormatError> {
        let pos = self.reader.stream_position()?;
        self.reader.get_ref().set_len(pos)?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        // Video: RTP clock rate is 90000 for H.264
        90000
    }
}

struct H264FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for H264FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Video { data, .. } => {
                let len = data.len() as u32;
                self.writer.write_all(&len.to_be_bytes())?;
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "H.264 writer expects video frames".into(),
            )),
        }
    }

    fn close(&mut self) -> Result<(), FormatError> {
        self.writer.flush()?;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        90000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nal_unit_types() {
        assert_eq!(NalUnitType::from_byte(0x65), NalUnitType::SliceIdr);
        assert!(NalUnitType::from_byte(0x65).is_keyframe());
        assert_eq!(NalUnitType::from_byte(0x67), NalUnitType::Sps);
        assert_eq!(NalUnitType::from_byte(0x68), NalUnitType::Pps);
        assert_eq!(NalUnitType::from_byte(0x41), NalUnitType::Slice);
        assert!(!NalUnitType::from_byte(0x41).is_keyframe());
    }
}
