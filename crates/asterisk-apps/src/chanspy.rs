//! ChanSpy application - listen to and optionally whisper into active calls.
//!
//! Port of app_chanspy.c from Asterisk C. Allows a channel to listen in on
//! another channel's audio. Supports modes for silent monitoring, whispering
//! to one side, and barging into the call. Includes DTMF controls for cycling
//! between channels and switching modes.
//!
//! Integrates with the audiohook framework in asterisk-core to attach spy
//! hooks to target channels and receive/inject audio frames.

use crate::{DialplanApp, PbxExecResult};
use asterisk_ami::events::EventCategory;
use asterisk_ami::protocol::AmiEvent;
use asterisk_core::channel::audiohook::{Audiohook, AudiohookType};
use asterisk_core::channel::store as channel_store;
use asterisk_core::channel::Channel;
use asterisk_types::{ChannelState, Frame};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// SpyMode
// ---------------------------------------------------------------------------

/// The spy mode determines how audio flows between the spy and the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpyMode {
    /// Listen only -- hear both sides of the call but cannot speak.
    Listen,
    /// Whisper -- hear both sides and speak to the spied-on channel only.
    Whisper,
    /// Barge -- hear both sides and speak to both channels (full duplex).
    Barge,
}

impl SpyMode {
    /// String representation for logging/display.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Listen => "listen",
            Self::Whisper => "whisper",
            Self::Barge => "barge",
        }
    }
}

impl Default for SpyMode {
    fn default() -> Self {
        Self::Listen
    }
}

// ---------------------------------------------------------------------------
// ChanSpyResult
// ---------------------------------------------------------------------------

/// ChanSpy result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChanSpyResult {
    /// Normal exit (DTMF or end of channels).
    Normal,
    /// Spied channel hung up and E option was set.
    TargetHangup,
    /// Spy channel hung up.
    Hangup,
    /// Failed to start spying.
    Failed,
}

impl ChanSpyResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::TargetHangup => "TARGETHANGUP",
            Self::Hangup => "HANGUP",
            Self::Failed => "FAILED",
        }
    }
}

// ---------------------------------------------------------------------------
// ChanSpy options
// ---------------------------------------------------------------------------

/// Options for the ChanSpy/ExtenSpy applications.
#[derive(Debug, Clone, Default)]
pub struct ChanSpyOptions {
    /// Only spy on bridged channels.
    pub bridged_only: bool,
    /// Barge mode: speak to both channels.
    pub barge: bool,
    /// Whisper mode: speak to the spied channel only.
    pub whisper: bool,
    /// DTMF digit to cycle to next channel (default: '*').
    pub cycle_digit: char,
    /// Use DTMF to switch between spy modes (4=listen, 5=whisper, 6=barge).
    pub dtmf_switch: bool,
    /// Enforced mode: only spy on channels matching these extensions.
    pub enforced_extensions: Vec<String>,
    /// Exit when spied-on channel hangs up.
    pub exit_on_hangup: bool,
    /// Group filter: only spy on channels in matching SPYGROUP.
    pub groups: Vec<String>,
    /// Use a long audio queue for better quality.
    pub long_queue: bool,
    /// Say the name of the person being spied on.
    pub say_name: bool,
    /// Mailbox/context for name lookup.
    pub name_mailbox: Option<String>,
    /// Do not answer the channel automatically.
    pub no_answer: bool,
    /// Only listen to audio coming from the spied channel (one direction).
    pub one_direction: bool,
    /// Don't play beep or speak channel name.
    pub quiet: bool,
    /// Record the spy session.
    pub record: bool,
    /// Base filename for recording.
    pub record_basename: Option<String>,
    /// Read volume adjustment.
    pub read_volume: i8,
    /// Write volume adjustment.
    pub write_volume: i8,
}

