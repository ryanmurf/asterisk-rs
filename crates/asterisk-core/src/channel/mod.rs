//! Channel abstraction and driver trait.
//!
//! The channel model is the heart of Asterisk -- every phone call, every
//! media stream, every signalling path is represented as a channel.
//!
//! This module contains:
//! - `Channel` -- the generic channel structure (C: `struct ast_channel`)
//! - `ChannelDriver` -- the technology driver trait (C: `struct ast_channel_tech`)
//! - `ChannelId` / `ChannelSnapshot` -- identifiers and immutable snapshots
//!
//! Sub-modules provide the deeper subsystems:
//! - `store` -- global channel container
//! - `softhangup` -- deferred hangup flags
//! - `dtmf` -- DTMF emulation / timing
//! - `generator` -- audio generator framework (MOH, tones)
//! - `audiohook` -- spy / whisper / manipulate hooks
//! - `framehook` -- generic frame interception hooks
//! - `readwrite` -- the read/write pipeline (`channel_read`, `channel_write`)

pub mod audiohook;
pub mod dtmf;
pub mod framehook;
pub mod generator;
pub mod readwrite;
pub mod softhangup;
pub mod store;
pub mod tech_registry;

use asterisk_types::{
    AsteriskResult, CallerId, ChannelFlags, ChannelState, ConnectedLine, DialedParty, Frame,
    HangupCause, Redirecting,
};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Hangup callback registry
// ---------------------------------------------------------------------------

/// Callback type for hangup notifications.
/// Receives the channel's unique_id and the hangup cause.
pub type HangupCallback = Box<dyn Fn(&str, &HangupCause) + Send + Sync>;

/// Global registry of hangup callbacks.
static HANGUP_CALLBACKS: LazyLock<parking_lot::Mutex<Vec<HangupCallback>>> =
    LazyLock::new(|| parking_lot::Mutex::new(Vec::new()));

/// Register a callback to be invoked whenever a channel hangs up.
///
/// The callback receives the channel's unique_id and the hangup cause.
pub fn register_hangup_callback(cb: HangupCallback) {
    HANGUP_CALLBACKS.lock().push(cb);
}

/// Invoke all registered hangup callbacks.
fn fire_hangup_callbacks(unique_id: &str, cause: &HangupCause) {
    let callbacks = HANGUP_CALLBACKS.lock();
    for cb in callbacks.iter() {
        cb(unique_id, cause);
    }
}

// ---------------------------------------------------------------------------
// Channel event publisher -- decoupled from AMI via a callback
// ---------------------------------------------------------------------------

/// Callback type for publishing channel lifecycle events.
///
/// The first argument is the event name (e.g. "Newchannel", "Hangup").
/// The second is a slice of (key, value) header pairs.
///
/// The startup code (or test harness) registers a closure that converts these
/// into AMI events and sends them on the global bus.
pub type ChannelEventPublisher = Box<dyn Fn(&str, &[(&str, &str)]) + Send + Sync>;

/// Global channel event publisher.
static CHANNEL_EVENT_PUBLISHER: LazyLock<parking_lot::Mutex<Option<ChannelEventPublisher>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));

/// Register a channel event publisher.
///
/// Typically called once at startup:
/// ```ignore
/// register_channel_event_publisher(Box::new(|name, headers| {
///     asterisk_ami::publish_event(
///         asterisk_ami::AmiEvent::new_with_headers(name, headers),
///     );
/// }));
/// ```
pub fn register_channel_event_publisher(publisher: ChannelEventPublisher) {
    *CHANNEL_EVENT_PUBLISHER.lock() = Some(publisher);
}

/// Publish a channel lifecycle event through the registered publisher.
///
/// If no publisher is registered this is a no-op.
fn publish_channel_event(name: &str, headers: &[(&str, &str)]) {
    let guard = CHANNEL_EVENT_PUBLISHER.lock();
    if let Some(ref publisher) = *guard {
        publisher(name, headers);
    }
}

use self::audiohook::AudiohookList;
use self::dtmf::DtmfState;
use self::framehook::FramehookList;
use self::generator::GeneratorState;

/// Unique identifier for a channel.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ChannelId(pub String);

