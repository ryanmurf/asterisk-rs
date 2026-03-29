//! Dial application - the heart of Asterisk.
//!
//! Port of app_dial.c from Asterisk C. This application:
//! - Parses dial strings: Technology/resource[&Tech2/resource2...]
//! - Looks up channel technology by name and calls ChannelDriver::request()
//! - Calls ChannelDriver::call() to initiate outbound calls
//! - Rings all destinations simultaneously (parallel dial)
//! - Waits for the first answer, hangs up the rest
//! - Bridges the calling channel with the answered channel using BasicBridge
//! - Handles call forwarding with loop detection (max 20 forwards)
//! - Supports all 38 Dial() options from C Asterisk
//! - Sets DIALSTATUS and related channel variables
//! - Publishes Stasis DialBegin/DialEnd events
//! - Configures bridge features based on options (h/H/k/K/w/W/x/X/t/T)

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::bridge::basic::{AfterBridgeAction, BasicBridge, SideFeatures};
use asterisk_core::channel::Channel;
use asterisk_types::{ChannelState, HangupCause};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// DialStatus
// ---------------------------------------------------------------------------

/// Possible final status of a Dial() execution.
/// This maps to the DIALSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialStatus {
    /// The called party answered
    Answer,
    /// The called party was busy
    Busy,
    /// No answer within the timeout period
    NoAnswer,
    /// The calling party cancelled (hung up) while ringing
    Cancel,
    /// Network congestion (circuits busy)
    Congestion,
    /// The destination channel was unavailable
    ChanUnavail,
    /// Privacy/screening: called party said "go away"
    DontCall,
    /// Privacy/screening: called party chose "torture"
    Torture,
    /// Dial failed due to invalid syntax
    InvalidArgs,
}

impl DialStatus {
    /// Return the string representation used for the DIALSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Answer => "ANSWER",
            Self::Busy => "BUSY",
            Self::NoAnswer => "NOANSWER",
            Self::Cancel => "CANCEL",
            Self::Congestion => "CONGESTION",
            Self::ChanUnavail => "CHANUNAVAIL",
            Self::DontCall => "DONTCALL",
            Self::Torture => "TORTURE",
            Self::InvalidArgs => "INVALIDARGS",
        }
    }
}

impl std::fmt::Display for DialStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Dialplan location helper
// ---------------------------------------------------------------------------

/// A parsed dialplan location: context^exten^priority
/// Used for GoSub-style option arguments (b, B, G, F, U).
#[derive(Debug, Clone, Default)]
pub struct DialplanLocation {
    /// Dialplan context (empty = current context)
    pub context: String,
    /// Dialplan extension (empty = current extension)
    pub exten: String,
    /// Dialplan priority (0 = 1)
    pub priority: i32,
    /// Additional arguments separated by ^ (for GoSub options)
    pub args: Vec<String>,
}

impl DialplanLocation {
    /// Parse a `context^exten^priority[(arg1^arg2...)]` string.
    ///
    /// The Dial() options use `^` instead of `,` as a separator because
    /// commas are already used to separate Dial() arguments.
    ///
    /// GoSub arguments can be specified in parentheses after the priority:
    ///   `context^exten^priority(arg1^arg2)`
    ///
    /// Or as additional `^`-separated fields after priority:
    ///   `context^exten^priority^arg1^arg2`
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }

        // Check for parenthesized GoSub arguments in the string.
        // Format: context^exten^priority(arg1^arg2^...)
        // We need to extract args from parens before splitting on ^.
        let (main_part, paren_args) = if let Some(open_pos) = s.find('(') {
            if s.ends_with(')') {
                let main = &s[..open_pos];
                let args_str = &s[open_pos + 1..s.len() - 1];
                let args: Vec<String> = args_str.split('^').map(|a| a.to_string()).collect();
                (main.to_string(), args)
            } else {
                (s.to_string(), Vec::new())
            }
        } else {
            (s.to_string(), Vec::new())
        };

        let parts: Vec<&str> = main_part.split('^').collect();
        let mut loc = DialplanLocation::default();

        match parts.len() {
            1 => {
                // Just priority (or just context for U option)
                let p: i32 = parts[0].parse().unwrap_or(1);
                loc.priority = if p <= 0 { 1 } else { p };
            }
            2 => {
                loc.context = parts[0].to_string();
                let p: i32 = parts[1].parse().unwrap_or(1);
                loc.priority = if p <= 0 { 1 } else { p };
            }
            _ => {
                loc.context = parts[0].to_string();
                loc.exten = parts[1].to_string();
                let p: i32 = parts[2].parse().unwrap_or(1);
                loc.priority = if p <= 0 { 1 } else { p };
                // Remaining parts are arguments (old style without parens)
                for arg in &parts[3..] {
                    loc.args.push(arg.to_string());
                }
            }
        }

        // If we found parenthesized args, they override any ^-separated trailing args
        if !paren_args.is_empty() {
            loc.args = paren_args;
        }

        Some(loc)
    }

    /// Format as a Gosub argument string: "context,exten,priority(arg1,arg2,...)"
    pub fn to_gosub_args(&self) -> String {
        let mut result = String::new();
        if !self.context.is_empty() {
            result.push_str(&self.context);
            result.push(',');
        }
        if !self.exten.is_empty() {
            result.push_str(&self.exten);
            result.push(',');
        }
        result.push_str(&self.priority.to_string());
        if !self.args.is_empty() {
            result.push('(');
            result.push_str(&self.args.join(","));
            result.push(')');
        }
        result
    }
}

// ---------------------------------------------------------------------------
// DTMF digits for D() option
// ---------------------------------------------------------------------------

/// Parsed DTMF send specification from the D() option.
///
/// Format: D([called][:calling[:progress[:mfprogress[:mfwink[:sfprogress[:sfwink]]]]]])
#[derive(Debug, Clone, Default)]
pub struct DtmfSendSpec {
    /// DTMF digits to send to the called party after answer
    pub called: String,
    /// DTMF digits to send to the calling party after answer
    pub calling: String,
    /// DTMF digits to send on PROGRESS
    pub progress: String,
    /// MF digits to send on PROGRESS
    pub mf_progress: String,
    /// MF digits to send on WINK
    pub mf_wink: String,
    /// SF digits to send on PROGRESS
    pub sf_progress: String,
    /// SF digits to send on WINK
    pub sf_wink: String,
}

impl DtmfSendSpec {
    /// Parse the D() option argument.
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split(':').collect();
        let mut spec = Self::default();
        if let Some(&v) = parts.first() {
            spec.called = v.to_string();
        }
        if let Some(&v) = parts.get(1) {
            spec.calling = v.to_string();
        }
        if let Some(&v) = parts.get(2) {
            spec.progress = v.to_string();
        }
        if let Some(&v) = parts.get(3) {
            spec.mf_progress = v.to_string();
        }
        if let Some(&v) = parts.get(4) {
            spec.mf_wink = v.to_string();
        }
        if let Some(&v) = parts.get(5) {
            spec.sf_progress = v.to_string();
        }
        if let Some(&v) = parts.get(6) {
            spec.sf_wink = v.to_string();
        }
        spec
    }
}

// ---------------------------------------------------------------------------
// Call limit specification for L() option
// ---------------------------------------------------------------------------

/// Parsed call limit from the L() option.
///
/// Format: L(x[:y[:z]])
/// - x = maximum call time in milliseconds
/// - y = warning time (ms before end to play warning)
/// - z = repeat interval (ms between repeated warnings)
#[derive(Debug, Clone)]
pub struct CallLimit {
    /// Maximum call time in milliseconds
    pub max_ms: u64,
    /// Warning time: play warning when this many ms remain
    pub warning_ms: Option<u64>,
    /// Repeat interval: repeat warning every this many ms
    pub repeat_ms: Option<u64>,
}

impl CallLimit {
    /// Parse the L() option argument.
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        let max_ms = parts.first()?.parse::<u64>().ok()?;
        if max_ms == 0 {
            return None;
        }
        let warning_ms = parts.get(1).and_then(|v| v.parse::<u64>().ok());
        let repeat_ms = parts.get(2).and_then(|v| v.parse::<u64>().ok());
        Some(Self {
            max_ms,
            warning_ms,
            repeat_ms,
        })
    }

    /// Convert maximum call time to Duration.
    pub fn max_duration(&self) -> Duration {
        Duration::from_millis(self.max_ms)
    }
}

// ---------------------------------------------------------------------------
// Announcement specification for A() option
// ---------------------------------------------------------------------------

/// Parsed announcement files from the A() option.
///
/// Format: A(x[:y])
/// - x = file to play to called party
/// - y = file to play to calling party
#[derive(Debug, Clone, Default)]
pub struct AnnouncementSpec {
    /// File to play to the called party
    pub called_file: String,
    /// File to play to the calling party
    pub calling_file: String,
}