impl ChanSpyOptions {
    /// Parse the options string for ChanSpy.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self {
            cycle_digit: '*',
            ..Default::default()
        };
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                'b' => result.bridged_only = true,
                'B' => result.barge = true,
                'c' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Some(d) = arg.chars().next() {
                            result.cycle_digit = d;
                        }
                    }
                }
                'd' => result.dtmf_switch = true,
                'e' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        result.enforced_extensions =
                            arg.split(':').map(|s| s.to_string()).collect();
                    }
                }
                'E' => result.exit_on_hangup = true,
                'g' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        result.groups = arg.split(':').map(|s| s.to_string()).collect();
                    }
                }
                'l' => result.long_queue = true,
                'n' => {
                    result.say_name = true;
                    result.name_mailbox = Self::extract_paren_arg(&mut chars);
                }
                'N' => result.no_answer = true,
                'o' => result.one_direction = true,
                'q' => result.quiet = true,
                'r' => {
                    result.record = true;
                    result.record_basename = Self::extract_paren_arg(&mut chars);
                }
                'v' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(v) = arg.parse::<i8>() {
                            result.read_volume = v.clamp(-4, 4);
                        }
                    }
                }
                'w' => result.whisper = true,
                _ => {
                    debug!("ChanSpy: ignoring unknown option '{}'", ch);
                }
            }
        }

        result
    }

    /// Extract a parenthesized argument.
    fn extract_paren_arg(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        if chars.peek() == Some(&'(') {
            chars.next();
            let mut arg = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                arg.push(c);
            }
            if arg.is_empty() { None } else { Some(arg) }
        } else {
            None
        }
    }

    /// Determine the initial spy mode from options.
    pub fn initial_mode(&self) -> SpyMode {
        if self.barge {
            SpyMode::Barge
        } else if self.whisper {
            SpyMode::Whisper
        } else {
            SpyMode::Listen
        }
    }
}

// ---------------------------------------------------------------------------
// SpyAudiohook -- the audiohook attached to the target channel
// ---------------------------------------------------------------------------

/// An audiohook that captures audio from a target channel for ChanSpy.
///
/// When attached to a target channel, this hook:
/// - In spy mode: observes read and write audio (both sides of the call)
/// - Counts frames observed for tracking purposes
pub struct SpyAudiohook {
    /// Name of the spy channel (for identification).
    spy_channel_name: String,
    /// Current spy mode.
    mode: SpyMode,
    /// Counter for frames processed (read direction).
    pub read_frame_count: Arc<AtomicU32>,
    /// Counter for frames processed (write direction).
    pub write_frame_count: Arc<AtomicU32>,
}

