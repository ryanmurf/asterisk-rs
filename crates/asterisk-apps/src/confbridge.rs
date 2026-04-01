//! ConfBridge application - multi-party conference bridge.
//!
//! Port of app_confbridge.c from Asterisk C. Provides named conference rooms
//! where multiple channels can join for multi-party audio mixing. Supports
//! user profiles, bridge profiles, DTMF menus, conference lifecycle (marked/
//! wait_marked/end_marked), AMI events, and CLI commands.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::bridge::Bridge;
use asterisk_core::channel::{Channel, ChannelId};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Global registry of active conferences.
static CONFERENCES: once_cell::sync::Lazy<DashMap<String, Arc<RwLock<Conference>>>> =
    once_cell::sync::Lazy::new(DashMap::new);

/// Global SFU event broadcast sender.  Every conference participant subscribes.
static SFU_EVENT_TX: once_cell::sync::Lazy<tokio::sync::broadcast::Sender<SfuEvent>> =
    once_cell::sync::Lazy::new(|| tokio::sync::broadcast::channel(64).0);

/// SFU event: a participant joined or left a conference.
#[derive(Debug, Clone)]
pub enum SfuEvent {
    /// A new participant joined — all other participants should be re-INVITEd.
    ParticipantJoined {
        conference_name: String,
        /// The channel ID of the participant that just joined.
        joined_channel_id: ChannelId,
    },
    /// A participant left — remaining participants should be re-INVITEd.
    ParticipantLeft {
        conference_name: String,
        /// The channel ID of the participant that left.
        left_channel_id: ChannelId,
        /// Video streams the departed participant had (to set port=0 in re-INVITE).
        departed_video_streams: Vec<VideoStreamInfo>,
    },
}

/// Global registry of user profiles loaded from confbridge.conf.
static USER_PROFILES: once_cell::sync::Lazy<DashMap<String, UserProfile>> =
    once_cell::sync::Lazy::new(DashMap::new);

/// Global registry of bridge profiles loaded from confbridge.conf.
static BRIDGE_PROFILES: once_cell::sync::Lazy<DashMap<String, BridgeProfile>> =
    once_cell::sync::Lazy::new(DashMap::new);

/// Global registry of menu profiles loaded from confbridge.conf.
static MENU_PROFILES: once_cell::sync::Lazy<DashMap<String, ConfMenu>> =
    once_cell::sync::Lazy::new(DashMap::new);

// ---------------------------------------------------------------------------
// Video Mode
// ---------------------------------------------------------------------------

/// Video mode for the conference bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConferenceVideoMode {
    /// No video
    #[default]
    None,
    /// Follow the active talker
    FollowTalker,
    /// Follow the last marked user
    LastMarked,
    /// Follow the first marked user
    FirstMarked,
    /// Selective Forwarding Unit (SFU) mode -- each participant gets all streams
    Sfu,
}

impl ConferenceVideoMode {
    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "none" => Self::None,
            "follow_talker" | "follow-talker" => Self::FollowTalker,
            "last_marked" | "last-marked" => Self::LastMarked,
            "first_marked" | "first-marked" => Self::FirstMarked,
            "sfu" => Self::Sfu,
            _ => {
                warn!("ConfBridge: unknown video mode '{}', defaulting to None", s);
                Self::None
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FollowTalker => "follow_talker",
            Self::LastMarked => "last_marked",
            Self::FirstMarked => "first_marked",
            Self::Sfu => "sfu",
        }
    }
}

// ---------------------------------------------------------------------------
// User Profile (confbridge.conf [user_profile])
// ---------------------------------------------------------------------------

/// User profile loaded from confbridge.conf.
///
/// Defines the behavior and permissions of a user joining a conference.
#[derive(Debug, Clone)]
pub struct UserProfile {
    /// Profile name
    pub name: String,
    /// Whether this user is an admin
    pub admin: bool,
    /// Whether this user is a marked user
    pub marked: bool,
    /// Start the user muted
    pub start_muted: bool,
    /// Play music-on-hold when only one person is in the conference
    pub music_on_hold_when_empty: bool,
    /// Announce this user's join/leave to the conference
    pub announce_join_leave: bool,
    /// Announce the user count when this user joins
    pub announce_user_count: bool,
    /// Quiet mode: suppress all announcements for this user
    pub quiet: bool,
    /// Wait for a marked user before hearing conference audio
    pub wait_marked: bool,
    /// Leave when the last marked user leaves
    pub end_marked: bool,
    /// PIN required to join (None = no PIN required)
    pub pin: Option<String>,
    /// Timeout after which the user is automatically removed (0 = no timeout)
    pub timeout: Duration,
    /// Pass DTMF through to the bridge (instead of interpreting as menu)
    pub dtmf_passthrough: bool,
    /// Enable noise reduction / denoise filter
    pub denoise: bool,
    /// Generate talk detection events (ConfbridgeTalking)
    pub talk_detection_events: bool,
    /// Talker optimization: mute audio from non-talking participants in the mix
    pub talker_optimization: bool,
    /// Announce user count on join (threshold: only if >= this many)
    pub announce_user_count_threshold: u32,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            name: "default_user".to_string(),
            admin: false,
            marked: false,
            start_muted: false,
            music_on_hold_when_empty: false,
            announce_join_leave: false,
            announce_user_count: false,
            quiet: false,
            wait_marked: false,
            end_marked: false,
            pin: None,
            timeout: Duration::ZERO,
            dtmf_passthrough: false,
            denoise: false,
            talk_detection_events: false,
            talker_optimization: false,
            announce_user_count_threshold: 1,
        }
    }
}

impl UserProfile {
    /// Parse a user profile from key=value pairs.
    pub fn from_config(name: &str, config: &HashMap<String, String>) -> Self {
        let mut profile = Self::default();
        profile.name = name.to_string();

        for (key, value) in config {
            match key.as_str() {
                "admin" => profile.admin = is_true(value),
                "marked" => profile.marked = is_true(value),
                "startmuted" => profile.start_muted = is_true(value),
                "music_on_hold_when_empty" => {
                    profile.music_on_hold_when_empty = is_true(value)
                }
                "announce_join_leave" => profile.announce_join_leave = is_true(value),
                "announce_user_count" => profile.announce_user_count = is_true(value),
                "quiet" => profile.quiet = is_true(value),
                "wait_marked" => profile.wait_marked = is_true(value),
                "end_marked" => profile.end_marked = is_true(value),
                "pin" => {
                    if !value.is_empty() {
                        profile.pin = Some(value.clone());
                    }
                }
                "timeout" => {
                    if let Ok(secs) = value.parse::<u64>() {
                        profile.timeout = Duration::from_secs(secs);
                    }
                }
                "dtmf_passthrough" => profile.dtmf_passthrough = is_true(value),
                "denoise" | "dsp_drop_silence" => profile.denoise = is_true(value),
                "talk_detection_events" => profile.talk_detection_events = is_true(value),
                "talker_optimization" => profile.talker_optimization = is_true(value),
                _ => {
                    debug!("ConfBridge: unknown user profile option '{}'", key);
                }
            }
        }

        profile
    }
}

// ---------------------------------------------------------------------------
// Bridge Profile (confbridge.conf [bridge_profile])
// ---------------------------------------------------------------------------

/// Bridge profile loaded from confbridge.conf.
///
/// Defines the technical characteristics of the conference bridge.
#[derive(Debug, Clone)]
pub struct BridgeProfile {
    /// Profile name
    pub name: String,
    /// Maximum number of participants (0 = unlimited)
    pub max_members: u32,
    /// Whether to record the conference
    pub record_conference: bool,
    /// Recording file path (if recording)
    pub record_file: Option<String>,
    /// Internal sample rate: auto, 8000, 16000, 32000, 44100, 48000
    pub internal_sample_rate: SampleRate,
    /// Mixing interval in milliseconds: 10, 20, 40, 80
    pub mixing_interval: u32,
    /// Video mode for the bridge
    pub video_mode: ConferenceVideoMode,
    /// Reference to a sound set (custom sounds)
    pub sound_set: String,
    /// Language for announcements
    pub language: String,
    /// Maximum duration for the entire conference (0 = unlimited)
    pub max_duration: Duration,
    /// Play music on hold when empty
    pub music_on_hold_when_empty: bool,
    /// Music class
    pub music_class: String,
    /// Enable binaural audio processing
    pub binaural_active: bool,
}

/// Sample rate setting for the conference bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleRate {
    Auto,
    Rate8000,
    Rate16000,
    Rate32000,
    Rate44100,
    Rate48000,
}

impl Default for SampleRate {
    fn default() -> Self {
        Self::Auto
    }
}

impl SampleRate {
    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "auto" => Self::Auto,
            "8000" => Self::Rate8000,
            "16000" => Self::Rate16000,
            "32000" => Self::Rate32000,
            "44100" => Self::Rate44100,
            "48000" => Self::Rate48000,
            _ => Self::Auto,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Auto => None,
            Self::Rate8000 => Some(8000),
            Self::Rate16000 => Some(16000),
            Self::Rate32000 => Some(32000),
            Self::Rate44100 => Some(44100),
            Self::Rate48000 => Some(48000),
        }
    }
}

impl Default for BridgeProfile {
    fn default() -> Self {
        Self {
            name: "default_bridge".to_string(),
            max_members: 0,
            record_conference: false,
            record_file: None,
            internal_sample_rate: SampleRate::Auto,
            mixing_interval: 20,
            video_mode: ConferenceVideoMode::None,
            sound_set: "default".to_string(),
            language: "en".to_string(),
            max_duration: Duration::ZERO,
            music_on_hold_when_empty: false,
            music_class: "default".to_string(),
            binaural_active: false,
        }
    }
}

impl BridgeProfile {
    /// Parse a bridge profile from key=value pairs.
    pub fn from_config(name: &str, config: &HashMap<String, String>) -> Self {
        let mut profile = Self::default();
        profile.name = name.to_string();

        for (key, value) in config {
            match key.as_str() {
                "max_members" => {
                    if let Ok(n) = value.parse() {
                        profile.max_members = n;
                    }
                }
                "record_conference" => profile.record_conference = is_true(value),
                "record_file" => profile.record_file = Some(value.clone()),
                "internal_sample_rate" => {
                    profile.internal_sample_rate = SampleRate::from_str_name(value)
                }
                "mixing_interval" => {
                    if let Ok(n) = value.parse::<u32>() {
                        if matches!(n, 10 | 20 | 40 | 80) {
                            profile.mixing_interval = n;
                        } else {
                            warn!(
                                "ConfBridge: invalid mixing_interval '{}', must be 10/20/40/80, keeping default",
                                n
                            );
                        }
                    }
                }
                "video_mode" => profile.video_mode = ConferenceVideoMode::from_str_name(value),
                "sound_set" | "sounds" => profile.sound_set = value.clone(),
                "language" => profile.language = value.clone(),
                "max_duration" => {
                    if let Ok(secs) = value.parse::<u64>() {
                        profile.max_duration = Duration::from_secs(secs);
                    }
                }
                _ => {
                    debug!("ConfBridge: unknown bridge profile option '{}'", key);
                }
            }
        }

        profile
    }

    /// Validate the mixing interval.
    pub fn validate_mixing_interval(ms: u32) -> bool {
        matches!(ms, 10 | 20 | 40 | 80)
    }
}

// ---------------------------------------------------------------------------
// Menu System (confbridge.conf [menu])
// ---------------------------------------------------------------------------

