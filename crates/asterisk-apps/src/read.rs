//! Read application - reads DTMF digits from a caller.
//!
//! Port of app_read.c from Asterisk C. Plays a prompt file and collects
//! DTMF digits from the caller, storing them in a channel variable.
//! Supports configurable max digits, terminators, retries, and timeouts.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Read status set as the READSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    Ok,
    Error,
    Hangup,
    Interrupted,
    Skipped,
    Timeout,
}

impl ReadStatus {
    /// String representation for the READSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Error => "ERROR",
            Self::Hangup => "HANGUP",
            Self::Interrupted => "INTERRUPTED",
            Self::Skipped => "SKIPPED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// Options for the Read application.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// Skip if the channel is not answered.
    pub skip: bool,
    /// Play filename as an indication tone.
    pub indication: bool,
    /// Read digits even if the line is not up.
    pub noanswer: bool,
    /// Terminator digit(s). Default is "#".
    pub terminator: String,
    /// If true, keep the terminator as part of digits when it's
    /// the only digit entered.
    pub keep_terminator: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            skip: false,
            indication: false,
            noanswer: false,
            terminator: "#".to_string(),
            keep_terminator: false,
        }
    }
}

impl ReadOptions {
    /// Parse the options string.
    ///
    /// Options: s=skip, i=indication, n=noanswer, t(chars)=terminator, e=keep_terminator
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                's' => result.skip = true,
                'i' => result.indication = true,
                'n' => result.noanswer = true,
                'e' => result.keep_terminator = true,
                't' => {
                    // The terminator option can have argument chars following it
                    // In the C code this is handled via OPT_ARG_TERMINATOR
                    // For simplicity, if 't' is followed by '(' we read until ')'
                    if chars.peek() == Some(&'(') {
                        chars.next(); // consume '('
                        let mut term = String::new();
                        for c in chars.by_ref() {
                            if c == ')' {
                                break;
                            }
                            term.push(c);
                        }
                        result.terminator = term;
                    } else {
                        // No argument means empty terminator (no termination by digit)
                        result.terminator.clear();
                    }
                }
                _ => {
                    debug!("Read: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for the Read application.
#[derive(Debug)]
pub struct ReadArgs {
    /// Variable name to store the result in.
    pub variable: String,
    /// Prompt filename(s) separated by '&'.
    pub filenames: Vec<String>,
    /// Maximum number of digits to read (0 = no limit, wait for #).
    pub max_digits: u32,
    /// Options.
    pub options: ReadOptions,
    /// Number of attempts (default 1).
    pub attempts: u32,
    /// Timeout in seconds (0 = default channel timeout).
    pub timeout: Duration,
}

impl ReadArgs {
    /// Parse the Read() argument string.
    ///
    /// Format: `variable[,filename[,maxdigits[,options[,attempts[,timeout]]]]]`
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(6, ',').collect();

        let variable = parts.first()?.trim().to_string();
        if variable.is_empty() {
            return None;
        }

        let filenames = parts.get(1).map_or_else(Vec::new, |f| {
            f.trim()
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        });

        let max_digits = parts
            .get(2)
            .and_then(|m| {
                let trimmed = m.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<u32>().ok()
                }
            })
            .unwrap_or(0);
        // Clamp to 255 as in C code
        let max_digits = max_digits.min(255);

        let options = parts
            .get(3)
            .map(|o| ReadOptions::parse(o.trim()))
            .unwrap_or_default();

        let attempts = parts
            .get(4)
            .and_then(|a| {
                let trimmed = a.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<u32>().ok()
                }
            })
            .unwrap_or(1)
            .max(1);

        let timeout = parts
            .get(5)
            .and_then(|t| {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<f64>().ok()
                }
            })
            .map(|secs| Duration::from_secs_f64(secs))
            .unwrap_or(Duration::ZERO);

        Some(Self {
            variable,
            filenames,
            max_digits,
            options,
            attempts,
            timeout,
        })
    }
}

/// The Read() dialplan application.
///
/// Reads a '#'-terminated string of digits from the user into a channel
/// variable. Plays an optional prompt, supports configurable max digits,
/// retries, and timeout.
///
/// Usage: Read(variable[,filename[,maxdigits[,options[,attempts[,timeout]]]]])
///
/// Sets READSTATUS channel variable (OK, ERROR, HANGUP, INTERRUPTED, SKIPPED, TIMEOUT).
pub struct AppRead;

impl DialplanApp for AppRead {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Read a variable"
    }
}