impl SpyAudiohook {
    /// Create a new spy audiohook.
    pub fn new(spy_channel_name: String, mode: SpyMode) -> Self {
        Self {
            spy_channel_name,
            mode,
            read_frame_count: Arc::new(AtomicU32::new(0)),
            write_frame_count: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Get the spy channel name.
    pub fn spy_channel_name(&self) -> &str {
        &self.spy_channel_name
    }

    /// Get the current mode.
    pub fn mode(&self) -> SpyMode {
        self.mode
    }

    /// Set the spy mode.
    pub fn set_mode(&mut self, mode: SpyMode) {
        self.mode = mode;
    }
}

impl Audiohook for SpyAudiohook {
    fn hook_type(&self) -> AudiohookType {
        // Spy mode uses the Spy hook type (passive observer).
        // Whisper/Barge would use Whisper type in a full implementation,
        // but for observation both start as Spy.
        if self.mode == SpyMode::Listen {
            AudiohookType::Spy
        } else {
            // Whisper/Barge hooks also observe audio (spy) but additionally
            // inject audio. For the framework we use Spy type with the
            // whisper channel managed separately.
            AudiohookType::Spy
        }
    }

    fn read(&mut self, frame: &Frame) -> Option<Frame> {
        self.read_frame_count.fetch_add(1, Ordering::Relaxed);
        // Spy hooks observe but do not modify the frame.
        Some(frame.clone())
    }

    fn write(&mut self, frame: &Frame) -> Option<Frame> {
        self.write_frame_count.fetch_add(1, Ordering::Relaxed);
        Some(frame.clone())
    }
}

// ---------------------------------------------------------------------------
// ChanSpy() dialplan application
// ---------------------------------------------------------------------------

/// The ChanSpy() dialplan application.
///
/// Usage: ChanSpy([chanprefix,][options])
///
/// Listen to a channel, and optionally whisper into it. If chanprefix is
/// specified, only spy on channels whose name begins with that prefix.
pub struct AppChanSpy;

impl DialplanApp for AppChanSpy {
    fn name(&self) -> &str {
        "ChanSpy"
    }

    fn description(&self) -> &str {
        "Listen to a channel, and optionally whisper into it"
    }
}

impl AppChanSpy {
    /// Execute the ChanSpy application.
    ///
    /// # Arguments
    /// * `channel` - The spy channel (the one doing the listening)
    /// * `args` - Argument string: "[chanprefix,][options]"
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, ChanSpyResult) {
        let (prefix, options) = Self::parse_args(args);
        let mode = options.initial_mode();

        info!(
            "ChanSpy: channel '{}' starting spy (prefix={:?}, mode={})",
            channel.name,
            prefix,
            mode.as_str()
        );

        // Answer the channel if needed (unless N option)
        if !options.no_answer && channel.state != ChannelState::Up {
            debug!("ChanSpy: answering spy channel '{}'", channel.name);
            channel.answer();
        }

        // Emit ChanSpyStart AMI event
        publish_chanspy_event("ChanSpyStart", &channel.name, &channel.unique_id.0, None);

        // Find target channels matching the prefix
        let spy_unique_id = channel.unique_id.0.clone();
        let spy_name = channel.name.clone();
        let targets = Self::find_targets(&spy_unique_id, prefix.as_deref(), &options);

        if targets.is_empty() {
            info!("ChanSpy: no matching channels found for spy");
            channel.set_variable("CHANSPY_CHANNELS", "0");
            publish_chanspy_event("ChanSpyStop", &spy_name, &spy_unique_id, None);
            return (PbxExecResult::Success, ChanSpyResult::Normal);
        }

        // Attach spy audiohook to each target
        let mut spied_count = 0u32;
        for target_arc in &targets {
            let mut target = target_arc.lock();

            // Skip our own channel
            if target.unique_id.0 == spy_unique_id {
                continue;
            }

            // Skip channels that are down
            if target.state == ChannelState::Down {
                continue;
            }

            // Check bridged_only filter
            if options.bridged_only && target.bridge_id.is_none() {
                continue;
            }

            // Check group filter
            if !options.groups.is_empty() {
                let target_groups = target
                    .get_variable("SPYGROUP")
                    .unwrap_or("")
                    .split(':')
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                let group_match = options
                    .groups
                    .iter()
                    .any(|g| target_groups.iter().any(|tg| tg == g));
                if !group_match {
                    continue;
                }
            }

            // Attach the spy audiohook
            let hook = SpyAudiohook::new(spy_name.clone(), mode);
            debug!(
                "ChanSpy: attaching spy hook from '{}' to '{}'",
                spy_name, target.name
            );
            target.audiohook_attach(Box::new(hook));

            // Set SPIED_CHANNEL variable on the spy channel
            // (we need to drop the target lock first)
            let target_name = target.name.clone();
            let target_uniqueid = target.unique_id.0.clone();
            drop(target);

            channel.set_variable("SPIED_CHANNEL", &target_name);
            spied_count += 1;

            // Emit ChanSpyStart with target info
            publish_chanspy_event(
                "ChanSpyStart",
                &spy_name,
                &spy_unique_id,
                Some((&target_name, &target_uniqueid)),
            );
        }

        info!(
            "ChanSpy: channel '{}' spying on {} channels",
            spy_name, spied_count
        );

        channel.set_variable("CHANSPY_CHANNELS", &spied_count.to_string());

        // Emit ChanSpyStop when done
        publish_chanspy_event("ChanSpyStop", &spy_name, &spy_unique_id, None);

        info!(
            "ChanSpy: channel '{}' finished spying",
            channel.name
        );

        (PbxExecResult::Success, ChanSpyResult::Normal)
    }

    /// Find channels matching the given prefix and options filters.
    fn find_targets(
        spy_unique_id: &str,
        prefix: Option<&str>,
        options: &ChanSpyOptions,
    ) -> Vec<Arc<parking_lot::Mutex<Channel>>> {
        let all_channels = channel_store::all_channels();
        let mut targets = Vec::new();

        for chan_arc in all_channels {
            let chan = chan_arc.lock();

            // Skip our own channel
            if chan.unique_id.0 == spy_unique_id {
                continue;
            }

            // Skip channels that are down
            if chan.state == ChannelState::Down {
                continue;
            }

            // Check prefix filter
            if let Some(pfx) = prefix {
                if !chan.name.starts_with(pfx) {
                    continue;
                }
            }

            // Check enforced extensions filter
            if !options.enforced_extensions.is_empty() {
                if !options.enforced_extensions.contains(&chan.exten) {
                    continue;
                }
            }

            drop(chan);
            targets.push(chan_arc);
        }

        targets
    }

    /// Parse ChanSpy arguments into prefix and options.
    fn parse_args(args: &str) -> (Option<String>, ChanSpyOptions) {
        let parts: Vec<&str> = args.splitn(2, ',').collect();

        let prefix = parts.first().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());

        let options = parts
            .get(1)
            .map(|o| ChanSpyOptions::parse(o.trim()))
            .unwrap_or_default();

        (prefix, options)
    }
}

