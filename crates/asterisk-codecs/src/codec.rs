//! Core codec type definitions.
//!
//! Port of `struct ast_codec` from asterisk/include/asterisk/codec.h

use asterisk_types::MediaType;
use std::fmt;

/// Unique codec identifier, assigned at registration time (starts at 1).
pub type CodecId = u32;

/// Represents a media codec within Asterisk.
///
/// This is the Rust equivalent of `struct ast_codec` from codec.h.
/// Codecs are immutable once registered and are shared via `Arc<Codec>`.
#[derive(Clone)]
pub struct Codec {
    /// Internal unique identifier for this codec, set at registration time (starts at 1).
    pub id: CodecId,
    /// Name for this codec (e.g., "ulaw", "alaw", "slin").
    pub name: &'static str,
    /// Brief description (e.g., "G.711 u-law").
    pub description: &'static str,
    /// Type of media this codec contains.
    pub media_type: MediaType,
    /// Sample rate (number of samples carried per second).
    pub sample_rate: u32,
    /// Minimum length of media that can be carried (in milliseconds) in a frame.
    pub minimum_ms: u32,
    /// Maximum length of media that can be carried (in milliseconds) in a frame.
    pub maximum_ms: u32,
    /// Default length of media carried (in milliseconds) in a frame.
    pub default_ms: u32,
    /// Length in bytes of the data payload of a minimum_ms frame.
    pub minimum_bytes: u32,
    /// Whether the media can be smoothed.
    pub smooth: bool,
    /// Format quality, on a scale from 0 to 150 (100 is ulaw, the reference).
    /// Higher values indicate better quality.
    pub quality: u32,
}

impl Codec {
    /// Calculate the number of samples for a given number of bytes of this codec.
    /// This is a simplified version - specific codecs may override via their translator.
    pub fn samples_for_bytes(&self, bytes: u32) -> u32 {
        if self.minimum_bytes == 0 || self.minimum_ms == 0 {
            return 0;
        }
        let samples_per_min_frame = self.sample_rate * self.minimum_ms / 1000;
        bytes * samples_per_min_frame / self.minimum_bytes
    }

    /// Calculate the length in milliseconds for a given number of samples.
    pub fn length_for_samples(&self, samples: u32) -> u32 {
        if self.sample_rate == 0 {
            return 0;
        }
        samples * 1000 / self.sample_rate
    }

    /// Calculate the number of bytes needed for a given number of samples.
    pub fn bytes_for_samples(&self, samples: u32) -> u32 {
        if self.sample_rate == 0 || self.minimum_ms == 0 {
            return 0;
        }
        let samples_per_min_frame = self.sample_rate * self.minimum_ms / 1000;
        if samples_per_min_frame == 0 {
            return 0;
        }
        samples * self.minimum_bytes / samples_per_min_frame
    }
}

impl fmt::Debug for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Codec")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("media_type", &self.media_type)
            .field("sample_rate", &self.sample_rate)
            .field("quality", &self.quality)
            .finish()
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({}Hz)", self.name, self.sample_rate)
    }
}

impl PartialEq for Codec {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Codec {}

impl std::hash::Hash for Codec {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
