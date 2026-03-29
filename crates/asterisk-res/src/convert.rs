//! Audio format conversion.
//!
//! Port of `res/res_convert.c`. Implements the CLI "file convert" command
//! which transcodes audio files between formats using the registered
//! format readers and writers.

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConvertError {
    #[error("unsupported input format: {0}")]
    UnsupportedInputFormat(String),
    #[error("unsupported output format: {0}")]
    UnsupportedOutputFormat(String),
    #[error("input file not found: {0}")]
    InputNotFound(String),
    #[error("conversion failed: {0}")]
    ConversionFailed(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    ParseError(String),
}

pub type ConvertResult<T> = Result<T, ConvertError>;

// ---------------------------------------------------------------------------
// File format detection
// ---------------------------------------------------------------------------

/// Detect audio format from file extension.
pub fn detect_format(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_lowercase())
}

/// Known audio format extensions and their names.
pub fn known_formats() -> Vec<(&'static str, &'static str)> {
    vec![
        ("wav", "WAV (PCM)"),
        ("wav49", "WAV (GSM)"),
        ("gsm", "GSM"),
        ("sln", "Signed Linear 8kHz"),
        ("sln16", "Signed Linear 16kHz"),
        ("sln32", "Signed Linear 32kHz"),
        ("sln48", "Signed Linear 48kHz"),
        ("alaw", "A-law"),
        ("ulaw", "mu-law"),
        ("g722", "G.722"),
        ("g726", "G.726"),
        ("g729", "G.729"),
        ("ogg", "Ogg Vorbis"),
        ("vox", "ADPCM VOX"),
        ("raw", "Raw audio"),
    ]
}

/// Check if a format extension is known.
pub fn is_known_format(ext: &str) -> bool {
    known_formats().iter().any(|(e, _)| *e == ext.to_lowercase())
}

// ---------------------------------------------------------------------------
// Conversion request
// ---------------------------------------------------------------------------

/// A file conversion request.
#[derive(Debug, Clone)]
pub struct ConvertRequest {
    /// Input file path.
    pub input: PathBuf,
    /// Output file path.
    pub output: PathBuf,
    /// Input format (detected or overridden).
    pub input_format: String,
    /// Output format (detected or overridden).
    pub output_format: String,
}

impl ConvertRequest {
    /// Create a conversion request, detecting formats from file extensions.
    pub fn new(input: impl Into<PathBuf>, output: impl Into<PathBuf>) -> ConvertResult<Self> {
        let input = input.into();
        let output = output.into();

        let input_format = detect_format(&input).ok_or_else(|| {
            ConvertError::UnsupportedInputFormat(
                input.display().to_string(),
            )
        })?;
        let output_format = detect_format(&output).ok_or_else(|| {
            ConvertError::UnsupportedOutputFormat(
                output.display().to_string(),
            )
        })?;

        Ok(Self {
            input,
            output,
            input_format,
            output_format,
        })
    }

    /// Validate the request.
    pub fn validate(&self) -> ConvertResult<()> {
        if !is_known_format(&self.input_format) {
            return Err(ConvertError::UnsupportedInputFormat(
                self.input_format.clone(),
            ));
        }
        if !is_known_format(&self.output_format) {
            return Err(ConvertError::UnsupportedOutputFormat(
                self.output_format.clone(),
            ));
        }
        Ok(())
    }

    /// Whether this is a same-format copy (no transcoding needed).
    pub fn is_same_format(&self) -> bool {
        self.input_format == self.output_format
    }
}

impl fmt::Display for ConvertRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}) -> {} ({})",
            self.input.display(),
            self.input_format,
            self.output.display(),
            self.output_format,
        )
    }
}

// ---------------------------------------------------------------------------
// CLI command parser
// ---------------------------------------------------------------------------

