//! Core traits for file format handlers.
//!
//! Port of asterisk/include/asterisk/file.h and mod_format.h.

use asterisk_codecs::format::Format;
use asterisk_types::Frame;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

/// Errors from file format operations.
#[derive(Error, Debug)]
pub enum FormatError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid format: {0}")]
    InvalidFormat(String),
    #[error("unsupported format: {0}")]
    Unsupported(String),
    #[error("end of file")]
    Eof,
    #[error("corrupt file: {0}")]
    Corrupt(String),
}

/// A file format handler that can open files for reading and writing.
pub trait FileFormat: Send + Sync {
    /// Human-readable name of this format.
    fn name(&self) -> &str;
    /// File extensions this format handles (e.g., ["wav"]).
    fn extensions(&self) -> &[&str];
    /// The media format (codec) this file format produces/consumes.
    fn format(&self) -> Arc<Format>;
    /// Open a file for reading.
    fn open(&self, path: &Path) -> Result<Box<dyn FileStream>, FormatError>;
    /// Create a new file for writing.
    fn create(&self, path: &Path) -> Result<Box<dyn FileWriter>, FormatError>;
}

/// A readable audio file stream.
pub trait FileStream: Send {
    /// Read the next frame from the file.
    /// Returns `Ok(None)` at end of file.
    fn read_frame(&mut self) -> Result<Option<Frame>, FormatError>;
    /// Seek to a sample offset.
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, FormatError>;
    /// Return the current position in samples.
    fn tell(&self) -> i64;
    /// Truncate the file at the current position.
    fn truncate(&mut self) -> Result<(), FormatError>;
    /// Get the sample rate of the stream.
    fn sample_rate(&self) -> u32;
}

/// A writable audio file stream.
pub trait FileWriter: Send {
    /// Write a frame to the file.
    fn write_frame(&mut self, frame: &Frame) -> Result<(), FormatError>;
    /// Finalize and close the file (updates headers, flushes, etc.).
    fn close(&mut self) -> Result<(), FormatError>;
    /// Get the sample rate being written.
    fn sample_rate(&self) -> u32;
}
