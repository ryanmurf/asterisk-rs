//! Stasis recording management.
//!
//! Port of `res/res_stasis_recording.c`. Provides start/stop/pause/unpause
//! control for recordings initiated through the Stasis (ARI) framework.
//! Tracks both live in-progress recordings and stored completed recordings.

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum RecordingError {
    #[error("recording not found: {0}")]
    NotFound(String),
    #[error("recording already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid state transition: {0:?} -> {1:?}")]
    InvalidTransition(RecordingState, RecordingState),
    #[error("recording error: {0}")]
    Other(String),
}

pub type RecordingResult<T> = Result<T, RecordingError>;

// ---------------------------------------------------------------------------
// Recording state
// ---------------------------------------------------------------------------

/// State of a live recording.
///
/// Mirrors `enum stasis_app_recording_state` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Queued but not yet started.
    Queued,
    /// Actively recording.
    Recording,
    /// Paused.
    Paused,
    /// Completed successfully.
    Complete,
    /// Failed.
    Failed,
    /// Cancelled by user.
    Canceled,
}

impl RecordingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Complete => "done",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    /// Whether this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Canceled)
    }
}

impl fmt::Display for RecordingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Recording options
// ---------------------------------------------------------------------------

/// Termination condition for when to stop recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateOn {
    /// No automatic termination.
    None,
    /// Terminate on any DTMF.
    Any,
    /// Terminate on '#'.
    Hash,
    /// Terminate on '*'.
    Star,
    /// Terminate on silence.
    Silence,
}

/// What to do if a recording with the same name exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfExists {
    /// Fail the new recording.
    Fail,
    /// Overwrite the old recording.
    Overwrite,
    /// Append to the old recording.
    Append,
}

/// Options for starting a recording.
///
/// Mirrors `struct stasis_app_recording_options` from the C source.
#[derive(Debug, Clone)]
pub struct RecordingOptions {
    /// Recording name (used as filename stem).
    pub name: String,
    /// File format (e.g., "wav", "gsm").
    pub format: String,
    /// Maximum recording duration in seconds (0 = unlimited).
    pub max_duration_seconds: u32,
    /// Maximum silence duration before auto-stop in seconds (0 = disabled).
    pub max_silence_seconds: u32,
    /// Whether to beep before recording.
    pub beep: bool,
    /// DTMF termination condition.
    pub terminate_on: TerminateOn,
    /// Behaviour when a recording with same name exists.
    pub if_exists: IfExists,
}

impl RecordingOptions {
    pub fn new(name: &str, format: &str) -> Self {
        Self {
            name: name.to_string(),
            format: format.to_string(),
            max_duration_seconds: 0,
            max_silence_seconds: 0,
            beep: false,
            terminate_on: TerminateOn::None,
            if_exists: IfExists::Fail,
        }
    }
}

// ---------------------------------------------------------------------------
// Live recording
// ---------------------------------------------------------------------------

/// A live (in-progress) recording.
///
/// Mirrors `struct stasis_app_recording` from the C source.
#[derive(Debug, Clone)]
pub struct LiveRecording {
    /// Recording name.
    pub name: String,
    /// Current state.
    pub state: RecordingState,
    /// Recording options.
    pub options: RecordingOptions,
    /// Total duration recorded so far (milliseconds).
    pub duration_ms: u64,
    /// Duration of non-silence audio (milliseconds).
    pub talking_duration_ms: u64,
    /// Whether the recording is currently muted.
    pub muted: bool,
    /// Channel ID being recorded.
    pub channel_id: String,
    /// Timestamp when recording started.
    pub started_at: u64,
}

impl LiveRecording {
    /// Create a new live recording.
    pub fn new(channel_id: &str, options: RecordingOptions) -> Self {
        Self {
            name: options.name.clone(),
            state: RecordingState::Queued,
            options,
            duration_ms: 0,
            talking_duration_ms: 0,
            muted: false,
            channel_id: channel_id.to_string(),
            started_at: current_timestamp(),
        }
    }

