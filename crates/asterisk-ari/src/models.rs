//! ARI data models -- JSON-serializable types for the Asterisk REST Interface.
//!
//! These correspond to the Swagger model definitions in rest-api/api-docs/
//! and represent the wire format for all ARI JSON messages.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Core resource models
// ---------------------------------------------------------------------------

/// Caller identification information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AriCallerId {
    /// Caller ID name
    pub name: String,
    /// Caller ID number
    pub number: String,
}

/// Dialplan location (context, extension, priority).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialplanCep {
    /// Dialplan context
    pub context: String,
    /// Dialplan extension
    pub exten: String,
    /// Dialplan priority
    pub priority: i64,
    /// Name of the application that is executing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Data passed to the application
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_data: Option<String>,
}

/// An active channel in Asterisk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    /// Unique identifier of the channel.
    pub id: String,
    /// Name of the channel (e.g. "PJSIP/alice-00000001").
    pub name: String,
    /// Current state of the channel.
    pub state: String,
    /// Caller ID information.
    pub caller: AriCallerId,
    /// Connected line information.
    pub connected: AriCallerId,
    /// Account code.
    #[serde(default)]
    pub accountcode: String,
    /// Current dialplan location.
    pub dialplan: DialplanCep,
    /// Timestamp when the channel was created.
    pub creationtime: String,
    /// Language.
    pub language: String,
    /// Channel protocol id (e.g. call-id for SIP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_id: Option<String>,
}

/// A bridge instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    /// Unique identifier for this bridge.
    pub id: String,
    /// Bridging technology in use.
    pub technology: String,
    /// Type of bridge (mixing, holding, etc.).
    pub bridge_type: String,
    /// Bridge class (same, feature, etc.).
    pub bridge_class: String,
    /// Creator of the bridge.
    pub creator: String,
    /// Name the creator gave the bridge.
    pub name: String,
    /// IDs of channels participating in this bridge.
    pub channels: Vec<String>,
    /// Video mode (none, single, talker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_mode: Option<String>,
    /// The ID of the channel that is the video source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_source_id: Option<String>,
    /// Timestamp when the bridge was created.
    pub creationtime: String,
}

/// An endpoint (a device or technology/resource pair).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    /// Technology (e.g. "PJSIP").
    pub technology: String,
    /// Resource identifier (e.g. "alice").
    pub resource: String,
    /// Endpoint state.
    pub state: Option<String>,
    /// IDs of channels associated with this endpoint.
    #[serde(default)]
    pub channel_ids: Vec<String>,
}

/// Device state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceState {
    /// Name of the device.
    pub name: String,
    /// Device state value.
    pub state: String,
}

/// Allowed device state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeviceStateValue {
    Unknown,
    NotInuse,
    Inuse,
    Busy,
    Invalid,
    Unavailable,
    Ringing,
    Ringinuse,
    Onhold,
}

impl std::fmt::Display for DeviceStateValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Unknown => "UNKNOWN",
            Self::NotInuse => "NOT_INUSE",
            Self::Inuse => "INUSE",
            Self::Busy => "BUSY",
            Self::Invalid => "INVALID",
            Self::Unavailable => "UNAVAILABLE",
            Self::Ringing => "RINGING",
            Self::Ringinuse => "RINGINUSE",
            Self::Onhold => "ONHOLD",
        };
        write!(f, "{}", s)
    }
}

/// A playback operation (media being played to a channel or bridge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playback {
    /// ID for this playback operation.
    pub id: String,
    /// URI for the media currently being played back.
    pub media_uri: String,
    /// If a list of URIs is being played, the next media URI to play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_media_uri: Option<String>,
    /// URI for the channel or bridge to play the media on.
    pub target_uri: String,
    /// For media types that support multiple languages, the language requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Current state of the playback operation.
    pub state: PlaybackState,
}

/// Playback state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Queued,
    Playing,
    Continuing,
    Done,
    Failed,
}

/// Playback control operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackOperation {
    Restart,
    Pause,
    Unpause,
    Reverse,
    Forward,
}

/// A live recording in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveRecording {
    /// Base name for the recording.
    pub name: String,
    /// Recording format (e.g. "wav").
    pub format: String,
    /// URI for the channel or bridge being recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_uri: Option<String>,
    /// Current state of the recording.
    pub state: RecordingState,
    /// Duration in seconds of the recording so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<i32>,
    /// Duration of silence in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silence_duration: Option<i32>,
    /// Duration of talking in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub talking_duration: Option<i32>,
    /// Cause for recording failure if state is failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

/// Recording state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingState {
    Queued,
    Recording,
    Paused,
    Done,
    Failed,
    Canceled,
}