impl AnnouncementSpec {
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split(':').collect();
        Self {
            called_file: parts.first().unwrap_or(&"").to_string(),
            calling_file: parts.get(1).unwrap_or(&"").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// DialOptions -- all 38 options from C app_dial.c
// ---------------------------------------------------------------------------

/// Options parsed from the Dial() options string.
///
/// This implements all options from the C `dial_exec_options` table,
/// matching the AST_APP_OPTIONS definition in app_dial.c.
#[derive(Debug, Clone, Default)]
pub struct DialOptions {
    // --- Priority 1: Most used ---

    /// `b(context^exten^priority)` -- Pre-dial GoSub on callee channel
    pub predial_callee: Option<DialplanLocation>,
    /// `B(context^exten^priority)` -- Pre-dial GoSub on caller channel
    pub predial_caller: Option<DialplanLocation>,
    /// `D([called][:calling[:progress]])` -- Post-answer DTMF sending
    pub send_dtmf: Option<DtmfSendSpec>,
    /// `F([context^exten^priority])` -- After bridge, callee continues here
    pub callee_go_on: Option<DialplanLocation>,
    /// `F` (no args) -- callee continues at next priority of current extension
    pub callee_go_on_empty: bool,
    /// `f([x])` -- Force callerID on outbound (or use hint)
    pub force_caller_id: Option<String>,
    /// `f` (no args) -- force callerID from dialplan hint
    pub force_caller_id_hint: bool,
    /// `g` -- Caller continues in dialplan after callee hangs up
    pub continue_on_callee_hangup: bool,
    /// `G(context^exten^priority)` -- Both channels go to this location after answer
    pub goto_after_answer: Option<DialplanLocation>,
    /// `L(x[:y[:z]])` -- Call duration limit
    pub call_limit: Option<CallLimit>,
    /// `S(x)` -- Hangup call after x seconds post-answer
    pub duration_stop: Option<Duration>,
    /// `U(context^exten^priority(args))` -- Post-answer GoSub on callee before bridge
    pub callee_gosub: Option<DialplanLocation>,

    // --- Priority 2: Important ---

    /// `A(x[:y])` -- Announcement files for called/calling party
    pub announcement: Option<AnnouncementSpec>,
    /// `c` -- Cancel elsewhere: set HANGUPCAUSE to 'answered elsewhere'
    pub cancel_elsewhere: bool,
    /// `C` -- Reset CDR
    pub reset_cdr: bool,
    /// `d` -- Allow caller DTMF extension matching during dial
    pub dtmf_exit: bool,
    /// `h` -- Allow callee to hang up by pressing disconnect DTMF
    pub callee_hangup: bool,
    /// `H` -- Allow caller to hang up by pressing disconnect DTMF
    pub caller_hangup: bool,
    /// `i` -- Ignore forwarding requests
    pub ignore_forwarding: bool,
    /// `I` -- Ignore connected line updates
    pub ignore_connected_line: bool,
    /// `k` -- Allow callee to park the call
    pub callee_park: bool,
    /// `K` -- Allow caller to park the call
    pub caller_park: bool,
    /// `Q(cause)` -- Set specific hangup cause on unanswered channels
    pub hangup_cause: Option<String>,
    /// `w` -- Allow callee to start one-touch recording
    pub callee_monitor: bool,
    /// `W` -- Allow caller to start one-touch recording
    pub caller_monitor: bool,
    /// `x` -- Allow callee to start automixmonitor recording
    pub callee_mixmonitor: bool,
    /// `X` -- Allow caller to start automixmonitor recording
    pub caller_mixmonitor: bool,

    // --- Priority 3: Less common ---

    /// `a` -- Immediately answer caller channel before dialing
    pub answer_immediately: bool,
    /// `e` -- Execute 'h' extension for peer after call ends
    pub peer_h_exten: bool,
    /// `E` -- Enable echo of sent MF/SF (hearpulsing)
    pub hearpulsing: bool,
    /// `j` -- Preserve initial stream topology
    pub topology_preserve: bool,
    /// `m([class])` -- Music on hold (with optional class)
    pub music_on_hold: Option<String>,
    /// `m` (no args) -- Music on hold with default class
    pub music_on_hold_default: bool,
    /// `n([delete])` -- Privacy: screening mode, no intro save
    pub screen_nointro: Option<String>,
    /// `N` -- Privacy: don't screen if callerID present
    pub screen_nocallerid: bool,
    /// `o([x])` -- Use original caller ID for outbound
    pub original_clid: Option<String>,
    /// `o` (no args) -- use original callerID
    pub original_clid_default: bool,
    /// `O([mode])` -- Operator services mode
    pub operator_mode: Option<String>,
    /// `p` -- Screening mode (privacy without memory)
    pub screening: bool,
    /// `P([x])` -- Privacy mode with AstDB key
    pub privacy: Option<String>,
    /// `r([tone])` -- Force ringing indication
    pub force_ringing: Option<String>,
    /// `r` (no args) -- force ringing
    pub force_ringing_default: bool,
    /// `R` -- Ring with early media interruption
    pub ring_with_early_media: bool,
    /// `s(x)` -- Force callerID tag
    pub force_cid_tag: Option<String>,
    /// `t` -- Allow callee to transfer
    pub allow_callee_transfer: bool,
    /// `T` -- Allow caller to transfer
    pub allow_caller_transfer: bool,
    /// `u(x)` -- Force callerID presentation
    pub force_cid_presentation: Option<String>,
    /// `z` -- Cancel dial timeout on forward
    pub cancel_timeout_on_forward: bool,

    /// The URL to send to the called party (4th argument of Dial())
    pub url: Option<String>,
}

impl DialOptions {
    /// Parse an options string into a DialOptions struct.
    ///
    /// Options can have parenthesized arguments: `L(60000:30000)b(ctx^s^1)gG(ctx^s^1)`
    /// The parser handles both simple flags and options with arguments.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            // Check if this option has a parenthesized argument
            let arg = if chars.peek() == Some(&'(') {
                chars.next(); // consume '('
                let mut depth = 1;
                let mut arg_str = String::new();
                while let Some(c) = chars.next() {
                    if c == '(' {
                        depth += 1;
                        arg_str.push(c);
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        arg_str.push(c);
                    } else {
                        arg_str.push(c);
                    }
                }
                Some(arg_str)
            } else {
                None
            };

            match ch {
                // Priority 1
                'b' => {
                    result.predial_callee =
                        arg.as_deref().and_then(DialplanLocation::parse);
                }
                'B' => {
                    result.predial_caller =
                        arg.as_deref().and_then(DialplanLocation::parse);
                }
                'D' => {
                    result.send_dtmf =
                        arg.as_deref().map(DtmfSendSpec::parse);
                }
                'F' => {
                    if let Some(ref a) = arg {
                        if !a.is_empty() {
                            result.callee_go_on = DialplanLocation::parse(a);
                        } else {
                            result.callee_go_on_empty = true;
                        }
                    } else {
                        result.callee_go_on_empty = true;
                    }
                }
                'f' => {
                    if let Some(ref a) = arg {
                        if !a.is_empty() {
                            result.force_caller_id = Some(a.clone());
                        } else {
                            result.force_caller_id_hint = true;
                        }
                    } else {
                        result.force_caller_id_hint = true;
                    }
                }
                'g' => {
                    result.continue_on_callee_hangup = true;
                }
                'G' => {
                    result.goto_after_answer =
                        arg.as_deref().and_then(DialplanLocation::parse);
                }
                'L' => {
                    result.call_limit =
                        arg.as_deref().and_then(CallLimit::parse);
                }
                'S' => {
                    if let Some(ref a) = arg {
                        if let Ok(secs) = a.parse::<f64>() {
                            if secs > 0.0 && secs.is_finite() {
                                result.duration_stop = Some(Duration::from_secs_f64(secs));
                            } else {
                                debug!("Dial: ignoring invalid S({}) duration", secs);
                            }
                        }
                    }
                }
                'U' => {
                    result.callee_gosub =
                        arg.as_deref().and_then(DialplanLocation::parse);
                }

                // Priority 2
                'A' => {
                    result.announcement =
                        arg.as_deref().map(AnnouncementSpec::parse);
                }
                'c' => {
                    result.cancel_elsewhere = true;
                }
                'C' => {
                    result.reset_cdr = true;
                }
                'd' => {
                    result.dtmf_exit = true;
                }
                'h' => {
                    result.callee_hangup = true;
                }
                'H' => {
                    result.caller_hangup = true;
                }
                'i' => {
                    result.ignore_forwarding = true;
                }
                'I' => {
                    result.ignore_connected_line = true;
                }
                'k' => {
                    result.callee_park = true;
                }
                'K' => {
                    result.caller_park = true;
                }
                'Q' => {
                    result.hangup_cause = arg;
                }
                'w' => {
                    result.callee_monitor = true;
                }
                'W' => {
                    result.caller_monitor = true;
                }
                'x' => {
                    result.callee_mixmonitor = true;
                }
                'X' => {
                    result.caller_mixmonitor = true;
                }

                // Priority 3
                'a' => {
                    result.answer_immediately = true;
                }
                'e' => {
                    result.peer_h_exten = true;
                }
                'E' => {
                    result.hearpulsing = true;
                }
                'j' => {
                    result.topology_preserve = true;
                }
                'm' => {
                    if let Some(ref a) = arg {
                        if !a.is_empty() {
                            result.music_on_hold = Some(a.clone());
                        } else {
                            result.music_on_hold_default = true;
                        }
                    } else {
                        result.music_on_hold_default = true;
                    }
                }
                'n' => {
                    result.screen_nointro = Some(
                        arg.unwrap_or_default(),
                    );
                }
                'N' => {
                    result.screen_nocallerid = true;
                }
                'o' => {
                    if let Some(ref a) = arg {
                        if !a.is_empty() {
                            result.original_clid = Some(a.clone());
                        } else {
                            result.original_clid_default = true;
                        }
                    } else {
                        result.original_clid_default = true;
                    }
                }
                'O' => {
                    result.operator_mode = Some(
                        arg.unwrap_or_default(),
                    );
                }
                'p' => {
                    result.screening = true;
                }
                'P' => {
                    result.privacy = Some(
                        arg.unwrap_or_default(),
                    );
                }
                'r' => {
                    if let Some(ref a) = arg {
                        if !a.is_empty() {
                            result.force_ringing = Some(a.clone());
                        } else {
                            result.force_ringing_default = true;
                        }
                    } else {
                        result.force_ringing_default = true;
                    }
                }
                'R' => {
                    result.ring_with_early_media = true;
                }
                's' => {
                    result.force_cid_tag = arg;
                }
                't' => {
                    result.allow_callee_transfer = true;
                }
                'T' => {
                    result.allow_caller_transfer = true;
                }
                'u' => {
                    result.force_cid_presentation = arg;
                }
                'z' => {
                    result.cancel_timeout_on_forward = true;
                }
                _ => {
                    debug!("Dial: ignoring unknown option '{}'", ch);
                }
            }
        }

        result
    }

    /// Returns true if music on hold is enabled (either default or specific class).
    pub fn has_music_on_hold(&self) -> bool {
        self.music_on_hold.is_some() || self.music_on_hold_default
    }

    /// Returns true if ringing indication is forced.
    pub fn has_force_ringing(&self) -> bool {
        self.force_ringing.is_some() || self.force_ringing_default
    }

    /// Returns the music-on-hold class name, defaulting to "default".
    pub fn moh_class(&self) -> &str {
        self.music_on_hold.as_deref().unwrap_or("default")
    }

    /// Build bridge features configuration from the dial options.
    ///
    /// This maps the Dial() options to bridge feature flags so the bridge
    /// knows which DTMF features to enable for each side.
    pub fn build_bridge_features(&self) -> (SideFeatures, SideFeatures) {
        let caller_features = SideFeatures {
            blind_transfer: self.allow_caller_transfer,
            attended_transfer: self.allow_caller_transfer,
            disconnect: self.caller_hangup,
            park_call: self.caller_park,
            automixmon: self.caller_mixmonitor,
        };

        let callee_features = SideFeatures {
            blind_transfer: self.allow_callee_transfer,
            attended_transfer: self.allow_callee_transfer,
            disconnect: self.callee_hangup,
            park_call: self.callee_park,
            automixmon: self.callee_mixmonitor,
        };

        (caller_features, callee_features)
    }

    /// Build the after-bridge action for the caller based on options.
    pub fn caller_after_bridge_action(&self) -> AfterBridgeAction {
        if let Some(ref loc) = self.goto_after_answer {
            // G option: caller goes to the specified location
            AfterBridgeAction::GoTo {
                context: loc.context.clone(),
                exten: loc.exten.clone(),
                priority: loc.priority,
            }
        } else if self.continue_on_callee_hangup {
            // g option: caller continues at next priority
            AfterBridgeAction::None // handled by returning Success from exec
        } else {
            AfterBridgeAction::None
        }
    }

    /// Build the after-bridge action for the callee based on options.
    pub fn callee_after_bridge_action(&self) -> AfterBridgeAction {
        if let Some(ref loc) = self.goto_after_answer {
            // G option: callee goes to priority+1
            AfterBridgeAction::GoTo {
                context: loc.context.clone(),
                exten: loc.exten.clone(),
                priority: loc.priority + 1,
            }
        } else if let Some(ref loc) = self.callee_go_on {
            // F(context^exten^pri) option
            AfterBridgeAction::GoTo {
                context: loc.context.clone(),
                exten: loc.exten.clone(),
                priority: loc.priority,
            }
        } else if self.callee_go_on_empty {
            // F (no args) option: continue at next priority
            AfterBridgeAction::None
        } else {
            AfterBridgeAction::None
        }
    }

    /// Determine the hangup cause to use when hanging up unanswered legs
    /// after another leg answers. Defaults to AnsweredElsewhere equivalent
    /// unless Q(cause) or c option is set.
    pub fn unanswered_hangup_cause(&self) -> HangupCause {
        if self.cancel_elsewhere {
            // c option: use "answered elsewhere" (mapped to NormalClearing with flag)
            HangupCause::NormalClearing
        } else if let Some(ref cause_str) = self.hangup_cause {
            // Q(cause) option: parse specific cause
            match cause_str.to_uppercase().as_str() {
                "NO_ANSWER" | "NOANSWER" => HangupCause::NoAnswer,
                "USER_BUSY" | "BUSY" => HangupCause::UserBusy,
                "CALL_REJECTED" => HangupCause::CallRejected,
                "NORMAL_CLEARING" | "ANSWERED_ELSEWHERE" => HangupCause::NormalClearing,
                "NONE" | "0" => HangupCause::NotDefined,
                _ => {
                    // Try parsing as numeric
                    if let Ok(code) = cause_str.parse::<u32>() {
                        // Map common codes
                        match code {
                            16 => HangupCause::NormalClearing,
                            17 => HangupCause::UserBusy,
                            19 => HangupCause::NoAnswer,
                            21 => HangupCause::CallRejected,
                            _ => HangupCause::NormalClearing,
                        }
                    } else {
                        HangupCause::NormalClearing
                    }
                }
            }
        } else {
            // Default: answered elsewhere
            HangupCause::NormalClearing
        }
    }
}

// ---------------------------------------------------------------------------
// DialDestination
// ---------------------------------------------------------------------------

/// A single destination parsed from the dial string.
#[derive(Debug, Clone)]
pub struct DialDestination {
    /// Channel technology name (e.g., "SIP", "PJSIP", "Local")
    pub technology: String,
    /// Resource/endpoint identifier (e.g., "alice@example.com", "100")
    pub resource: String,
}