/// DTMF menu actions that can be triggered in a conference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Toggle mute for the user
    ToggleMute,
    /// Toggle deaf for the user
    ToggleDeaf,
    /// Increase listening volume
    IncreaseVolume,
    /// Decrease listening volume
    DecreaseVolume,
    /// Admin: kick the last user who joined
    AdminKickLast,
    /// Admin: toggle conference lock
    AdminToggleLock,
    /// Leave the conference
    LeaveConference,
    /// Admin: toggle mute on all non-admin participants
    AdminToggleMuteParticipants,
    /// Set this user as the single video source
    SetAsSingleVideoSrc,
    /// Release this user as the single video source
    ReleaseAsSingleVideoSrc,
    /// No operation (ignore this DTMF)
    NoOp,
    /// Play a sound file
    Playback(String),
    /// Dial a number (extension@context)
    Dial(String),
    /// Admin: kick all non-admin participants
    AdminKickAll,
}

impl MenuAction {
    pub fn from_str_name(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.starts_with("playback(") && s.ends_with(')') {
            let file = &s[9..s.len() - 1];
            return Some(Self::Playback(file.to_string()));
        }
        if s.starts_with("dialplan_exec(") && s.ends_with(')') {
            let target = &s[14..s.len() - 1];
            return Some(Self::Dial(target.to_string()));
        }
        match s {
            "toggle_mute" => Some(Self::ToggleMute),
            "toggle_deaf" | "toggle_deaf_mute" => Some(Self::ToggleDeaf),
            "increase_listening_volume" | "increase_volume" => Some(Self::IncreaseVolume),
            "decrease_listening_volume" | "decrease_volume" => Some(Self::DecreaseVolume),
            "admin_kick_last" => Some(Self::AdminKickLast),
            "admin_toggle_lock" | "admin_toggle_conference_lock" => Some(Self::AdminToggleLock),
            "leave_conference" => Some(Self::LeaveConference),
            "admin_toggle_mute_participants" => Some(Self::AdminToggleMuteParticipants),
            "set_as_single_video_src" => Some(Self::SetAsSingleVideoSrc),
            "release_as_single_video_src" => Some(Self::ReleaseAsSingleVideoSrc),
            "no_op" | "noop" => Some(Self::NoOp),
            "admin_kick_all" => Some(Self::AdminKickAll),
            _ => None,
        }
    }
}

/// A single menu entry mapping a DTMF sequence to actions.
#[derive(Debug, Clone)]
pub struct MenuEntry {
    /// The DTMF digit sequence (e.g., "1", "**", "99")
    pub dtmf: String,
    /// Actions to execute when this DTMF sequence is matched
    pub actions: Vec<MenuAction>,
}

/// A conference DTMF menu.
#[derive(Debug, Clone)]
pub struct ConfMenu {
    /// Menu name
    pub name: String,
    /// Menu entries mapping DTMF sequences to actions
    pub entries: Vec<MenuEntry>,
    /// Timeout for multi-digit sequences (milliseconds)
    pub dtmf_timeout_ms: u64,
}

impl Default for ConfMenu {
    fn default() -> Self {
        Self {
            name: "default_menu".to_string(),
            entries: vec![
                MenuEntry {
                    dtmf: "1".to_string(),
                    actions: vec![MenuAction::ToggleMute],
                },
                MenuEntry {
                    dtmf: "2".to_string(),
                    actions: vec![MenuAction::ToggleDeaf],
                },
                MenuEntry {
                    dtmf: "3".to_string(),
                    actions: vec![MenuAction::IncreaseVolume],
                },
                MenuEntry {
                    dtmf: "4".to_string(),
                    actions: vec![MenuAction::DecreaseVolume],
                },
                MenuEntry {
                    dtmf: "*1".to_string(),
                    actions: vec![MenuAction::AdminToggleLock],
                },
                MenuEntry {
                    dtmf: "*2".to_string(),
                    actions: vec![MenuAction::AdminKickLast],
                },
                MenuEntry {
                    dtmf: "*3".to_string(),
                    actions: vec![MenuAction::AdminToggleMuteParticipants],
                },
            ],
            dtmf_timeout_ms: 2000,
        }
    }
}

impl ConfMenu {
    /// Create a menu from configuration key-value pairs.
    /// Keys are DTMF sequences, values are comma-separated action names.
    pub fn from_config(name: &str, config: &HashMap<String, String>) -> Self {
        let mut menu = Self {
            name: name.to_string(),
            entries: Vec::new(),
            dtmf_timeout_ms: 2000,
        };

        for (dtmf, actions_str) in config {
            if dtmf == "type" || dtmf == "template" {
                continue;
            }
            let actions: Vec<MenuAction> = actions_str
                .split(',')
                .filter_map(|s| MenuAction::from_str_name(s.trim()))
                .collect();
            if !actions.is_empty() {
                menu.entries.push(MenuEntry {
                    dtmf: dtmf.clone(),
                    actions,
                });
            }
        }

        menu
    }

    /// Look up the action(s) for a DTMF sequence.
    pub fn find_entry(&self, dtmf: &str) -> Option<&MenuEntry> {
        self.entries.iter().find(|e| e.dtmf == dtmf)
    }

    /// Check if any entry starts with the given prefix (for multi-digit matching).
    pub fn has_prefix(&self, prefix: &str) -> bool {
        self.entries.iter().any(|e| e.dtmf.starts_with(prefix) && e.dtmf != prefix)
    }
}

// ---------------------------------------------------------------------------
// Conference Sounds
// ---------------------------------------------------------------------------

/// Conference sound file names (customizable per bridge profile).
#[derive(Debug, Clone)]
pub struct ConferenceSounds {
    pub has_joined: String,
    pub has_left: String,
    pub kicked: String,
    pub muted: String,
    pub unmuted: String,
    pub only_one: String,
    pub there_are: String,
    pub other_in_party: String,
    pub place_in_conf: String,
    pub wait_for_leader: String,
    pub leader_has_left: String,
    pub get_pin: String,
    pub invalid_pin: String,
    pub only_person: String,
    pub locked: String,
    pub locked_now: String,
    pub unlocked_now: String,
    pub error_menu: String,
    pub join: String,
    pub leave: String,
    pub participants_muted: String,
    pub participants_unmuted: String,
    pub begin: String,
}

impl Default for ConferenceSounds {
    fn default() -> Self {
        Self {
            has_joined: "conf-hasjoin".to_string(),
            has_left: "conf-hasleft".to_string(),
            kicked: "conf-kicked".to_string(),
            muted: "conf-muted".to_string(),
            unmuted: "conf-unmuted".to_string(),
            only_one: "conf-onlyone".to_string(),
            there_are: "conf-thereare".to_string(),
            other_in_party: "conf-otherinparty".to_string(),
            place_in_conf: "conf-placeintoconf".to_string(),
            wait_for_leader: "conf-waitforleader".to_string(),
            leader_has_left: "conf-leaderhasleft".to_string(),
            get_pin: "conf-getpin".to_string(),
            invalid_pin: "conf-invalidpin".to_string(),
            only_person: "conf-onlyperson".to_string(),
            locked: "conf-locked".to_string(),
            locked_now: "conf-lockednow".to_string(),
            unlocked_now: "conf-unlockednow".to_string(),
            error_menu: "conf-errormenu".to_string(),
            join: "confbridge-join".to_string(),
            leave: "confbridge-leave".to_string(),
            participants_muted: "conf-now-muted".to_string(),
            participants_unmuted: "conf-now-unmuted".to_string(),
            begin: "confbridge-conf-begin".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// AMI Events
// ---------------------------------------------------------------------------

/// AMI events generated by the ConfBridge application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfBridgeEvent {
    /// Conference created (first participant joined)
    ConfbridgeStart {
        conference: String,
    },
    /// Conference destroyed (last participant left)
    ConfbridgeEnd {
        conference: String,
    },
    /// A participant joined the conference
    ConfbridgeJoin {
        conference: String,
        channel: String,
        admin: bool,
        marked: bool,
    },
    /// A participant left the conference
    ConfbridgeLeave {
        conference: String,
        channel: String,
        admin: bool,
        marked: bool,
    },
    /// A participant started or stopped talking
    ConfbridgeTalking {
        conference: String,
        channel: String,
        talking: bool,
    },
    /// A participant was muted
    ConfbridgeMute {
        conference: String,
        channel: String,
    },
    /// A participant was unmuted
    ConfbridgeUnmute {
        conference: String,
        channel: String,
    },
}

impl ConfBridgeEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::ConfbridgeStart { .. } => "ConfbridgeStart",
            Self::ConfbridgeEnd { .. } => "ConfbridgeEnd",
            Self::ConfbridgeJoin { .. } => "ConfbridgeJoin",
            Self::ConfbridgeLeave { .. } => "ConfbridgeLeave",
            Self::ConfbridgeTalking { .. } => "ConfbridgeTalking",
            Self::ConfbridgeMute { .. } => "ConfbridgeMute",
            Self::ConfbridgeUnmute { .. } => "ConfbridgeUnmute",
        }
    }
}

/// In-memory event log for AMI events (in production, these go to AMI subscribers).
static EVENT_LOG: once_cell::sync::Lazy<RwLock<Vec<ConfBridgeEvent>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(Vec::new()));

fn emit_event(event: ConfBridgeEvent) {
    info!("ConfBridge AMI: {}", event.event_name());
    EVENT_LOG.write().push(event);
}

/// Get all emitted events (for testing).
pub fn get_events() -> Vec<ConfBridgeEvent> {
    EVENT_LOG.read().clone()
}

// ---------------------------------------------------------------------------
// Video Stream Info (for SFU)
// ---------------------------------------------------------------------------

/// Info about a participant's video stream, extracted from their SDP offer.
#[derive(Debug, Clone)]
pub struct VideoStreamInfo {
    /// RTP payload type number (e.g. 34 for H263, 96 for H264)
    pub payload_type: u8,
    /// Codec name (e.g. "H263", "H264")
    pub codec_name: String,
    /// Sample rate (typically 90000 for video)
    pub sample_rate: u32,
}

// ---------------------------------------------------------------------------
// Conference User
// ---------------------------------------------------------------------------

/// A participant in a conference.
#[derive(Debug, Clone)]
pub struct ConferenceUser {
    /// The channel ID of the participant
    pub channel_id: ChannelId,
    /// The channel name for display
    pub channel_name: String,
    /// Whether this user is an administrator
    pub is_admin: bool,
    /// Whether this user is a marked user (conference waits for them)
    pub is_marked: bool,
    /// Whether this user is currently muted
    pub muted: bool,
    /// Whether this user can hear other participants
    pub deaf: bool,
    /// Whether this user is currently talking
    pub talking: bool,
    /// Whether this user is waiting for a marked user
    pub waiting: bool,
    /// When this user joined the conference
    pub join_time: Instant,
    /// CallerID name for announcements
    pub caller_name: Option<String>,
    /// CallerID number for announcements
    pub caller_number: Option<String>,
    /// This user's listening volume adjustment (-4 to 4)
    pub volume_adjustment: i32,
    /// This user's talking volume adjustment (-4 to 4)
    pub talking_volume_adjustment: i32,
    /// User profile that was applied
    pub profile_name: String,
    /// Menu profile name
    pub menu_name: String,
    /// SIP Call-ID (for SFU re-INVITE routing).
    pub sip_call_id: Option<String>,
    /// Video codec info from the participant's SDP offer (media_type, payload_type, codec_name, sample_rate).
    pub video_streams: Vec<VideoStreamInfo>,
}

// ---------------------------------------------------------------------------
// Conference Settings (merged from bridge profile)
// ---------------------------------------------------------------------------