impl ChannelId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn from_name(s: &str) -> Self {
        Self(s.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ChannelId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A PBX channel -- the generic channel structure.
///
/// This is the Rust equivalent of `struct ast_channel`.
pub struct Channel {
    /// Unique identifier
    pub unique_id: ChannelId,
    /// Channel name (e.g., "SIP/alice-00000001")
    pub name: String,
    /// Current channel state
    pub state: ChannelState,
    /// Caller party identification
    pub caller: CallerId,
    /// Connected line information
    pub connected: ConnectedLine,
    /// Dialed party information
    pub dialed: DialedParty,
    /// Redirecting information
    pub redirecting: Redirecting,
    /// Current dialplan context
    pub context: String,
    /// Current dialplan extension
    pub exten: String,
    /// Current dialplan priority
    pub priority: i32,
    /// Channel variables
    pub variables: HashMap<String, String>,
    /// Read format (codec) name
    pub read_format: String,
    /// Write format (codec) name
    pub write_format: String,
    /// Bridge unique ID this channel is in, if any
    pub bridge_id: Option<String>,
    /// Hangup cause code
    pub hangup_cause: HangupCause,
    /// Channel flags
    pub flags: ChannelFlags,
    /// Technology-specific private data
    pub tech_pvt: Option<Box<dyn Any + Send + Sync>>,
    /// Datastores attached to this channel
    pub datastores: HashMap<String, Box<dyn Any + Send + Sync>>,
    /// Queue of frames waiting to be read
    pub frame_queue: VecDeque<Frame>,
    /// Language for prompts
    pub language: String,
    /// Music on hold class
    pub musicclass: String,
    /// Account code for CDR
    pub accountcode: String,
    /// Linked channel ID
    pub linkedid: String,

    // -----------------------------------------------------------------------
    // New fields for deep channel subsystem
    // -----------------------------------------------------------------------
    /// Soft-hangup flags (bitfield of `AST_SOFTHANGUP_*` constants).
    /// When non-zero, `check_hangup()` returns true and the read pipeline
    /// will return a null/hangup frame.
    pub softhangup_flags: u32,

    /// Per-channel DTMF emulation state.
    pub dtmf_state: DtmfState,

    /// Active audio generator (MOH, tones, silence).
    pub generator: GeneratorState,

    /// Audiohooks attached to this channel (spy/whisper/manipulate).
    pub audiohooks: AudiohookList,

    /// Framehooks attached to this channel.
    pub framehooks: FramehookList,

    /// Queue of frames to be written out to the channel driver.
    ///
    /// When a channel is in a bridge, the bridge event loop places frames
    /// routed from other channels into this queue. The channel driver
    /// consumes from here to send data to the remote end (e.g., RTP).
    /// This is separate from `frame_queue` to avoid re-routing loops
    /// (frames read from `frame_queue` go to the bridge; frames in
    /// `write_queue` come from the bridge and go to the driver).
    pub write_queue: VecDeque<Frame>,
}

impl Channel {
    /// Create a new channel with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        let unique_id = ChannelId::new();
        let id_str = unique_id.0.clone();
        Channel {
            unique_id,
            name: name.into(),
            state: ChannelState::Down,
            caller: CallerId::default(),
            connected: ConnectedLine::default(),
            dialed: DialedParty::default(),
            redirecting: Redirecting::default(),
            context: "default".to_string(),
            exten: "s".to_string(),
            priority: 1,
            variables: HashMap::new(),
            read_format: "ulaw".to_string(),
            write_format: "ulaw".to_string(),
            bridge_id: None,
            hangup_cause: HangupCause::default(),
            flags: ChannelFlags::default(),
            tech_pvt: None,
            datastores: HashMap::new(),
            frame_queue: VecDeque::new(),
            language: "en".to_string(),
            musicclass: "default".to_string(),
            accountcode: String::new(),
            linkedid: id_str,
            // New subsystem fields
            softhangup_flags: 0,
            dtmf_state: DtmfState::new(),
            generator: GeneratorState::new(),
            audiohooks: AudiohookList::new(),
            framehooks: FramehookList::new(),
            write_queue: VecDeque::new(),
        }
    }

    /// Set the channel state.
    pub fn set_state(&mut self, state: ChannelState) {
        tracing::debug!(channel = %self.name, old = %self.state, new = %state, "state change");
        self.state = state;

        // Emit Newstate AMI event
        let state_num = state as u8;
        let state_num_str = state_num.to_string();
        let state_desc = state.to_string();
        publish_channel_event("Newstate", &[
            ("Channel", &self.name),
            ("ChannelState", &state_num_str),
            ("ChannelStateDesc", &state_desc),
            ("Uniqueid", &self.unique_id.0),
        ]);
    }

    /// Answer the channel.
    pub fn answer(&mut self) {
        self.set_state(ChannelState::Up);
    }

    /// Hangup the channel with the given cause.
    ///
    /// If the channel is already down, this is a no-op (safe to call twice).
    /// The hangup cause is only set if the channel is not already hung up,
    /// to preserve the original cause.
    pub fn hangup(&mut self, cause: HangupCause) {
        if self.state == ChannelState::Down {
            tracing::debug!(channel = %self.name, "hangup called on already-down channel, ignoring");
            return;
        }
        tracing::info!(channel = %self.name, cause = %cause, "hangup");
        self.hangup_cause = cause;
        self.set_state(ChannelState::Down);
        // Deactivate generator on hangup
        self.generator.deactivate();
        // Fire hangup callbacks for CDR and other subsystems
        fire_hangup_callbacks(&self.unique_id.0, &self.hangup_cause);

        // Emit Hangup AMI event
        let cause_str = (self.hangup_cause as u32).to_string();
        let cause_txt = self.hangup_cause.to_string();
        publish_channel_event("Hangup", &[
            ("Channel", &self.name),
            ("Uniqueid", &self.unique_id.0),
            ("Cause", &cause_str),
            ("Cause-txt", &cause_txt),
        ]);
    }

    // -----------------------------------------------------------------------
    // Softhangup API
    // -----------------------------------------------------------------------

    /// Request a soft hangup with the given cause flags.
    ///
    /// Mirrors `ast_softhangup_nolock`.  The flags are OR-ed into the
    /// existing softhangup_flags.  A null frame is queued to wake up any
    /// blocked reader.
    pub fn softhangup(&mut self, cause: u32) {
        tracing::debug!(
            channel = %self.name,
            cause = format_args!("{:#06x}", cause),
            "softhangup requested"
        );
        self.softhangup_flags |= cause;
        // Queue a null frame to wake up the reader (matches C behavior)
        self.frame_queue.push_back(Frame::Null);
    }

    /// Check if the channel should hang up.
    ///
    /// Mirrors `ast_check_hangup`.  Returns true if any softhangup flag is set.
    pub fn check_hangup(&self) -> bool {
        self.softhangup_flags != 0
    }

    /// Clear specific softhangup flags.
    ///
    /// Mirrors `ast_channel_clear_softhangup`.  Use `AST_SOFTHANGUP_ALL` to
    /// clear everything.
    pub fn clear_softhangup(&mut self, flag: u32) {
        self.softhangup_flags &= !flag;
    }

    // -----------------------------------------------------------------------
    // Generator convenience API
    // -----------------------------------------------------------------------

    /// Activate a generator on this channel.
    pub fn activate_generator(&mut self, gen: Box<dyn generator::Generator>) {
        self.generator.activate(gen);
    }

    /// Deactivate the current generator.
    pub fn deactivate_generator(&mut self) {
        self.generator.deactivate();
    }

    // -----------------------------------------------------------------------
    // Audiohook convenience API
    // -----------------------------------------------------------------------

    /// Attach an audiohook to this channel.
    pub fn audiohook_attach(&mut self, hook: Box<dyn audiohook::Audiohook>) {
        self.audiohooks.attach(hook);
    }

    /// Detach an audiohook by type and index.
    pub fn audiohook_detach(
        &mut self,
        hook_type: audiohook::AudiohookType,
        index: usize,
    ) -> Option<Box<dyn audiohook::Audiohook>> {
        self.audiohooks.detach(hook_type, index)
    }

    // -----------------------------------------------------------------------
    // Framehook convenience API
    // -----------------------------------------------------------------------

    /// Attach a framehook.  Returns the hook ID.
    pub fn framehook_attach(&mut self, callback: framehook::FramehookCallback) -> u32 {
        self.framehooks.attach(callback)
    }

    /// Detach a framehook by ID.
    pub fn framehook_detach(&mut self, id: u32) -> bool {
        self.framehooks.detach(id)
    }

    // -----------------------------------------------------------------------
    // Existing API (unchanged)
    // -----------------------------------------------------------------------

    /// Get a channel variable.
    pub fn get_variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }

