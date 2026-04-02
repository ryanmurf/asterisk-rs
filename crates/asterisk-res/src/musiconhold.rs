//! Music on Hold (MOH) resource module.
//!
//! Port of `res/res_musiconhold.c`. Provides background music playback for
//! channels that are on hold, parked, or waiting. Supports multiple MOH
//! classes, each with their own directory of audio files and playback mode.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use rand::Rng;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum MohError {
    #[error("MOH class not found: {0}")]
    ClassNotFound(String),
    #[error("MOH no files available for class: {0}")]
    NoFiles(String),
    #[error("MOH I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MOH configuration error: {0}")]
    Config(String),
    #[error("MOH player error: {0}")]
    Player(String),
}

pub type MohResult<T> = Result<T, MohError>;

// ---------------------------------------------------------------------------
// MOH mode
// ---------------------------------------------------------------------------

/// Music on Hold playback mode (from `res_musiconhold.c` MOH_* flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub enum MohMode {
    /// Play audio files from a directory. This is the most common mode.
    #[default]
    Files,
    /// Use a custom external application to stream audio.
    Custom,
    /// Use mpg123 to stream MP3 files.
    Mp3,
    /// Use mpg123 in quiet mode (no console output).
    QuietMp3,
}

impl MohMode {
    /// Parse from configuration string.
    pub fn from_config(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "files" | "file" => Some(Self::Files),
            "custom" => Some(Self::Custom),
            "mp3" => Some(Self::Mp3),
            "quietmp3" | "mp3nb" => Some(Self::QuietMp3),
            _ => None,
        }
    }

    /// Return the configuration name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Custom => "custom",
            Self::Mp3 => "mp3",
            Self::QuietMp3 => "quietmp3",
        }
    }
}


// ---------------------------------------------------------------------------
// Sort mode
// ---------------------------------------------------------------------------

/// How files are ordered within a MOH class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub enum MohSortMode {
    /// Linear playback in filesystem order.
    #[default]
    Linear,
    /// Fully random selection each time.
    Random,
    /// Alphabetically sorted.
    Alpha,
    /// Alphabetically sorted but start at a random position.
    RandStart,
}

impl MohSortMode {
    pub fn from_config(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "linear" | "" => Some(Self::Linear),
            "random" | "randomize" => Some(Self::Random),
            "alpha" | "alphabetical" => Some(Self::Alpha),
            "randstart" | "random_start" => Some(Self::RandStart),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Random => "random",
            Self::Alpha => "alpha",
            Self::RandStart => "randstart",
        }
    }
}


// ---------------------------------------------------------------------------
// MOH class flags (mirror C MOH_* flags)
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Flags associated with a MOH class.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MohFlags: u32 {
        /// Suppress mpg123 output.
        const QUIET           = 1 << 0;
        /// Single-file mode.
        const SINGLE          = 1 << 1;
        /// Custom application mode.
        const CUSTOM          = 1 << 2;
        /// Randomize file order.
        const RANDOMIZE       = 1 << 3;
        /// Sort alphabetically.
        const SORT_ALPHA      = 1 << 4;
        /// Cache realtime classes.
        const CACHE_RT        = 1 << 5;
        /// Play announcement between songs.
        const ANNOUNCEMENT    = 1 << 6;
        /// Prefer channel class over queue class.
        const PREFER_CHAN     = 1 << 7;
        /// Loop the last file instead of wrapping.
        const LOOP_LAST       = 1 << 8;
    }
}

// ---------------------------------------------------------------------------
// MOH class
// ---------------------------------------------------------------------------