/// Conference settings (runtime, derived from bridge profile).
#[derive(Debug, Clone)]
pub struct ConferenceSettings {
    /// Maximum number of participants (0 = unlimited)
    pub max_members: usize,
    /// Play music on hold when only one participant is present
    pub music_on_hold_when_empty: bool,
    /// Music class to use for hold music
    pub music_class: String,
    /// Announce user count on join
    pub announce_user_count: bool,
    /// Announce join/leave of users
    pub announce_join_leave: bool,
    /// Wait for a marked user before starting the conference
    pub wait_for_marked: bool,
    /// End the conference when the last marked user leaves
    pub end_when_marked_leaves: bool,
    /// Bridge profile name
    pub bridge_profile: String,
    /// Video mode
    pub video_mode: ConferenceVideoMode,
    /// Internal sample rate
    pub internal_sample_rate: SampleRate,
    /// Mixing interval (ms)
    pub mixing_interval: u32,
    /// Record the conference
    pub record_conference: bool,
    /// Recording file
    pub record_file: Option<String>,
    /// Conference sounds
    pub sounds: ConferenceSounds,
}

impl Default for ConferenceSettings {
    fn default() -> Self {
        Self {
            max_members: 0,
            music_on_hold_when_empty: false,
            music_class: "default".to_string(),
            announce_user_count: false,
            announce_join_leave: false,
            wait_for_marked: false,
            end_when_marked_leaves: false,
            bridge_profile: "default_bridge".to_string(),
            video_mode: ConferenceVideoMode::None,
            internal_sample_rate: SampleRate::Auto,
            mixing_interval: 20,
            record_conference: false,
            record_file: None,
            sounds: ConferenceSounds::default(),
        }
    }
}

impl ConferenceSettings {
    /// Create settings from a bridge profile.
    pub fn from_bridge_profile(bp: &BridgeProfile) -> Self {
        Self {
            max_members: bp.max_members as usize,
            music_on_hold_when_empty: bp.music_on_hold_when_empty,
            music_class: bp.music_class.clone(),
            bridge_profile: bp.name.clone(),
            video_mode: bp.video_mode,
            internal_sample_rate: bp.internal_sample_rate,
            mixing_interval: bp.mixing_interval,
            record_conference: bp.record_conference,
            record_file: bp.record_file.clone(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Conference
// ---------------------------------------------------------------------------

/// A conference bridge instance.
#[derive(Debug)]
pub struct Conference {
    /// Unique conference ID
    pub id: String,
    /// Conference name (as specified in the dialplan)
    pub name: String,
    /// The underlying mixing bridge
    pub bridge: Bridge,
    /// Active participants
    pub participants: HashMap<ChannelId, ConferenceUser>,
    /// Conference settings
    pub settings: ConferenceSettings,
    /// When the conference was created
    pub created: Instant,
    /// Whether the conference is locked (no new participants)
    pub locked: bool,
    /// Whether the conference is muted (all non-admin users muted)
    pub all_muted: bool,
    /// Whether the conference is actively running (a marked user is present)
    pub active: bool,
    /// Number of marked users currently in the conference
    pub marked_count: usize,
    /// The channel ID of the last user who joined (for admin_kick_last)
    pub last_joined: Option<ChannelId>,
    /// Whether recording is active
    pub recording: bool,
}

impl Conference {
    /// Count of admin users.
    pub fn admin_count(&self) -> usize {
        self.participants.values().filter(|u| u.is_admin).count()
    }

    /// Count of marked users.
    pub fn marked_user_count(&self) -> usize {
        self.participants.values().filter(|u| u.is_marked).count()
    }

    /// Count of waiting users (waiting for marked).
    pub fn waiting_count(&self) -> usize {
        self.participants.values().filter(|u| u.waiting).count()
    }

    /// Count of active (non-waiting) users.
    pub fn active_count(&self) -> usize {
        self.participants.values().filter(|u| !u.waiting).count()
    }
}

// ---------------------------------------------------------------------------
// ConfBridgeResult
// ---------------------------------------------------------------------------

/// Result of joining a conference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfBridgeResult {
    /// Exited normally (hung up)
    Hangup,
    /// Kicked by admin
    Kicked,
    /// Conference ended (marked user left)
    EndMarked,
    /// Left via DTMF menu
    Dtmf,
    /// Timeout reached
    Timeout,
    /// Error joining
    Failed,
}

impl ConfBridgeResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hangup => "HANGUP",
            Self::Kicked => "KICKED",
            Self::EndMarked => "ENDMARKED",
            Self::Dtmf => "DTMF",
            Self::Timeout => "TIMEOUT",
            Self::Failed => "FAILED",
        }
    }
}

// ---------------------------------------------------------------------------
// AppConfBridge
// ---------------------------------------------------------------------------

/// The ConfBridge() dialplan application.
///
/// Usage: ConfBridge(conference_name[,bridge_profile[,user_profile[,menu]]])
pub struct AppConfBridge;

impl DialplanApp for AppConfBridge {
    fn name(&self) -> &str {
        "ConfBridge"
    }

    fn description(&self) -> &str {
        "Conference bridge application"
    }
}