    /// Set a channel variable.
    pub fn set_variable(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();

        // Emit VarSet AMI event
        publish_channel_event("VarSet", &[
            ("Channel", &self.name),
            ("Variable", &name),
            ("Value", &value),
            ("Uniqueid", &self.unique_id.0),
        ]);

        self.variables.insert(name, value);
    }

    /// Maximum number of frames that can be queued on a channel.
    /// Beyond this, oldest frames are dropped to prevent unbounded memory growth.
    const MAX_FRAME_QUEUE_SIZE: usize = 1000;

    /// Queue a frame for reading.
    /// If the queue is full, the oldest frame is dropped.
    pub fn queue_frame(&mut self, frame: Frame) {
        if self.frame_queue.len() >= Self::MAX_FRAME_QUEUE_SIZE {
            tracing::warn!(
                channel = %self.name,
                "frame queue full ({}), dropping oldest frame",
                Self::MAX_FRAME_QUEUE_SIZE
            );
            self.frame_queue.pop_front();
        }
        self.frame_queue.push_back(frame);
    }

    /// Dequeue a frame.
    pub fn dequeue_frame(&mut self) -> Option<Frame> {
        self.frame_queue.pop_front()
    }

    /// Queue a frame for writing out to the channel driver.
    ///
    /// Used by the bridge event loop to deliver frames routed from other
    /// channels. The channel driver should consume these via
    /// `dequeue_write_frame()`.
    pub fn queue_write_frame(&mut self, frame: Frame) {
        if self.write_queue.len() >= Self::MAX_FRAME_QUEUE_SIZE {
            tracing::warn!(
                channel = %self.name,
                "write queue full ({}), dropping oldest frame",
                Self::MAX_FRAME_QUEUE_SIZE
            );
            self.write_queue.pop_front();
        }
        self.write_queue.push_back(frame);
    }

