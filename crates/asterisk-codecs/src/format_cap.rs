//! Format Capabilities - a set of formats a channel supports.
//!
//! Port of `struct ast_format_cap` from asterisk/include/asterisk/format_cap.h

use crate::format::{Format, FormatCmp};
use asterisk_types::MediaType;
use std::sync::Arc;

/// An entry in the format capabilities structure, pairing a format with framing.
#[derive(Debug, Clone)]
struct FormatCapEntry {
    format: Arc<Format>,
    framing: u32,
}

/// Format capabilities structure - holds a set of formats with preference ordering.
///
/// This is the Rust equivalent of `struct ast_format_cap` from format_cap.h.
/// Formats are stored in preference order (first added = most preferred).
#[derive(Debug, Clone)]
pub struct FormatCap {
    entries: Vec<FormatCapEntry>,
    global_framing: u32,
}

impl FormatCap {
    /// Create a new empty FormatCap.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            global_framing: 0,
        }
    }

    /// Set the global framing in milliseconds.
    pub fn set_framing(&mut self, framing: u32) {
        self.global_framing = framing;
    }

    /// Get the global framing.
    pub fn get_framing(&self) -> u32 {
        if self.global_framing > 0 {
            return self.global_framing;
        }
        self.entries
            .iter()
            .map(|e| if e.framing > 0 { e.framing } else { e.format.default_ms() })
            .min()
            .unwrap_or(0)
    }

    /// Add a format with optional framing override (0 = use default).
    /// Also available as `append` for backward compatibility.
    pub fn add(&mut self, format: Arc<Format>, framing: u32) {
        let fmt_ms = if framing > 0 { framing } else { format.default_ms() };
        if self.global_framing == 0 || (fmt_ms > 0 && fmt_ms < self.global_framing) {
            self.global_framing = fmt_ms;
        }
        self.entries.push(FormatCapEntry { format, framing });
    }

    /// Alias for `add` - backward compatibility with the other agent's API.
    pub fn append(&mut self, format: Arc<Format>, framing: u32) {
        self.add(format, framing);
    }

    /// Remove a format by exact Arc pointer match.
    pub fn remove(&mut self, format: &Arc<Format>) -> bool {
        let len_before = self.entries.len();
        self.entries.retain(|e| !Arc::ptr_eq(&e.format, format));
        self.entries.len() != len_before
    }

    /// Remove all formats of a specific media type.
    pub fn remove_by_type(&mut self, media_type: MediaType) {
        if media_type == MediaType::Unknown {
            self.entries.clear();
        } else {
            self.entries.retain(|e| e.format.media_type() != media_type);
        }
    }

    /// Get the number of formats.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the format at a specific index (zero-based, preference order).
    pub fn get_format(&self, index: usize) -> Option<&Arc<Format>> {
        self.entries.get(index).map(|e| &e.format)
    }

    /// Get the framing for a specific format.
    pub fn get_format_framing(&self, format: &Format) -> u32 {
        for entry in &self.entries {
            if entry.format.compare(format) == FormatCmp::Equal {
                return if entry.framing > 0 { entry.framing } else { self.global_framing };
            }
        }
        self.global_framing
    }

    /// Get the most preferred format for a particular media type.
    pub fn best_by_type(&self, media_type: MediaType) -> Option<Arc<Format>> {
        if media_type == MediaType::Unknown {
            return self.entries.first().map(|e| Arc::clone(&e.format));
        }
        self.entries
            .iter()
            .find(|e| e.format.media_type() == media_type)
            .map(|e| Arc::clone(&e.format))
    }

    /// Check if a format is compatible with the capabilities.
    pub fn is_compatible_format(&self, format: &Format) -> FormatCmp {
        let mut best = FormatCmp::NotEqual;
        for entry in &self.entries {
            let cmp = entry.format.compare(format);
            match cmp {
                FormatCmp::Equal => return FormatCmp::Equal,
                FormatCmp::Subset => best = FormatCmp::Subset,
                FormatCmp::NotEqual => {}
            }
        }
        best
    }

    /// Get the compatible format from capabilities matching the input format.
    pub fn get_compatible_format(&self, format: &Format) -> Option<Arc<Format>> {
        for entry in &self.entries {
            match entry.format.compare(format) {
                FormatCmp::Equal | FormatCmp::Subset => return Some(Arc::clone(&entry.format)),
                FormatCmp::NotEqual => {}
            }
        }
        None
    }

    /// Find compatible formats between two capability structures.
    pub fn get_joint(&self, other: &FormatCap) -> FormatCap {
        let mut result = FormatCap::new();
        for entry in &self.entries {
            for other_entry in &other.entries {
                if let Some(joint) = entry.format.joint(&other_entry.format) {
                    result.add(Arc::new(joint), entry.framing);
                    break;
                }
            }
        }
        result
    }

    /// Check if any joint capabilities exist between two structures.
    pub fn is_compatible(&self, other: &FormatCap) -> bool {
        for entry in &self.entries {
            for other_entry in &other.entries {
                if entry.format.codec_id() == other_entry.format.codec_id() {
                    return true;
                }
            }
        }
        false
    }

    /// Check if two capability structures are identical.
    pub fn is_identical(&self, other: &FormatCap) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        for (a, b) in self.entries.iter().zip(other.entries.iter()) {
            if a.format.compare(&b.format) != FormatCmp::Equal {
                return false;
            }
        }
        true
    }

    /// Check if the capabilities have any formats of a specific type.
    pub fn has_type(&self, media_type: MediaType) -> bool {
        self.entries.iter().any(|e| e.format.media_type() == media_type)
    }

    /// Append formats from another FormatCap, optionally filtering by type.
    pub fn append_from(&mut self, src: &FormatCap, media_type: MediaType) {
        for entry in &src.entries {
            if media_type == MediaType::Unknown || entry.format.media_type() == media_type {
                self.entries.push(entry.clone());
            }
        }
    }

    /// Get a comma-separated string of format names.
    pub fn get_names(&self) -> String {
        self.entries
            .iter()
            .map(|e| e.format.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Iterate over all formats.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Format>> {
        self.entries.iter().map(|e| &e.format)
    }
}

impl Default for FormatCap {
    fn default() -> Self {
        Self::new()
    }
}