impl AppConfBridge {
    /// Execute the ConfBridge application.
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, ConfBridgeResult) {
        let parts: Vec<&str> = args.splitn(4, ',').collect();

        let conf_name = match parts.first() {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => {
                warn!("ConfBridge: conference name is required");
                channel
                    .variables
                    .insert("CONFBRIDGE_RESULT".to_string(), "FAILED".to_string());
                return (PbxExecResult::Failed, ConfBridgeResult::Failed);
            }
        };

        // Look up profiles
        let bridge_profile_name = parts
            .get(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("default_bridge");
        let user_profile_name = parts
            .get(2)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("default_user");
        let menu_name = parts
            .get(3)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("default_menu");

        // Load user profile
        let user_profile = USER_PROFILES
            .get(user_profile_name)
            .map(|p| p.value().clone())
            .unwrap_or_else(|| {
                // Fall back to parsing from the profile name string
                Self::parse_user_profile(user_profile_name)
            });

        // Check PIN
        if let Some(ref pin) = user_profile.pin {
            // In production: prompt for PIN via DTMF, compare
            info!(
                "ConfBridge: would prompt for PIN for conference '{}'",
                conf_name
            );
            let _expected_pin = pin;
            // If PIN fails: return (PbxExecResult::Failed, ConfBridgeResult::Failed);
        }

        info!(
            "ConfBridge: channel '{}' joining conference '{}' (user_profile={}, bridge_profile={}, menu={})",
            channel.name, conf_name, user_profile_name, bridge_profile_name, menu_name
        );

        // Get or create the conference
        let conference = Self::get_or_create_conference(&conf_name, bridge_profile_name);

        // Check for CONFBRIDGE(bridge,video_mode) channel variable override
        // (set by Set(CONFBRIDGE(bridge,video_mode)=sfu) in dialplan).
        if let Some(vm) = channel.variables.get("CONFBRIDGE(bridge,video_mode)") {
            let mode = ConferenceVideoMode::from_str_name(vm);
            let mut conf = conference.write();
            conf.settings.video_mode = mode;
        }

        // Extract SIP Call-ID and video stream info for SFU
        let sip_call_id = channel.variables.get("__SIP_CALL_ID").cloned();
        let video_streams = Self::extract_video_streams_for_channel(&sip_call_id);

        // Create the conference user
        let conf_user = ConferenceUser {
            channel_id: channel.unique_id.clone(),
            channel_name: channel.name.clone(),
            is_admin: user_profile.admin,
            is_marked: user_profile.marked,
            muted: user_profile.start_muted,
            deaf: false,
            talking: false,
            waiting: false,
            join_time: Instant::now(),
            caller_name: Some(channel.caller.id.name.name.clone()).filter(|s| !s.is_empty()),
            caller_number: Some(channel.caller.id.number.number.clone()).filter(|s| !s.is_empty()),
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: user_profile_name.to_string(),
            menu_name: menu_name.to_string(),
            sip_call_id,
            video_streams,
        };

        // Join the conference
        let join_result = Self::join_conference(&conference, conf_user, channel).await;

        match join_result {
            Ok(result) => {
                Self::leave_conference(&conference, &channel.unique_id, &conf_name);

                channel
                    .variables
                    .insert("CONFBRIDGE_RESULT".to_string(), result.as_str().to_string());

                let exec_result = match result {
                    ConfBridgeResult::Hangup => PbxExecResult::Hangup,
                    ConfBridgeResult::Failed => PbxExecResult::Failed,
                    _ => PbxExecResult::Success,
                };

                (exec_result, result)
            }
            Err(e) => {
                warn!(
                    "ConfBridge: error joining conference '{}': {}",
                    conf_name, e
                );
                channel
                    .variables
                    .insert("CONFBRIDGE_RESULT".to_string(), "FAILED".to_string());
                (PbxExecResult::Failed, ConfBridgeResult::Failed)
            }
        }
    }

    /// Get an existing conference or create a new one.
    fn get_or_create_conference(
        name: &str,
        bridge_profile_name: &str,
    ) -> Arc<RwLock<Conference>> {
        if let Some(conf) = CONFERENCES.get(name) {
            return conf.value().clone();
        }

        // Load bridge profile
        let bridge_profile = BRIDGE_PROFILES
            .get(bridge_profile_name)
            .map(|p| p.value().clone())
            .unwrap_or_default();

        let settings = ConferenceSettings::from_bridge_profile(&bridge_profile);

        let conference = Conference {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            bridge: Bridge::new(format!("confbridge-{}", name)),
            participants: HashMap::new(),
            settings,
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };

        let conf = Arc::new(RwLock::new(conference));
        CONFERENCES.insert(name.to_string(), conf.clone());

        // Emit ConfbridgeStart event
        emit_event(ConfBridgeEvent::ConfbridgeStart {
            conference: name.to_string(),
        });

        info!("ConfBridge: created new conference '{}'", name);
        conf
    }

    /// Extract video stream information from the SIP session's remote SDP.
    fn extract_video_streams_for_channel(sip_call_id: &Option<String>) -> Vec<VideoStreamInfo> {
        let call_id = match sip_call_id {
            Some(id) => id,
            None => return Vec::new(),
        };

        let handler = match asterisk_sip::get_global_event_handler() {
            Some(h) => h,
            None => return Vec::new(),
        };

        let remote_sdp = match handler.get_remote_sdp(call_id) {
            Some(sdp) => sdp,
            None => return Vec::new(),
        };

        let mut streams = Vec::new();
        for media in &remote_sdp.media_descriptions {
            if media.media_type == "video" && media.port > 0 {
                for &pt in &media.formats {
                    if let Some(rtpmap) = media.get_rtpmap(pt) {
                        // rtpmap value: "34 H263/90000"
                        let parts: Vec<&str> = rtpmap.splitn(2, ' ').collect();
                        if parts.len() == 2 {
                            let codec_parts: Vec<&str> = parts[1].split('/').collect();
                            let codec_name = codec_parts[0].to_string();
                            let sample_rate = codec_parts.get(1)
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(90000);
                            streams.push(VideoStreamInfo {
                                payload_type: pt,
                                codec_name,
                                sample_rate,
                            });
                        }
                    }
                }
            }
        }
        streams
    }

    /// Build an SDP for a re-INVITE in SFU mode.
    ///
    /// The SDP contains:
    /// 1. The participant's own audio m= line
    /// 2. The participant's own video m= line
    /// 3. One video m= line for each other participant's video stream
    ///
    /// `departed_streams` (if any) get port=0 to signal removal.
    fn build_sfu_sdp(
        own_sdp: &asterisk_sip::sdp::SessionDescription,
        other_video_streams: &[(VideoStreamInfo, bool)], // (stream, is_departed)
        local_addr: &str,
    ) -> asterisk_sip::sdp::SessionDescription {
        use asterisk_sip::sdp::{SessionDescription, Origin, ConnectionData, MediaDescription, MediaDirection};

        let mut sdp = SessionDescription {
            version: 0,
            origin: Origin {
                username: "asterisk".to_string(),
                session_id: "1".to_string(),
                session_version: "1".to_string(),
                net_type: "IN".to_string(),
                addr_type: "IP4".to_string(),
                addr: local_addr.to_string(),
            },
            session_name: "Asterisk".to_string(),
            connection: Some(ConnectionData {
                net_type: "IN".to_string(),
                addr_type: "IP4".to_string(),
                addr: local_addr.to_string(),
            }),
            time: (0, 0),
            media_descriptions: Vec::new(),
            attributes: Vec::new(),
        };

        // Copy the participant's own media lines from their SDP answer (local_sdp).
        for media in &own_sdp.media_descriptions {
            sdp.media_descriptions.push(media.clone());
        }

        // Assign unique payload types for other participants' video streams.
        // We need to avoid colliding with payload types already used in the SDP.
        let mut used_pts: std::collections::HashSet<u8> = std::collections::HashSet::new();
        for media in &own_sdp.media_descriptions {
            for &pt in &media.formats {
                used_pts.insert(pt);
            }
        }

        // Add video m= lines for other participants' video streams.
        // Always assign new dynamic PTs (starting from 99) for additional streams,
        // matching Asterisk's PJSIP SFU behavior.  Use well-known (static) PTs
        // (< 96) as-is if they don't collide.
        let mut next_dynamic_pt: u8 = 99;
        for (stream, is_departed) in other_video_streams {
            let pt = if stream.payload_type < 96 && !used_pts.contains(&stream.payload_type) {
                // Static/well-known PT (e.g. H263=34) — use as-is.
                stream.payload_type
            } else {
                // Dynamic codec — assign from pool.
                while used_pts.contains(&next_dynamic_pt) && next_dynamic_pt < 127 {
                    next_dynamic_pt += 1;
                }
                let pt = next_dynamic_pt;
                next_dynamic_pt += 1;
                pt
            };
            used_pts.insert(pt);

            let port = if *is_departed { 0 } else { 4050 };

            let mut attrs = vec![
                ("rtpmap".to_string(), Some(format!("{} {}/{}", pt, stream.codec_name, stream.sample_rate))),
            ];
            if !*is_departed {
                attrs.push(("sendrecv".to_string(), None));
            }

            sdp.media_descriptions.push(MediaDescription {
                media_type: "video".to_string(),
                port,
                protocol: "RTP/AVP".to_string(),
                formats: vec![pt],
                connection: None,
                attributes: attrs,
                direction: if *is_departed { MediaDirection::Inactive } else { MediaDirection::SendRecv },
                fingerprint: None,
                setup: None,
                rtcp_mux: false,
                ice_candidates: Vec::new(),
                bandwidth: Vec::new(),
            });
        }

        sdp
    }

    /// Send SFU re-INVITEs to all participants in a conference.
    ///
    /// For each participant, build an SDP with their own media lines plus
    /// video streams from all other participants.
    async fn send_sfu_reinvites(
        conference: &Arc<RwLock<Conference>>,
        departed_channel_id: Option<&ChannelId>,
        departed_streams: &[VideoStreamInfo],
    ) {
        let handler = match asterisk_sip::get_global_event_handler() {
            Some(h) => h,
            None => {
                warn!("ConfBridge SFU: no global SIP event handler available");
                return;
            }
        };

        // Collect participant info under the lock, then release it.
        let participants: Vec<(String, Option<String>, Vec<VideoStreamInfo>)> = {
            let conf = conference.read();
            conf.participants.values().map(|u| {
                (u.channel_id.0.clone(), u.sip_call_id.clone(), u.video_streams.clone())
            }).collect()
        };

        for (ch_id, sip_call_id, _own_streams) in &participants {
            let call_id = match sip_call_id {
                Some(id) => id,
                None => continue,
            };

            // Get the local SDP for this participant (the SDP answer we sent them).
            let local_sdp = match handler.get_initial_local_sdp(call_id) {
                Some(sdp) => sdp,
                None => {
                    warn!(call_id = %call_id, "ConfBridge SFU: no local SDP for participant");
                    continue;
                }
            };

            let local_addr = handler.local_addr_for_call(call_id)
                .unwrap_or_else(|| "127.0.0.1".to_string());

            // Collect video streams from all OTHER participants (active and departed).
            let mut other_streams: Vec<(VideoStreamInfo, bool)> = Vec::new();

            for (other_ch_id, _other_call_id, other_video) in &participants {
                if other_ch_id == ch_id {
                    continue; // Skip self
                }
                for vs in other_video {
                    other_streams.push((vs.clone(), false));
                }
            }

            // Add departed streams with is_departed=true.
            if let Some(departed_id) = departed_channel_id {
                // Only add if this participant is NOT the departed one.
                if ch_id != &departed_id.0 {
                    for vs in departed_streams {
                        other_streams.push((vs.clone(), true));
                    }
                }
            }

            if other_streams.is_empty() {
                continue; // No other video streams to add
            }

            let sdp = Self::build_sfu_sdp(&local_sdp, &other_streams, &local_addr);
            let sent = handler.send_reinvite(call_id, sdp).await;
            if sent {
                info!(call_id = %call_id, "ConfBridge SFU: sent re-INVITE with {} other video streams",
                    other_streams.len());
            }
        }
    }

    /// Join a channel to a conference.
    async fn join_conference(
        conference: &Arc<RwLock<Conference>>,
        user: ConferenceUser,
        channel: &Channel,
    ) -> Result<ConfBridgeResult, String> {
        let is_admin = user.is_admin;
        let is_marked = user.is_marked;
        let my_channel_id = user.channel_id.clone();

        // Check if conference is locked
        {
            let conf = conference.read();
            if conf.locked && !is_admin {
                info!(
                    "ConfBridge: conference '{}' is locked, denying non-admin entry",
                    conf.name
                );
                return Ok(ConfBridgeResult::Failed);
            }

            // Check max members
            if conf.settings.max_members > 0
                && conf.participants.len() >= conf.settings.max_members
            {
                warn!(
                    "ConfBridge: conference '{}' is full ({} members)",
                    conf.name, conf.settings.max_members
                );
                return Ok(ConfBridgeResult::Failed);
            }
        }

        let conf_name;
        let user_count;
        let wait_for_marked;
        let is_sfu;

        // Add user to conference
        {
            let mut conf = conference.write();
            conf_name = conf.name.clone();
            is_sfu = conf.settings.video_mode == ConferenceVideoMode::Sfu;
            let channel_id = user.channel_id.clone();
            let channel_name = user.channel_name.clone();

            conf.bridge
                .add_channel(channel_id.clone(), channel.name.clone());
            conf.last_joined = Some(channel_id.clone());

            // Determine if this user should be waiting
            let mut user = user;
            wait_for_marked = conf.settings.wait_for_marked && conf.marked_count == 0 && !is_marked;
            if wait_for_marked {
                user.waiting = true;
                user.muted = true;
            }

            // If conference was all-muted and user is not admin, start muted
            if conf.all_muted && !is_admin {
                user.muted = true;
            }

            conf.participants.insert(channel_id, user);
            user_count = conf.participants.len();

            // Track marked users
            if is_marked {
                conf.marked_count += 1;
                if conf.marked_count == 1 && conf.settings.wait_for_marked {
                    info!(
                        "ConfBridge: marked user joined '{}', activating conference",
                        conf.name
                    );
                    conf.active = true;
                    for u in conf.participants.values_mut() {
                        if u.waiting {
                            u.waiting = false;
                            u.muted = false;
                        }
                    }
                }
            }

            info!(
                "ConfBridge: '{}' joined conference '{}' ({} participants, admin={}, marked={}, sfu={})",
                channel_name, conf.name, user_count, is_admin, is_marked, is_sfu
            );

            if conf.settings.announce_user_count && user_count > 1 {
                debug!(
                    "ConfBridge: there are now {} participants in '{}'",
                    user_count, conf.name
                );
            }

            if conf.settings.announce_join_leave {
                debug!(
                    "ConfBridge: announcing join of '{}' to conference '{}'",
                    channel_name, conf.name
                );
            }
        }

        // Emit AMI event
        emit_event(ConfBridgeEvent::ConfbridgeJoin {
            conference: conf_name.clone(),
            channel: channel.name.clone(),
            admin: is_admin,
            marked: is_marked,
        });

        if wait_for_marked {
            debug!(
                "ConfBridge: '{}' is waiting for marked user in '{}'",
                channel.name, conf_name
            );
        }

        // SFU mode: send re-INVITEs when a new participant joins
        if is_sfu && user_count > 1 {
            // Brief delay to allow the ACK for the initial INVITE to be processed.
            tokio::time::sleep(Duration::from_millis(200)).await;
            Self::send_sfu_reinvites(conference, None, &[]).await;
        }

        // Subscribe to SFU events and wait for hangup.
        let mut sfu_rx = SFU_EVENT_TX.subscribe();

        // Broadcast that we joined (so other participants can react).
        let _ = SFU_EVENT_TX.send(SfuEvent::ParticipantJoined {
            conference_name: conf_name.clone(),
            joined_channel_id: my_channel_id.clone(),
        });

        // Register a hangup notification for this channel.
        let hangup_notify = Arc::new(tokio::sync::Notify::new());
        let hangup_notify_clone = hangup_notify.clone();
        let my_uid = channel.unique_id.0.clone();
        asterisk_core::channel::register_hangup_callback(Box::new(move |uid, _cause| {
            if uid == my_uid {
                hangup_notify_clone.notify_one();
            }
        }));

        // Also detect softhangup by polling (for BYE-triggered hangup).
        let channel_name_for_poll = channel.name.clone();
        let my_sip_call_id = channel.variables.get("__SIP_CALL_ID").cloned();

        // Subscribe to SIP call hangup events (BYE received on the SIP side).
        let mut sip_hangup_rx = asterisk_sip::subscribe_sip_hangup();

        // Block in the conference until hangup or SFU event.
        let result = loop {
            tokio::select! {
                _ = hangup_notify.notified() => {
                    info!("ConfBridge: '{}' received hangup in conference '{}'", channel_name_for_poll, conf_name);
                    break ConfBridgeResult::Hangup;
                }
                sip_id = sip_hangup_rx.recv() => {
                    if let Ok(id) = sip_id {
                        if my_sip_call_id.as_deref() == Some(id.as_str()) {
                            info!("ConfBridge: '{}' SIP BYE received (call_id={})", channel_name_for_poll, id);
                            break ConfBridgeResult::Hangup;
                        }
                    }
                }
                event = sfu_rx.recv() => {
                    match event {
                        Ok(SfuEvent::ParticipantJoined { conference_name, joined_channel_id })
                            if conference_name == conf_name && joined_channel_id != my_channel_id =>
                        {
                            // Another participant joined — they will trigger the re-INVITEs
                            // from their own join_conference call above, so we just continue.
                            debug!("ConfBridge SFU: noticed join of {:?} in '{}'", joined_channel_id, conf_name);
                        }
                        Ok(SfuEvent::ParticipantLeft { conference_name, left_channel_id, departed_video_streams })
                            if conference_name == conf_name && left_channel_id != my_channel_id =>
                        {
                            info!("ConfBridge SFU: participant {:?} left '{}', sending re-INVITEs", left_channel_id, conf_name);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            Self::send_sfu_reinvites(conference, Some(&left_channel_id), &departed_video_streams).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            break ConfBridgeResult::Hangup;
                        }
                        _ => {
                            // Event for a different conference or from ourselves, ignore.
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    // Poll for softhangup
                    if let Some(ch_arc) = asterisk_core::channel::store::find_by_name(&channel_name_for_poll) {
                        let ch = ch_arc.lock();
                        if ch.check_hangup() {
                            info!("ConfBridge: '{}' softhangup detected in conference '{}'", channel_name_for_poll, conf_name);
                            break ConfBridgeResult::Hangup;
                        }
                    } else {
                        // Channel no longer exists
                        break ConfBridgeResult::Hangup;
                    }
                }
            }
        };

        Ok(result)
    }

    /// Remove a channel from a conference.
    fn leave_conference(
        conference: &Arc<RwLock<Conference>>,
        channel_id: &ChannelId,
        conf_name: &str,
    ) {
        let mut should_destroy = false;
        let mut end_marked = false;
        let mut departed_video_streams = Vec::new();
        let mut is_sfu = false;

        {
            let mut conf = conference.write();
            is_sfu = conf.settings.video_mode == ConferenceVideoMode::Sfu;
            let user = conf.participants.remove(channel_id);
            conf.bridge.remove_channel(channel_id);
            let remaining = conf.participants.len();

            // Update last_joined if the leaving user was the last to join
            if conf.last_joined.as_ref() == Some(channel_id) {
                conf.last_joined = conf.participants.values()
                    .max_by_key(|u| u.join_time)
                    .map(|u| u.channel_id.clone());
            }

            if let Some(ref user) = user {
                // Capture video streams for SFU departure notification.
                departed_video_streams = user.video_streams.clone();

                // Emit leave event
                emit_event(ConfBridgeEvent::ConfbridgeLeave {
                    conference: conf_name.to_string(),
                    channel: user.channel_name.clone(),
                    admin: user.is_admin,
                    marked: user.is_marked,
                });

                // Handle marked user leaving
                if user.is_marked {
                    conf.marked_count = conf.marked_count.saturating_sub(1);
                    if conf.marked_count == 0 && conf.settings.end_when_marked_leaves {
                        info!(
                            "ConfBridge: last marked user left '{}', ending conference for end_marked users",
                            conf_name
                        );
                        end_marked = true;
                    }
                    if conf.marked_count == 0 && conf.settings.wait_for_marked {
                        info!(
                            "ConfBridge: last marked user left '{}', muting waiting participants",
                            conf_name
                        );
                        conf.active = false;
                        for u in conf.participants.values_mut() {
                            u.waiting = true;
                            u.muted = true;
                        }
                    }
                }

                // Announce leave
                if conf.settings.announce_join_leave {
                    debug!(
                        "ConfBridge: announcing departure of '{}' from '{}'",
                        user.channel_name, conf_name
                    );
                }
            }

            info!(
                "ConfBridge: channel left conference '{}' ({} remaining)",
                conf_name, remaining
            );

            if remaining == 0 {
                should_destroy = true;
            }
        }

        // If end_marked, kick all end_marked participants
        if end_marked {
            let mut conf = conference.write();
            let to_kick: Vec<ChannelId> = conf
                .participants
                .iter()
                .filter(|(_, u)| !u.is_admin) // Or check u.end_marked from user profile
                .map(|(id, _)| id.clone())
                .collect();
            for id in &to_kick {
                conf.participants.remove(id);
                conf.bridge.remove_channel(id);
            }
            if conf.participants.is_empty() {
                should_destroy = true;
            }
        }

        // Broadcast SFU participant left event so remaining participants get re-INVITEd.
        if is_sfu && !departed_video_streams.is_empty() {
            let _ = SFU_EVENT_TX.send(SfuEvent::ParticipantLeft {
                conference_name: conf_name.to_string(),
                left_channel_id: channel_id.clone(),
                departed_video_streams,
            });
        }

        if should_destroy {
            CONFERENCES.remove(conf_name);
            emit_event(ConfBridgeEvent::ConfbridgeEnd {
                conference: conf_name.to_string(),
            });
            info!("ConfBridge: destroyed empty conference '{}'", conf_name);
        }
    }

    /// Parse a user profile string (backward compat).
    fn parse_user_profile(profile: &str) -> UserProfile {
        let mut up = UserProfile::default();
        let lower = profile.to_lowercase();
        if lower.contains("admin") {
            up.admin = true;
        }
        if lower.contains("marked") {
            up.marked = true;
        }
        if lower.contains("muted") {
            up.start_muted = true;
        }
        if lower.contains("wait_marked") {
            up.wait_marked = true;
        }
        if lower.contains("end_marked") {
            up.end_marked = true;
        }
        up
    }

    // -----------------------------------------------------------------------
    // Conference management (CLI/AMI)
    // -----------------------------------------------------------------------

    /// Mute a specific user in a conference.
    pub fn mute_user(conf_name: &str, channel_id: &ChannelId) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.get_mut(channel_id) {
                user.muted = true;
                let channel_name = user.channel_name.clone();
                emit_event(ConfBridgeEvent::ConfbridgeMute {
                    conference: conf_name.to_string(),
                    channel: channel_name.clone(),
                });
                info!(
                    "ConfBridge: muted '{}' in conference '{}'",
                    channel_name, conf_name
                );
                return true;
            }
        }
        false
    }

    /// Unmute a specific user in a conference.
    pub fn unmute_user(conf_name: &str, channel_id: &ChannelId) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.get_mut(channel_id) {
                user.muted = false;
                let channel_name = user.channel_name.clone();
                emit_event(ConfBridgeEvent::ConfbridgeUnmute {
                    conference: conf_name.to_string(),
                    channel: channel_name.clone(),
                });
                info!(
                    "ConfBridge: unmuted '{}' in conference '{}'",
                    channel_name, conf_name
                );
                return true;
            }
        }
        false
    }

    /// Toggle mute for a user.
    pub fn toggle_mute(conf_name: &str, channel_id: &ChannelId) -> Option<bool> {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.get_mut(channel_id) {
                user.muted = !user.muted;
                let muted = user.muted;
                let channel_name = user.channel_name.clone();
                if muted {
                    emit_event(ConfBridgeEvent::ConfbridgeMute {
                        conference: conf_name.to_string(),
                        channel: channel_name,
                    });
                } else {
                    emit_event(ConfBridgeEvent::ConfbridgeUnmute {
                        conference: conf_name.to_string(),
                        channel: channel_name,
                    });
                }
                return Some(muted);
            }
        }
        None
    }

    /// Toggle deaf for a user.
    pub fn toggle_deaf(conf_name: &str, channel_id: &ChannelId) -> Option<bool> {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.get_mut(channel_id) {
                user.deaf = !user.deaf;
                return Some(user.deaf);
            }
        }
        None
    }

    /// Adjust listening volume for a user.
    pub fn adjust_volume(conf_name: &str, channel_id: &ChannelId, delta: i32) -> Option<i32> {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.get_mut(channel_id) {
                user.volume_adjustment = (user.volume_adjustment + delta).clamp(-4, 4);
                return Some(user.volume_adjustment);
            }
        }
        None
    }

    /// Kick a user from a conference (admin function).
    ///
    /// Returns false if the user is not in the conference (already left),
    /// which is safe and does not panic.
    pub fn kick_user(conf_name: &str, channel_id: &ChannelId) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if let Some(user) = conf.participants.remove(channel_id) {
                conf.bridge.remove_channel(channel_id);
                emit_event(ConfBridgeEvent::ConfbridgeLeave {
                    conference: conf_name.to_string(),
                    channel: user.channel_name.clone(),
                    admin: user.is_admin,
                    marked: user.is_marked,
                });
                if user.is_marked {
                    conf.marked_count = conf.marked_count.saturating_sub(1);
                }
                // Update last_joined if the kicked user was the last to join
                if conf.last_joined.as_ref() == Some(channel_id) {
                    conf.last_joined = conf.participants.values()
                        .max_by_key(|u| u.join_time)
                        .map(|u| u.channel_id.clone());
                }
                info!(
                    "ConfBridge: kicked '{}' from conference '{}'",
                    user.channel_name, conf_name
                );
                return true;
            }
        }
        false
    }

    /// Kick all non-admin participants from a conference.
    pub fn kick_all_participants(conf_name: &str) -> usize {
        let mut kicked = 0;
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            let to_kick: Vec<ChannelId> = conf
                .participants
                .iter()
                .filter(|(_, u)| !u.is_admin)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &to_kick {
                if let Some(user) = conf.participants.remove(id) {
                    conf.bridge.remove_channel(id);
                    emit_event(ConfBridgeEvent::ConfbridgeLeave {
                        conference: conf_name.to_string(),
                        channel: user.channel_name.clone(),
                        admin: user.is_admin,
                        marked: user.is_marked,
                    });
                    if user.is_marked {
                        conf.marked_count = conf.marked_count.saturating_sub(1);
                    }
                    kicked += 1;
                }
            }
            if kicked > 0 {
                info!(
                    "ConfBridge: kicked {} participants from conference '{}'",
                    kicked, conf_name
                );
            }
        }
        kicked
    }

    /// Admin: kick the last user who joined.
    ///
    /// If the last-joined user has already left, this is a no-op that
    /// returns false (no panic, no stale state).
    pub fn kick_last(conf_name: &str) -> bool {
        let last_id = if let Some(conf) = CONFERENCES.get(conf_name) {
            let c = conf.read();
            // Only return the last_joined id if the user is still in the conference.
            c.last_joined.as_ref().and_then(|id| {
                if c.participants.contains_key(id) {
                    Some(id.clone())
                } else {
                    None
                }
            })
        } else {
            None
        };
        if let Some(id) = last_id {
            let result = Self::kick_user(conf_name, &id);
            // Update last_joined to the most recently joined remaining participant
            if result {
                if let Some(conf) = CONFERENCES.get(conf_name) {
                    let mut c = conf.write();
                    c.last_joined = c.participants.values()
                        .max_by_key(|u| u.join_time)
                        .map(|u| u.channel_id.clone());
                }
            }
            result
        } else {
            false
        }
    }

    /// Admin: toggle mute on all non-admin participants.
    pub fn toggle_mute_participants(conf_name: &str) -> Option<bool> {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            conf.all_muted = !conf.all_muted;
            let muted = conf.all_muted;
            for user in conf.participants.values_mut() {
                if !user.is_admin {
                    user.muted = muted;
                }
            }
            info!(
                "ConfBridge: {} all participants in conference '{}'",
                if muted { "muted" } else { "unmuted" },
                conf_name
            );
            return Some(muted);
        }
        None
    }

    /// Lock a conference (prevent new non-admin joins).
    pub fn lock_conference(conf_name: &str) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            conf.write().locked = true;
            info!("ConfBridge: locked conference '{}'", conf_name);
            return true;
        }
        false
    }

    /// Unlock a conference.
    pub fn unlock_conference(conf_name: &str) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            conf.write().locked = false;
            info!("ConfBridge: unlocked conference '{}'", conf_name);
            return true;
        }
        false
    }

    /// Toggle conference lock.
    pub fn toggle_lock(conf_name: &str) -> Option<bool> {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut c = conf.write();
            c.locked = !c.locked;
            let locked = c.locked;
            info!(
                "ConfBridge: conference '{}' {}",
                conf_name,
                if locked { "locked" } else { "unlocked" }
            );
            return Some(locked);
        }
        None
    }

    /// Start recording a conference.
    pub fn start_recording(conf_name: &str, file: Option<&str>) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if conf.recording {
                warn!("ConfBridge: conference '{}' is already recording", conf_name);
                return false;
            }
            conf.recording = true;
            let record_file = file
                .map(|s| s.to_string())
                .or_else(|| conf.settings.record_file.clone())
                .unwrap_or_else(|| format!("/var/spool/asterisk/confbridge-{}.wav", conf_name));
            info!(
                "ConfBridge: started recording conference '{}' to '{}'",
                conf_name, record_file
            );
            return true;
        }
        false
    }

    /// Stop recording a conference.
    pub fn stop_recording(conf_name: &str) -> bool {
        if let Some(conf) = CONFERENCES.get(conf_name) {
            let mut conf = conf.write();
            if !conf.recording {
                return false;
            }
            conf.recording = false;
            info!("ConfBridge: stopped recording conference '{}'", conf_name);
            return true;
        }
        false
    }

    // -----------------------------------------------------------------------
    // CLI commands
    // -----------------------------------------------------------------------

    /// CLI: confbridge list - list all active conferences.
    pub fn list_conferences() -> Vec<(String, usize)> {
        CONFERENCES
            .iter()
            .map(|entry| {
                let conf = entry.value().read();
                (conf.name.clone(), conf.participants.len())
            })
            .collect()
    }

    /// CLI: confbridge show <conference> - show conference details.
    pub fn show_conference(conf_name: &str) -> Option<ConferenceInfo> {
        CONFERENCES.get(conf_name).map(|conf| {
            let conf = conf.read();
            ConferenceInfo {
                name: conf.name.clone(),
                participant_count: conf.participants.len(),
                marked_count: conf.marked_count,
                locked: conf.locked,
                muted: conf.all_muted,
                recording: conf.recording,
                bridge_profile: conf.settings.bridge_profile.clone(),
            }
        })
    }

    /// CLI: confbridge show <conference> - get detailed participant list.
    pub fn list_participants(conf_name: &str) -> Option<Vec<ConferenceUser>> {
        CONFERENCES.get(conf_name).map(|conf| {
            let conf = conf.read();
            conf.participants.values().cloned().collect()
        })
    }

    // -----------------------------------------------------------------------
    // Profile management
    // -----------------------------------------------------------------------

    /// Register a user profile.
    pub fn register_user_profile(profile: UserProfile) {
        info!(
            "ConfBridge: registered user profile '{}'",
            profile.name
        );
        USER_PROFILES.insert(profile.name.clone(), profile);
    }

    /// Register a bridge profile.
    pub fn register_bridge_profile(profile: BridgeProfile) {
        info!(
            "ConfBridge: registered bridge profile '{}'",
            profile.name
        );
        BRIDGE_PROFILES.insert(profile.name.clone(), profile);
    }

    /// Register a menu.
    pub fn register_menu(menu: ConfMenu) {
        info!("ConfBridge: registered menu '{}'", menu.name);
        MENU_PROFILES.insert(menu.name.clone(), menu);
    }

    /// Execute a menu action for a user.
    pub fn execute_menu_action(
        conf_name: &str,
        channel_id: &ChannelId,
        action: &MenuAction,
    ) -> bool {
        match action {
            MenuAction::ToggleMute => Self::toggle_mute(conf_name, channel_id).is_some(),
            MenuAction::ToggleDeaf => Self::toggle_deaf(conf_name, channel_id).is_some(),
            MenuAction::IncreaseVolume => {
                Self::adjust_volume(conf_name, channel_id, 1).is_some()
            }
            MenuAction::DecreaseVolume => {
                Self::adjust_volume(conf_name, channel_id, -1).is_some()
            }
            MenuAction::AdminKickLast => Self::kick_last(conf_name),
            MenuAction::AdminToggleLock => Self::toggle_lock(conf_name).is_some(),
            MenuAction::LeaveConference => true, // Handled by caller
            MenuAction::AdminToggleMuteParticipants => {
                Self::toggle_mute_participants(conf_name).is_some()
            }
            MenuAction::AdminKickAll => Self::kick_all_participants(conf_name) > 0,
            MenuAction::NoOp => true,
            MenuAction::SetAsSingleVideoSrc | MenuAction::ReleaseAsSingleVideoSrc => {
                // Stub: video source management
                true
            }
            MenuAction::Playback(file) => {
                info!("ConfBridge: would play '{}' to user", file);
                true
            }
            MenuAction::Dial(target) => {
                info!("ConfBridge: would dial '{}' for user", target);
                true
            }
        }
    }
}