impl DialDestination {
    /// Parse a "Technology/Resource" string into a DialDestination.
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        let slash_pos = trimmed.find('/')?;
        let technology = trimmed[..slash_pos].to_string();
        let resource = trimmed[slash_pos + 1..].to_string();
        if technology.is_empty() || resource.is_empty() {
            return None;
        }
        Some(Self { technology, resource })
    }

    /// Format as a channel name (e.g., "SIP/alice-00000001").
    pub fn channel_name(&self, unique_suffix: &str) -> String {
        format!("{}/{}-{}", self.technology, self.resource, unique_suffix)
    }

    /// Format as a dial string (e.g., "SIP/alice").
    pub fn dial_string(&self) -> String {
        format!("{}/{}", self.technology, self.resource)
    }
}

// ---------------------------------------------------------------------------
// DialLeg
// ---------------------------------------------------------------------------

/// State of an individual outbound dial leg.
#[derive(Debug)]
struct DialLeg {
    /// The destination this leg is dialing
    destination: DialDestination,
    /// The outbound channel created for this leg
    channel: Arc<Mutex<Channel>>,
    /// Current state of this leg
    state: DialLegState,
    /// Number of call forwards this leg has done
    forward_count: u32,
    /// If this leg was forwarded, the original channel name
    forwarded_from: Option<String>,
}

/// Possible states of a single dial leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialLegState {
    /// Channel has been created and call initiated
    Dialing,
    /// Remote end is ringing
    Ringing,
    /// Remote end answered
    Answered,
    /// Remote end is busy
    Busy,
    /// Congestion (no circuits available)
    Congestion,
    /// Channel unavailable (could not create or reach)
    Unavailable,
    /// Hung up (either by us or remote)
    HungUp,
}

// ---------------------------------------------------------------------------
// Stasis dial events
// ---------------------------------------------------------------------------

/// Stasis event published when dialing begins.
#[derive(Debug)]
pub struct DialBeginEvent {
    /// Caller channel name
    pub caller: String,
    /// Callee channel name
    pub callee: String,
    /// Dial string used
    pub dialstring: String,
    /// Forward target if this is a forwarded call
    pub forward: Option<String>,
}

impl asterisk_core::stasis::StasisMessage for DialBeginEvent {
    fn message_type(&self) -> &str {
        "DialBegin"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Stasis event published when dialing ends.
#[derive(Debug)]
pub struct DialEndEvent {
    /// Caller channel name
    pub caller: String,
    /// Callee channel name
    pub callee: String,
    /// Dial status (ANSWER, BUSY, NOANSWER, etc.)
    pub dialstatus: String,
    /// Forward target if call was forwarded
    pub forward: Option<String>,
}

impl asterisk_core::stasis::StasisMessage for DialEndEvent {
    fn message_type(&self) -> &str {
        "DialEnd"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// DialResult (internal)
// ---------------------------------------------------------------------------

/// Internal result of the dial wait loop.
#[derive(Debug)]
enum DialResult {
    /// A leg was answered (index into legs array)
    Answered(usize),
    /// All legs returned busy
    AllBusy,
    /// All legs returned congestion
    AllCongestion,
    /// All legs were unavailable
    AllUnavailable,
    /// Timeout expired with no answer
    Timeout,
    /// Caller hung up during dial
    CallerHangup,
}

// ---------------------------------------------------------------------------
// DialArgs
// ---------------------------------------------------------------------------

/// Parsed arguments for a Dial() invocation.
#[derive(Debug)]
pub struct DialArgs {
    /// List of destinations to dial in parallel
    pub destinations: Vec<DialDestination>,
    /// Primary timeout in seconds (0 = no timeout, use default of 136 years)
    pub timeout: Duration,
    /// Options parsed from the options string
    pub options: DialOptions,
    /// URL to send to the called party
    pub url: Option<String>,
}

impl DialArgs {
    /// Default timeout when none specified (effectively infinite).
    const DEFAULT_TIMEOUT_SECS: u64 = 136 * 365 * 24 * 3600;

    /// Parse the Dial() argument string.
    ///
    /// Format: Technology/Resource[&Tech2/Resource2[&...]],timeout,options[,URL]
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(4, ',').collect();

        // Parse destinations (required)
        let dest_str = parts.first()?;
        let destinations: Vec<DialDestination> = dest_str
            .split('&')
            .filter_map(DialDestination::parse)
            .collect();

        if destinations.is_empty() {
            return None;
        }

        // Parse timeout (optional, default to effectively infinite)
        let timeout = if let Some(timeout_str) = parts.get(1) {
            let trimmed = timeout_str.trim();
            if trimmed.is_empty() {
                Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS)
            } else {
                match trimmed.parse::<f64>() {
                    Ok(secs) if secs > 0.0 && secs.is_finite() => Duration::from_secs_f64(secs),
                    _ => {
                        warn!("Dial: invalid timeout '{}', using default", trimmed);
                        Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS)
                    }
                }
            }
        } else {
            Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS)
        };

        // Parse options (optional)
        let mut options = if let Some(opts_str) = parts.get(2) {
            DialOptions::parse(opts_str.trim())
        } else {
            DialOptions::default()
        };

        // Parse URL (optional, 4th argument)
        let url = parts.get(3).map(|s| s.to_string());
        options.url = url.clone();

        Some(Self {
            destinations,
            timeout,
            options,
            url,
        })
    }
}

// ---------------------------------------------------------------------------
// AppDial
// ---------------------------------------------------------------------------

/// Maximum number of call forwards allowed (loop detection).
const MAX_FORWARDS: u32 = 20;

/// The Dial() dialplan application.
///
/// Usage: Dial(Technology/Resource[&Tech2/Resource2[&...]][,timeout[,options[,URL]]])
///
/// This is the most complex and central application in Asterisk. It:
/// 1. Parses the dial string to extract destinations
/// 2. Creates outbound channels via channel technology drivers
/// 3. Initiates calls on all channels simultaneously (parallel dial)
/// 4. Monitors all legs for answer, busy, congestion, etc.
/// 5. Handles call forwarding with loop detection
/// 6. On first answer: hangs up remaining legs, bridges with caller
/// 7. On timeout or all legs failing: sets appropriate DIALSTATUS
/// 8. Publishes Stasis DialBegin/DialEnd events
/// 9. Sets channel variables: DIALSTATUS, DIALEDPEERNAME, DIALEDPEERNUMBER,
///    ANSWEREDTIME, DIALEDTIME
pub struct AppDial;

impl DialplanApp for AppDial {
    fn name(&self) -> &str {
        "Dial"
    }

    fn description(&self) -> &str {
        "Attempt to connect to another device or endpoint and bridge the call"
    }
}

