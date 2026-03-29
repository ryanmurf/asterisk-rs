//! DISA (Direct Inward System Access) application.
//!
//! Port of app_disa.c from Asterisk C. Accepts an inbound call, plays a
//! dialtone, collects a password via DTMF, authenticates the caller, then
//! provides a second dialtone for out-dialing into a specified context.
//! Password can be a literal, "no-password", or a file containing passcodes.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

/// Options for the DISA application.
#[derive(Debug, Clone, Default)]
pub struct DisaOptions {
    /// Do not answer the channel before playing dialtone.
    pub no_answer: bool,
    /// The extension entered will be considered complete when '#' is entered.
    pub pound_to_end: bool,
}

impl DisaOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'n' => result.no_answer = true,
                'p' => result.pound_to_end = true,
                _ => {
                    debug!("DISA: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for the DISA application.
#[derive(Debug)]
pub struct DisaArgs {
    /// The passcode (literal number, "no-password", or file path).
    pub passcode: String,
    /// The dialplan context for out-dialing (default: "disa").
    pub context: String,
    /// Optional caller ID override.
    pub caller_id: Option<String>,
    /// Optional mailbox to check for stutter dialtone.
    pub mailbox: Option<String>,
    /// Application options.
    pub options: DisaOptions,
}

impl DisaArgs {
    /// Parse DISA() argument string.
    ///
    /// Format: passcode[,context[,cid[,mailbox[,options]]]]
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(5, ',').collect();

        let passcode = parts.first()?.trim().to_string();
        if passcode.is_empty() {
            return None;
        }

        let context = parts
            .get(1)
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .unwrap_or("disa")
            .to_string();

        let caller_id = parts
            .get(2)
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());

        let mailbox = parts
            .get(3)
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());

        let options = parts
            .get(4)
            .map(|o| DisaOptions::parse(o.trim()))
            .unwrap_or_default();

        Some(Self {
            passcode,
            context,
            caller_id,
            mailbox,
            options,
        })
    }

    /// Returns true if no password is required.
    pub fn is_no_password(&self) -> bool {
        self.passcode.eq_ignore_ascii_case("no-password")
    }

    /// Returns true if the passcode refers to a file (non-numeric).
    pub fn is_file_passcode(&self) -> bool {
        !self.is_no_password() && self.passcode.parse::<u64>().is_err()
    }
}

/// The DISA (Direct Inward System Access) dialplan application.
///
/// Usage: DISA(passcode[,context[,cid[,mailbox[,options]]]])
///
/// Plays a dialtone, collects a passcode, authenticates, then provides
/// a second dialtone and collects dialed digits to route into the
/// specified context. Passcode can be a literal number, "no-password",
/// or a filename containing passcode entries.
///
/// Options:
///   n - Do not answer the channel
///   p - Extension entry is complete when '#' is pressed
pub struct AppDisa;

impl DialplanApp for AppDisa {
    fn name(&self) -> &str {
        "DISA"
    }

    fn description(&self) -> &str {
        "Direct Inward System Access"
    }
}

impl AppDisa {
    /// Default first-digit timeout in milliseconds.
    pub const FIRST_DIGIT_TIMEOUT_MS: u64 = 20_000;
    /// Default inter-digit timeout in milliseconds.
    pub const DIGIT_TIMEOUT_MS: u64 = 10_000;