    /// Transition to a new state. Returns error on invalid transitions.
    pub fn set_state(&mut self, new_state: RecordingState) -> RecordingResult<()> {
        if self.state.is_terminal() {
            return Err(RecordingError::InvalidTransition(self.state, new_state));
        }
        let valid = matches!(
            (self.state, new_state),
            (RecordingState::Queued, RecordingState::Recording)
                | (RecordingState::Recording, RecordingState::Paused)
                | (RecordingState::Recording, RecordingState::Complete)
                | (RecordingState::Recording, RecordingState::Failed)
                | (RecordingState::Recording, RecordingState::Canceled)
                | (RecordingState::Paused, RecordingState::Recording)
                | (RecordingState::Paused, RecordingState::Complete)
                | (RecordingState::Paused, RecordingState::Canceled)
                | (RecordingState::Queued, RecordingState::Failed)
                | (RecordingState::Queued, RecordingState::Canceled)
        );
        if !valid {
            return Err(RecordingError::InvalidTransition(self.state, new_state));
        }
        debug!(recording = %self.name, from = ?self.state, to = ?new_state, "Recording state change");
        self.state = new_state;
        Ok(())
    }

    /// Pause the recording.
    pub fn pause(&mut self) -> RecordingResult<()> {
        self.set_state(RecordingState::Paused)
    }

    /// Unpause (resume) the recording.
    pub fn unpause(&mut self) -> RecordingResult<()> {
        self.set_state(RecordingState::Recording)
    }

    /// Stop the recording successfully.
    pub fn stop(&mut self) -> RecordingResult<()> {
        self.set_state(RecordingState::Complete)
    }

    /// Cancel the recording.
    pub fn cancel(&mut self) -> RecordingResult<()> {
        self.set_state(RecordingState::Canceled)
    }

    /// Mute the recording input.
    pub fn mute(&mut self) {
        self.muted = true;
    }

    /// Unmute the recording input.
    pub fn unmute(&mut self) {
        self.muted = false;
    }
}

// ---------------------------------------------------------------------------
// Stored recording
// ---------------------------------------------------------------------------

/// A completed stored recording.
#[derive(Debug, Clone)]
pub struct StoredRecording {
    /// Recording name.
    pub name: String,
    /// File format.
    pub format: String,
    /// Full file path (absolute, minus extension).
    pub file_path: String,
    /// Duration in seconds.
    pub duration_seconds: u32,
    /// Timestamp when recorded.
    pub recorded_at: u64,
}

impl StoredRecording {
    pub fn new(name: &str, format: &str, file_path: &str) -> Self {
        Self {
            name: name.to_string(),
            format: format.to_string(),
            file_path: file_path.to_string(),
            duration_seconds: 0,
            recorded_at: current_timestamp(),
        }
    }

    /// Full filename including format extension.
    pub fn full_path(&self) -> String {
        format!("{}.{}", self.file_path, self.format)
    }
}

// ---------------------------------------------------------------------------
// Recording manager
// ---------------------------------------------------------------------------

/// Manages live and stored recordings for the Stasis framework.
pub struct RecordingManager {
    /// Live recordings keyed by name.
    live: RwLock<HashMap<String, LiveRecording>>,
    /// Stored recordings keyed by name.
    stored: RwLock<HashMap<String, StoredRecording>>,
}

impl RecordingManager {
    pub fn new() -> Self {
        Self {
            live: RwLock::new(HashMap::new()),
            stored: RwLock::new(HashMap::new()),
        }
    }

    /// Start a new live recording.
    pub fn start(
        &self,
        channel_id: &str,
        options: RecordingOptions,
    ) -> RecordingResult<LiveRecording> {
        let name = options.name.clone();
        let mut live = self.live.write();
        if live.contains_key(&name) {
            return Err(RecordingError::AlreadyExists(name));
        }
        let mut rec = LiveRecording::new(channel_id, options);
        rec.set_state(RecordingState::Recording)?;
        live.insert(name.clone(), rec.clone());
        info!(recording = %name, channel = channel_id, "Started live recording");
        Ok(rec)
    }