/// Conference info for CLI display.
#[derive(Debug, Clone)]
pub struct ConferenceInfo {
    pub name: String,
    pub participant_count: usize,
    pub marked_count: usize,
    pub locked: bool,
    pub muted: bool,
    pub recording: bool,
    pub bridge_profile: String,
}

/// Helper: check if a config value means "true".
fn is_true(value: &str) -> bool {
    matches!(
        value.to_lowercase().as_str(),
        "yes" | "true" | "1" | "on"
    )
}

// Simple once_cell implementation
mod once_cell {
    pub mod sync {
        pub struct Lazy<T> {
            inner: std::sync::OnceLock<T>,
            init: fn() -> T,
        }

        impl<T> Lazy<T> {
            pub const fn new(init: fn() -> T) -> Self {
                Self {
                    inner: std::sync::OnceLock::new(),
                    init,
                }
            }
        }

        impl<T> std::ops::Deref for Lazy<T> {
            type Target = T;

            fn deref(&self) -> &T {
                self.inner.get_or_init(self.init)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Profile tests ---

    #[test]
    fn test_user_profile_default() {
        let up = UserProfile::default();
        assert!(!up.admin);
        assert!(!up.marked);
        assert!(!up.start_muted);
        assert!(!up.wait_marked);
        assert!(!up.end_marked);
        assert!(up.pin.is_none());
        assert_eq!(up.timeout, Duration::ZERO);
        assert!(!up.dtmf_passthrough);
        assert!(!up.denoise);
        assert!(!up.talk_detection_events);
        assert!(!up.talker_optimization);
    }

    #[test]
    fn test_user_profile_from_config() {
        let mut config = HashMap::new();
        config.insert("admin".to_string(), "yes".to_string());
        config.insert("marked".to_string(), "true".to_string());
        config.insert("startmuted".to_string(), "yes".to_string());
        config.insert("wait_marked".to_string(), "yes".to_string());
        config.insert("end_marked".to_string(), "yes".to_string());
        config.insert("pin".to_string(), "1234".to_string());
        config.insert("timeout".to_string(), "3600".to_string());
        config.insert("dtmf_passthrough".to_string(), "yes".to_string());
        config.insert("denoise".to_string(), "yes".to_string());
        config.insert("talk_detection_events".to_string(), "yes".to_string());
        config.insert("talker_optimization".to_string(), "yes".to_string());
        config.insert("announce_join_leave".to_string(), "yes".to_string());
        config.insert("announce_user_count".to_string(), "yes".to_string());
        config.insert("quiet".to_string(), "no".to_string());

        let up = UserProfile::from_config("admin_profile", &config);
        assert!(up.admin);
        assert!(up.marked);
        assert!(up.start_muted);
        assert!(up.wait_marked);
        assert!(up.end_marked);
        assert_eq!(up.pin, Some("1234".to_string()));
        assert_eq!(up.timeout, Duration::from_secs(3600));
        assert!(up.dtmf_passthrough);
        assert!(up.denoise);
        assert!(up.talk_detection_events);
        assert!(up.talker_optimization);
        assert!(up.announce_join_leave);
        assert!(up.announce_user_count);
        assert!(!up.quiet);
    }

    #[test]
    fn test_bridge_profile_default() {
        let bp = BridgeProfile::default();
        assert_eq!(bp.max_members, 0);
        assert!(!bp.record_conference);
        assert_eq!(bp.internal_sample_rate, SampleRate::Auto);
        assert_eq!(bp.mixing_interval, 20);
        assert_eq!(bp.video_mode, ConferenceVideoMode::None);
    }

    #[test]
    fn test_bridge_profile_from_config() {
        let mut config = HashMap::new();
        config.insert("max_members".to_string(), "50".to_string());
        config.insert("record_conference".to_string(), "yes".to_string());
        config.insert("internal_sample_rate".to_string(), "16000".to_string());
        config.insert("mixing_interval".to_string(), "40".to_string());
        config.insert("video_mode".to_string(), "follow_talker".to_string());
        config.insert("language".to_string(), "fr".to_string());

        let bp = BridgeProfile::from_config("my_bridge", &config);
        assert_eq!(bp.max_members, 50);
        assert!(bp.record_conference);
        assert_eq!(bp.internal_sample_rate, SampleRate::Rate16000);
        assert_eq!(bp.mixing_interval, 40);
        assert_eq!(bp.video_mode, ConferenceVideoMode::FollowTalker);
        assert_eq!(bp.language, "fr");
    }

    #[test]
    fn test_bridge_profile_invalid_mixing_interval() {
        let mut config = HashMap::new();
        config.insert("mixing_interval".to_string(), "15".to_string());
        let bp = BridgeProfile::from_config("test", &config);
        assert_eq!(bp.mixing_interval, 20); // Should keep default
    }

    // --- Video mode tests ---

    #[test]
    fn test_video_mode_parse() {
        assert_eq!(
            ConferenceVideoMode::from_str_name("follow_talker"),
            ConferenceVideoMode::FollowTalker
        );
        assert_eq!(
            ConferenceVideoMode::from_str_name("last_marked"),
            ConferenceVideoMode::LastMarked
        );
        assert_eq!(
            ConferenceVideoMode::from_str_name("first_marked"),
            ConferenceVideoMode::FirstMarked
        );
        assert_eq!(
            ConferenceVideoMode::from_str_name("sfu"),
            ConferenceVideoMode::Sfu
        );
        assert_eq!(
            ConferenceVideoMode::from_str_name("none"),
            ConferenceVideoMode::None
        );
        assert_eq!(
            ConferenceVideoMode::from_str_name("invalid"),
            ConferenceVideoMode::None
        );
    }

    // --- Sample rate tests ---

    #[test]
    fn test_sample_rate_parse() {
        assert_eq!(SampleRate::from_str_name("auto"), SampleRate::Auto);
        assert_eq!(SampleRate::from_str_name("8000"), SampleRate::Rate8000);
        assert_eq!(SampleRate::from_str_name("16000"), SampleRate::Rate16000);
        assert_eq!(SampleRate::from_str_name("48000"), SampleRate::Rate48000);
        assert_eq!(SampleRate::Auto.as_u32(), None);
        assert_eq!(SampleRate::Rate16000.as_u32(), Some(16000));
    }

    // --- Menu tests ---

    #[test]
    fn test_menu_default() {
        let menu = ConfMenu::default();
        assert_eq!(menu.name, "default_menu");
        assert!(!menu.entries.is_empty());

        let mute = menu.find_entry("1");
        assert!(mute.is_some());
        assert_eq!(mute.unwrap().actions[0], MenuAction::ToggleMute);
    }

    #[test]
    fn test_menu_from_config() {
        let mut config = HashMap::new();
        config.insert("1".to_string(), "toggle_mute".to_string());
        config.insert("*".to_string(), "admin_toggle_lock".to_string());
        config.insert("9".to_string(), "leave_conference".to_string());
        config.insert(
            "0".to_string(),
            "playback(conf-usermenu)".to_string(),
        );

        let menu = ConfMenu::from_config("my_menu", &config);
        assert_eq!(menu.entries.len(), 4);
        assert!(menu.find_entry("1").is_some());
        assert!(menu.find_entry("9").is_some());

        let playback_entry = menu.find_entry("0").unwrap();
        assert_eq!(
            playback_entry.actions[0],
            MenuAction::Playback("conf-usermenu".to_string())
        );
    }

    #[test]
    fn test_menu_multi_digit() {
        let mut config = HashMap::new();
        config.insert("*1".to_string(), "admin_toggle_lock".to_string());
        config.insert("*2".to_string(), "admin_kick_last".to_string());
        config.insert("*3".to_string(), "admin_toggle_mute_participants".to_string());

        let menu = ConfMenu::from_config("admin_menu", &config);
        assert!(menu.has_prefix("*")); // '*' is a prefix for multi-digit
        assert!(!menu.has_prefix("1")); // '1' is not a prefix

        let entry = menu.find_entry("*1");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().actions[0], MenuAction::AdminToggleLock);
    }