/// A stored (completed) recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRecording {
    /// Name of the recording.
    pub name: String,
    /// Recording format (e.g. "wav").
    pub format: String,
}

/// Format/language pair for a sound file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatLangPair {
    /// Language code.
    pub language: String,
    /// Format name (e.g. "gsm", "wav").
    pub format: String,
}

/// A sound file that may be played back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sound {
    /// Sound's identifier.
    pub id: String,
    /// Text description of the sound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The formats and languages in which this sound is available.
    pub formats: Vec<FormatLangPair>,
}

/// A channel variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    /// Variable value.
    pub value: String,
}

/// A text message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessage {
    /// From URI.
    pub from: String,
    /// To URI.
    pub to: String,
    /// Message body.
    #[serde(default)]
    pub body: String,
    /// Technology-specific key/value pairs.
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

/// A mailbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mailbox {
    /// Name of the mailbox.
    pub name: String,
    /// Count of old messages.
    pub old_messages: i32,
    /// Count of new messages.
    pub new_messages: i32,
}

/// Details of a Stasis application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Application {
    /// Name of this application.
    pub name: String,
    /// IDs of channels subscribed to.
    pub channel_ids: Vec<String>,
    /// IDs of bridges subscribed to.
    pub bridge_ids: Vec<String>,
    /// tech/resource for endpoints subscribed to.
    pub endpoint_ids: Vec<String>,
    /// Names of devices subscribed to.
    pub device_names: Vec<String>,
    /// Event types sent to the application.
    #[serde(default)]
    pub events_allowed: Vec<EventTypeFilter>,
    /// Event types not sent to the application.
    #[serde(default)]
    pub events_disallowed: Vec<EventTypeFilter>,
}

/// Event type filter entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTypeFilter {
    /// The type name of the event to filter.
    #[serde(rename = "type")]
    pub event_type: String,
}

/// Configuration tuple (for sorcery dynamic config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigTuple {
    /// Configuration attribute name.
    pub attribute: String,
    /// Configuration attribute value.
    pub value: String,
}

/// Asterisk system information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsteriskInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusInfo>,
}

/// Build information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    pub os: String,
    pub kernel: String,
    pub machine: String,
    pub options: String,
    pub date: String,
    pub user: String,
}

/// System information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub entity_id: String,
}

/// Configuration information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInfo {
    pub name: String,
    pub default_language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_channels: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_open_files: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_load: Option<f64>,
    pub setid: SetId,
}

/// setid (user/group).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetId {
    pub user: String,
    pub group: String,
}

/// Status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub startup_time: String,
    pub last_reload_time: String,
}

/// Asterisk ping response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsteriskPing {
    pub asterisk_id: String,
    pub ping: String,
    pub timestamp: String,
}

/// Module information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AriModule {
    /// Module name.
    pub name: String,
    /// Module description.
    pub description: String,
    /// Module support level.
    pub support_level: String,
    /// Whether the module is loaded.
    #[serde(default)]
    pub use_count: i32,
    /// Module status.
    pub status: String,
}

/// Contact information for an endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactInfo {
    /// The location of the contact.
    pub uri: String,
    /// The current status of the contact.
    pub contact_status: String,
    /// The Address of Record this contact belongs to.
    pub aor: String,
    /// Current round trip time, in microseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roundtrip_usec: Option<String>,
}

/// Peer information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Current state of the peer.
    pub peer_status: String,
    /// An optional reason associated with the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// The IP address of the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The port of the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The last known time the peer was contacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

// ---------------------------------------------------------------------------
// ARI Events
// ---------------------------------------------------------------------------

/// Base event fields common to all ARI events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBase {
    /// Name of the application receiving the event.
    pub application: String,
    /// Timestamp when the event was created.
    pub timestamp: String,
    /// The unique ID for the Asterisk instance that raised this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asterisk_id: Option<String>,
}

