//! Global file format registry.
//!
//! Provides format detection and lookup by file extension.

use crate::traits::{FileFormat, FormatError};
use crate::wav::{WavFormat8k, WavFormat16k};
use crate::sln;
use crate::pcm::{PcmUlawFormat, PcmAlawFormat};
use crate::gsm::GsmFormat;
use crate::g729::G729Format;
use crate::g723::G723Format;
use crate::g726::G726Format;
use crate::h263::H263Format;
use crate::h264::H264Format;
use crate::ilbc_format::IlbcFormat;
use crate::speex_format::OggSpeexFormat;
use crate::vox::VoxFormat;
use crate::siren7::Siren7Format;
use crate::siren14::Siren14Format;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Registry of file format handlers, keyed by extension.
pub struct FormatRegistry {
    /// Map from file extension (lowercase, no dot) to format handler.
    by_extension: HashMap<String, Arc<dyn FileFormat>>,
    /// All registered formats.
    formats: Vec<Arc<dyn FileFormat>>,
}

impl FormatRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            by_extension: HashMap::new(),
            formats: Vec::new(),
        }
    }

    /// Create a registry pre-populated with all built-in formats.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register all built-in file formats.
    pub fn register_builtins(&mut self) {
        self.register(Arc::new(WavFormat8k::new()));
        self.register(Arc::new(WavFormat16k::new()));
        self.register(Arc::new(PcmUlawFormat::new()));
        self.register(Arc::new(PcmAlawFormat::new()));
        self.register(Arc::new(GsmFormat::new()));
        self.register(Arc::new(G729Format::new()));
        self.register(Arc::new(G723Format::new()));
        self.register(Arc::new(G726Format::new()));
        self.register(Arc::new(H263Format::new()));
        self.register(Arc::new(H264Format::new()));
        self.register(Arc::new(IlbcFormat::new()));
        self.register(Arc::new(OggSpeexFormat::new()));
        self.register(Arc::new(VoxFormat::new()));
        self.register(Arc::new(Siren7Format::new()));
        self.register(Arc::new(Siren14Format::new()));
        for sln_fmt in sln::all_sln_formats() {
            self.register(Arc::new(sln_fmt));
        }
    }

    /// Register a file format handler.
    pub fn register(&mut self, format: Arc<dyn FileFormat>) {
        for ext in format.extensions() {
            self.by_extension
                .insert(ext.to_lowercase(), Arc::clone(&format));
        }
        self.formats.push(format);
    }

    /// Look up a format handler by file extension (without the dot).
    pub fn get_by_extension(&self, ext: &str) -> Option<Arc<dyn FileFormat>> {
        self.by_extension.get(&ext.to_lowercase()).cloned()
    }

    /// List all registered format names.
    pub fn format_names(&self) -> Vec<&str> {
        self.formats.iter().map(|f| f.name()).collect()
    }

    /// List all registered extensions.
    pub fn extensions(&self) -> Vec<&String> {
        self.by_extension.keys().collect()
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Detect the file format from a file path's extension.
///
/// Returns the format handler and the extension that matched.
pub fn detect_format(path: &Path) -> Result<(Arc<dyn FileFormat>, String), FormatError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| FormatError::InvalidFormat("no file extension".into()))?
        .to_lowercase();

    let registry = FormatRegistry::with_builtins();
    match registry.get_by_extension(&ext) {
        Some(f) => Ok((f, ext)),
        None => Err(FormatError::Unsupported(format!("unknown extension: {}", ext))),
    }
}