    /// Dequeue a frame from the write queue (for the channel driver).
    pub fn dequeue_write_frame(&mut self) -> Option<Frame> {
        self.write_queue.pop_front()
    }

    /// Create an immutable snapshot.
    pub fn snapshot(&self) -> ChannelSnapshot {
        ChannelSnapshot {
            unique_id: self.unique_id.clone(),
            name: self.name.clone(),
            state: self.state,
            caller: self.caller.clone(),
            connected: self.connected.clone(),
            dialed: self.dialed.clone(),
            context: self.context.clone(),
            exten: self.exten.clone(),
            priority: self.priority,
            hangup_cause: self.hangup_cause,
            bridge_id: self.bridge_id.clone(),
            language: self.language.clone(),
            accountcode: self.accountcode.clone(),
            linkedid: self.linkedid.clone(),
        }
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("unique_id", &self.unique_id)
            .field("name", &self.name)
            .field("state", &self.state)
            .field("context", &self.context)
            .field("exten", &self.exten)
            .field("priority", &self.priority)
            .field("softhangup_flags", &format_args!("{:#010x}", self.softhangup_flags))
            .field("generator", &self.generator)
            .field("audiohooks", &self.audiohooks)
            .field("framehooks", &self.framehooks)
            .finish()
    }
}

/// Immutable snapshot of a channel.
#[derive(Debug, Clone)]
pub struct ChannelSnapshot {
    pub unique_id: ChannelId,
    pub name: String,
    pub state: ChannelState,
    pub caller: CallerId,
    pub connected: ConnectedLine,
    pub dialed: DialedParty,
    pub context: String,
    pub exten: String,
    pub priority: i32,
    pub hangup_cause: HangupCause,
    pub bridge_id: Option<String>,
    pub language: String,
    pub accountcode: String,
    pub linkedid: String,
}

/// The channel driver trait. Each channel technology implements this.
#[async_trait::async_trait]
pub trait ChannelDriver: Send + Sync + fmt::Debug {
    /// The technology name.
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Request a new channel.
    async fn request(
        &self,
        dest: &str,
        caller: Option<&Channel>,
    ) -> AsteriskResult<Channel>;

    /// Initiate an outgoing call.
    async fn call(
        &self,
        channel: &mut Channel,
        dest: &str,
        timeout: i32,
    ) -> AsteriskResult<()>;

    /// Hang up the channel.
    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()>;

    /// Answer an incoming call.
    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()>;

    /// Read a frame from the channel.
    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame>;

    /// Write a frame to the channel.
    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()>;

    /// Indicate a condition.
    async fn indicate(
        &self,
        _channel: &mut Channel,
        _condition: i32,
        _data: &[u8],
    ) -> AsteriskResult<()> {
        Ok(())
    }

    /// Start sending a DTMF digit.
    async fn send_digit_begin(&self, _channel: &mut Channel, _digit: char) -> AsteriskResult<()> {
        Ok(())
    }

    /// Stop sending a DTMF digit.
    async fn send_digit_end(
        &self,
        _channel: &mut Channel,
        _digit: char,
        _duration: u32,
    ) -> AsteriskResult<()> {
        Ok(())
    }

    /// Send text to the channel.
    async fn send_text(&self, _channel: &mut Channel, _text: &str) -> AsteriskResult<()> {
        Err(asterisk_types::AsteriskError::NotSupported(
            "send_text not supported".into(),
        ))
    }

    /// Fixup after masquerade.
    async fn fixup(
        &self,
        _old_channel: &Channel,
        _new_channel: &mut Channel,
    ) -> AsteriskResult<()> {
        Ok(())
    }
}