/// A Music on Hold class defining how audio is delivered.
///
/// Corresponds to `struct mohclass` in the C source.
#[derive(Debug, Clone)]
pub struct MohClass {
    /// Class name (e.g., "default", "jazz").
    pub name: String,
    /// Playback mode.
    pub mode: MohMode,
    /// Directory containing audio files (for Files mode).
    pub directory: PathBuf,
    /// Audio format name (e.g., "slin", "ulaw").
    pub format: String,
    /// Sort mode for file ordering.
    pub sort: MohSortMode,
    /// Optional announcement file played between songs.
    pub announcement: Option<String>,
    /// Custom application command line (for Custom mode).
    pub application: Option<String>,
    /// Application arguments.
    pub application_args: Option<String>,
    /// Digit that selects this class (for DTMF-based selection).
    pub digit: Option<char>,
    /// Class flags.
    pub flags: MohFlags,
    /// Only play if channel is answered.
    pub answered_only: bool,
    /// Cached list of filenames discovered in `directory`.
    files: Vec<String>,
}

impl MohClass {
    /// Create a new MOH class with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mode: MohMode::Files,
            directory: PathBuf::from("/var/lib/asterisk/moh"),
            format: "slin".to_string(),
            sort: MohSortMode::Linear,
            announcement: None,
            application: None,
            application_args: None,
            digit: None,
            flags: MohFlags::empty(),
            answered_only: false,
            files: Vec::new(),
        }
    }

    /// Scan the directory for audio files and populate the file list.
    ///
    /// This mirrors the file-scanning logic in `moh_scan_files()` from C.
    pub fn scan_files(&mut self) -> MohResult<usize> {
        self.files.clear();

        if !self.directory.exists() {
            return Err(MohError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("MOH directory not found: {}", self.directory.display()),
            )));
        }

        let entries = std::fs::read_dir(&self.directory)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    // Asterisk stores file names without extension; the
                    // playback subsystem resolves the best-match format at
                    // runtime. We store the stem only, as the C code does.
                    if !self.files.iter().any(|f| f == name) {
                        self.files.push(name.to_string());
                    }
                }
            }
        }

        // Apply sort order.
        match self.sort {
            MohSortMode::Alpha | MohSortMode::RandStart => {
                self.files.sort();
            }
            MohSortMode::Random => {
                // Shuffle using Fisher-Yates.
                let mut rng = rand::thread_rng();
                let len = self.files.len();
                for i in (1..len).rev() {
                    let j = rng.gen_range(0..=i);
                    self.files.swap(i, j);
                }
            }
            MohSortMode::Linear => {
                // Keep filesystem order.
            }
        }

        let count = self.files.len();
        debug!(class = %self.name, count, "MOH class scanned files");
        Ok(count)
    }

    /// Get the current list of audio file names (stems, no extension).
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Set the file list directly (useful for testing).
    pub fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
    }

    /// Return the total number of available files.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

// ---------------------------------------------------------------------------
// Per-channel player state
// ---------------------------------------------------------------------------

/// Tracks MOH playback state for a single channel.
///
/// Corresponds to `struct moh_files_state` in the C source.
#[derive(Debug, Clone)]
pub struct MohPlayer {
    /// Name of the MOH class being played.
    pub class_name: String,
    /// Current file position index within the class file list.
    pub pos: usize,
    /// Total number of samples played in the current file.
    pub samples: u64,
    /// Whether the announcement file is currently being played.
    pub playing_announcement: bool,
    /// Saved position for resume after MOH stop/start.
    pub save_pos: Option<usize>,
    /// Saved filename for resume validation.
    pub save_pos_filename: Option<String>,
    /// Channel unique ID this player is associated with.
    pub channel_id: String,
    /// Whether the player is active.
    pub active: bool,
}

impl MohPlayer {
    /// Create a new MOH player for the given channel and class.
    pub fn new(channel_id: &str, class_name: &str) -> Self {
        Self {
            class_name: class_name.to_string(),
            pos: 0,
            samples: 0,
            playing_announcement: false,
            save_pos: None,
            save_pos_filename: None,
            channel_id: channel_id.to_string(),
            active: false,
        }
    }