/// All ARI events that can be sent to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AriEvent {
    // Stasis lifecycle
    StasisStart {
        #[serde(flatten)]
        base: EventBase,
        args: Vec<String>,
        channel: Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replace_channel: Option<Channel>,
    },
    StasisEnd {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },

    // Channel events
    ChannelCreated {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
    ChannelDestroyed {
        #[serde(flatten)]
        base: EventBase,
        cause: i32,
        cause_txt: String,
        channel: Channel,
    },
    ChannelEnteredBridge {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
        channel: Channel,
    },
    ChannelLeftBridge {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
        channel: Channel,
    },
    ChannelStateChange {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
    ChannelDtmfReceived {
        #[serde(flatten)]
        base: EventBase,
        digit: String,
        duration_ms: i32,
        channel: Channel,
    },
    ChannelHangupRequest {
        #[serde(flatten)]
        base: EventBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        soft: Option<bool>,
        channel: Channel,
    },
    ChannelDialplan {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
        dialplan_app: String,
        dialplan_app_data: String,
    },
    ChannelCallerId {
        #[serde(flatten)]
        base: EventBase,
        caller_presentation: i32,
        caller_presentation_txt: String,
        channel: Channel,
    },
    ChannelVarset {
        #[serde(flatten)]
        base: EventBase,
        variable: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<Channel>,
    },
    ChannelHold {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        musicclass: Option<String>,
    },
    ChannelUnhold {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
    ChannelTalkingStarted {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
    ChannelTalkingFinished {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
        duration: i32,
    },
    ChannelConnectedLine {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
    ChannelUserevent {
        #[serde(flatten)]
        base: EventBase,
        eventname: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bridge: Option<Bridge>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<Endpoint>,
        #[serde(default)]
        userevent: serde_json::Value,
    },

    // Bridge events
    BridgeCreated {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
    },
    BridgeDestroyed {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
    },
    BridgeMerged {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
        bridge_from: Bridge,
    },
    BridgeVideoSourceChanged {
        #[serde(flatten)]
        base: EventBase,
        bridge: Bridge,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_video_source_id: Option<String>,
    },
    BridgeBlindTransfer {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replace_channel: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transferee: Option<Channel>,
        exten: String,
        context: String,
        result: String,
        is_external: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bridge: Option<Bridge>,
    },
    BridgeAttendedTransfer {
        #[serde(flatten)]
        base: EventBase,
        transferer_first_leg: Channel,
        transferer_second_leg: Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replace_channel: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transferee: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer_target: Option<Channel>,
        result: String,
        is_external: bool,
        destination_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_bridge: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_application: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_link_first_leg: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_link_second_leg: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_threeway_channel: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination_threeway_bridge: Option<Bridge>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transferer_first_leg_bridge: Option<Bridge>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transferer_second_leg_bridge: Option<Bridge>,
    },

    // Playback events
    PlaybackStarted {
        #[serde(flatten)]
        base: EventBase,
        playback: Playback,
    },
    PlaybackContinuing {
        #[serde(flatten)]
        base: EventBase,
        playback: Playback,
    },
    PlaybackFinished {
        #[serde(flatten)]
        base: EventBase,
        playback: Playback,
    },

    // Recording events
    RecordingStarted {
        #[serde(flatten)]
        base: EventBase,
        recording: LiveRecording,
    },
    RecordingFinished {
        #[serde(flatten)]
        base: EventBase,
        recording: LiveRecording,
    },
    RecordingFailed {
        #[serde(flatten)]
        base: EventBase,
        recording: LiveRecording,
    },

    // Endpoint / device events
    EndpointStateChange {
        #[serde(flatten)]
        base: EventBase,
        endpoint: Endpoint,
    },
    DeviceStateChanged {
        #[serde(flatten)]
        base: EventBase,
        device_state: DeviceState,
    },
    ContactStatusChange {
        #[serde(flatten)]
        base: EventBase,
        endpoint: Endpoint,
        contact_info: ContactInfo,
    },
    PeerStatusChange {
        #[serde(flatten)]
        base: EventBase,
        endpoint: Endpoint,
        peer: Peer,
    },

    // Dial
    Dial {
        #[serde(flatten)]
        base: EventBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<Channel>,
        peer: Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forwarded: Option<Channel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dialstring: Option<String>,
        dialstatus: String,
    },

    // Text message
    TextMessageReceived {
        #[serde(flatten)]
        base: EventBase,
        message: TextMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<Endpoint>,
    },

    // Application lifecycle
    ApplicationMoveFailed {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
        destination: String,
        args: Vec<String>,
    },
    ApplicationReplaced {
        #[serde(flatten)]
        base: EventBase,
    },

    // Channel tone detected
    ChannelToneDetected {
        #[serde(flatten)]
        base: EventBase,
        channel: Channel,
    },
}

// ---------------------------------------------------------------------------
// Request/response types used by route handlers
// ---------------------------------------------------------------------------

/// Error response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AriError {
    pub message: String,
}

/// Missing parameters error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingParams {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub params: Vec<String>,
}

// ---------------------------------------------------------------------------
// Channel request/response types
// ---------------------------------------------------------------------------

/// Parameters for POST /channels (originate).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OriginateRequest {
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, rename = "appArgs", skip_serializing_if = "Option::is_none")]
    pub app_args: Option<String>,
    #[serde(default, rename = "callerId", skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    #[serde(default)]
    pub timeout: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
    #[serde(default, rename = "channelId", skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, rename = "otherChannelId", skip_serializing_if = "Option::is_none")]
    pub other_channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formats: Option<String>,
}

