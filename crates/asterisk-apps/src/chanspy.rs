//! ChanSpy application - listen to and optionally whisper into active calls.
//!
//! Port of app_chanspy.c from Asterisk C. Allows a channel to listen in on
//! another channel's audio. Supports modes for silent monitoring, whispering
//! to one side, and barging into the call. Includes DTMF controls for cycling
//! between channels and switching modes.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

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

        info!(
            "ChanSpy: channel '{}' starting spy (prefix={:?}, mode={})",
            channel.name,
            prefix,
            options.initial_mode().as_str()
        );

        // Answer the channel if needed (unless N option)
        if !options.no_answer && channel.state != ChannelState::Up {
            debug!("ChanSpy: answering spy channel '{}'", channel.name);
            channel.state = ChannelState::Up;
        }

        let _mode = options.initial_mode();

        // In a real implementation:
        //
        //   // Find channels matching prefix/group/enforced extensions
        //   let mut targets = find_spy_targets(&prefix, &options);
        //
        //   loop {
        //       let target = match targets.next() {
        //           Some(t) => t,
        //           None => break, // no more channels to spy on
        //       };
        //
        //       // Skip our own channel
        //       if target.unique_id == channel.unique_id { continue; }
        //
        //       // Check group filter
        //       if !options.groups.is_empty() {
        //           let target_groups = target.get_variable("SPYGROUP")
        //               .unwrap_or_default()
        //               .split(':')
        //               .collect::<Vec<_>>();
        //           if !options.groups.iter().any(|g| target_groups.contains(&g.as_str())) {
        //               continue;
        //           }
        //       }
        //
        //       // Announce channel name if not quiet
        //       if !options.quiet {
        //           if options.say_name {
        //               say_channel_name(channel, &target).await;
        //           }
        //           play_file(channel, "beep").await;
        //       }
        //
        //       // Attach spy audiohook to target
        //       let audiohook = AudioHook::new(AudioHookType::Spy, "ChanSpy");
        //       if mode == SpyMode::Whisper || mode == SpyMode::Barge {
        //           audiohook.set_whisper(true);
        //       }
        //       target.attach_audiohook(audiohook);
        //
        //       // Main spy loop
        //       loop {
        //           select! {
        //               frame = audiohook.read_mixed() => {
        //                   // Write mixed audio to spy channel
        //                   channel.write_frame(&frame);
        //               }
        //               frame = channel.read_frame() => {
        //                   match frame.frame_type {
        //                       FrameType::Voice if mode != SpyMode::Listen => {
        //                           // Forward voice to target (whisper/barge)
        //                           audiohook.write_whisper(&frame);
        //                       }
        //                       FrameType::DtmfEnd => {
        //                           let digit = frame.subclass as u8 as char;
        //                           if digit == options.cycle_digit {
        //                               break; // move to next channel
        //                           }
        //                           if options.dtmf_switch {
        //                               match digit {
        //                                   '4' => mode = SpyMode::Listen,
        //                                   '5' => mode = SpyMode::Whisper,
        //                                   '6' => mode = SpyMode::Barge,
        //                                   _ => {}
        //                               }
        //                           }
        //                       }
        //                       _ => {}
        //                   }
        //               }
        //               _ = target.hangup_signal() => {
        //                   if options.exit_on_hangup {
        //                       return ChanSpyResult::TargetHangup;
        //                   }
        //                   break; // move to next channel
        //               }
        //               _ = channel.hangup_signal() => {
        //                   return ChanSpyResult::Hangup;
        //               }
        //           }
        //       }
        //
        //       target.detach_audiohook("ChanSpy");
        //   }

        info!(
            "ChanSpy: channel '{}' finished spying",
            channel.name
        );

        (PbxExecResult::Success, ChanSpyResult::Normal)
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

        info!(
            "ExtenSpy: channel '{}' spying on extension {}@{} (mode={})",
            channel.name,
            exten,
            context_str,
            options.initial_mode().as_str()
        );

        // Answer if needed
        if !options.no_answer && channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // In a real implementation, we would:
        // 1. Find channels where channel.exten matches exten and channel.context matches context
        // 2. Apply the same spy logic as ChanSpy but filtering by extension

        (PbxExecResult::Success, ChanSpyResult::Normal)
    }
}

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

    #[tokio::test]
    async fn test_chanspy_exec() {
        let mut channel = Channel::new("SIP/spy-001");
        let (result, status) = AppChanSpy::exec(&mut channel, "SIP,q").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(status, ChanSpyResult::Normal);
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
}