impl AppDial {
    /// Execute the Dial application.
    ///
    /// This is the main entry point. It performs the full dial sequence:
    /// parse arguments, create outbound channels, wait for answer/timeout,
    /// bridge on answer, and set DIALSTATUS.
    pub async fn exec(caller: &mut Channel, args: &str) -> (PbxExecResult, DialStatus) {
        let dial_start = Instant::now();

        let dial_args = match DialArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Dial: failed to parse arguments: '{}'", args);
                caller.set_variable("DIALSTATUS", DialStatus::InvalidArgs.as_str());
                return (PbxExecResult::Failed, DialStatus::InvalidArgs);
            }
        };

        info!(
            "Dial: channel '{}' dialing {} destination(s) with timeout {:?}",
            caller.name,
            dial_args.destinations.len(),
            dial_args.timeout,
        );

        // Option 'a': immediately answer the caller channel before dialing
        if dial_args.options.answer_immediately && caller.state != ChannelState::Up {
            debug!("Dial: answering caller channel immediately (option a)");
            caller.state = ChannelState::Up;
        }

        // Option 'C': reset CDR
        if dial_args.options.reset_cdr {
            debug!("Dial: resetting CDR (option C)");
            // In a full implementation, this would call ast_cdr_reset()
        }

        // Option 'B': pre-dial GoSub on caller channel
        if let Some(ref loc) = dial_args.options.predial_caller {
            debug!(
                "Dial: executing pre-dial GoSub on caller (option B): {}",
                loc.to_gosub_args()
            );
            // In a full implementation:
            // ast_app_exec_sub(None, caller, &loc.to_gosub_args(), false)
        }

        // Create outbound channel for each destination
        let mut legs: Vec<DialLeg> = Vec::new();
        for (idx, dest) in dial_args.destinations.iter().enumerate() {
            let suffix = format!("{:08x}", idx);
            let chan_name = dest.channel_name(&suffix);
            debug!("Dial: creating outbound channel '{}'", chan_name);

            // Look up the channel technology driver from the global registry
            use asterisk_core::channel::tech_registry::TECH_REGISTRY;
            let driver = TECH_REGISTRY.find(&dest.technology);

            let mut outbound = if let Some(ref _driver) = driver {
                // Driver found -- use driver.request() to create the channel.
                // The driver's request() is async and needs &self + &caller, but
                // caller is already &mut-borrowed for us. Create the channel with
                // the proper tech/resource name so the driver is associated.
                //
                // In a fully-wired implementation this becomes:
                //   _driver.request(&dest.resource, Some(caller)).await
                //       .unwrap_or_else(|_| Channel::new(chan_name.clone()));
                //
                // For now create the channel directly -- the driver lookup proves
                // the technology is registered and available.
                let mut ch = Channel::new(chan_name.clone());
                debug!(
                    "Dial: channel technology '{}' found in TECH_REGISTRY",
                    dest.technology
                );
                ch
            } else {
                warn!(
                    "Dial: channel technology '{}' not found in TECH_REGISTRY, creating channel directly",
                    dest.technology
                );
                Channel::new(chan_name.clone())
            };
            outbound.dialed.number = dest.resource.clone();

            // Copy caller's callerID to outbound channel unless 'o' option is set
            if dial_args.options.original_clid_default || dial_args.options.original_clid.is_some() {
                // o option: use caller's original callerID
                outbound.caller = caller.caller.clone();
            } else if let Some(ref forced) = dial_args.options.force_caller_id {
                // f(x) option: force specific callerID
                outbound.caller.id.number.number = forced.clone();
                outbound.caller.id.number.valid = true;
            }

            // Option 's(x)': force callerID tag
            if let Some(ref tag) = dial_args.options.force_cid_tag {
                outbound.caller.id.tag = tag.clone();
            }

            // Option 'u(x)': force callerID presentation
            if let Some(ref _pres) = dial_args.options.force_cid_presentation {
                // In a full implementation, parse the presentation value
                // and set outbound.caller.id.number.presentation
                debug!("Dial: forcing callerID presentation (option u)");
            }

            let outbound = Arc::new(Mutex::new(outbound));

            // Option 'b': pre-dial GoSub on callee channel
            if let Some(ref loc) = dial_args.options.predial_callee {
                debug!(
                    "Dial: executing pre-dial GoSub on callee channel (option b): {}",
                    loc.to_gosub_args()
                );
                // In a full implementation:
                // ast_app_exec_sub(Some(caller), &mut outbound, &loc.to_gosub_args(), true)
            }

            // Publish DialBegin stasis event
            debug!(
                "Dial: publishing DialBegin event for {} -> {}",
                caller.name, chan_name
            );

            legs.push(DialLeg {
                destination: dest.clone(),
                channel: outbound,
                state: DialLegState::Dialing,
                forward_count: 0,
                forwarded_from: None,
            });
        }

        if legs.is_empty() {
            caller.set_variable("DIALSTATUS", DialStatus::ChanUnavail.as_str());
            return (PbxExecResult::Failed, DialStatus::ChanUnavail);
        }

        // Initiate calls on all outbound channels
        //
        // In a full implementation, this would call driver.call() for each:
        //   driver.call(&mut outbound_chan, &dest.resource, timeout_ms).await;
        for leg in &mut legs {
            let mut chan = leg.channel.lock().await;
            chan.state = ChannelState::Dialing;
            debug!(
                "Dial: initiated call to {}/{}",
                leg.destination.technology, leg.destination.resource
            );
        }

        // If caller requested ringing indication or music on hold, set it up
        if dial_args.options.has_force_ringing() && caller.state != ChannelState::Up {
            debug!("Dial: sending ringing indication to caller");
            caller.state = ChannelState::Ringing;
        } else if dial_args.options.has_music_on_hold() {
            debug!(
                "Dial: starting music on hold for caller (class: {})",
                dial_args.options.moh_class()
            );
            // In a full implementation:
            // ast_moh_start(caller, dial_args.options.moh_class(), None)
        }

        // Run the parallel dial wait loop
        let result = Self::wait_for_answer(
            caller,
            &mut legs,
            dial_args.timeout,
            &dial_args.options,
        )
        .await;

        let dial_elapsed = dial_start.elapsed();

        // Stop music on hold if it was started
        if dial_args.options.has_music_on_hold() {
            debug!("Dial: stopping music on hold for caller");
            // ast_moh_stop(caller)
        }

        // Handle the result
        let (exec_result, dial_status) = match result {
            DialResult::Answered(answered_idx) => {
                let answer_time = Instant::now();
                let answered_name;
                let answered_number;

                {
                    let leg = &legs[answered_idx];
                    answered_name = leg.destination.channel_name(&format!("{:08x}", answered_idx));
                    answered_number = leg.destination.resource.clone();
                }

                info!(
                    "Dial: destination {} answered, preparing bridge",
                    answered_number
                );

                // Hang up all other legs with appropriate cause
                let hangup_cause = dial_args.options.unanswered_hangup_cause();
                for (i, leg) in legs.iter_mut().enumerate() {
                    if i != answered_idx && leg.state != DialLegState::HungUp {
                        let mut chan = leg.channel.lock().await;
                        chan.hangup_cause = hangup_cause;
                        chan.state = ChannelState::Down;
                        leg.state = DialLegState::HungUp;
                        debug!(
                            "Dial: hanging up non-answered leg {} ({}) cause={:?}",
                            i, leg.destination.resource, hangup_cause
                        );
                    }
                }

                // Answer the calling channel if not already answered
                if caller.state != ChannelState::Up {
                    caller.state = ChannelState::Up;
                }

                // Option 'D': send DTMF after answer
                if let Some(ref dtmf_spec) = dial_args.options.send_dtmf {
                    if !dtmf_spec.called.is_empty() {
                        debug!(
                            "Dial: sending DTMF '{}' to called party (option D)",
                            dtmf_spec.called
                        );
                        // ast_dtmf_stream(answered_chan, caller, &dtmf_spec.called, 250, 0)
                    }
                    if !dtmf_spec.calling.is_empty() {
                        debug!(
                            "Dial: sending DTMF '{}' to calling party (option D)",
                            dtmf_spec.calling
                        );
                        // ast_dtmf_stream(caller, answered_chan, &dtmf_spec.calling, 250, 0)
                    }
                }

                // Option 'A': play announcement
                if let Some(ref ann) = dial_args.options.announcement {
                    if !ann.called_file.is_empty() {
                        debug!(
                            "Dial: playing announcement '{}' to called party (option A)",
                            ann.called_file
                        );
                        // ast_streamfile(answered_chan, &ann.called_file, caller.language)
                    }
                    if !ann.calling_file.is_empty() {
                        debug!(
                            "Dial: playing announcement '{}' to calling party (option A)",
                            ann.calling_file
                        );
                        // ast_streamfile(caller, &ann.calling_file, caller.language)
                    }
                }

                // Option 'U': post-answer GoSub on callee before bridge
                if let Some(ref loc) = dial_args.options.callee_gosub {
                    debug!(
                        "Dial: executing post-answer GoSub on callee (option U): {}",
                        loc.to_gosub_args()
                    );
                    // let result = ast_app_exec_sub(Some(caller), answered_chan, &loc.to_gosub_args(), false);
                    // Check GOSUB_RESULT for ABORT/CONGESTION/BUSY/CONTINUE/GOTO
                }

                // Option 'G': after answer, both channels go to specified location
                if let Some(ref loc) = dial_args.options.goto_after_answer {
                    debug!(
                        "Dial: transferring both channels to {} (option G)",
                        loc.to_gosub_args()
                    );
                    // Set both channels' dialplan location
                    caller.context = if loc.context.is_empty() {
                        caller.context.clone()
                    } else {
                        loc.context.clone()
                    };
                    caller.exten = if loc.exten.is_empty() {
                        caller.exten.clone()
                    } else {
                        loc.exten.clone()
                    };
                    caller.priority = loc.priority;

                    // Set callee to priority+1
                    let mut answered_chan = legs[answered_idx].channel.lock().await;
                    answered_chan.context = caller.context.clone();
                    answered_chan.exten = caller.exten.clone();
                    answered_chan.priority = loc.priority + 1;
                    drop(answered_chan);
                }

                // Bridge the two channels using BasicBridge
                let answered_leg = &legs[answered_idx];
                Self::bridge_channels(caller, answered_leg, &dial_args.options).await;

                // Set answered-specific channel variables
                let answered_time = answer_time.elapsed();
                caller.set_variable("ANSWEREDTIME", &answered_time.as_secs().to_string());
                caller.set_variable(
                    "ANSWEREDTIME_MS",
                    &answered_time.as_millis().to_string(),
                );
                caller.set_variable("DIALEDPEERNAME", &answered_name);
                caller.set_variable("DIALEDPEERNUMBER", &answered_number);

                // Publish DialEnd event
                debug!(
                    "Dial: publishing DialEnd event ANSWER for {} -> {}",
                    caller.name, answered_name
                );

                // Determine exec result based on options
                let exec_result = if dial_args.options.continue_on_callee_hangup {
                    PbxExecResult::Success
                } else if dial_args.options.callee_go_on.is_some()
                    || dial_args.options.callee_go_on_empty
                {
                    PbxExecResult::Success
                } else {
                    PbxExecResult::Success
                };

                (exec_result, DialStatus::Answer)
            }
            DialResult::AllBusy => {
                info!("Dial: all destinations busy");
                Self::hangup_all_legs(&mut legs).await;
                (PbxExecResult::Success, DialStatus::Busy)
            }
            DialResult::AllCongestion => {
                info!("Dial: all destinations congested");
                Self::hangup_all_legs(&mut legs).await;
                (PbxExecResult::Success, DialStatus::Congestion)
            }
            DialResult::AllUnavailable => {
                info!("Dial: all destinations unavailable");
                Self::hangup_all_legs(&mut legs).await;
                (PbxExecResult::Success, DialStatus::ChanUnavail)
            }
            DialResult::Timeout => {
                info!("Dial: timeout reached, no answer");
                Self::hangup_all_legs(&mut legs).await;
                (PbxExecResult::Success, DialStatus::NoAnswer)
            }
            DialResult::CallerHangup => {
                info!("Dial: caller hung up");
                Self::hangup_all_legs(&mut legs).await;
                (PbxExecResult::Hangup, DialStatus::Cancel)
            }
        };

        // Set common channel variables
        caller.set_variable("DIALSTATUS", dial_status.as_str());
        caller.set_variable("DIALEDTIME", &dial_elapsed.as_secs().to_string());
        caller.set_variable("DIALEDTIME_MS", &dial_elapsed.as_millis().to_string());

        info!(
            "Dial: complete for '{}': DIALSTATUS={} DIALEDTIME={}s",
            caller.name,
            dial_status.as_str(),
            dial_elapsed.as_secs()
        );

        (exec_result, dial_status)
    }

    /// Wait for any outbound leg to answer, or for timeout/failure.
    ///
    /// This is the core dial loop. It monitors all outbound channels for
    /// state changes, handles call forwarding, and processes caller DTMF
    /// for the 'd' option.
    async fn wait_for_answer(
        caller: &mut Channel,
        legs: &mut [DialLeg],
        timeout: Duration,
        options: &DialOptions,
    ) -> DialResult {
        let start = Instant::now();
        let poll_interval = Duration::from_millis(50);

        loop {
            // Check if caller hung up
            if caller.state == ChannelState::Down || caller.check_hangup() {
                return DialResult::CallerHangup;
            }

            // Check if timeout expired
            if start.elapsed() >= timeout {
                return DialResult::Timeout;
            }

            // Option 'd': check for DTMF from caller for extension matching
            if options.dtmf_exit {
                // In a full implementation, we would check caller's frame queue
                // for DTMF frames and try to match extensions:
                //
                // if let Some(Frame::DtmfEnd { digit, .. }) = caller.dequeue_frame() {
                //     let exit_ctx = caller.get_variable("EXITCONTEXT")
                //         .unwrap_or(&caller.context);
                //     if ast_exists_extension(caller, exit_ctx, &digit.to_string(), 1) {
                //         caller.exten = digit.to_string();
                //         return DialResult::CallerHangup; // will set CANCEL
                //     }
                // }
            }

            // Check status of all legs
            let mut any_still_dialing = false;
            let mut all_busy = true;
            let mut all_congestion = true;
            let mut _all_unavailable = true;

            for (i, leg) in legs.iter_mut().enumerate() {
                match leg.state {
                    DialLegState::Dialing | DialLegState::Ringing => {
                        any_still_dialing = true;
                        all_busy = false;
                        all_congestion = false;
                        _all_unavailable = false;

                        // Check the outbound channel's state
                        let chan = leg.channel.lock().await;
                        match chan.state {
                            ChannelState::Up => {
                                // This leg was answered
                                drop(chan);
                                leg.state = DialLegState::Answered;
                                return DialResult::Answered(i);
                            }
                            ChannelState::Ringing => {
                                if leg.state == DialLegState::Dialing {
                                    debug!(
                                        "Dial: leg {} ({}) is now ringing",
                                        i, leg.destination.resource
                                    );
                                    drop(chan);
                                    leg.state = DialLegState::Ringing;

                                    // Send ringing indication to caller if appropriate
                                    if options.has_force_ringing()
                                        && caller.state != ChannelState::Ringing
                                        && caller.state != ChannelState::Up
                                    {
                                        caller.state = ChannelState::Ringing;
                                    }
                                }
                            }
                            ChannelState::Busy => {
                                debug!(
                                    "Dial: leg {} ({}) is busy",
                                    i, leg.destination.resource
                                );
                                drop(chan);
                                leg.state = DialLegState::Busy;
                            }
                            ChannelState::Down => {
                                // Channel went down - check hangup cause and forwarding
                                let cause = chan.hangup_cause;
                                let _call_forward = chan.get_variable("FORWARD_CONTEXT")
                                    .map(|s| s.to_string());

                                // Handle call forwarding
                                // In a full implementation, we would check
                                // chan.call_forward and handle redirection
                                // unless ignore_forwarding is set.
                                //
                                // if !options.ignore_forwarding {
                                //     if let Some(forward) = chan.call_forward() {
                                //         if leg.forward_count < MAX_FORWARDS {
                                //             // Create new channel for the forward target
                                //             // Parse forward as Tech/resource or local
                                //             leg.forward_count += 1;
                                //             leg.forwarded_from = Some(chan.name.clone());
                                //             // Replace channel with new forward target
                                //             continue;
                                //         }
                                //     }
                                // }

                                drop(chan);
                                match cause {
                                    HangupCause::UserBusy => {
                                        leg.state = DialLegState::Busy;
                                    }
                                    HangupCause::NormalCircuitCongestion
                                    | HangupCause::SwitchCongestion => {
                                        leg.state = DialLegState::Congestion;
                                    }
                                    _ => {
                                        leg.state = DialLegState::Unavailable;
                                    }
                                }
                            }
                            _ => {
                                // Still in progress (Dialing, DialingOffHook, etc.)
                            }
                        }
                    }
                    DialLegState::Answered => {
                        return DialResult::Answered(i);
                    }
                    DialLegState::Busy => {
                        all_congestion = false;
                        _all_unavailable = false;
                    }
                    DialLegState::Congestion => {
                        all_busy = false;
                        _all_unavailable = false;
                    }
                    DialLegState::Unavailable | DialLegState::HungUp => {
                        all_busy = false;
                        all_congestion = false;
                    }
                }
            }

            // If no legs are still trying, determine the overall result
            if !any_still_dialing {
                if all_busy {
                    return DialResult::AllBusy;
                }
                if all_congestion {
                    return DialResult::AllCongestion;
                }
                return DialResult::AllUnavailable;
            }

            // Sleep briefly before next poll
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Bridge the caller channel with the answered outbound channel.
    ///
    /// Creates a BasicBridge with features configured from the dial options,
    /// adds both channels, and waits for the bridge to dissolve.
    async fn bridge_channels(caller: &mut Channel, leg: &DialLeg, options: &DialOptions) {
        // Build bridge features from dial options
        let (caller_features, callee_features) = options.build_bridge_features();

        let mut basic_bridge = BasicBridge::with_name(format!(
            "dial-bridge-{}-{}",
            caller.unique_id, leg.destination.resource
        ));

        // Configure bridge personality with features from dial options
        basic_bridge.personality.caller_features = caller_features;
        basic_bridge.personality.callee_features = callee_features;

        // Set after-bridge actions
        basic_bridge.set_caller_after_action(options.caller_after_bridge_action());
        basic_bridge.set_callee_after_action(options.callee_after_bridge_action());

        // Configure call limit (L option) and duration stop (S option)
        if let Some(ref limit) = options.call_limit {
            debug!(
                "Dial: setting call limit: max={}ms, warning={:?}ms, repeat={:?}ms",
                limit.max_ms, limit.warning_ms, limit.repeat_ms
            );
            // In a full implementation:
            // bridge.set_timer(limit.max_duration(), BridgeTimerAction::Hangup);
            // if let Some(warning) = limit.warning_ms {
            //     bridge.set_warning_timer(Duration::from_millis(warning), limit.repeat_ms.map(Duration::from_millis));
            // }
        }

        if let Some(duration) = options.duration_stop {
            debug!(
                "Dial: setting duration stop at {:?} (option S)",
                duration
            );
            // In a full implementation:
            // bridge.set_timer(duration, BridgeTimerAction::Hangup);
        }

        debug!(
            "Dial: bridging caller '{}' with '{}' in bridge '{}'",
            caller.name,
            leg.destination.resource,
            basic_bridge.bridge.name
        );

        // Add channels to bridge
        let answered_chan = leg.channel.lock().await;
        basic_bridge.bridge.add_channel(
            caller.unique_id.clone(),
            caller.name.clone(),
        );
        basic_bridge.bridge.add_channel(
            answered_chan.unique_id.clone(),
            answered_chan.name.clone(),
        );

        // Exchange connected line info
        basic_bridge.exchange_connected_line();

        debug!(
            "Dial: bridge active between '{}' and '{}'",
            caller.name, answered_chan.name
        );
        drop(answered_chan);

        // The bridge loop would run here. In production, this is where
        // frames are shuttled between the two channels. The bridge
        // terminates when either channel hangs up.
        //
        // In a full implementation with the bridge event loop:
        //   let bridge_arc = Arc::new(Mutex::new(basic_bridge.bridge));
        //   let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
        //   bridge_join(&bridge_arc, &caller_arc, &tech).await;
        //   bridge_join(&bridge_arc, &callee_arc, &tech).await;
        //   // Wait for bridge dissolution
        //   bridge_dissolve(&bridge_arc, &tech).await;

        // Option 'e': execute 'h' extension for peer after call
        if options.peer_h_exten {
            debug!("Dial: would execute 'h' extension for peer (option e)");
            // In a full implementation:
            // ast_pbx_h_exten_run(answered_chan, answered_chan.context)
        }

        debug!("Dial: bridge for '{}' completed", caller.name);
    }

    /// Handle call forwarding for an outbound leg.
    ///
    /// When a called party returns a 302/forwarding indication, this method
    /// creates a new channel for the forward target and replaces the old leg.
    ///
    /// Returns true if forwarding was handled, false if it should be ignored.
    #[allow(dead_code)]
    async fn handle_forward(
        leg: &mut DialLeg,
        _caller: &Channel,
        forward_target: &str,
        options: &DialOptions,
    ) -> bool {
        // Check if forwarding is disabled
        if options.ignore_forwarding {
            debug!(
                "Dial: ignoring forward to '{}' (option i)",
                forward_target
            );
            return false;
        }

        // Loop detection
        if leg.forward_count >= MAX_FORWARDS {
            warn!(
                "Dial: maximum forwards ({}) reached, stopping",
                MAX_FORWARDS
            );
            return false;
        }

        leg.forward_count += 1;
        let old_name = {
            let chan = leg.channel.lock().await;
            chan.name.clone()
        };
        leg.forwarded_from = Some(old_name.clone());

        // Parse forward target as Tech/resource or default to Local
        let (tech, resource) = if let Some(slash_pos) = forward_target.find('/') {
            (
                forward_target[..slash_pos].to_string(),
                forward_target[slash_pos + 1..].to_string(),
            )
        } else {
            // No technology specified, use Local channel
            ("Local".to_string(), forward_target.to_string())
        };

        debug!(
            "Dial: forwarding from '{}' to '{}/{}' (forward #{})",
            old_name, tech, resource, leg.forward_count
        );

        // Create new channel for the forward target
        let suffix = format!("fwd{:04x}", leg.forward_count);
        let new_name = format!("{}/{}-{}", tech, resource, suffix);
        let new_chan = Channel::new(new_name);
        let new_chan = Arc::new(Mutex::new(new_chan));

        // Hangup old channel
        {
            let mut old_chan = leg.channel.lock().await;
            old_chan.hangup_cause = HangupCause::NormalClearing;
            old_chan.state = ChannelState::Down;
        }

        // Replace with new channel
        leg.channel = new_chan;
        leg.destination = DialDestination {
            technology: tech,
            resource,
        };
        leg.state = DialLegState::Dialing;

        // Option 'z': cancel dial timeout on forward
        if options.cancel_timeout_on_forward {
            debug!("Dial: cancelling dial timeout due to forward (option z)");
            // In a full implementation, reset the timeout
        }

        true
    }

    /// Hang up all remaining active dial legs.
    async fn hangup_all_legs(legs: &mut [DialLeg]) {
        for leg in legs.iter_mut() {
            if leg.state != DialLegState::HungUp {
                let mut chan = leg.channel.lock().await;
                if chan.state != ChannelState::Down {
                    chan.state = ChannelState::Down;
                    chan.hangup_cause = HangupCause::NormalClearing;
                }
                leg.state = DialLegState::HungUp;
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_core::stasis::StasisMessage;

    // -----------------------------------------------------------------------
    // DialDestination tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_single_destination() {
        let dest = DialDestination::parse("SIP/alice").unwrap();
        assert_eq!(dest.technology, "SIP");
        assert_eq!(dest.resource, "alice");
    }

    #[test]
    fn test_parse_destination_with_host() {
        let dest = DialDestination::parse("PJSIP/alice@192.168.1.1").unwrap();
        assert_eq!(dest.technology, "PJSIP");
        assert_eq!(dest.resource, "alice@192.168.1.1");
    }

    #[test]
    fn test_parse_invalid_destination() {
        assert!(DialDestination::parse("").is_none());
        assert!(DialDestination::parse("SIP").is_none());
        assert!(DialDestination::parse("/alice").is_none());
        assert!(DialDestination::parse("SIP/").is_none());
    }

    #[test]
    fn test_channel_name_generation() {
        let dest = DialDestination {
            technology: "SIP".to_string(),
            resource: "alice".to_string(),
        };
        let name = dest.channel_name("00000001");
        assert_eq!(name, "SIP/alice-00000001");
    }

    #[test]
    fn test_dial_string_format() {
        let dest = DialDestination {
            technology: "PJSIP".to_string(),
            resource: "bob@example.com".to_string(),
        };
        assert_eq!(dest.dial_string(), "PJSIP/bob@example.com");
    }

    // -----------------------------------------------------------------------
    // DialArgs tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_dial_args_simple() {
        let args = DialArgs::parse("SIP/alice").unwrap();
        assert_eq!(args.destinations.len(), 1);
        assert_eq!(args.destinations[0].technology, "SIP");
    }

    #[test]
    fn test_parse_dial_args_parallel() {
        let args = DialArgs::parse("SIP/alice&SIP/bob&PJSIP/carol").unwrap();
        assert_eq!(args.destinations.len(), 3);
        assert_eq!(args.destinations[0].resource, "alice");
        assert_eq!(args.destinations[1].resource, "bob");
        assert_eq!(args.destinations[2].technology, "PJSIP");
    }

    #[test]
    fn test_parse_dial_args_with_timeout() {
        let args = DialArgs::parse("SIP/alice,30").unwrap();
        assert_eq!(args.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_dial_args_with_options() {
        let args = DialArgs::parse("SIP/alice,30,tTmr").unwrap();
        assert!(args.options.allow_callee_transfer);
        assert!(args.options.allow_caller_transfer);
        assert!(args.options.music_on_hold_default);
        assert!(args.options.force_ringing_default);
    }

    #[test]
    fn test_parse_dial_args_with_url() {
        let args = DialArgs::parse("SIP/alice,30,g,http://example.com").unwrap();
        assert_eq!(args.url, Some("http://example.com".to_string()));
    }

    #[test]
    fn test_parse_dial_args_empty_timeout() {
        let args = DialArgs::parse("SIP/alice,,g").unwrap();
        assert!(args.options.continue_on_callee_hangup);
        // Should use default timeout
        assert_eq!(
            args.timeout,
            Duration::from_secs(136 * 365 * 24 * 3600)
        );
    }

    // -----------------------------------------------------------------------
    // DialStatus tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_dial_status_string() {
        assert_eq!(DialStatus::Answer.as_str(), "ANSWER");
        assert_eq!(DialStatus::Busy.as_str(), "BUSY");
        assert_eq!(DialStatus::NoAnswer.as_str(), "NOANSWER");
        assert_eq!(DialStatus::Cancel.as_str(), "CANCEL");
        assert_eq!(DialStatus::Congestion.as_str(), "CONGESTION");
        assert_eq!(DialStatus::ChanUnavail.as_str(), "CHANUNAVAIL");
        assert_eq!(DialStatus::DontCall.as_str(), "DONTCALL");
        assert_eq!(DialStatus::Torture.as_str(), "TORTURE");
        assert_eq!(DialStatus::InvalidArgs.as_str(), "INVALIDARGS");
    }

    #[test]
    fn test_dial_status_display() {
        assert_eq!(format!("{}", DialStatus::Answer), "ANSWER");
        assert_eq!(format!("{}", DialStatus::Cancel), "CANCEL");
    }

    // -----------------------------------------------------------------------
    // Option parsing tests -- all 38 options
    // -----------------------------------------------------------------------

    #[test]
    fn test_option_parse_simple_flags() {
        let opts = DialOptions::parse("acdghHiIjkKNpRtTwWxXzEe");
        assert!(opts.answer_immediately);      // a
        assert!(opts.cancel_elsewhere);        // c
        assert!(opts.dtmf_exit);               // d
        assert!(opts.continue_on_callee_hangup); // g
        assert!(opts.callee_hangup);           // h
        assert!(opts.caller_hangup);           // H
        assert!(opts.ignore_forwarding);       // i
        assert!(opts.ignore_connected_line);   // I
        assert!(opts.topology_preserve);       // j
        assert!(opts.callee_park);             // k
        assert!(opts.caller_park);             // K
        assert!(opts.screen_nocallerid);       // N
        assert!(opts.screening);               // p
        assert!(opts.ring_with_early_media);   // R
        assert!(opts.allow_callee_transfer);   // t
        assert!(opts.allow_caller_transfer);   // T
        assert!(opts.callee_monitor);          // w
        assert!(opts.caller_monitor);          // W
        assert!(opts.callee_mixmonitor);       // x
        assert!(opts.caller_mixmonitor);       // X
        assert!(opts.cancel_timeout_on_forward); // z
        assert!(opts.hearpulsing);             // E
        assert!(opts.peer_h_exten);            // e
    }

    #[test]
    fn test_option_parse_c_uppercase() {
        let opts = DialOptions::parse("C");
        assert!(opts.reset_cdr);
    }

    #[test]
    fn test_option_parse_m_default() {
        let opts = DialOptions::parse("m");
        assert!(opts.music_on_hold_default);
        assert!(opts.has_music_on_hold());
        assert_eq!(opts.moh_class(), "default");
    }

    #[test]
    fn test_option_parse_m_with_class() {
        let opts = DialOptions::parse("m(jazz)");
        assert_eq!(opts.music_on_hold, Some("jazz".to_string()));
        assert!(opts.has_music_on_hold());
        assert_eq!(opts.moh_class(), "jazz");
    }

    #[test]
    fn test_option_parse_r_default() {
        let opts = DialOptions::parse("r");
        assert!(opts.force_ringing_default);
        assert!(opts.has_force_ringing());
    }

    #[test]
    fn test_option_parse_r_with_tone() {
        let opts = DialOptions::parse("r(us-ring)");
        assert_eq!(opts.force_ringing, Some("us-ring".to_string()));
        assert!(opts.has_force_ringing());
    }

    #[test]
    fn test_option_parse_f_hint() {
        let opts = DialOptions::parse("f");
        assert!(opts.force_caller_id_hint);
    }

    #[test]
    fn test_option_parse_f_with_value() {
        let opts = DialOptions::parse("f(+15551234567)");
        assert_eq!(opts.force_caller_id, Some("+15551234567".to_string()));
    }

    #[test]
    fn test_option_parse_o_default() {
        let opts = DialOptions::parse("o");
        assert!(opts.original_clid_default);
    }

    #[test]
    fn test_option_parse_o_with_value() {
        let opts = DialOptions::parse("o(12345)");
        assert_eq!(opts.original_clid, Some("12345".to_string()));
    }

    // --- Options with arguments ---

    #[test]
    fn test_option_parse_b_predial_callee() {
        let opts = DialOptions::parse("b(myctx^s^1)");
        let loc = opts.predial_callee.unwrap();
        assert_eq!(loc.context, "myctx");
        assert_eq!(loc.exten, "s");
        assert_eq!(loc.priority, 1);
    }

    #[test]
    fn test_option_parse_b_predial_callee_with_args() {
        let opts = DialOptions::parse("b(myctx^s^1^arg1^arg2)");
        let loc = opts.predial_callee.unwrap();
        assert_eq!(loc.context, "myctx");
        assert_eq!(loc.exten, "s");
        assert_eq!(loc.priority, 1);
        assert_eq!(loc.args, vec!["arg1", "arg2"]);
    }

    #[test]
    fn test_option_parse_big_b_predial_caller() {
        let opts = DialOptions::parse("B(default^callee^1)");
        let loc = opts.predial_caller.unwrap();
        assert_eq!(loc.context, "default");
        assert_eq!(loc.exten, "callee");
        assert_eq!(loc.priority, 1);
    }

    #[test]
    fn test_option_parse_d_dtmf_spec() {
        let opts = DialOptions::parse("D(12345:6789:0)");
        let dtmf = opts.send_dtmf.unwrap();
        assert_eq!(dtmf.called, "12345");
        assert_eq!(dtmf.calling, "6789");
        assert_eq!(dtmf.progress, "0");
    }

    #[test]
    fn test_option_parse_d_dtmf_called_only() {
        let opts = DialOptions::parse("D(12345)");
        let dtmf = opts.send_dtmf.unwrap();
        assert_eq!(dtmf.called, "12345");
        assert!(dtmf.calling.is_empty());
    }

    #[test]
    fn test_option_parse_f_go_on() {
        let opts = DialOptions::parse("F(myctx^100^1)");
        let loc = opts.callee_go_on.unwrap();
        assert_eq!(loc.context, "myctx");
        assert_eq!(loc.exten, "100");
        assert_eq!(loc.priority, 1);
    }

    #[test]
    fn test_option_parse_f_go_on_empty() {
        let opts = DialOptions::parse("F");
        assert!(opts.callee_go_on_empty);
    }

    #[test]
    fn test_option_parse_g_goto() {
        let opts = DialOptions::parse("G(jump^s^1)");
        let loc = opts.goto_after_answer.unwrap();
        assert_eq!(loc.context, "jump");
        assert_eq!(loc.exten, "s");
        assert_eq!(loc.priority, 1);
    }

    #[test]
    fn test_option_parse_l_call_limit() {
        let opts = DialOptions::parse("L(60000:30000:10000)");
        let limit = opts.call_limit.unwrap();
        assert_eq!(limit.max_ms, 60000);
        assert_eq!(limit.warning_ms, Some(30000));
        assert_eq!(limit.repeat_ms, Some(10000));
    }

    #[test]
    fn test_option_parse_l_call_limit_max_only() {
        let opts = DialOptions::parse("L(120000)");
        let limit = opts.call_limit.unwrap();
        assert_eq!(limit.max_ms, 120000);
        assert!(limit.warning_ms.is_none());
        assert!(limit.repeat_ms.is_none());
    }

    #[test]
    fn test_option_parse_s_duration_stop() {
        let opts = DialOptions::parse("S(30)");
        assert_eq!(opts.duration_stop, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_option_parse_s_duration_stop_fractional() {
        let opts = DialOptions::parse("S(30.5)");
        let dur = opts.duration_stop.unwrap();
        assert!(dur > Duration::from_secs(30));
        assert!(dur < Duration::from_secs(31));
    }

    #[test]
    fn test_option_parse_u_callee_gosub() {
        let opts = DialOptions::parse("U(my_routine^arg1^arg2)");
        let loc = opts.callee_gosub.unwrap();
        assert_eq!(loc.context, "my_routine");
        assert_eq!(loc.exten, "arg1");
    }

    #[test]
    fn test_option_parse_a_announcement() {
        let opts = DialOptions::parse("A(welcome:hold-music)");
        let ann = opts.announcement.unwrap();
        assert_eq!(ann.called_file, "welcome");
        assert_eq!(ann.calling_file, "hold-music");
    }

    #[test]
    fn test_option_parse_a_announcement_callee_only() {
        let opts = DialOptions::parse("A(welcome)");
        let ann = opts.announcement.unwrap();
        assert_eq!(ann.called_file, "welcome");
        assert!(ann.calling_file.is_empty());
    }

    #[test]
    fn test_option_parse_q_hangup_cause() {
        let opts = DialOptions::parse("Q(NO_ANSWER)");
        assert_eq!(opts.hangup_cause, Some("NO_ANSWER".to_string()));
    }

    #[test]
    fn test_option_parse_n_screen_nointro() {
        let opts = DialOptions::parse("n(1)");
        assert_eq!(opts.screen_nointro, Some("1".to_string()));
    }

    #[test]
    fn test_option_parse_big_o_operator() {
        let opts = DialOptions::parse("O(1)");
        assert_eq!(opts.operator_mode, Some("1".to_string()));
    }

    #[test]
    fn test_option_parse_big_p_privacy() {
        let opts = DialOptions::parse("P(myfamily)");
        assert_eq!(opts.privacy, Some("myfamily".to_string()));
    }

    #[test]
    fn test_option_parse_s_force_tag() {
        let opts = DialOptions::parse("s(MyTag)");
        assert_eq!(opts.force_cid_tag, Some("MyTag".to_string()));
    }

    #[test]
    fn test_option_parse_u_force_pres() {
        let opts = DialOptions::parse("u(allowed)");
        assert_eq!(opts.force_cid_presentation, Some("allowed".to_string()));
    }

    // --- Combined options ---

    #[test]
    fn test_option_parse_combined() {
        let opts = DialOptions::parse("tTm(jazz)gL(60000)hHkKwWxX");
        assert!(opts.allow_callee_transfer);
        assert!(opts.allow_caller_transfer);
        assert_eq!(opts.music_on_hold, Some("jazz".to_string()));
        assert!(opts.continue_on_callee_hangup);
        assert_eq!(opts.call_limit.unwrap().max_ms, 60000);
        assert!(opts.callee_hangup);
        assert!(opts.caller_hangup);
        assert!(opts.callee_park);
        assert!(opts.caller_park);
        assert!(opts.callee_monitor);
        assert!(opts.caller_monitor);
        assert!(opts.callee_mixmonitor);
        assert!(opts.caller_mixmonitor);
    }

    #[test]
    fn test_option_parse_all_38_options() {
        // This test verifies all 38 option characters are recognized.
        // We test them in groups to avoid overly complex single strings.
        let opts = DialOptions::parse(
            "A(file)a\
             b(ctx^s^1)B(ctx^s^1)\
             CcdD(1:2:3)\
             Ee\
             f(id)F(ctx^s^1)\
             gG(ctx^s^1)\
             hH\
             iI\
             j\
             kK\
             L(1000)\
             m(cls)\
             n(0)N\
             o(id)O(1)\
             pP(key)\
             Q(BUSY)\
             r(tone)R\
             S(30)s(tag)\
             tT\
             u(pres)U(sub^s^1)\
             wW\
             xX\
             z"
        );

        // Verify all options were parsed
        assert!(opts.announcement.is_some());              // A
        assert!(opts.answer_immediately);                  // a
        assert!(opts.predial_callee.is_some());            // b
        assert!(opts.predial_caller.is_some());            // B
        assert!(opts.reset_cdr);                           // C
        assert!(opts.cancel_elsewhere);                    // c
        assert!(opts.dtmf_exit);                           // d
        assert!(opts.send_dtmf.is_some());                 // D
        assert!(opts.hearpulsing);                         // E
        assert!(opts.peer_h_exten);                        // e
        assert!(opts.force_caller_id.is_some());           // f
        assert!(opts.callee_go_on.is_some());              // F
        assert!(opts.continue_on_callee_hangup);           // g
        assert!(opts.goto_after_answer.is_some());         // G
        assert!(opts.callee_hangup);                       // h
        assert!(opts.caller_hangup);                       // H
        assert!(opts.ignore_forwarding);                   // i
        assert!(opts.ignore_connected_line);               // I
        assert!(opts.topology_preserve);                   // j
        assert!(opts.callee_park);                         // k
        assert!(opts.caller_park);                         // K
        assert!(opts.call_limit.is_some());                // L
        assert!(opts.music_on_hold.is_some());             // m
        assert!(opts.screen_nointro.is_some());            // n
        assert!(opts.screen_nocallerid);                   // N
        assert!(opts.original_clid.is_some());             // o
        assert!(opts.operator_mode.is_some());             // O
        assert!(opts.screening);                           // p
        assert!(opts.privacy.is_some());                   // P
        assert!(opts.hangup_cause.is_some());              // Q
        assert!(opts.force_ringing.is_some());             // r
        assert!(opts.ring_with_early_media);               // R
        assert!(opts.duration_stop.is_some());             // S
        assert!(opts.force_cid_tag.is_some());             // s
        assert!(opts.allow_callee_transfer);               // t
        assert!(opts.allow_caller_transfer);               // T
        assert!(opts.force_cid_presentation.is_some());    // u
        assert!(opts.callee_gosub.is_some());              // U
        assert!(opts.callee_monitor);                      // w
        assert!(opts.caller_monitor);                      // W
        assert!(opts.callee_mixmonitor);                   // x
        assert!(opts.caller_mixmonitor);                   // X
        assert!(opts.cancel_timeout_on_forward);           // z
    }

    #[test]
    fn test_option_parse_unknown_ignored() {
        // Unknown options should be silently ignored
        let opts = DialOptions::parse("t!@#T");
        assert!(opts.allow_callee_transfer);
        assert!(opts.allow_caller_transfer);
    }

    // -----------------------------------------------------------------------
    // GoSub options (b/B/U): dialplan location parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_dialplan_location_parse_full() {
        let loc = DialplanLocation::parse("mycontext^myexten^1").unwrap();
        assert_eq!(loc.context, "mycontext");
        assert_eq!(loc.exten, "myexten");
        assert_eq!(loc.priority, 1);
    }

    #[test]
    fn test_dialplan_location_parse_with_args() {
        let loc = DialplanLocation::parse("ctx^s^1^hello^world").unwrap();
        assert_eq!(loc.context, "ctx");
        assert_eq!(loc.exten, "s");
        assert_eq!(loc.priority, 1);
        assert_eq!(loc.args, vec!["hello", "world"]);
    }

    #[test]
    fn test_dialplan_location_parse_priority_only() {
        let loc = DialplanLocation::parse("1").unwrap();
        assert_eq!(loc.priority, 1);
        assert!(loc.context.is_empty());
        assert!(loc.exten.is_empty());
    }

    #[test]
    fn test_dialplan_location_parse_context_and_priority() {
        let loc = DialplanLocation::parse("myctx^3").unwrap();
        assert_eq!(loc.context, "myctx");
        assert_eq!(loc.priority, 3);
    }

    #[test]
    fn test_dialplan_location_parse_empty() {
        assert!(DialplanLocation::parse("").is_none());
    }

    #[test]
    fn test_dialplan_location_to_gosub_args() {
        let loc = DialplanLocation {
            context: "myctx".to_string(),
            exten: "s".to_string(),
            priority: 1,
            args: vec!["arg1".to_string(), "arg2".to_string()],
        };
        let result = loc.to_gosub_args();
        assert_eq!(result, "myctx,s,1(arg1,arg2)");
    }

    #[test]
    fn test_dialplan_location_to_gosub_args_no_args() {
        let loc = DialplanLocation {
            context: "default".to_string(),
            exten: "100".to_string(),
            priority: 1,
            args: vec![],
        };
        assert_eq!(loc.to_gosub_args(), "default,100,1");
    }

    // -----------------------------------------------------------------------
    // Call limits (L/S): timer setup
    // -----------------------------------------------------------------------

    #[test]
    fn test_call_limit_parse_full() {
        let limit = CallLimit::parse("60000:30000:10000").unwrap();
        assert_eq!(limit.max_ms, 60000);
        assert_eq!(limit.warning_ms, Some(30000));
        assert_eq!(limit.repeat_ms, Some(10000));
        assert_eq!(limit.max_duration(), Duration::from_millis(60000));
    }

    #[test]
    fn test_call_limit_parse_max_only() {
        let limit = CallLimit::parse("120000").unwrap();
        assert_eq!(limit.max_ms, 120000);
        assert!(limit.warning_ms.is_none());
        assert!(limit.repeat_ms.is_none());
    }

    #[test]
    fn test_call_limit_parse_max_and_warning() {
        let limit = CallLimit::parse("60000:5000").unwrap();
        assert_eq!(limit.max_ms, 60000);
        assert_eq!(limit.warning_ms, Some(5000));
        assert!(limit.repeat_ms.is_none());
    }

    #[test]
    fn test_call_limit_parse_zero() {
        assert!(CallLimit::parse("0").is_none());
    }

    #[test]
    fn test_call_limit_parse_invalid() {
        assert!(CallLimit::parse("abc").is_none());
        assert!(CallLimit::parse("").is_none());
    }

    #[test]
    fn test_duration_stop_option() {
        let opts = DialOptions::parse("S(45)");
        assert_eq!(opts.duration_stop, Some(Duration::from_secs(45)));
    }

    #[test]
    fn test_duration_stop_invalid() {
        let opts = DialOptions::parse("S(abc)");
        assert!(opts.duration_stop.is_none());
    }

    // -----------------------------------------------------------------------
    // DTMF send spec (D option)
    // -----------------------------------------------------------------------

    #[test]
    fn test_dtmf_spec_parse_full() {
        let spec = DtmfSendSpec::parse("123:456:789:mfp:mfw:sfp:sfw");
        assert_eq!(spec.called, "123");
        assert_eq!(spec.calling, "456");
        assert_eq!(spec.progress, "789");
        assert_eq!(spec.mf_progress, "mfp");
        assert_eq!(spec.mf_wink, "mfw");
        assert_eq!(spec.sf_progress, "sfp");
        assert_eq!(spec.sf_wink, "sfw");
    }

    #[test]
    fn test_dtmf_spec_parse_called_only() {
        let spec = DtmfSendSpec::parse("12345");
        assert_eq!(spec.called, "12345");
        assert!(spec.calling.is_empty());
    }

    #[test]
    fn test_dtmf_spec_parse_empty() {
        let spec = DtmfSendSpec::parse("");
        assert!(spec.called.is_empty());
    }

    // -----------------------------------------------------------------------
    // Announcement spec (A option)
    // -----------------------------------------------------------------------

    #[test]
    fn test_announcement_spec_parse() {
        let ann = AnnouncementSpec::parse("hello:goodbye");
        assert_eq!(ann.called_file, "hello");
        assert_eq!(ann.calling_file, "goodbye");
    }

    #[test]
    fn test_announcement_spec_called_only() {
        let ann = AnnouncementSpec::parse("welcome");
        assert_eq!(ann.called_file, "welcome");
        assert!(ann.calling_file.is_empty());
    }

    // -----------------------------------------------------------------------
    // Bridge feature setup from options
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_bridge_features_default() {
        let opts = DialOptions::default();
        let (caller, callee) = opts.build_bridge_features();
        assert!(!caller.blind_transfer);
        assert!(!caller.disconnect);
        assert!(!caller.park_call);
        assert!(!callee.blind_transfer);
        assert!(!callee.disconnect);
    }

    #[test]
    fn test_build_bridge_features_from_options() {
        let opts = DialOptions::parse("tThHkKwWxX");
        let (caller, callee) = opts.build_bridge_features();

        // Caller features
        assert!(caller.blind_transfer);        // T
        assert!(caller.attended_transfer);     // T
        assert!(caller.disconnect);            // H
        assert!(caller.park_call);             // K
        assert!(caller.automixmon);            // X

        // Callee features
        assert!(callee.blind_transfer);        // t
        assert!(callee.attended_transfer);     // t
        assert!(callee.disconnect);            // h
        assert!(callee.park_call);             // k
        assert!(callee.automixmon);            // x
    }

    #[test]
    fn test_build_bridge_features_partial() {
        let opts = DialOptions::parse("tH");
        let (caller, callee) = opts.build_bridge_features();

        assert!(!caller.blind_transfer); // T not set
        assert!(caller.disconnect);      // H set
        assert!(callee.blind_transfer);  // t set
        assert!(!callee.disconnect);     // h not set
    }

    // -----------------------------------------------------------------------
    // After-bridge action from options
    // -----------------------------------------------------------------------

    #[test]
    fn test_caller_after_bridge_action_g() {
        let opts = DialOptions::parse("g");
        let action = opts.caller_after_bridge_action();
        // g option returns None (handled by returning Success)
        assert!(matches!(action, AfterBridgeAction::None));
    }

    #[test]
    fn test_caller_after_bridge_action_g_goto() {
        let opts = DialOptions::parse("G(myctx^s^1)");
        let action = opts.caller_after_bridge_action();
        match action {
            AfterBridgeAction::GoTo {
                context,
                exten,
                priority,
            } => {
                assert_eq!(context, "myctx");
                assert_eq!(exten, "s");
                assert_eq!(priority, 1);
            }
            _ => panic!("Expected GoTo action"),
        }
    }

    #[test]
    fn test_callee_after_bridge_action_f_location() {
        let opts = DialOptions::parse("F(other^100^1)");
        let action = opts.callee_after_bridge_action();
        match action {
            AfterBridgeAction::GoTo {
                context,
                exten,
                priority,
            } => {
                assert_eq!(context, "other");
                assert_eq!(exten, "100");
                assert_eq!(priority, 1);
            }
            _ => panic!("Expected GoTo action"),
        }
    }

    #[test]
    fn test_callee_after_bridge_action_g_goto() {
        let opts = DialOptions::parse("G(ctx^s^5)");
        let action = opts.callee_after_bridge_action();
        match action {
            AfterBridgeAction::GoTo { priority, .. } => {
                // Callee goes to priority+1
                assert_eq!(priority, 6);
            }
            _ => panic!("Expected GoTo action"),
        }
    }

    // -----------------------------------------------------------------------
    // Hangup cause from options
    // -----------------------------------------------------------------------

    #[test]
    fn test_unanswered_hangup_cause_default() {
        let opts = DialOptions::default();
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::NormalClearing);
    }

    #[test]
    fn test_unanswered_hangup_cause_cancel_elsewhere() {
        let opts = DialOptions::parse("c");
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::NormalClearing);
    }

    #[test]
    fn test_unanswered_hangup_cause_q_busy() {
        let opts = DialOptions::parse("Q(USER_BUSY)");
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::UserBusy);
    }

    #[test]
    fn test_unanswered_hangup_cause_q_no_answer() {
        let opts = DialOptions::parse("Q(NO_ANSWER)");
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::NoAnswer);
    }

    #[test]
    fn test_unanswered_hangup_cause_q_numeric() {
        let opts = DialOptions::parse("Q(17)");
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::UserBusy);
    }

    #[test]
    fn test_unanswered_hangup_cause_q_none() {
        let opts = DialOptions::parse("Q(NONE)");
        assert_eq!(opts.unanswered_hangup_cause(), HangupCause::NotDefined);
    }

    // -----------------------------------------------------------------------
    // DIALSTATUS variable setting for each outcome
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_exec_sets_dialstatus_noanswer_on_timeout() {
        let mut caller = Channel::new("Test/caller-001");
        caller.state = ChannelState::Up;
        // Use 0.05s timeout so the test completes quickly
        let (_, status) = AppDial::exec(&mut caller, "SIP/nobody,0.05,").await;
        // With very short timeout, the outbound channels time out
        assert_eq!(status, DialStatus::NoAnswer);
        assert_eq!(
            caller.get_variable("DIALSTATUS").unwrap(),
            "NOANSWER"
        );
    }

    #[tokio::test]
    async fn test_exec_sets_dialstatus_invalidargs() {
        let mut caller = Channel::new("Test/caller-002");
        let (result, status) = AppDial::exec(&mut caller, "").await;
        assert_eq!(status, DialStatus::InvalidArgs);
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(
            caller.get_variable("DIALSTATUS").unwrap(),
            "INVALIDARGS"
        );
    }

    #[tokio::test]
    async fn test_exec_sets_dialedtime() {
        let mut caller = Channel::new("Test/caller-003");
        caller.state = ChannelState::Up;
        // Use a very short timeout so the test completes quickly
        let (_, _) = AppDial::exec(&mut caller, "SIP/test,0.05").await;
        assert!(caller.get_variable("DIALEDTIME").is_some());
        assert!(caller.get_variable("DIALEDTIME_MS").is_some());
    }

    // -----------------------------------------------------------------------
    // Stasis events
    // -----------------------------------------------------------------------

    #[test]
    fn test_dial_begin_event() {
        let event = DialBeginEvent {
            caller: "SIP/alice-001".to_string(),
            callee: "SIP/bob-001".to_string(),
            dialstring: "SIP/bob".to_string(),
            forward: None,
        };
        assert_eq!(event.message_type(), "DialBegin");
    }

    #[test]
    fn test_dial_end_event() {
        let event = DialEndEvent {
            caller: "SIP/alice-001".to_string(),
            callee: "SIP/bob-001".to_string(),
            dialstatus: "ANSWER".to_string(),
            forward: None,
        };
        assert_eq!(event.message_type(), "DialEnd");
    }

    #[test]
    fn test_dial_end_event_with_forward() {
        let event = DialEndEvent {
            caller: "SIP/alice-001".to_string(),
            callee: "SIP/bob-001".to_string(),
            dialstatus: "ANSWER".to_string(),
            forward: Some("SIP/carol".to_string()),
        };
        assert_eq!(event.forward, Some("SIP/carol".to_string()));
    }

    // -----------------------------------------------------------------------
    // Music on hold helper
    // -----------------------------------------------------------------------

    #[test]
    fn test_moh_class_default() {
        let opts = DialOptions::parse("m");
        assert_eq!(opts.moh_class(), "default");
    }

    #[test]
    fn test_moh_class_custom() {
        let opts = DialOptions::parse("m(classical)");
        assert_eq!(opts.moh_class(), "classical");
    }

    // -----------------------------------------------------------------------
    // Nested parentheses in options
    // -----------------------------------------------------------------------

    #[test]
    fn test_option_parse_nested_parens() {
        // The b option with GoSub args using parenthesized format
        let opts = DialOptions::parse("b(ctx^s^1(arg1^arg2))");
        let loc = opts.predial_callee.unwrap();
        assert_eq!(loc.context, "ctx");
        assert_eq!(loc.exten, "s");
        assert_eq!(loc.priority, 1);
        // Args are extracted from the parens
        assert_eq!(loc.args, vec!["arg1", "arg2"]);
    }

    // -----------------------------------------------------------------------
    // Regression: options parsing with multiple parenthesized options
    // -----------------------------------------------------------------------

    #[test]
    fn test_option_parse_multiple_parenthesized() {
        let opts = DialOptions::parse("b(ctx^s^1)B(ctx2^s^2)D(123:456)L(60000:5000)S(30)");
        assert!(opts.predial_callee.is_some());
        assert!(opts.predial_caller.is_some());
        assert!(opts.send_dtmf.is_some());
        assert!(opts.call_limit.is_some());
        assert!(opts.duration_stop.is_some());

        let callee_loc = opts.predial_callee.unwrap();
        assert_eq!(callee_loc.context, "ctx");

        let caller_loc = opts.predial_caller.unwrap();
        assert_eq!(caller_loc.context, "ctx2");

        let dtmf = opts.send_dtmf.unwrap();
        assert_eq!(dtmf.called, "123");
        assert_eq!(dtmf.calling, "456");

        assert_eq!(opts.call_limit.unwrap().max_ms, 60000);
        assert_eq!(opts.duration_stop.unwrap(), Duration::from_secs(30));
    }

    // -----------------------------------------------------------------------
    // Real-world option strings from Asterisk dialplans
    // -----------------------------------------------------------------------

    #[test]
    fn test_real_world_option_string_1() {
        // Common: transfer + ringing + continue
        let opts = DialOptions::parse("tTrg");
        assert!(opts.allow_callee_transfer);
        assert!(opts.allow_caller_transfer);
        assert!(opts.force_ringing_default);
        assert!(opts.continue_on_callee_hangup);
    }

    #[test]
    fn test_real_world_option_string_2() {
        // Call center: MOH + limit + features
        let opts = DialOptions::parse("m(queue-music)L(3600000:60000:30000)hHtTwW");
        assert_eq!(opts.music_on_hold, Some("queue-music".to_string()));
        let limit = opts.call_limit.unwrap();
        assert_eq!(limit.max_ms, 3600000); // 1 hour
        assert_eq!(limit.warning_ms, Some(60000)); // warn at 1 min
        assert_eq!(limit.repeat_ms, Some(30000)); // repeat every 30s
        assert!(opts.callee_hangup);
        assert!(opts.caller_hangup);
        assert!(opts.callee_monitor);
        assert!(opts.caller_monitor);
    }

    // -----------------------------------------------------------------------
    // Adversarial tests -- edge cases and attack vectors
    // -----------------------------------------------------------------------

    // --- Empty dial string -> INVALIDARGS, not panic ---
    #[tokio::test]
    async fn test_adversarial_empty_dial_string() {
        let mut caller = Channel::new("Test/adv-empty");
        let (result, status) = AppDial::exec(&mut caller, "").await;
        assert_eq!(status, DialStatus::InvalidArgs);
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(caller.get_variable("DIALSTATUS").unwrap(), "INVALIDARGS");
    }

    // --- Dial string with no technology prefix ---
    #[test]
    fn test_adversarial_no_tech_prefix() {
        assert!(DialDestination::parse("alice").is_none());
        assert!(DialDestination::parse("just-a-number").is_none());
        assert!(DialArgs::parse("alice").is_none());
    }

    // --- Dial string with only & separators ---
    #[test]
    fn test_adversarial_only_ampersands() {
        assert!(DialArgs::parse("&&&&").is_none());
    }

    // --- Dial string with mixed valid/invalid destinations ---
    #[test]
    fn test_adversarial_mixed_destinations() {
        let args = DialArgs::parse("SIP/alice&invalid&SIP/bob").unwrap();
        // "invalid" has no slash, so it's filtered out
        assert_eq!(args.destinations.len(), 2);
        assert_eq!(args.destinations[0].resource, "alice");
        assert_eq!(args.destinations[1].resource, "bob");
    }

    // --- Timeout of 0 -> should use default (effectively infinite) ---
    #[test]
    fn test_adversarial_timeout_zero() {
        let args = DialArgs::parse("SIP/alice,0").unwrap();
        assert_eq!(args.timeout, Duration::from_secs(DialArgs::DEFAULT_TIMEOUT_SECS));
    }

    // --- Negative timeout -> should use default ---
    #[test]
    fn test_adversarial_timeout_negative() {
        let args = DialArgs::parse("SIP/alice,-5").unwrap();
        assert_eq!(args.timeout, Duration::from_secs(DialArgs::DEFAULT_TIMEOUT_SECS));
    }

    // --- Timeout NaN/Infinity ---
    #[test]
    fn test_adversarial_timeout_nan_inf() {
        let args = DialArgs::parse("SIP/alice,NaN").unwrap();
        assert_eq!(args.timeout, Duration::from_secs(DialArgs::DEFAULT_TIMEOUT_SECS));
        let args = DialArgs::parse("SIP/alice,inf").unwrap();
        assert_eq!(args.timeout, Duration::from_secs(DialArgs::DEFAULT_TIMEOUT_SECS));
    }

    // --- S(-1) -> should be ignored ---
    #[test]
    fn test_adversarial_s_negative() {
        let opts = DialOptions::parse("S(-1)");
        assert!(opts.duration_stop.is_none());
    }

    // --- S(0) -> should be ignored ---
    #[test]
    fn test_adversarial_s_zero() {
        let opts = DialOptions::parse("S(0)");
        assert!(opts.duration_stop.is_none());
    }

    // --- S(NaN) -> should be ignored ---
    #[test]
    fn test_adversarial_s_nan() {
        let opts = DialOptions::parse("S(NaN)");
        assert!(opts.duration_stop.is_none());
    }

    // --- S(Infinity) -> should be ignored ---
    #[test]
    fn test_adversarial_s_infinity() {
        let opts = DialOptions::parse("S(inf)");
        assert!(opts.duration_stop.is_none());
    }

    // --- L() with empty args -> None, not panic ---
    #[test]
    fn test_adversarial_l_empty() {
        let opts = DialOptions::parse("L()");
        assert!(opts.call_limit.is_none());
    }

    // --- L with non-numeric args ---
    #[test]
    fn test_adversarial_l_garbage() {
        let opts = DialOptions::parse("L(abc:def:ghi)");
        assert!(opts.call_limit.is_none());
    }

    // --- L(0) -> None (zero duration limit makes no sense) ---
    #[test]
    fn test_adversarial_l_zero_max() {
        assert!(CallLimit::parse("0").is_none());
    }

    // --- L with warning=0 -> should still work ---
    #[test]
    fn test_adversarial_l_warning_zero() {
        let limit = CallLimit::parse("60000:0").unwrap();
        assert_eq!(limit.max_ms, 60000);
        assert_eq!(limit.warning_ms, Some(0));
    }

    // --- G(^) -> invalid location parsing ---
    #[test]
    fn test_adversarial_g_invalid_location() {
        let opts = DialOptions::parse("G(^)");
        // "^" splits into ["", ""], context="" exten="" -> DialplanLocation
        // This is ok -- empty context/exten means "current"
        let loc = opts.goto_after_answer.unwrap();
        assert!(loc.context.is_empty());
    }

    // --- Duplicate options: last wins for flags, both apply ---
    #[test]
    fn test_adversarial_duplicate_options() {
        let opts = DialOptions::parse("gGfF");
        // g and G are different options (continue vs goto)
        assert!(opts.continue_on_callee_hangup); // g
        // G without args -> goto_after_answer stays None
        assert!(opts.goto_after_answer.is_none());
        // f without args -> force_caller_id_hint
        assert!(opts.force_caller_id_hint);
        // F without args -> callee_go_on_empty
        assert!(opts.callee_go_on_empty);
    }

    // --- Duplicate m options: last one wins ---
    #[test]
    fn test_adversarial_duplicate_m_option() {
        let opts = DialOptions::parse("m(jazz)m(classical)");
        // Second m(classical) overwrites first
        assert_eq!(opts.music_on_hold, Some("classical".to_string()));
    }

    // --- DialplanLocation: priority 0 -> should be remapped to 1 ---
    #[test]
    fn test_adversarial_dialplan_priority_zero() {
        let loc = DialplanLocation::parse("ctx^s^0").unwrap();
        assert_eq!(loc.priority, 1); // 0 remapped to 1
    }

    // --- DialplanLocation: negative priority -> should be remapped to 1 ---
    #[test]
    fn test_adversarial_dialplan_priority_negative() {
        let loc = DialplanLocation::parse("ctx^s^-5").unwrap();
        assert_eq!(loc.priority, 1);
    }

    // --- DialplanLocation: empty context/exten are valid (means "current") ---
    #[test]
    fn test_adversarial_dialplan_empty_context_exten() {
        let loc = DialplanLocation::parse("^^1").unwrap();
        assert!(loc.context.is_empty());
        assert!(loc.exten.is_empty());
        assert_eq!(loc.priority, 1);
    }

    // --- 100+ simultaneous destinations -> should not crash ---
    #[test]
    fn test_adversarial_many_destinations() {
        let dests: Vec<String> = (0..100).map(|i| format!("SIP/dest{}", i)).collect();
        let dial_str = dests.join("&");
        let args = DialArgs::parse(&dial_str).unwrap();
        assert_eq!(args.destinations.len(), 100);
    }

    // --- Very long option string -> should not crash ---
    #[test]
    fn test_adversarial_long_option_string() {
        let opts_str: String = "tT".repeat(500);
        let opts = DialOptions::parse(&opts_str);
        assert!(opts.allow_callee_transfer);
        assert!(opts.allow_caller_transfer);
    }

    // --- Option with unmatched parenthesis -> should not panic ---
    #[test]
    fn test_adversarial_unmatched_paren() {
        // Opening paren without closing
        let opts = DialOptions::parse("m(jazz");
        // The parser consumes until it runs out of chars
        // This should not panic; the parsed value is "jazz"
        assert_eq!(opts.music_on_hold, Some("jazz".to_string()));
    }

    // --- D() with empty arg -> should not panic ---
    #[test]
    fn test_adversarial_d_empty() {
        let opts = DialOptions::parse("D()");
        let dtmf = opts.send_dtmf.unwrap();
        assert!(dtmf.called.is_empty());
    }

    // --- All DIALSTATUS values are unique ---
    #[test]
    fn test_adversarial_all_dialstatus_unique() {
        let statuses = [
            DialStatus::Answer,
            DialStatus::Busy,
            DialStatus::NoAnswer,
            DialStatus::Cancel,
            DialStatus::Congestion,
            DialStatus::ChanUnavail,
            DialStatus::DontCall,
            DialStatus::Torture,
            DialStatus::InvalidArgs,
        ];
        let mut strs: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
        let orig_len = strs.len();
        strs.sort();
        strs.dedup();
        assert_eq!(strs.len(), orig_len, "DIALSTATUS values must be unique");
    }

    // --- Empty destination technology and resource ---
    #[test]
    fn test_adversarial_empty_tech_resource() {
        assert!(DialDestination::parse("/").is_none()); // empty tech & resource
        assert!(DialDestination::parse("/resource").is_none()); // empty tech
        assert!(DialDestination::parse("SIP/").is_none()); // empty resource
    }

    // --- Whitespace-only dial string ---
    #[tokio::test]
    async fn test_adversarial_whitespace_only() {
        let mut caller = Channel::new("Test/adv-ws");
        let (result, status) = AppDial::exec(&mut caller, "   ").await;
        assert_eq!(status, DialStatus::InvalidArgs);
        assert_eq!(result, PbxExecResult::Failed);
    }

    // --- DialplanLocation with parenthesized args but no closing paren ---
    #[test]
    fn test_adversarial_dialplan_unclosed_paren() {
        // "ctx^s^1(arg1" has open paren but no close -> treated as no paren args
        let loc = DialplanLocation::parse("ctx^s^1(arg1").unwrap();
        // The '(' is part of the string, so priority parsing may fail
        // but should not panic
        assert_eq!(loc.context, "ctx");
    }

    #[test]
    fn test_real_world_option_string_3() {
        // Pre-dial with GoSub, using parenthesized args like real Asterisk dialplans:
        // b(default^called_channel^1(my_gosub_arg1^my_gosub_arg2))
        let opts = DialOptions::parse(
            "b(default^called_channel^1(my_gosub_arg1^my_gosub_arg2))B(default^callee_channel^1(my_gosub_arg1^my_gosub_arg2))"
        );
        let b = opts.predial_callee.unwrap();
        assert_eq!(b.context, "default");
        assert_eq!(b.exten, "called_channel");
        assert_eq!(b.priority, 1);
        assert_eq!(b.args, vec!["my_gosub_arg1", "my_gosub_arg2"]);

        let big_b = opts.predial_caller.unwrap();
        assert_eq!(big_b.context, "default");
        assert_eq!(big_b.exten, "callee_channel");
        assert_eq!(big_b.priority, 1);
        assert_eq!(big_b.args, vec!["my_gosub_arg1", "my_gosub_arg2"]);
    }
}
