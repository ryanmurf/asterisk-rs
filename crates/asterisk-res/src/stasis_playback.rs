//! Stasis playback management.
//!
//! Port of `res/res_stasis_playback.c`. Manages media playback operations
//! initiated through the Stasis (ARI) framework. Supports multiple media
//! URI schemes (sound:, recording:, number:, digits:, characters:, tone:)
//! and playback control (pause, unpause, restart, reverse, forward).

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum PlaybackError {
    #[error("playback not found: {0}")]
    NotFound(String),
    #[error("invalid state for operation: {0:?}")]
    InvalidState(PlaybackState),
    #[error("playback error: {0}")]
    Other(String),
}

pub type PlaybackResult<T> = Result<T, PlaybackError>;

// ---------------------------------------------------------------------------
// Playback state
// ---------------------------------------------------------------------------

/// State of a playback operation.
///
/// Mirrors `enum stasis_app_playback_state` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    /// Queued but not yet started.
    Queued,
    /// Currently playing.
    Playing,
    /// Continuing to next media in list.
    Continuing,
    /// Paused.
    Paused,
    /// Completed (all media played).
    Complete,
    /// Failed.
    Failed,
    /// Cancelled.
    Canceled,
}

impl PlaybackState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Playing => "playing",
            Self::Continuing => "continuing",
            Self::Paused => "paused",
            Self::Complete => "done",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Canceled)
    }
}

impl fmt::Display for PlaybackState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Media URI scheme
// ---------------------------------------------------------------------------

/// Media URI scheme for playback targets.
///
/// Mirrors the URI scheme constants in the C source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaUri {
    /// Play a sound file (e.g., "sound:hello-world").
    Sound(String),
    /// Play a stored recording (e.g., "recording:myrecording").
    Recording(String),
    /// Say a number (e.g., "number:42").
    Number(String),
    /// Say digits (e.g., "digits:1234").
    Digits(String),
    /// Say characters (e.g., "characters:abc").
    Characters(String),
    /// Play an indication tone (e.g., "tone:busy").
    Tone(String),
}

impl MediaUri {
    /// Parse a media URI string into a typed variant.
    pub fn parse(uri: &str) -> Option<Self> {
        if let Some(rest) = uri.strip_prefix("sound:") {
            Some(Self::Sound(rest.to_string()))
        } else if let Some(rest) = uri.strip_prefix("recording:") {
            Some(Self::Recording(rest.to_string()))
        } else if let Some(rest) = uri.strip_prefix("number:") {
            Some(Self::Number(rest.to_string()))
        } else if let Some(rest) = uri.strip_prefix("digits:") {
            Some(Self::Digits(rest.to_string()))
        } else if let Some(rest) = uri.strip_prefix("characters:") {
            Some(Self::Characters(rest.to_string()))
        } else { uri.strip_prefix("tone:").map(|rest| Self::Tone(rest.to_string())) }
    }

    /// Convert back to URI string.
    pub fn to_uri(&self) -> String {
        match self {
            Self::Sound(s) => format!("sound:{}", s),
            Self::Recording(s) => format!("recording:{}", s),
            Self::Number(s) => format!("number:{}", s),
            Self::Digits(s) => format!("digits:{}", s),
            Self::Characters(s) => format!("characters:{}", s),
            Self::Tone(s) => format!("tone:{}", s),
        }
    }
}

// ---------------------------------------------------------------------------
// Playback control operations
// ---------------------------------------------------------------------------

/// Control operations for an active playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackOperation {
    /// Restart from the beginning.
    Restart,
    /// Pause playback.
    Pause,
    /// Resume from paused state.
    Unpause,
    /// Skip forward by `skipms` milliseconds.
    Forward,
    /// Skip backward by `skipms` milliseconds.
    Reverse,
    /// Stop/cancel playback.
    Stop,
}

// ---------------------------------------------------------------------------
// Playback object
// ---------------------------------------------------------------------------