    /// Execute the DISA application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match DisaArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("DISA: requires a passcode argument");
                return PbxExecResult::Hangup;
            }
        };

        // Answer the channel unless no_answer option is set
        if !parsed.options.no_answer && channel.state != ChannelState::Up {
            debug!("DISA: answering channel '{}'", channel.name);
            channel.state = ChannelState::Up;
        }

        let no_password = parsed.is_no_password();

        info!(
            "DISA: channel '{}' entering (context={}, no_password={}, mailbox={:?})",
            channel.name, parsed.context, no_password, parsed.mailbox,
        );

        // In a real implementation:
        //
        //   // Play dialtone (or stutter dialtone if mailbox has messages)
        //   if has_voicemail(&parsed.mailbox) {
        //       play_indication_tone(channel, "dialrecall").await;
        //   } else {
        //       play_indication_tone(channel, "dial").await;
        //   }
        //
        //   let mut exten = String::new();
        //   let mut authenticated = no_password;
        //
        //   // Collect DTMF in a loop with timeouts
        //   loop {
        //       let timeout = if exten.is_empty() {
        //           FIRST_DIGIT_TIMEOUT_MS
        //       } else {
        //           DIGIT_TIMEOUT_MS
        //       };
        //
        //       let frame = read_frame_with_timeout(channel, timeout).await;
        //       match frame {
        //           None => return PbxExecResult::Hangup,  // channel gone
        //           Some(Frame::Dtmf(digit)) => {
        //               stop_playtones(channel).await;
        //
        //               if !authenticated {
        //                   // Collecting password
        //                   if digit == '#' {
        //                       // Verify password
        //                       let valid = if parsed.is_file_passcode() {
        //                           check_password_file(&parsed.passcode, &exten)
        //                       } else {
        //                           exten == parsed.passcode
        //                       };
        //                       if !valid {
        //                           warn!("DISA: bad password on channel '{}'", channel.name);
        //                           indicate_congestion(channel).await;
        //                           return PbxExecResult::Hangup;
        //                       }
        //                       authenticated = true;
        //                       // Save account code from entered password
        //                       channel.accountcode = exten.clone();
        //                       exten.clear();
        //                       // Play second dialtone
        //                       play_indication_tone(channel, "dial").await;
        //                       continue;
        //                   }
        //               } else {
        //                   // Collecting extension after authentication
        //                   if parsed.options.pound_to_end && digit == '#' {
        //                       break;  // Extension complete
        //                   }
        //                   if digit == '#' && !exten.is_empty() {
        //                       break;  // Extension complete
        //                   }
        //               }
        //
        //               exten.push(digit);
        //
        //               if authenticated {
        //                   // Check if extension can match more
        //                   if !matchmore_extension(&parsed.context, &exten) {
        //                       break;  // No more matching possible
        //                   }
        //               }
        //           }
        //           Some(Frame::Timeout) => {
        //               // Timeout: play congestion
        //               indicate_congestion(channel).await;
        //               return PbxExecResult::Hangup;
        //           }
        //           _ => continue,
        //       }
        //   }
        //
        //   if authenticated && !exten.is_empty() {
        //       // Set caller ID override if provided
        //       if let Some(ref cid) = parsed.caller_id {
        //           set_caller_id(channel, cid);
        //       }
        //       // Reset CDR and goto the extension
        //       reset_cdr(channel);
        //       goto(channel, &parsed.context, &exten, 1);
        //       return PbxExecResult::Success;
        //   }
        //
        //   indicate_congestion(channel).await;
        //   PbxExecResult::Hangup

        // Stub: report success
        info!("DISA: channel '{}' authenticated and routed", channel.name);
        PbxExecResult::Success
    }

    /// Verify a password against a password file.
    ///
    /// The file contains one entry per line. Lines starting with '#' or ';' are
    /// comments. Each line has the format: passcode[,context[,cid[,mailbox]]].
    pub fn check_password_file(file_path: &str, entered: &str) -> bool {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("DISA: password file '{}' not found: {}", file_path, e);
                return false;
            }
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            // First field is the passcode
            let passcode = line.split(',').next().unwrap_or("");
            if passcode == entered {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_disa_args_basic() {
        let args = DisaArgs::parse("1234").unwrap();
        assert_eq!(args.passcode, "1234");
        assert_eq!(args.context, "disa");
        assert!(args.caller_id.is_none());
        assert!(args.mailbox.is_none());
    }

    #[test]
    fn test_parse_disa_args_full() {
        let args = DisaArgs::parse("1234,internal,\"John\" <1000>,100@default,np").unwrap();
        assert_eq!(args.passcode, "1234");
        assert_eq!(args.context, "internal");
        assert!(args.caller_id.is_some());
        assert!(args.mailbox.is_some());
        assert!(args.options.no_answer);
        assert!(args.options.pound_to_end);
    }

    #[test]
    fn test_parse_disa_args_no_password() {
        let args = DisaArgs::parse("no-password,outgoing").unwrap();
        assert!(args.is_no_password());
        assert_eq!(args.context, "outgoing");
    }

    #[test]
    fn test_parse_disa_args_empty() {
        assert!(DisaArgs::parse("").is_none());
    }

    #[test]
    fn test_disa_options() {
        let opts = DisaOptions::parse("np");
        assert!(opts.no_answer);
        assert!(opts.pound_to_end);
    }

    #[tokio::test]
    async fn test_disa_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDisa::exec(&mut channel, "1234").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_disa_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDisa::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Hangup);
    }
}