    /// Get a live recording by name.
    pub fn get_live(&self, name: &str) -> RecordingResult<LiveRecording> {
        self.live
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| RecordingError::NotFound(name.to_string()))
    }

    /// Perform an operation on a live recording.
    pub fn with_live_mut<F, R>(&self, name: &str, f: F) -> RecordingResult<R>
    where
        F: FnOnce(&mut LiveRecording) -> RecordingResult<R>,
    {
        let mut live = self.live.write();
        let rec = live
            .get_mut(name)
            .ok_or_else(|| RecordingError::NotFound(name.to_string()))?;
        f(rec)
    }

    /// Complete a recording and move it to stored.
    pub fn complete(&self, name: &str, file_path: &str) -> RecordingResult<StoredRecording> {
        let rec = {
            let mut live = self.live.write();
            let mut rec = live
                .remove(name)
                .ok_or_else(|| RecordingError::NotFound(name.to_string()))?;
            rec.stop()?;
            rec
        };

        let stored = StoredRecording {
            name: rec.name.clone(),
            format: rec.options.format.clone(),
            file_path: file_path.to_string(),
            duration_seconds: (rec.duration_ms / 1000) as u32,
            recorded_at: rec.started_at,
        };
        self.stored
            .write()
            .insert(stored.name.clone(), stored.clone());
        info!(recording = %name, "Recording completed and stored");
        Ok(stored)
    }

    /// Get a stored recording by name.
    pub fn get_stored(&self, name: &str) -> RecordingResult<StoredRecording> {
        self.stored
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| RecordingError::NotFound(name.to_string()))
    }

    /// List all stored recording names.
    pub fn stored_names(&self) -> Vec<String> {
        self.stored.read().keys().cloned().collect()
    }

    /// Delete a stored recording.
    pub fn delete_stored(&self, name: &str) -> RecordingResult<StoredRecording> {
        self.stored
            .write()
            .remove(name)
            .ok_or_else(|| RecordingError::NotFound(name.to_string()))
    }

    /// Number of live recordings.
    pub fn live_count(&self) -> usize {
        self.live.read().len()
    }

    /// Number of stored recordings.
    pub fn stored_count(&self) -> usize {
        self.stored.read().len()
    }
}

impl Default for RecordingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RecordingManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingManager")
            .field("live", &self.live.read().len())
            .field("stored", &self.stored.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_state_transitions() {
        let opts = RecordingOptions::new("test", "wav");
        let mut rec = LiveRecording::new("chan-001", opts);
        assert_eq!(rec.state, RecordingState::Queued);

        rec.set_state(RecordingState::Recording).unwrap();
        rec.pause().unwrap();
        assert_eq!(rec.state, RecordingState::Paused);

        rec.unpause().unwrap();
        assert_eq!(rec.state, RecordingState::Recording);

        rec.stop().unwrap();
        assert!(rec.state.is_terminal());
    }

    #[test]
    fn test_invalid_transition() {
        let opts = RecordingOptions::new("test", "wav");
        let mut rec = LiveRecording::new("chan-001", opts);
        // Cannot go directly from Queued to Paused.
        assert!(rec.set_state(RecordingState::Paused).is_err());
    }

    #[test]
    fn test_terminal_state_blocks_transition() {
        let opts = RecordingOptions::new("test", "wav");
        let mut rec = LiveRecording::new("chan-001", opts);
        rec.set_state(RecordingState::Recording).unwrap();
        rec.cancel().unwrap();
        assert!(rec.set_state(RecordingState::Recording).is_err());
    }

    #[test]
    fn test_recording_manager() {
        let mgr = RecordingManager::new();
        let opts = RecordingOptions::new("greeting", "wav");
        mgr.start("chan-001", opts).unwrap();
        assert_eq!(mgr.live_count(), 1);

        mgr.with_live_mut("greeting", |rec| {
            rec.duration_ms = 5000;
            Ok(())
        })
        .unwrap();

        let stored = mgr.complete("greeting", "/var/spool/recording/greeting").unwrap();
        assert_eq!(stored.duration_seconds, 5);
        assert_eq!(mgr.live_count(), 0);
        assert_eq!(mgr.stored_count(), 1);
    }

    #[test]
    fn test_stored_recording_path() {
        let stored = StoredRecording::new("greeting", "wav", "/var/spool/recording/greeting");
        assert_eq!(stored.full_path(), "/var/spool/recording/greeting.wav");
    }
}