    /// Advance to the next file, respecting the sort and looping rules.
    ///
    /// Returns the filename to play, or `None` if no files are available.
    pub fn next_file(&mut self, class: &MohClass) -> Option<String> {
        let file_count = class.file_count();
        if file_count == 0 {
            return None;
        }

        // Handle announcement insertion.
        if class.announcement.is_some() && !self.playing_announcement && self.pos > 0 {
            self.playing_announcement = true;
            return class.announcement.clone();
        }
        self.playing_announcement = false;

        // Determine next position based on sort mode.
        match class.sort {
            MohSortMode::Random => {
                let mut rng = rand::thread_rng();
                self.pos = rng.gen_range(0..file_count);
            }
            MohSortMode::RandStart if self.save_pos.is_none() && self.pos == 0 => {
                // First play: start at random position within sorted list.
                let mut rng = rand::thread_rng();
                self.pos = rng.gen_range(0..file_count);
            }
            _ => {
                // Check saved position for resume.
                if let Some(saved) = self.save_pos.take() {
                    if saved < file_count {
                        if let Some(ref saved_name) = self.save_pos_filename {
                            if class.files().get(saved).map(|f| f.as_str()) == Some(saved_name) {
                                self.pos = saved;
                                self.save_pos_filename = None;
                                return Some(class.files()[self.pos].clone());
                            }
                        }
                    }
                }

                self.pos += 1;
                if class.flags.contains(MohFlags::LOOP_LAST) {
                    self.pos = self.pos.min(file_count - 1);
                } else {
                    self.pos %= file_count;
                }
            }
        }

        self.samples = 0;
        let filename = class.files()[self.pos].clone();
        self.save_pos_filename = Some(filename.clone());
        Some(filename)
    }

    /// Save the current position for later resume.
    pub fn save_position(&mut self) {
        self.save_pos = Some(self.pos);
    }

    /// Start playback.
    pub fn start(&mut self) {
        self.active = true;
        debug!(
            channel = %self.channel_id,
            class = %self.class_name,
            "MOH started"
        );
    }

    /// Stop playback and save position for resume.
    pub fn stop(&mut self) {
        self.save_position();
        self.playing_announcement = false;
        self.active = false;
        debug!(
            channel = %self.channel_id,
            class = %self.class_name,
            "MOH stopped"
        );
    }
}

// ---------------------------------------------------------------------------
// MOH manager (global registry)
// ---------------------------------------------------------------------------

/// Central manager for all MOH classes.
///
/// This replaces the `mohclasses` ao2_container from the C source.
pub struct MohManager {
    /// Registered MOH classes by name.
    classes: RwLock<HashMap<String, Arc<MohClass>>>,
    /// Active MOH players indexed by channel unique ID.
    players: RwLock<HashMap<String, MohPlayer>>,
    /// Name of the default class.
    default_class: RwLock<String>,
}

impl MohManager {
    /// Create a new MOH manager.
    pub fn new() -> Self {
        Self {
            classes: RwLock::new(HashMap::new()),
            players: RwLock::new(HashMap::new()),
            default_class: RwLock::new("default".to_string()),
        }
    }

    /// Register a MOH class.
    pub fn register_class(&self, class: MohClass) {
        info!(name = %class.name, mode = ?class.mode, "Registered MOH class");
        self.classes.write().insert(class.name.clone(), Arc::new(class));
    }

    /// Unregister a MOH class by name.
    pub fn unregister_class(&self, name: &str) -> bool {
        self.classes.write().remove(name).is_some()
    }

    /// Get a MOH class by name, falling back to the default if not found.
    pub fn get_class(&self, name: &str) -> Option<Arc<MohClass>> {
        let classes = self.classes.read();
        if name.is_empty() {
            let default_name = self.default_class.read();
            classes.get(default_name.as_str()).cloned()
        } else {
            classes.get(name).cloned()
        }
    }

    /// Set the default MOH class name.
    pub fn set_default_class(&self, name: &str) {
        *self.default_class.write() = name.to_string();
    }

    /// Get the default class name.
    pub fn default_class_name(&self) -> String {
        self.default_class.read().clone()
    }