/// Default number of milliseconds to skip for forward/reverse.
pub const DEFAULT_SKIP_MS: i64 = 3000;

/// A Stasis playback operation.
///
/// Mirrors `struct stasis_app_playback` from the C source.
#[derive(Debug, Clone)]
pub struct Playback {
    /// Unique playback ID.
    pub id: String,
    /// Current media being played (URI string).
    pub media: String,
    /// List of all media URIs to play in sequence.
    pub media_list: Vec<String>,
    /// Current index in the media list.
    pub media_index: usize,
    /// Preferred language.
    pub language: String,
    /// Target channel or bridge URI.
    pub target: String,
    /// Current state.
    pub state: PlaybackState,
    /// Milliseconds offset to start playback from.
    pub offset_ms: i64,
    /// Milliseconds to skip for forward/reverse.
    pub skip_ms: i64,
    /// Milliseconds of media played so far.
    pub played_ms: i64,
    /// Whether the playback can be controlled (pause/ff/rw).
    pub controllable: bool,
}

impl Playback {
    /// Create a new playback.
    pub fn new(media_list: Vec<String>, target: &str, language: &str) -> Self {
        let current = media_list.first().cloned().unwrap_or_default();
        Self {
            id: Uuid::new_v4().to_string(),
            media: current,
            media_list,
            media_index: 0,
            language: language.to_string(),
            target: target.to_string(),
            state: PlaybackState::Queued,
            offset_ms: 0,
            skip_ms: DEFAULT_SKIP_MS,
            played_ms: 0,
            controllable: true,
        }
    }

    /// Advance to the next media item in the list.
    pub fn advance(&mut self) -> bool {
        if self.media_index + 1 < self.media_list.len() {
            self.media_index += 1;
            self.media = self.media_list[self.media_index].clone();
            self.played_ms = 0;
            true
        } else {
            false
        }
    }