    #[test]
    fn test_menu_action_parse() {
        assert_eq!(
            MenuAction::from_str_name("toggle_mute"),
            Some(MenuAction::ToggleMute)
        );
        assert_eq!(
            MenuAction::from_str_name("toggle_deaf"),
            Some(MenuAction::ToggleDeaf)
        );
        assert_eq!(
            MenuAction::from_str_name("increase_listening_volume"),
            Some(MenuAction::IncreaseVolume)
        );
        assert_eq!(
            MenuAction::from_str_name("decrease_volume"),
            Some(MenuAction::DecreaseVolume)
        );
        assert_eq!(
            MenuAction::from_str_name("admin_kick_last"),
            Some(MenuAction::AdminKickLast)
        );
        assert_eq!(
            MenuAction::from_str_name("admin_toggle_lock"),
            Some(MenuAction::AdminToggleLock)
        );
        assert_eq!(
            MenuAction::from_str_name("leave_conference"),
            Some(MenuAction::LeaveConference)
        );
        assert_eq!(
            MenuAction::from_str_name("no_op"),
            Some(MenuAction::NoOp)
        );
        assert_eq!(
            MenuAction::from_str_name("playback(test.wav)"),
            Some(MenuAction::Playback("test.wav".to_string()))
        );
        assert_eq!(
            MenuAction::from_str_name("dialplan_exec(default,s,1)"),
            Some(MenuAction::Dial("default,s,1".to_string()))
        );
        assert_eq!(MenuAction::from_str_name("unknown_action"), None);
    }