    /// List all registered class names.
    pub fn class_names(&self) -> Vec<String> {
        self.classes.read().keys().cloned().collect()
    }

    /// Start MOH on a channel.
    pub fn start_moh(&self, channel_id: &str, class_name: &str) -> MohResult<String> {
        let class_name = if class_name.is_empty() {
            self.default_class.read().clone()
        } else {
            class_name.to_string()
        };

        let class = self.get_class(&class_name)
            .ok_or_else(|| MohError::ClassNotFound(class_name.clone()))?;

        if class.mode == MohMode::Files && class.file_count() == 0 {
            return Err(MohError::NoFiles(class_name));
        }

        let mut player = MohPlayer::new(channel_id, &class_name);
        player.start();

        // Get first file to play.
        let first_file = if class.mode == MohMode::Files {
            player.next_file(&class)
        } else {
            None
        };

        self.players.write().insert(channel_id.to_string(), player);

        info!(
            channel = %channel_id,
            class = %class_name,
            "Started music on hold"
        );

        Ok(first_file.unwrap_or_default())
    }

    /// Stop MOH on a channel.
    pub fn stop_moh(&self, channel_id: &str) {
        if let Some(mut player) = self.players.write().remove(channel_id) {
            player.stop();
            info!(channel = %channel_id, "Stopped music on hold");
        }
    }

    /// Get the next file for a channel's MOH playback.
    pub fn next_file(&self, channel_id: &str) -> MohResult<Option<String>> {
        let mut players = self.players.write();
        let player = players.get_mut(channel_id)
            .ok_or_else(|| MohError::Player(format!("No active MOH for channel {}", channel_id)))?;

        let class = self.get_class(&player.class_name)
            .ok_or_else(|| MohError::ClassNotFound(player.class_name.clone()))?;

        Ok(player.next_file(&class))
    }

    /// Check if a channel currently has MOH active.
    pub fn is_moh_active(&self, channel_id: &str) -> bool {
        self.players.read().get(channel_id).is_some_and(|p| p.active)
    }

    /// Get the number of active MOH sessions.
    pub fn active_count(&self) -> usize {
        self.players.read().len()
    }
}