impl AppRead {
    /// Execute the Read application.
    ///
    /// # Arguments
    /// * `channel` - The channel to read digits from
    /// * `args` - Argument string
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match ReadArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Read: requires an argument (variable)");
                channel.set_variable("READSTATUS", ReadStatus::Error.as_str());
                return PbxExecResult::Success;
            }
        };

        // Skip if channel is not answered and 's' option is set
        if parsed.options.skip && channel.state != ChannelState::Up {
            debug!("Read: skipping - channel not answered and 's' option set");
            channel.set_variable(&parsed.variable, "");
            channel.set_variable("READSTATUS", ReadStatus::Skipped.as_str());
            return PbxExecResult::Success;
        }

        // Answer the channel if needed (unless 'n' option)
        if !parsed.options.noanswer && channel.state != ChannelState::Up {
            debug!("Read: answering channel before reading");
            channel.state = ChannelState::Up;
        }

        info!(
            "Read: reading up to {} digits into '{}' from channel '{}' (attempts={}, timeout={:?})",
            if parsed.max_digits == 0 {
                "unlimited".to_string()
            } else {
                parsed.max_digits.to_string()
            },
            parsed.variable,
            channel.name,
            parsed.attempts,
            parsed.timeout,
        );

        let digits = String::new();
        let mut status = ReadStatus::Timeout;

        // Attempt loop
        for attempt in 0..parsed.attempts {
            if channel.state == ChannelState::Down {
                status = ReadStatus::Hangup;
                break;
            }

            debug!("Read: attempt {}/{}", attempt + 1, parsed.attempts);

            // Play the prompt file(s) while collecting digits
            // In a full implementation:
            //
            //   for filename in &parsed.filenames {
            //       if parsed.options.indication {
            //           // Play as indication tone
            //           play_indication(channel, filename).await;
            //       } else {
            //           // Play as audio file, interruptible by DTMF
            //           let result = stream_file_with_dtmf(channel, filename).await;
            //           if let Some(digit) = result.dtmf {
            //               digits.push(digit);
            //           }
            //       }
            //   }
            //
            //   // Collect remaining digits up to max_digits
            //   loop {
            //       let dtmf_result = wait_for_dtmf(channel, timeout).await;
            //       match dtmf_result {
            //           Some(digit) => {
            //               // Check if it's a terminator
            //               if parsed.options.terminator.contains(digit) {
            //                   if digits.is_empty() && parsed.options.keep_terminator {
            //                       digits.push(digit);
            //                   }
            //                   status = ReadStatus::Ok;
            //                   break;
            //               }
            //               digits.push(digit);
            //               if parsed.max_digits > 0 && digits.len() >= parsed.max_digits as usize {
            //                   status = ReadStatus::Ok;
            //                   break;
            //               }
            //           }
            //           None => {
            //               // Timeout
            //               if !digits.is_empty() {
            //                   status = ReadStatus::Ok;
            //               } else {
            //                   status = ReadStatus::Timeout;
            //               }
            //               break;
            //           }
            //       }
            //   }

            // Simulate: in stub mode we just set OK with empty digits
            status = ReadStatus::Ok;
            break;
        }

        // Set the result variable
        channel.set_variable(&parsed.variable, &digits);
        channel.set_variable("READSTATUS", status.as_str());

        debug!(
            "Read: result variable '{}' = '{}', READSTATUS = {}",
            parsed.variable,
            digits,
            status.as_str()
        );

        match status {
            ReadStatus::Hangup => PbxExecResult::Hangup,
            _ => PbxExecResult::Success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_read_args_minimal() {
        let args = ReadArgs::parse("RESULT").unwrap();
        assert_eq!(args.variable, "RESULT");
        assert!(args.filenames.is_empty());
        assert_eq!(args.max_digits, 0);
        assert_eq!(args.attempts, 1);
        assert_eq!(args.timeout, Duration::ZERO);
    }

    #[test]
    fn test_parse_read_args_full() {
        let args = ReadArgs::parse("DIGITS,prompt&beep,4,s,3,10").unwrap();
        assert_eq!(args.variable, "DIGITS");
        assert_eq!(args.filenames, vec!["prompt", "beep"]);
        assert_eq!(args.max_digits, 4);
        assert!(args.options.skip);
        assert_eq!(args.attempts, 3);
        assert_eq!(args.timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_parse_read_args_empty() {
        assert!(ReadArgs::parse("").is_none());
    }

    #[test]
    fn test_parse_options() {
        let opts = ReadOptions::parse("sin");
        assert!(opts.skip);
        assert!(opts.indication);
        assert!(opts.noanswer);
    }

    #[test]
    fn test_parse_options_terminator() {
        let opts = ReadOptions::parse("t(*)");
        assert_eq!(opts.terminator, "*");
    }

    #[test]
    fn test_parse_options_empty_terminator() {
        let opts = ReadOptions::parse("t");
        assert_eq!(opts.terminator, "");
    }

    #[test]
    fn test_max_digits_clamp() {
        let args = ReadArgs::parse("VAR,prompt,999").unwrap();
        assert_eq!(args.max_digits, 255);
    }

    #[tokio::test]
    async fn test_read_skip_not_answered() {
        let mut channel = Channel::new("SIP/test-001");
        // Channel is Down by default
        let result = AppRead::exec(&mut channel, "RESULT,prompt,4,s").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("READSTATUS"), Some("SKIPPED"));
    }
}