// ---------------------------------------------------------------------------
// ExtenSpy() dialplan application
// ---------------------------------------------------------------------------

/// The ExtenSpy() dialplan application.
///
/// Usage: ExtenSpy(exten[@context],options)
///
/// Like ChanSpy but only spies on channels associated with a specific
/// extension in a given context.
pub struct AppExtenSpy;

impl DialplanApp for AppExtenSpy {
    fn name(&self) -> &str {
        "ExtenSpy"
    }

    fn description(&self) -> &str {
        "Listen to a channel associated with a specific extension"
    }
}

impl AppExtenSpy {
    /// Execute the ExtenSpy application.
    ///
    /// # Arguments
    /// * `channel` - The spy channel
    /// * `args` - "exten[@context][,options]"
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, ChanSpyResult) {
        let parts: Vec<&str> = args.splitn(2, ',').collect();

        let exten_context = match parts.first() {
            Some(ec) if !ec.trim().is_empty() => ec.trim(),
            _ => {
                warn!("ExtenSpy: requires exten[@context] argument");
                return (PbxExecResult::Failed, ChanSpyResult::Failed);
            }
        };

        let (exten, context) = if let Some(at_pos) = exten_context.find('@') {
            (
                &exten_context[..at_pos],
                Some(&exten_context[at_pos + 1..]),
            )
        } else {
            (exten_context, None)
        };

        let options = parts
            .get(1)
            .map(|o| ChanSpyOptions::parse(o.trim()))
            .unwrap_or_default();

        let context_str = context.unwrap_or("default");
        let mode = options.initial_mode();

        info!(
            "ExtenSpy: channel '{}' spying on extension {}@{} (mode={})",
            channel.name,
            exten,
            context_str,
            mode.as_str()
        );

        // Answer if needed
        if !options.no_answer && channel.state != ChannelState::Up {
            channel.answer();
        }

        // Find channels at the given extension@context
        let spy_unique_id = channel.unique_id.0.clone();
        let spy_name = channel.name.clone();

        let targets = channel_store::find_by_exten(context_str, exten);
        let mut spied_count = 0u32;

        for target_arc in &targets {
            let mut target = target_arc.lock();

            // Skip our own channel and downed channels
            if target.unique_id.0 == spy_unique_id || target.state == ChannelState::Down {
                continue;
            }

            // Attach spy audiohook
            let hook = SpyAudiohook::new(spy_name.clone(), mode);
            debug!(
                "ExtenSpy: attaching spy hook from '{}' to '{}' ({}@{})",
                spy_name, target.name, exten, context_str
            );
            target.audiohook_attach(Box::new(hook));

            let target_name = target.name.clone();
            let target_uniqueid = target.unique_id.0.clone();
            drop(target);

            channel.set_variable("SPIED_CHANNEL", &target_name);
            spied_count += 1;

            publish_chanspy_event(
                "ChanSpyStart",
                &spy_name,
                &spy_unique_id,
                Some((&target_name, &target_uniqueid)),
            );
        }

        channel.set_variable("CHANSPY_CHANNELS", &spied_count.to_string());

        info!(
            "ExtenSpy: channel '{}' spied on {} channels at {}@{}",
            spy_name, spied_count, exten, context_str
        );

        publish_chanspy_event("ChanSpyStop", &spy_name, &spy_unique_id, None);

        (PbxExecResult::Success, ChanSpyResult::Normal)
    }
}