/// Parameters for POST /channels/{channelId}/continue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContinueRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Parameters for POST /channels/{channelId}/redirect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectRequest {
    pub endpoint: String,
}

/// Parameters for POST /channels/{channelId}/dtmf.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendDtmfRequest {
    pub dtmf: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub between: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i32>,
}

/// Parameters for POST /channels/{channelId}/mute.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MuteRequest {
    #[serde(default)]
    pub direction: Option<String>,
}

/// Parameters for POST /channels/{channelId}/play.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayMediaRequest {
    pub media: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offsetms: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipms: Option<i32>,
    #[serde(default, rename = "playbackId", skip_serializing_if = "Option::is_none")]
    pub playback_id: Option<String>,
}

/// Parameters for POST /channels/{channelId}/record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordRequest {
    pub name: String,
    pub format: String,
    #[serde(default, rename = "maxDurationSeconds", skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<i32>,
    #[serde(default, rename = "maxSilenceSeconds", skip_serializing_if = "Option::is_none")]
    pub max_silence_seconds: Option<i32>,
    #[serde(default, rename = "ifExists", skip_serializing_if = "Option::is_none")]
    pub if_exists: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beep: Option<bool>,
    #[serde(default, rename = "terminateOn", skip_serializing_if = "Option::is_none")]
    pub terminate_on: Option<String>,
}

/// Parameters for POST /channels/{channelId}/snoop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnoopRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper: Option<String>,
    pub app: String,
    #[serde(default, rename = "appArgs", skip_serializing_if = "Option::is_none")]
    pub app_args: Option<String>,
    #[serde(default, rename = "snoopId", skip_serializing_if = "Option::is_none")]
    pub snoop_id: Option<String>,
}

/// Parameters for POST /channels/{channelId}/dial.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i32>,
}

// ---------------------------------------------------------------------------
// Bridge request types
// ---------------------------------------------------------------------------

/// Parameters for POST /bridges.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateBridgeRequest {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub bridge_type: Option<String>,
    #[serde(default, rename = "bridgeId", skip_serializing_if = "Option::is_none")]
    pub bridge_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters for POST /bridges/{bridgeId}/addChannel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddChannelRequest {
    pub channel: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, rename = "absorbDTMF", skip_serializing_if = "Option::is_none")]
    pub absorb_dtmf: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, rename = "inhibitConnectedLineUpdates", skip_serializing_if = "Option::is_none")]
    pub inhibit_connected_line_updates: Option<bool>,
}

/// Parameters for POST /bridges/{bridgeId}/removeChannel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveChannelRequest {
    pub channel: Vec<String>,
}

// ---------------------------------------------------------------------------
// Endpoint request types
// ---------------------------------------------------------------------------

/// Parameters for PUT /endpoints/sendMessage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub to: String,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Application request types
// ---------------------------------------------------------------------------

/// Parameters for POST /applications/{appName}/subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    #[serde(rename = "eventSource")]
    pub event_source: Vec<String>,
}

/// Parameters for PUT /applications/{appName}/eventFilter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilterRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<EventTypeFilter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed: Option<Vec<EventTypeFilter>>,
}

// ---------------------------------------------------------------------------
// Asterisk resource request types
// ---------------------------------------------------------------------------

/// Parameters for PUT /asterisk/config/dynamic/{configClass}/{objectType}/{id}.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateConfigRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<ConfigTuple>>,
}

/// Parameters for POST /asterisk/variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetVariableRequest {
    pub variable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

// ---------------------------------------------------------------------------
// Mailbox request types
// ---------------------------------------------------------------------------

/// Parameters for PUT /mailboxes/{mailboxName}.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMailboxRequest {
    #[serde(rename = "oldMessages")]
    pub old_messages: i32,
    #[serde(rename = "newMessages")]
    pub new_messages: i32,
}

// ---------------------------------------------------------------------------
// Device state request types
// ---------------------------------------------------------------------------

/// Parameters for PUT /deviceStates/{deviceName}.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDeviceStateRequest {
    #[serde(rename = "deviceState")]
    pub device_state: String,
}

// ---------------------------------------------------------------------------
// Events user event request
// ---------------------------------------------------------------------------

/// Parameters for POST /events/user/{eventName}.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEventRequest {
    pub application: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Copy recording request
// ---------------------------------------------------------------------------

/// Parameters for POST /recordings/stored/{name}/copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyStoredRecordingRequest {
    #[serde(rename = "destinationRecordingName")]
    pub destination_recording_name: String,
}