    // --- Conference settings tests ---

    #[test]
    fn test_conference_settings_default() {
        let settings = ConferenceSettings::default();
        assert_eq!(settings.max_members, 0);
        assert!(!settings.music_on_hold_when_empty);
        assert_eq!(settings.music_class, "default");
        assert_eq!(settings.mixing_interval, 20);
        assert_eq!(settings.video_mode, ConferenceVideoMode::None);
    }

    #[test]
    fn test_conference_settings_from_bridge_profile() {
        let mut bp = BridgeProfile::default();
        bp.max_members = 100;
        bp.record_conference = true;
        bp.video_mode = ConferenceVideoMode::Sfu;
        bp.mixing_interval = 40;

        let settings = ConferenceSettings::from_bridge_profile(&bp);
        assert_eq!(settings.max_members, 100);
        assert!(settings.record_conference);
        assert_eq!(settings.video_mode, ConferenceVideoMode::Sfu);
        assert_eq!(settings.mixing_interval, 40);
    }

    // --- User profile parsing (backward compat) ---

    #[test]
    fn test_user_profile_parsing_compat() {
        let up = AppConfBridge::parse_user_profile("admin");
        assert!(up.admin);
        assert!(!up.marked);

        let up = AppConfBridge::parse_user_profile("admin,marked");
        assert!(up.admin);
        assert!(up.marked);

        let up = AppConfBridge::parse_user_profile("muted,wait_marked,end_marked");
        assert!(up.start_muted);
        assert!(up.wait_marked);
        assert!(up.end_marked);
    }

    // --- ConfBridgeResult tests ---

    #[test]
    fn test_confbridge_result_strings() {
        assert_eq!(ConfBridgeResult::Hangup.as_str(), "HANGUP");
        assert_eq!(ConfBridgeResult::Kicked.as_str(), "KICKED");
        assert_eq!(ConfBridgeResult::EndMarked.as_str(), "ENDMARKED");
        assert_eq!(ConfBridgeResult::Dtmf.as_str(), "DTMF");
        assert_eq!(ConfBridgeResult::Timeout.as_str(), "TIMEOUT");
        assert_eq!(ConfBridgeResult::Failed.as_str(), "FAILED");
    }

    // --- AMI event tests ---

    #[test]
    fn test_event_names() {
        let ev = ConfBridgeEvent::ConfbridgeStart {
            conference: "test".to_string(),
        };
        assert_eq!(ev.event_name(), "ConfbridgeStart");

        let ev = ConfBridgeEvent::ConfbridgeJoin {
            conference: "test".to_string(),
            channel: "SIP/alice".to_string(),
            admin: true,
            marked: false,
        };
        assert_eq!(ev.event_name(), "ConfbridgeJoin");

        let ev = ConfBridgeEvent::ConfbridgeTalking {
            conference: "test".to_string(),
            channel: "SIP/alice".to_string(),
            talking: true,
        };
        assert_eq!(ev.event_name(), "ConfbridgeTalking");
    }

    // --- Conference sounds ---

    #[test]
    fn test_conference_sounds_default() {
        let sounds = ConferenceSounds::default();
        assert_eq!(sounds.has_joined, "conf-hasjoin");
        assert_eq!(sounds.muted, "conf-muted");
        assert_eq!(sounds.join, "confbridge-join");
        assert_eq!(sounds.get_pin, "conf-getpin");
    }

    // --- Conference info ---