/// Parse a "file convert" CLI command.
///
/// Syntax: `file convert <infile> <outfile>`
pub fn parse_convert_command(args: &[&str]) -> ConvertResult<ConvertRequest> {
    let args: Vec<&str> = if args.first() == Some(&"file") {
        if args.get(1) == Some(&"convert") {
            args[2..].to_vec()
        } else {
            args[1..].to_vec()
        }
    } else {
        args.to_vec()
    };

    if args.len() < 2 {
        return Err(ConvertError::ParseError(
            "Usage: file convert <infile> <outfile>".into(),
        ));
    }

    ConvertRequest::new(args[0], args[1])
}

// ---------------------------------------------------------------------------
// Conversion statistics
// ---------------------------------------------------------------------------

/// Statistics for a completed conversion.
#[derive(Debug, Clone)]
pub struct ConvertStats {
    /// Number of audio frames read from input.
    pub frames_read: u64,
    /// Number of audio frames written to output.
    pub frames_written: u64,
    /// Input file size in bytes.
    pub input_size: u64,
    /// Output file size in bytes.
    pub output_size: u64,
    /// Duration of the audio in milliseconds.
    pub duration_ms: u64,
}

impl ConvertStats {
    pub fn new() -> Self {
        Self {
            frames_read: 0,
            frames_written: 0,
            input_size: 0,
            output_size: 0,
            duration_ms: 0,
        }
    }

    /// Compression ratio (output/input).
    pub fn compression_ratio(&self) -> f64 {
        if self.input_size == 0 {
            0.0
        } else {
            self.output_size as f64 / self.input_size as f64
        }
    }
}

impl Default for ConvertStats {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ConvertStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "frames: {} -> {}, size: {} -> {} bytes, ratio: {:.2}, duration: {}ms",
            self.frames_read,
            self.frames_written,
            self.input_size,
            self.output_size,
            self.compression_ratio(),
            self.duration_ms,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format() {
        assert_eq!(detect_format(Path::new("test.wav")), Some("wav".to_string()));
        assert_eq!(detect_format(Path::new("test.gsm")), Some("gsm".to_string()));
        assert_eq!(
            detect_format(Path::new("/path/to/file.sln16")),
            Some("sln16".to_string())
        );
        assert_eq!(detect_format(Path::new("noext")), None);
    }

    #[test]
    fn test_known_format() {
        assert!(is_known_format("wav"));
        assert!(is_known_format("GSM"));
        assert!(is_known_format("ulaw"));
        assert!(!is_known_format("mp3"));
    }

    #[test]
    fn test_convert_request() {
        let req = ConvertRequest::new("input.wav", "output.gsm").unwrap();
        assert_eq!(req.input_format, "wav");
        assert_eq!(req.output_format, "gsm");
        assert!(!req.is_same_format());
    }

    #[test]
    fn test_same_format() {
        let req = ConvertRequest::new("input.wav", "output.wav").unwrap();
        assert!(req.is_same_format());
    }

    #[test]
    fn test_parse_command() {
        let req =
            parse_convert_command(&["file", "convert", "in.wav", "out.gsm"]).unwrap();
        assert_eq!(req.input_format, "wav");
        assert_eq!(req.output_format, "gsm");
    }

    #[test]
    fn test_parse_command_short() {
        let req = parse_convert_command(&["in.ulaw", "out.alaw"]).unwrap();
        assert_eq!(req.input_format, "ulaw");
        assert_eq!(req.output_format, "alaw");
    }

    #[test]
    fn test_parse_too_few_args() {
        assert!(parse_convert_command(&["in.wav"]).is_err());
    }

    #[test]
    fn test_validate() {
        let req = ConvertRequest::new("in.wav", "out.gsm").unwrap();
        assert!(req.validate().is_ok());

        // Construct with unknown format manually.
        let req2 = ConvertRequest {
            input: PathBuf::from("in.xyz"),
            output: PathBuf::from("out.wav"),
            input_format: "xyz".to_string(),
            output_format: "wav".to_string(),
        };
        assert!(req2.validate().is_err());
    }

    #[test]
    fn test_convert_stats() {
        let stats = ConvertStats {
            frames_read: 100,
            frames_written: 100,
            input_size: 1000,
            output_size: 500,
            duration_ms: 5000,
        };
        assert!((stats.compression_ratio() - 0.5).abs() < 0.01);
    }
}