    /// Apply a control operation.
    pub fn apply_operation(&mut self, op: PlaybackOperation) -> PlaybackResult<()> {
        if self.state.is_terminal() {
            return Err(PlaybackError::InvalidState(self.state));
        }
        match op {
            PlaybackOperation::Pause => {
                if self.state != PlaybackState::Playing {
                    return Err(PlaybackError::InvalidState(self.state));
                }
                self.state = PlaybackState::Paused;
            }
            PlaybackOperation::Unpause => {
                if self.state != PlaybackState::Paused {
                    return Err(PlaybackError::InvalidState(self.state));
                }
                self.state = PlaybackState::Playing;
            }
            PlaybackOperation::Restart => {
                self.played_ms = 0;
                self.media_index = 0;
                self.media = self.media_list.first().cloned().unwrap_or_default();
                self.state = PlaybackState::Playing;
            }
            PlaybackOperation::Forward => {
                self.played_ms += self.skip_ms;
            }
            PlaybackOperation::Reverse => {
                self.played_ms = (self.played_ms - self.skip_ms).max(0);
            }
            PlaybackOperation::Stop => {
                self.state = PlaybackState::Canceled;
            }
        }
        debug!(playback = %self.id, op = ?op, state = ?self.state, "Playback operation applied");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Playback manager
// ---------------------------------------------------------------------------

/// Manages active playback operations for the Stasis framework.
pub struct PlaybackManager {
    playbacks: RwLock<HashMap<String, Playback>>,
}

impl PlaybackManager {
    pub fn new() -> Self {
        Self {
            playbacks: RwLock::new(HashMap::new()),
        }
    }

    /// Create and start a new playback.
    pub fn start(
        &self,
        media_list: Vec<String>,
        target: &str,
        language: &str,
    ) -> Playback {
        let mut pb = Playback::new(media_list, target, language);
        pb.state = PlaybackState::Playing;
        let id = pb.id.clone();
        self.playbacks.write().insert(id.clone(), pb.clone());
        info!(playback = %id, target = target, "Started playback");
        pb
    }

    /// Get a playback by ID.
    pub fn get(&self, id: &str) -> PlaybackResult<Playback> {
        self.playbacks
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| PlaybackError::NotFound(id.to_string()))
    }

    /// Apply a control operation to a playback.
    pub fn control(
        &self,
        id: &str,
        op: PlaybackOperation,
    ) -> PlaybackResult<()> {
        let mut playbacks = self.playbacks.write();
        let pb = playbacks
            .get_mut(id)
            .ok_or_else(|| PlaybackError::NotFound(id.to_string()))?;
        pb.apply_operation(op)?;
        if pb.state.is_terminal() {
            drop(playbacks);
            self.playbacks.write().remove(id);
        }
        Ok(())
    }

    /// Remove a completed/failed playback.
    pub fn remove(&self, id: &str) -> PlaybackResult<Playback> {
        self.playbacks
            .write()
            .remove(id)
            .ok_or_else(|| PlaybackError::NotFound(id.to_string()))
    }

    /// Number of active playbacks.
    pub fn active_count(&self) -> usize {
        self.playbacks.read().len()
    }
}

impl Default for PlaybackManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PlaybackManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaybackManager")
            .field("active", &self.playbacks.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_uri_parse() {
        assert_eq!(
            MediaUri::parse("sound:hello-world"),
            Some(MediaUri::Sound("hello-world".into()))
        );
        assert_eq!(
            MediaUri::parse("recording:myrecording"),
            Some(MediaUri::Recording("myrecording".into()))
        );
        assert_eq!(
            MediaUri::parse("number:42"),
            Some(MediaUri::Number("42".into()))
        );
        assert!(MediaUri::parse("invalid").is_none());
    }

    #[test]
    fn test_media_uri_roundtrip() {
        let uri = MediaUri::Sound("tt-monkeys".into());
        assert_eq!(MediaUri::parse(&uri.to_uri()), Some(uri));
    }

    #[test]
    fn test_playback_advance() {
        let mut pb = Playback::new(
            vec!["sound:a".into(), "sound:b".into(), "sound:c".into()],
            "channel:chan-001",
            "en",
        );
        assert_eq!(pb.media, "sound:a");
        assert!(pb.advance());
        assert_eq!(pb.media, "sound:b");
        assert!(pb.advance());
        assert_eq!(pb.media, "sound:c");
        assert!(!pb.advance());
    }

    #[test]
    fn test_playback_operations() {
        let mut pb = Playback::new(vec!["sound:a".into()], "channel:chan-001", "en");
        pb.state = PlaybackState::Playing;

        pb.apply_operation(PlaybackOperation::Pause).unwrap();
        assert_eq!(pb.state, PlaybackState::Paused);

        pb.apply_operation(PlaybackOperation::Unpause).unwrap();
        assert_eq!(pb.state, PlaybackState::Playing);

        pb.apply_operation(PlaybackOperation::Forward).unwrap();
        assert_eq!(pb.played_ms, DEFAULT_SKIP_MS);

        pb.apply_operation(PlaybackOperation::Reverse).unwrap();
        assert_eq!(pb.played_ms, 0);

        pb.apply_operation(PlaybackOperation::Stop).unwrap();
        assert!(pb.state.is_terminal());
    }

    #[test]
    fn test_playback_manager() {
        let mgr = PlaybackManager::new();
        let pb = mgr.start(vec!["sound:hello".into()], "channel:chan-001", "en");
        assert_eq!(mgr.active_count(), 1);

        mgr.control(&pb.id, PlaybackOperation::Pause).unwrap();
        let state = mgr.get(&pb.id).unwrap();
        assert_eq!(state.state, PlaybackState::Paused);

        mgr.control(&pb.id, PlaybackOperation::Stop).unwrap();
        // Terminal state removes from active
        assert_eq!(mgr.active_count(), 0);
    }
}
