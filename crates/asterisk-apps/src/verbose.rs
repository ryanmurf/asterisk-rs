//! Verbose, Log, and NoOp applications - logging and debug tools.
//!
//! Port of app_verbose.c from Asterisk C. Provides applications for
//! sending messages to the verbose output, to specific log levels,
//! and a no-operation application useful for dialplan debugging.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, error, info, trace, warn};

/// The Verbose() dialplan application.
///
/// Sends arbitrary text to verbose output at a specified verbosity level.
///
/// Usage: Verbose([level,]message)
///
/// If level is not specified, defaults to 0.
/// Level is clamped to range 0-4.
pub struct AppVerbose;

impl DialplanApp for AppVerbose {
    fn name(&self) -> &str {
        "Verbose"
    }

    fn description(&self) -> &str {
        "Send arbitrary text to verbose output"
    }
}

impl AppVerbose {
    /// Execute the Verbose application.
    ///
    /// # Arguments
    /// * `_channel` - The current channel (unused but part of the interface)
    /// * `args` - `[level,]message`
    pub fn exec(_channel: &Channel, args: &str) -> PbxExecResult {
        if args.is_empty() {
            return PbxExecResult::Success;
        }

        let (level, message) = Self::parse_args(args);

        // Map verbose levels to tracing levels:
        // 0 -> info (always shown)
        // 1 -> info
        // 2 -> debug
        // 3 -> debug
        // 4 -> trace
        match level {
            0 | 1 => info!("Verbose: {}", message),
            2 | 3 => debug!("Verbose: {}", message),
            _ => trace!("Verbose: {}", message),
        }

        PbxExecResult::Success
    }

    /// Parse the argument string into level and message.
    ///
    /// If the first field is a number, it's treated as the level.
    /// Otherwise, the entire string is the message at level 0.
    fn parse_args(args: &str) -> (u32, &str) {
        if let Some(comma_pos) = args.find(',') {
            let potential_level = args[..comma_pos].trim();
            if let Ok(level) = potential_level.parse::<u32>() {
                let level = level.min(4);
                let message = args[comma_pos + 1..].trim();
                return (level, message);
            }
        }
        // No comma or first part is not a number -- treat entire string as message
        (0, args)
    }
}

/// Log level for the Log() application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warning,
    Notice,
    Debug,
    Verbose,
    Dtmf,
}

impl LogLevel {
    /// Parse a log level string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().as_str() {
            "ERROR" => Some(Self::Error),
            "WARNING" => Some(Self::Warning),
            "NOTICE" => Some(Self::Notice),
            "DEBUG" => Some(Self::Debug),
            "VERBOSE" => Some(Self::Verbose),
            "DTMF" => Some(Self::Dtmf),
            _ => None,
        }
    }
}

/// The Log() dialplan application.
///
/// Sends arbitrary text to a specified log level.
///
/// Usage: Log(level,message)
///
/// Level must be one of: ERROR, WARNING, NOTICE, DEBUG, VERBOSE, DTMF
pub struct AppLog;

impl DialplanApp for AppLog {
    fn name(&self) -> &str {
        "Log"
    }

    fn description(&self) -> &str {
        "Send arbitrary text to a selected log level"
    }
}

impl AppLog {
    /// Execute the Log application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - `level,message`
    pub fn exec(channel: &Channel, args: &str) -> PbxExecResult {
        if args.is_empty() {
            return PbxExecResult::Success;
        }

        let (level_str, message) = if let Some(comma_pos) = args.find(',') {
            (&args[..comma_pos], args[comma_pos + 1..].trim())
        } else {
            // No comma -- missing message
            warn!("Log: requires a level and message (level,message)");
            return PbxExecResult::Success;
        };

        let level = match LogLevel::parse(level_str) {
            Some(l) => l,
            None => {
                warn!("Log: unknown log level '{}' for channel '{}'", level_str, channel.name);
                return PbxExecResult::Success;
            }
        };

        let context_info = format!(
            "[{}@{} prio {}]",
            channel.exten, channel.context, channel.priority
        );

        match level {
            LogLevel::Error => error!("{} {}", context_info, message),
            LogLevel::Warning => warn!("{} {}", context_info, message),
            LogLevel::Notice => info!("{} {}", context_info, message),
            LogLevel::Debug => debug!("{} {}", context_info, message),
            LogLevel::Verbose => info!("{} {}", context_info, message),
            LogLevel::Dtmf => debug!("DTMF {} {}", context_info, message),
        }

        PbxExecResult::Success
    }
}

/// The NoOp() dialplan application.
///
/// Does absolutely nothing. Used in dialplans for debugging purposes --
/// the arguments are printed to the verbose output when the dialplan
/// is executing with sufficient verbosity.
///
/// Usage: NoOp([text])
pub struct AppNoOp;

impl DialplanApp for AppNoOp {
    fn name(&self) -> &str {
        "NoOp"
    }

    fn description(&self) -> &str {
        "Do Nothing (No Operation)"
    }
}

impl AppNoOp {
    /// Execute the NoOp application.
    ///
    /// Simply logs the arguments at debug level and returns success.
    pub fn exec(_channel: &Channel, args: &str) -> PbxExecResult {
        if !args.is_empty() {
            debug!("NoOp: {}", args);
        }
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verbose_parse_with_level() {
        let (level, msg) = AppVerbose::parse_args("3,Hello World");
        assert_eq!(level, 3);
        assert_eq!(msg, "Hello World");
    }

    #[test]
    fn test_verbose_parse_without_level() {
        let (level, msg) = AppVerbose::parse_args("Hello World");
        assert_eq!(level, 0);
        assert_eq!(msg, "Hello World");
    }

    #[test]
    fn test_verbose_parse_level_clamp() {
        let (level, _) = AppVerbose::parse_args("10,message");
        assert_eq!(level, 4);
    }

    #[test]
    fn test_log_level_parse() {
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warning));
        assert_eq!(LogLevel::parse("Notice"), Some(LogLevel::Notice));
        assert_eq!(LogLevel::parse("DEBUG"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("VERBOSE"), Some(LogLevel::Verbose));
        assert_eq!(LogLevel::parse("DTMF"), Some(LogLevel::Dtmf));
        assert_eq!(LogLevel::parse("UNKNOWN"), None);
    }

    #[test]
    fn test_verbose_exec() {
        let channel = Channel::new("SIP/test-001");
        let result = AppVerbose::exec(&channel, "2,Test message");
        assert_eq!(result, PbxExecResult::Success);
    }

    #[test]
    fn test_noop_exec() {
        let channel = Channel::new("SIP/test-001");
        let result = AppNoOp::exec(&channel, "debugging info here");
        assert_eq!(result, PbxExecResult::Success);
    }

    #[test]
    fn test_log_exec() {
        let channel = Channel::new("SIP/test-001");
        let result = AppLog::exec(&channel, "DEBUG,Test log message");
        assert_eq!(result, PbxExecResult::Success);
    }
}
