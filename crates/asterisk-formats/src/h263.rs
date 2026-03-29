//! H.263 video file format handler.
//!
//! Port of asterisk/formats/format_h263.c.
//!
//! H.263 frames are stored with a 32-bit big-endian length prefix followed
//! by the frame data. Each frame may contain a complete video frame or a
//! fragment. RTP timestamps are used for timing.
//!
//! File layout:
//!   [4-byte length][frame data][4-byte length][frame data]...

use crate::traits::{FileFormat, FileStream, FileWriter, FormatError};
use asterisk_codecs::builtin_codecs::{CODEC_H263, ID_H263};
use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use bytes::Bytes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

/// Maximum frame size for H.263 (to prevent OOM on corrupt files).
const H263_MAX_FRAME_SIZE: u32 = 4 * 1024 * 1024; // 4 MB

/// H.263 video file format handler.
pub struct H263Format {
    format: Arc<Format>,
}

impl H263Format {
    pub fn new() -> Self {
        let codec = Arc::new(CODEC_H263.clone());
        Self {
            format: Arc::new(Format::new_named("h263", codec)),
        }
    }
}

impl Default for H263Format {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormat for H263Format {
    fn name(&self) -> &str { "h263" }
    fn extensions(&self) -> &[&str] { &["h263"] }
    fn format(&self) -> Arc<Format> { Arc::clone(&self.format) }

    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError> {
        let file = std::fs::File::open(path)?;
        Ok(Box::new(H263FileStream {
            reader: io::BufReader::new(file),
            frame_count: 0,
        }))
    }

    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError> {
        let file = std::fs::File::create(path)?;
        Ok(Box::new(H263FileWriter {
            writer: io::BufWriter::new(file),
        }))
    }
}

struct H263FileStream {
    reader: io::BufReader<std::fs::File>,
    frame_count: i64,
}

impl FileStream for H263FileStream {
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError> {
        // Read the 4-byte big-endian length prefix
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

        if frame_len > H263_MAX_FRAME_SIZE {
            return Err(FormatError::Corrupt(format!(
                "H.263: frame size {} exceeds maximum {}",
                frame_len, H263_MAX_FRAME_SIZE
            )));
        }

        let mut buf = vec![0u8; frame_len as usize];
        self.reader.read_exact(&mut buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                FormatError::Corrupt("H.263: truncated frame data".into())
            } else {
                FormatError::Io(e)
            }
        })?;

        self.frame_count += 1;

        Ok(Some(Frame::Video {
            codec_id: ID_H263,
            data: Bytes::from(buf),
            timestamp_ms: self.frame_count as u64 * 33, // ~30 fps
            seqno: self.frame_count as i32,
            frame_ending: true,
            stream_num: 0,
        }))
    }

    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError> {
        // Video files with length-prefixed frames require sequential scan for seeking.
        match pos {
            SeekFrom::Start(0) => {
                self.reader.seek(SeekFrom::Start(0))?;
                self.frame_count = 0;
                Ok(0)
            }
            SeekFrom::Start(target_frame) => {
                self.reader.seek(SeekFrom::Start(0))?;
                self.frame_count = 0;
                for _ in 0..target_frame {
                    let mut len_buf = [0u8; 4];
                    match self.reader.read_exact(&mut len_buf) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(FormatError::Io(e)),
                    }
                    let frame_len = u32::from_be_bytes(len_buf);
                    if frame_len > H263_MAX_FRAME_SIZE {
                        return Err(FormatError::Corrupt("H.263: corrupt frame during seek".into()));
                    }
                    self.reader.seek(SeekFrom::Current(frame_len as i64))?;
                    self.frame_count += 1;
                }
                Ok(self.frame_count as u64)
            }
            _ => {
                Err(FormatError::Unsupported(
                    "H.263: only SeekFrom::Start is supported".into(),
                ))
            }
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
        // Video: RTP clock rate is 90000 for H.263
        90000
    }
}

struct H263FileWriter {
    writer: io::BufWriter<std::fs::File>,
}

impl FileWriter for H263FileWriter {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError> {
        match frame {
            Frame::Video { data, .. } => {
                // Write length prefix then data
                let len = data.len() as u32;
                self.writer.write_all(&len.to_be_bytes())?;
                self.writer.write_all(data)?;
                Ok(())
            }
            _ => Err(FormatError::InvalidFormat(
                "H.263 writer expects video frames".into(),
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
