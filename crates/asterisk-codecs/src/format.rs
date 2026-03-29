//! Format type - a codec with optional attributes.
//!
//! Port of `struct ast_format` from asterisk/include/asterisk/format.h

use crate::codec::Codec;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Comparison result for formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatCmp {
    /// Both formats are equivalent to each other.
    Equal,
    /// Both formats are completely different.
    NotEqual,
    /// Both formats are similar but not equivalent (subset relationship).
    Subset,
}

/// A media format: a codec plus optional codec-specific attributes.
///
/// This is the Rust equivalent of `struct ast_format` from format.h.
/// For example, Opus might have attributes for FEC, DTX, stereo, etc.
#[derive(Clone)]
pub struct Format {
    /// The underlying codec.
    pub codec: Arc<Codec>,
    /// Optional format name override (for codecs with multiple sample rates
    /// like slin8, slin16, etc.).
    pub name: String,
    /// Codec-specific attributes (e.g., "fec" -> "1", "stereo" -> "0").
    pub attributes: HashMap<String, String>,
    /// Number of audio channels (default 1).
    pub channel_count: u32,
}

impl Format {
    /// Create a new format from a codec with no attributes.
    pub fn new(codec: Arc<Codec>) -> Self {
        let name = codec.name.to_string();
        Self {
            codec,
            name,
            attributes: HashMap::new(),
            channel_count: 1,
        }
    }

    /// Create a new format with a specific name.
    pub fn new_named(name: impl Into<String>, codec: Arc<Codec>) -> Self {
        Self {
            codec,
            name: name.into(),
            attributes: HashMap::new(),
            channel_count: 1,
        }
    }

    /// Clone the format (for modification).
    pub fn clone_format(&self) -> Self {
        self.clone()
    }

    /// Compare two formats.
    pub fn compare(&self, other: &Format) -> FormatCmp {
        // Different codecs are never equal
        if self.codec.id != other.codec.id {
            return FormatCmp::NotEqual;
        }
        // If both have the same codec, check attributes
        if self.attributes == other.attributes {
            FormatCmp::Equal
        } else if self.attributes.is_empty() || other.attributes.is_empty() {
            // One has no attributes - it's a subset
            FormatCmp::Subset
        } else {
            // Both have attributes but they differ
            FormatCmp::NotEqual
        }
    }

    /// Get the joint format between two formats (intersection of attributes).
    pub fn joint(&self, other: &Format) -> Option<Format> {
        if self.codec.id != other.codec.id {
            return None;
        }
        // For formats without attribute interfaces, same codec = joint
        let mut joint = self.clone();
        // Merge: keep attributes that match
        let mut merged = HashMap::new();
        for (k, v) in &self.attributes {
            if let Some(ov) = other.attributes.get(k) {
                if v == ov {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }
        joint.attributes = merged;
        Some(joint)
    }

    /// Set an attribute on this format.
    pub fn set_attribute(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.attributes.insert(name.into(), value.into());
    }

    /// Get an attribute value.
    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }

    /// Get the codec name.
    pub fn codec_name(&self) -> &str {
        self.codec.name
    }

    /// Get the codec ID.
    pub fn codec_id(&self) -> u32 {
        self.codec.id
    }

    /// Get the sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.codec.sample_rate
    }

    /// Get the media type.
    pub fn media_type(&self) -> asterisk_types::MediaType {
        self.codec.media_type
    }

    /// Get the default framing in milliseconds.
    pub fn default_ms(&self) -> u32 {
        self.codec.default_ms
    }

    /// Get minimum framing in milliseconds.
    pub fn minimum_ms(&self) -> u32 {
        self.codec.minimum_ms
    }

    /// Get maximum framing in milliseconds.
    pub fn maximum_ms(&self) -> u32 {
        self.codec.maximum_ms
    }

    /// Get minimum bytes per frame.
    pub fn minimum_bytes(&self) -> u32 {
        self.codec.minimum_bytes
    }

    /// Whether the format can be smoothed.
    pub fn can_be_smoothed(&self) -> bool {
        self.codec.smooth
    }
}

impl fmt::Debug for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Format")
            .field("name", &self.name)
            .field("codec", &self.codec.name)
            .field("sample_rate", &self.codec.sample_rate)
            .field("attributes", &self.attributes)
            .finish()
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl PartialEq for Format {
    fn eq(&self, other: &Self) -> bool {
        self.compare(other) == FormatCmp::Equal
    }
}

impl Eq for Format {}