impl Default for MohManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_class(name: &str, files: Vec<&str>) -> MohClass {
        let mut class = MohClass::new(name);
        class.set_files(files.into_iter().map(|s| s.to_string()).collect());
        class
    }

    #[test]
    fn test_moh_mode_parse() {
        assert_eq!(MohMode::from_config("files"), Some(MohMode::Files));
        assert_eq!(MohMode::from_config("custom"), Some(MohMode::Custom));
        assert_eq!(MohMode::from_config("mp3"), Some(MohMode::Mp3));
        assert_eq!(MohMode::from_config("quietmp3"), Some(MohMode::QuietMp3));
        assert_eq!(MohMode::from_config("invalid"), None);
    }

    #[test]
    fn test_sort_mode_parse() {
        assert_eq!(MohSortMode::from_config("random"), Some(MohSortMode::Random));
        assert_eq!(MohSortMode::from_config("alpha"), Some(MohSortMode::Alpha));
        assert_eq!(MohSortMode::from_config("randstart"), Some(MohSortMode::RandStart));
        assert_eq!(MohSortMode::from_config("linear"), Some(MohSortMode::Linear));
    }

    #[test]
    fn test_moh_class_creation() {
        let class = MohClass::new("jazz");
        assert_eq!(class.name, "jazz");
        assert_eq!(class.mode, MohMode::Files);
        assert_eq!(class.file_count(), 0);
    }

    #[test]
    fn test_moh_player_linear() {
        let class = make_test_class("default", vec!["song1", "song2", "song3"]);
        let mut player = MohPlayer::new("chan-001", "default");
        player.start();

        // First call: advance from 0 to 1.
        let f1 = player.next_file(&class).unwrap();
        assert_eq!(f1, "song2");

        let f2 = player.next_file(&class).unwrap();
        assert_eq!(f2, "song3");

        // Wrap around.
        let f3 = player.next_file(&class).unwrap();
        assert_eq!(f3, "song1");
    }

    #[test]
    fn test_moh_player_loop_last() {
        let mut class = make_test_class("hold", vec!["intro", "loop"]);
        class.flags |= MohFlags::LOOP_LAST;

        let mut player = MohPlayer::new("chan-002", "hold");
        player.start();

        let f1 = player.next_file(&class).unwrap();
        assert_eq!(f1, "loop");

        // Should stay on last file.
        let f2 = player.next_file(&class).unwrap();
        assert_eq!(f2, "loop");
    }

    #[test]
    fn test_moh_player_empty_class() {
        let class = make_test_class("empty", vec![]);
        let mut player = MohPlayer::new("chan-003", "empty");
        assert!(player.next_file(&class).is_none());
    }

    #[test]
    fn test_moh_player_announcement() {
        let mut class = make_test_class("announce", vec!["song1", "song2"]);
        class.announcement = Some("please_hold".to_string());

        let mut player = MohPlayer::new("chan-004", "announce");
        player.start();

        // First next: advance to song2 (pos 0 -> 1), no announcement on first song.
        let f1 = player.next_file(&class).unwrap();
        assert_eq!(f1, "song2");

        // Next call should play announcement first.
        let f2 = player.next_file(&class).unwrap();
        assert_eq!(f2, "please_hold");

        // Then the actual next song.
        let f3 = player.next_file(&class).unwrap();
        assert_eq!(f3, "song1");
    }

    #[test]
    fn test_moh_manager_basic() {
        let mgr = MohManager::new();
        let class = make_test_class("default", vec!["song1", "song2"]);
        mgr.register_class(class);

        assert!(mgr.get_class("default").is_some());
        assert!(mgr.get_class("nonexistent").is_none());
        assert_eq!(mgr.class_names().len(), 1);
    }

    #[test]
    fn test_moh_manager_start_stop() {
        let mgr = MohManager::new();
        let class = make_test_class("default", vec!["song1", "song2"]);
        mgr.register_class(class);

        mgr.start_moh("chan-010", "default").unwrap();
        assert!(mgr.is_moh_active("chan-010"));
        assert_eq!(mgr.active_count(), 1);

        mgr.stop_moh("chan-010");
        assert!(!mgr.is_moh_active("chan-010"));
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_moh_manager_default_class() {
        let mgr = MohManager::new();
        let class = make_test_class("default", vec!["song1"]);
        mgr.register_class(class);

        // Empty class name should resolve to default.
        let result = mgr.start_moh("chan-011", "");
        assert!(result.is_ok());
    }

    #[test]
    fn test_moh_manager_class_not_found() {
        let mgr = MohManager::new();
        let result = mgr.start_moh("chan-012", "nonexistent");
        assert!(matches!(result, Err(MohError::ClassNotFound(_))));
    }

    #[test]
    fn test_moh_flags() {
        let flags = MohFlags::RANDOMIZE | MohFlags::ANNOUNCEMENT;
        assert!(flags.contains(MohFlags::RANDOMIZE));
        assert!(flags.contains(MohFlags::ANNOUNCEMENT));
        assert!(!flags.contains(MohFlags::LOOP_LAST));
    }

    #[test]
    fn test_moh_player_save_resume() {
        let class = make_test_class("default", vec!["a", "b", "c", "d"]);
        let mut player = MohPlayer::new("chan-020", "default");
        player.start();

        // Advance to position 1 ("b").
        player.next_file(&class);
        // Advance to position 2 ("c").
        player.next_file(&class);
        assert_eq!(player.pos, 2);

        // Save and stop.
        player.stop();
        assert_eq!(player.save_pos, Some(2));

        // Restart: resume should restore position.
        player.start();
        let resumed = player.next_file(&class).unwrap();
        // save_pos=2, save_pos_filename="c" -> should restore pos 2 and return "c".
        assert_eq!(resumed, "c");
    }
}