    #[test]
    fn test_conference_counts() {
        let mut conf = Conference {
            id: "test-id".to_string(),
            name: "test-conf".to_string(),
            bridge: Bridge::new("test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };

        let admin = ConferenceUser {
            channel_id: ChannelId::from_name("admin-chan"),
            channel_name: "SIP/admin".to_string(),
            is_admin: true,
            is_marked: true,
            muted: false,
            deaf: false,
            talking: false,
            waiting: false,
            join_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: "default".to_string(),
            menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
        };

        let waiting_user = ConferenceUser {
            channel_id: ChannelId::from_name("wait-chan"),
            channel_name: "SIP/waiter".to_string(),
            is_admin: false,
            is_marked: false,
            muted: true,
            deaf: false,
            talking: false,
            waiting: true,
            join_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: "default".to_string(),
            menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
        };

        conf.participants
            .insert(admin.channel_id.clone(), admin);
        conf.participants
            .insert(waiting_user.channel_id.clone(), waiting_user);
        conf.marked_count = 1;

        assert_eq!(conf.admin_count(), 1);
        assert_eq!(conf.marked_user_count(), 1);
        assert_eq!(conf.waiting_count(), 1);
        assert_eq!(conf.active_count(), 1);
    }

    // --- is_true helper ---

    #[test]
    fn test_is_true() {
        assert!(is_true("yes"));
        assert!(is_true("true"));
        assert!(is_true("1"));
        assert!(is_true("on"));
        assert!(is_true("YES"));
        assert!(is_true("True"));
        assert!(!is_true("no"));
        assert!(!is_true("false"));
        assert!(!is_true("0"));
        assert!(!is_true("off"));
        assert!(!is_true(""));
    }

    // --- Mixing interval validation ---

    #[test]
    fn test_validate_mixing_interval() {
        assert!(BridgeProfile::validate_mixing_interval(10));
        assert!(BridgeProfile::validate_mixing_interval(20));
        assert!(BridgeProfile::validate_mixing_interval(40));
        assert!(BridgeProfile::validate_mixing_interval(80));
        assert!(!BridgeProfile::validate_mixing_interval(15));
        assert!(!BridgeProfile::validate_mixing_interval(0));
        assert!(!BridgeProfile::validate_mixing_interval(100));
    }

    // -----------------------------------------------------------------------
    // Adversarial tests -- edge cases and attack vectors
    // -----------------------------------------------------------------------

    // --- Conference with max_members=0 -> unlimited ---
    #[test]
    fn test_adversarial_max_members_zero() {
        let bp = BridgeProfile::default();
        assert_eq!(bp.max_members, 0); // 0 = unlimited
        let settings = ConferenceSettings::from_bridge_profile(&bp);
        assert_eq!(settings.max_members, 0);
    }

    // --- Conference with max_members=1 -> only one participant ---
    #[test]
    fn test_adversarial_max_members_one() {
        let mut bp = BridgeProfile::default();
        bp.max_members = 1;
        let settings = ConferenceSettings::from_bridge_profile(&bp);
        assert_eq!(settings.max_members, 1);
    }

    // --- All participants are admin -> marked user logic ---
    #[test]
    fn test_adversarial_all_admin() {
        let mut conf = Conference {
            id: "adv-1".to_string(),
            name: "all-admin".to_string(),
            bridge: Bridge::new("adv-bridge-1"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };

        for i in 0..3 {
            let user = ConferenceUser {
                channel_id: ChannelId::from_name(&format!("admin{}", i)),
                channel_name: format!("SIP/admin{}", i),
                is_admin: true,
                is_marked: true,
                muted: false,
                deaf: false,
                talking: false,
                waiting: false,
                join_time: Instant::now(),
                caller_name: None,
                caller_number: None,
                volume_adjustment: 0,
                talking_volume_adjustment: 0,
                profile_name: "default".to_string(),
                menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
            };
            conf.participants.insert(user.channel_id.clone(), user);
            conf.marked_count += 1;
        }

        assert_eq!(conf.admin_count(), 3);
        assert_eq!(conf.marked_user_count(), 3);
        assert_eq!(conf.active_count(), 3);
        assert_eq!(conf.waiting_count(), 0);
    }

    // --- Menu with conflicting single/multi-digit DTMF ---
    #[test]
    fn test_adversarial_menu_dtmf_conflict() {
        let mut config = HashMap::new();
        config.insert("1".to_string(), "toggle_mute".to_string());
        config.insert("12".to_string(), "admin_kick_last".to_string());
        let menu = ConfMenu::from_config("conflict_menu", &config);
        // "1" is a prefix for "12"
        assert!(menu.has_prefix("1"));
        // Direct lookup for "1" should still work
        assert!(menu.find_entry("1").is_some());
        assert!(menu.find_entry("12").is_some());
    }

    // --- Conference with mixing_interval=0 -> should keep default ---
    #[test]
    fn test_adversarial_mixing_interval_zero() {
        let mut config = HashMap::new();
        config.insert("mixing_interval".to_string(), "0".to_string());
        let bp = BridgeProfile::from_config("zero_mix", &config);
        assert_eq!(bp.mixing_interval, 20); // Should keep default (20)
    }

    // --- Video mode with no video streams -> should not panic ---
    #[test]
    fn test_adversarial_video_mode_no_streams() {
        let mut config = HashMap::new();
        config.insert("video_mode".to_string(), "follow_talker".to_string());
        let bp = BridgeProfile::from_config("video_test", &config);
        assert_eq!(bp.video_mode, ConferenceVideoMode::FollowTalker);
        // Even with no video streams, the mode is set -- actual stream
        // handling is in the mixing engine, not the config
    }

    // --- PIN with special characters ---
    #[test]
    fn test_adversarial_pin_special_chars() {
        let mut config = HashMap::new();
        config.insert("pin".to_string(), "12#*34".to_string());
        let up = UserProfile::from_config("pin_test", &config);
        assert_eq!(up.pin, Some("12#*34".to_string()));
    }

    // --- Lock then unlock then lock -> state consistency ---
    #[test]
    fn test_adversarial_lock_unlock_lock() {
        // Create a conference directly in the registry
        let conf = Conference {
            id: "lock-test-id".to_string(),
            name: "lock_test_conf".to_string(),
            bridge: Bridge::new("lock-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };
        CONFERENCES.insert("lock_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        // Lock
        assert!(AppConfBridge::lock_conference("lock_test_conf"));
        assert!(CONFERENCES.get("lock_test_conf").unwrap().read().locked);

        // Unlock
        assert!(AppConfBridge::unlock_conference("lock_test_conf"));
        assert!(!CONFERENCES.get("lock_test_conf").unwrap().read().locked);

        // Lock again
        assert!(AppConfBridge::lock_conference("lock_test_conf"));
        assert!(CONFERENCES.get("lock_test_conf").unwrap().read().locked);

        // Toggle lock
        let result = AppConfBridge::toggle_lock("lock_test_conf");
        assert_eq!(result, Some(false)); // Was locked -> now unlocked

        let result = AppConfBridge::toggle_lock("lock_test_conf");
        assert_eq!(result, Some(true)); // Was unlocked -> now locked

        // Clean up
        CONFERENCES.remove("lock_test_conf");
    }

    // --- Kick user who already left -> no panic ---
    #[test]
    fn test_adversarial_kick_already_left() {
        let conf = Conference {
            id: "kick-test-id".to_string(),
            name: "kick_test_conf".to_string(),
            bridge: Bridge::new("kick-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };
        CONFERENCES.insert("kick_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        let ghost_id = ChannelId::from_name("ghost");
        // Kicking a non-existent user should return false, not panic
        assert!(!AppConfBridge::kick_user("kick_test_conf", &ghost_id));

        // Clean up
        CONFERENCES.remove("kick_test_conf");
    }

    // --- Kick last when no users -> no panic ---
    #[test]
    fn test_adversarial_kick_last_empty() {
        let conf = Conference {
            id: "kicklast-test-id".to_string(),
            name: "kicklast_test_conf".to_string(),
            bridge: Bridge::new("kicklast-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };
        CONFERENCES.insert("kicklast_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        assert!(!AppConfBridge::kick_last("kicklast_test_conf"));

        CONFERENCES.remove("kicklast_test_conf");
    }

    // --- Kick last when last_joined is stale (user already left) ---
    #[test]
    fn test_adversarial_kick_last_stale() {
        let ghost_id = ChannelId::from_name("ghost_user");
        let conf = Conference {
            id: "stale-test-id".to_string(),
            name: "stale_test_conf".to_string(),
            bridge: Bridge::new("stale-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: Some(ghost_id), // Points to user not in participants
            recording: false,
        };
        CONFERENCES.insert("stale_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        // Should return false (stale last_joined, user not in participants)
        assert!(!AppConfBridge::kick_last("stale_test_conf"));

        CONFERENCES.remove("stale_test_conf");
    }

    // --- Empty conference name -> should fail ---
    #[tokio::test]
    async fn test_adversarial_empty_conf_name() {
        let mut channel = Channel::new("Test/adv-conf");
        let (result, status) = AppConfBridge::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(status, ConfBridgeResult::Failed);
    }

    // --- Mute/unmute on nonexistent conference -> false ---
    #[test]
    fn test_adversarial_mute_nonexistent_conf() {
        let id = ChannelId::from_name("test");
        assert!(!AppConfBridge::mute_user("nonexistent_conf", &id));
        assert!(!AppConfBridge::unmute_user("nonexistent_conf", &id));
        assert!(AppConfBridge::toggle_mute("nonexistent_conf", &id).is_none());
        assert!(AppConfBridge::toggle_deaf("nonexistent_conf", &id).is_none());
    }

    // --- Adjust volume on nonexistent conference -> None ---
    #[test]
    fn test_adversarial_volume_nonexistent_conf() {
        let id = ChannelId::from_name("test");
        assert!(AppConfBridge::adjust_volume("nonexistent_conf", &id, 1).is_none());
    }

    // --- Volume clamping ---
    #[test]
    fn test_adversarial_volume_clamping() {
        let conf = Conference {
            id: "vol-test-id".to_string(),
            name: "vol_test_conf".to_string(),
            bridge: Bridge::new("vol-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };
        let chan_id = ChannelId::from_name("vol_user");
        let user = ConferenceUser {
            channel_id: chan_id.clone(),
            channel_name: "SIP/vol".to_string(),
            is_admin: false,
            is_marked: false,
            muted: false,
            deaf: false,
            talking: false,
            waiting: false,
            join_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: "default".to_string(),
            menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
        };
        let mut c = conf;
        c.participants.insert(chan_id.clone(), user);
        CONFERENCES.insert("vol_test_conf".to_string(), Arc::new(RwLock::new(c)));

        // Increase beyond max
        for _ in 0..10 {
            AppConfBridge::adjust_volume("vol_test_conf", &chan_id, 1);
        }
        let vol = AppConfBridge::adjust_volume("vol_test_conf", &chan_id, 0).unwrap();
        assert_eq!(vol, 4); // Clamped to 4

        // Decrease beyond min
        for _ in 0..20 {
            AppConfBridge::adjust_volume("vol_test_conf", &chan_id, -1);
        }
        let vol = AppConfBridge::adjust_volume("vol_test_conf", &chan_id, 0).unwrap();
        assert_eq!(vol, -4); // Clamped to -4

        CONFERENCES.remove("vol_test_conf");
    }

    // --- Start/stop recording on nonexistent conference ---
    #[test]
    fn test_adversarial_recording_nonexistent() {
        assert!(!AppConfBridge::start_recording("nonexistent_conf", None));
        assert!(!AppConfBridge::stop_recording("nonexistent_conf"));
    }

    // --- Double start recording ---
    #[test]
    fn test_adversarial_double_start_recording() {
        let conf = Conference {
            id: "rec-test-id".to_string(),
            name: "rec_test_conf".to_string(),
            bridge: Bridge::new("rec-test-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };
        CONFERENCES.insert("rec_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        assert!(AppConfBridge::start_recording("rec_test_conf", None));
        // Second start should return false (already recording)
        assert!(!AppConfBridge::start_recording("rec_test_conf", None));
        // Stop
        assert!(AppConfBridge::stop_recording("rec_test_conf"));
        // Double stop
        assert!(!AppConfBridge::stop_recording("rec_test_conf"));

        CONFERENCES.remove("rec_test_conf");
    }

    // --- Menu action parsing: unknown actions return None ---
    #[test]
    fn test_adversarial_menu_action_unknown() {
        assert!(MenuAction::from_str_name("").is_none());
        assert!(MenuAction::from_str_name("  ").is_none());
        assert!(MenuAction::from_str_name("garbage_action").is_none());
    }

    // --- Menu with no entries ---
    #[test]
    fn test_adversarial_menu_empty() {
        let config: HashMap<String, String> = HashMap::new();
        let menu = ConfMenu::from_config("empty_menu", &config);
        assert!(menu.entries.is_empty());
        assert!(menu.find_entry("1").is_none());
        assert!(!menu.has_prefix("1"));
    }

    // --- Bridge profile with all defaults ---
    #[test]
    fn test_adversarial_bridge_profile_all_defaults() {
        let config: HashMap<String, String> = HashMap::new();
        let bp = BridgeProfile::from_config("empty_profile", &config);
        assert_eq!(bp.max_members, 0);
        assert_eq!(bp.mixing_interval, 20);
        assert_eq!(bp.video_mode, ConferenceVideoMode::None);
    }

    // --- User profile from unknown fields -> should not crash ---
    #[test]
    fn test_adversarial_user_profile_unknown_fields() {
        let mut config = HashMap::new();
        config.insert("unknown_field".to_string(), "yes".to_string());
        config.insert("another_unknown".to_string(), "42".to_string());
        let up = UserProfile::from_config("unk_profile", &config);
        // Should just skip unknown fields
        assert!(!up.admin);
    }

    // --- Conference info for nonexistent conference ---
    #[test]
    fn test_adversarial_show_nonexistent_conference() {
        assert!(AppConfBridge::show_conference("nonexistent_conf_xyz").is_none());
        assert!(AppConfBridge::list_participants("nonexistent_conf_xyz").is_none());
    }

    // --- Lock/unlock nonexistent conference ---
    #[test]
    fn test_adversarial_lock_nonexistent() {
        assert!(!AppConfBridge::lock_conference("nonexistent_conf_abc"));
        assert!(!AppConfBridge::unlock_conference("nonexistent_conf_abc"));
        assert!(AppConfBridge::toggle_lock("nonexistent_conf_abc").is_none());
    }

    // --- Toggle mute participants on nonexistent conference ---
    #[test]
    fn test_adversarial_toggle_mute_participants_nonexistent() {
        assert!(AppConfBridge::toggle_mute_participants("nonexistent_conf_def").is_none());
    }

    // --- Kick all on nonexistent conference ---
    #[test]
    fn test_adversarial_kick_all_nonexistent() {
        assert_eq!(AppConfBridge::kick_all_participants("nonexistent_conf_ghi"), 0);
    }

    // --- Execute menu action on nonexistent conference ---
    #[test]
    fn test_adversarial_menu_action_nonexistent() {
        let id = ChannelId::from_name("test");
        assert!(!AppConfBridge::execute_menu_action(
            "nonexistent_conf_jkl",
            &id,
            &MenuAction::ToggleMute
        ));
    }

    // --- Kick all participants: only non-admins ---
    #[test]
    fn test_adversarial_kick_all_keeps_admins() {
        let mut conf = Conference {
            id: "kickall-test".to_string(),
            name: "kickall_test_conf".to_string(),
            bridge: Bridge::new("kickall-bridge"),
            participants: HashMap::new(),
            settings: ConferenceSettings::default(),
            created: Instant::now(),
            locked: false,
            all_muted: false,
            active: false,
            marked_count: 0,
            last_joined: None,
            recording: false,
        };

        let admin_id = ChannelId::from_name("admin");
        conf.participants.insert(admin_id.clone(), ConferenceUser {
            channel_id: admin_id,
            channel_name: "SIP/admin".to_string(),
            is_admin: true,
            is_marked: false,
            muted: false,
            deaf: false,
            talking: false,
            waiting: false,
            join_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: "default".to_string(),
            menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
        });

        let user_id = ChannelId::from_name("user");
        conf.participants.insert(user_id.clone(), ConferenceUser {
            channel_id: user_id,
            channel_name: "SIP/user".to_string(),
            is_admin: false,
            is_marked: false,
            muted: false,
            deaf: false,
            talking: false,
            waiting: false,
            join_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            volume_adjustment: 0,
            talking_volume_adjustment: 0,
            profile_name: "default".to_string(),
            menu_name: "default".to_string(),
            sip_call_id: None,
            video_streams: Vec::new(),
        });

        CONFERENCES.insert("kickall_test_conf".to_string(), Arc::new(RwLock::new(conf)));

        let kicked = AppConfBridge::kick_all_participants("kickall_test_conf");
        assert_eq!(kicked, 1); // Only the non-admin was kicked

        let remaining = {
            let c = CONFERENCES.get("kickall_test_conf").unwrap();
            let count = c.read().participants.len();
            count
        };
        assert_eq!(remaining, 1); // Admin remains

        CONFERENCES.remove("kickall_test_conf");
    }

    // --- SampleRate edge cases ---
    #[test]
    fn test_adversarial_sample_rate_garbage() {
        assert_eq!(SampleRate::from_str_name(""), SampleRate::Auto);
        assert_eq!(SampleRate::from_str_name("0"), SampleRate::Auto);
        assert_eq!(SampleRate::from_str_name("99999"), SampleRate::Auto);
        assert_eq!(SampleRate::from_str_name("garbage"), SampleRate::Auto);
    }

    // --- Video mode edge cases ---
    #[test]
    fn test_adversarial_video_mode_garbage() {
        assert_eq!(ConferenceVideoMode::from_str_name(""), ConferenceVideoMode::None);
        assert_eq!(ConferenceVideoMode::from_str_name("garbage"), ConferenceVideoMode::None);
    }

    // --- is_true edge cases ---
    #[test]
    fn test_adversarial_is_true_edge_cases() {
        assert!(!is_true(""));
        assert!(!is_true("maybe"));
        assert!(!is_true("2"));
        assert!(is_true("YES"));
        assert!(is_true("TRUE"));
        assert!(is_true("ON"));
    }
}