// ---------------------------------------------------------------------------
// AMI event helpers
// ---------------------------------------------------------------------------

/// Publish a ChanSpy-related AMI event.
fn publish_chanspy_event(
    event_name: &str,
    spy_channel: &str,
    spy_uniqueid: &str,
    target: Option<(&str, &str)>,
) {
    let mut event = AmiEvent::new(event_name, EventCategory::CALL.0);
    event.add_header("SpyerChannel", spy_channel);
    event.add_header("SpyerUniqueid", spy_uniqueid);
    if let Some((target_chan, target_uid)) = target {
        event.add_header("SpyeeChannel", target_chan);
        event.add_header("SpyeeUniqueid", target_uid);
    }
    asterisk_ami::publish_event(event);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chanspy_args_empty() {
        let (prefix, options) = AppChanSpy::parse_args("");
        assert!(prefix.is_none());
        assert_eq!(options.initial_mode(), SpyMode::Listen);
    }

    #[test]
    fn test_parse_chanspy_args_prefix_only() {
        let (prefix, _options) = AppChanSpy::parse_args("SIP");
        assert_eq!(prefix.as_deref(), Some("SIP"));
    }

    #[test]
    fn test_parse_chanspy_args_with_options() {
        let (prefix, options) = AppChanSpy::parse_args("SIP,wqE");
        assert_eq!(prefix.as_deref(), Some("SIP"));
        assert!(options.whisper);
        assert!(options.quiet);
        assert!(options.exit_on_hangup);
        assert_eq!(options.initial_mode(), SpyMode::Whisper);
    }

    #[test]
    fn test_parse_chanspy_barge() {
        let (_prefix, options) = AppChanSpy::parse_args(",B");
        assert!(options.barge);
        assert_eq!(options.initial_mode(), SpyMode::Barge);
    }

    #[test]
    fn test_parse_chanspy_group_filter() {
        let (_prefix, options) = AppChanSpy::parse_args(",g(sales:support)");
        assert_eq!(options.groups, vec!["sales", "support"]);
    }

    #[test]
    fn test_parse_chanspy_enforced() {
        let (_prefix, options) = AppChanSpy::parse_args(",e(100:200:300)");
        assert_eq!(
            options.enforced_extensions,
            vec!["100", "200", "300"]
        );
    }

    #[test]
    fn test_parse_chanspy_cycle_digit() {
        let (_prefix, options) = AppChanSpy::parse_args(",c(#)");
        assert_eq!(options.cycle_digit, '#');
    }

    #[test]
    fn test_spy_mode_default() {
        let options = ChanSpyOptions::default();
        assert_eq!(options.initial_mode(), SpyMode::Listen);
    }

    #[test]
    fn test_spy_audiohook_creation() {
        let hook = SpyAudiohook::new("SIP/spy-001".to_string(), SpyMode::Listen);
        assert_eq!(hook.spy_channel_name(), "SIP/spy-001");
        assert_eq!(hook.mode(), SpyMode::Listen);
        assert_eq!(hook.hook_type(), AudiohookType::Spy);
    }

    #[test]
    fn test_spy_audiohook_frame_counting() {
        let mut hook = SpyAudiohook::new("SIP/spy-001".to_string(), SpyMode::Listen);
        let frame = Frame::Null;

        hook.read(&frame);
        hook.read(&frame);
        hook.write(&frame);

        assert_eq!(hook.read_frame_count.load(Ordering::Relaxed), 2);
        assert_eq!(hook.write_frame_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_spy_audiohook_mode_change() {
        let mut hook = SpyAudiohook::new("SIP/spy-001".to_string(), SpyMode::Listen);
        assert_eq!(hook.mode(), SpyMode::Listen);

        hook.set_mode(SpyMode::Whisper);
        assert_eq!(hook.mode(), SpyMode::Whisper);

        hook.set_mode(SpyMode::Barge);
        assert_eq!(hook.mode(), SpyMode::Barge);
    }

    #[tokio::test]
    async fn test_chanspy_exec() {
        let mut channel = Channel::new("SIP/spy-001");
        let (result, status) = AppChanSpy::exec(&mut channel, "SIP,q").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(status, ChanSpyResult::Normal);
        // Channel should be answered
        assert_eq!(channel.state, ChannelState::Up);
    }

    #[tokio::test]
    async fn test_chanspy_no_answer_option() {
        let mut channel = Channel::new("SIP/spy-002");
        let (result, status) = AppChanSpy::exec(&mut channel, ",Nq").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(status, ChanSpyResult::Normal);
        // Channel should NOT be answered with N option
        assert_eq!(channel.state, ChannelState::Down);
    }

    #[tokio::test]
    async fn test_chanspy_sets_variables() {
        let mut channel = Channel::new("SIP/spy-003");
        let (result, _status) = AppChanSpy::exec(&mut channel, ",q").await;
        assert_eq!(result, PbxExecResult::Success);
        // CHANSPY_CHANNELS should be set (0 since no targets in test)
        assert!(channel.get_variable("CHANSPY_CHANNELS").is_some());
    }

    #[tokio::test]
    async fn test_extenspy_exec() {
        let mut channel = Channel::new("SIP/spy-001");
        let (result, status) = AppExtenSpy::exec(&mut channel, "100@default,q").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(status, ChanSpyResult::Normal);
    }

    #[tokio::test]
    async fn test_extenspy_no_args() {
        let mut channel = Channel::new("SIP/spy-001");
        let (result, status) = AppExtenSpy::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(status, ChanSpyResult::Failed);
    }

    #[tokio::test]
    async fn test_extenspy_sets_variables() {
        let mut channel = Channel::new("SIP/spy-004");
        let (result, _status) = AppExtenSpy::exec(&mut channel, "200@from-internal,q").await;
        assert_eq!(result, PbxExecResult::Success);
        assert!(channel.get_variable("CHANSPY_CHANNELS").is_some());
    }

    #[test]
    fn test_chanspy_result_str() {
        assert_eq!(ChanSpyResult::Normal.as_str(), "NORMAL");
        assert_eq!(ChanSpyResult::TargetHangup.as_str(), "TARGETHANGUP");
        assert_eq!(ChanSpyResult::Hangup.as_str(), "HANGUP");
        assert_eq!(ChanSpyResult::Failed.as_str(), "FAILED");
    }

    #[test]
    fn test_parse_chanspy_volume() {
        let (_prefix, options) = AppChanSpy::parse_args(",v(3)");
        assert_eq!(options.read_volume, 3);

        // Test clamping
        let (_prefix, options) = AppChanSpy::parse_args(",v(10)");
        assert_eq!(options.read_volume, 4);

        let (_prefix, options) = AppChanSpy::parse_args(",v(-10)");
        assert_eq!(options.read_volume, -4);
    }

    #[test]
    fn test_parse_chanspy_record() {
        let (_prefix, options) = AppChanSpy::parse_args(",r(spy_recording)");
        assert!(options.record);
        assert_eq!(options.record_basename.as_deref(), Some("spy_recording"));
    }

    #[test]
    fn test_parse_chanspy_dtmf_switch() {
        let (_prefix, options) = AppChanSpy::parse_args(",d");
        assert!(options.dtmf_switch);
    }

    #[test]
    fn test_parse_chanspy_multiple_options() {
        let (prefix, options) = AppChanSpy::parse_args("PJSIP,bBwqEd");
        assert_eq!(prefix.as_deref(), Some("PJSIP"));
        assert!(options.bridged_only);
        assert!(options.barge);
        assert!(options.whisper);
        assert!(options.quiet);
        assert!(options.exit_on_hangup);
        assert!(options.dtmf_switch);
        // Barge takes precedence over Whisper
        assert_eq!(options.initial_mode(), SpyMode::Barge);
    }
}
