use serde::{Deserialize, Serialize};

/// Media types for streams and format capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum MediaType {
    #[default]
    Unknown = 0,
    Audio = 1,
    Video = 2,
    Image = 3,
    Text = 4,
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown"),
            Self::Audio => write!(f, "audio"),
            Self::Video => write!(f, "video"),
            Self::Image => write!(f, "image"),
            Self::Text => write!(f, "text"),
        }
    }
}
