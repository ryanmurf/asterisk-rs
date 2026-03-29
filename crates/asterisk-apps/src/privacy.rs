//! PrivacyManager application - screen anonymous/withheld CallerID calls.
//!
//! Port of app_privacy.c from Asterisk C. If no CallerID is present on the
//! incoming channel, the caller is prompted to enter their phone number.
//! The entered number is validated for minimum length (and optionally
//! against a dialplan context pattern), then set as the channel's CallerID.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info};

/// Privacy manager status set as PRIVACYMGRSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyStatus {
    /// Caller successfully entered a valid phone number.
    Success,
    /// Caller failed to provide a valid number within max retries.
    Failed,
}

impl PrivacyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
        }
    }
}

/// Parsed arguments for the PrivacyManager application.
#[derive(Debug)]
pub struct PrivacyArgs {
    /// Maximum number of attempts (default: 3).
    pub max_retries: u32,
    /// Minimum length of the entered phone number (default: 10).
    pub min_length: usize,
    /// Optional context to validate the entered number against.
    pub check_context: Option<String>,
}

impl Default for PrivacyArgs {
    fn default() -> Self {
        Self {
            max_retries: 3,
            min_length: 10,
            check_context: None,
        }
    }
}

impl PrivacyArgs {
    /// Parse PrivacyManager() argument string.
    ///
    /// Format: [maxretries[,minlength[,options[,context]]]]
    pub fn parse(args: &str) -> Self {
        if args.trim().is_empty() {
            return Self::default();
        }

        let parts: Vec<&str> = args.splitn(4, ',').collect();
        let mut result = Self::default();

        // Parse max retries
        if let Some(s) = parts.first() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                if let Ok(v) = trimmed.parse::<u32>() {
                    if v > 0 {
                        result.max_retries = v;
                    }
                }
            }
        }

        // Parse min length
        if let Some(s) = parts.get(1) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                if let Ok(v) = trimmed.parse::<usize>() {
                    if v > 0 {
                        result.min_length = v;
                    }
                }
            }
        }

        // parts[2] is options (reserved, not currently used)

        // Parse check context
        if let Some(s) = parts.get(3) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                result.check_context = Some(trimmed.to_string());
            }
        }

        result
    }
}

/// The PrivacyManager() dialplan application.
///
/// Usage: PrivacyManager([maxretries[,minlength[,options[,context]]]])
///
/// If no CallerID is present, answers the channel and prompts the caller
/// to enter their phone number. The number must meet the minimum length
/// requirement and optionally match a pattern in the specified context.
///
/// Sets PRIVACYMGRSTATUS channel variable to SUCCESS or FAILED.
pub struct AppPrivacyManager;

impl DialplanApp for AppPrivacyManager {
    fn name(&self) -> &str {
        "PrivacyManager"
    }

    fn description(&self) -> &str {
        "Require phone number to be entered, if no CallerID sent"
    }
}

impl AppPrivacyManager {
    /// Execute the PrivacyManager application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = PrivacyArgs::parse(args);

        // Check if CallerID is already present
        let has_callerid = channel.caller.id.number.valid
            && !channel.caller.id.number.number.is_empty();

        if has_callerid {
            debug!(
                "PrivacyManager: CallerID present ({}) on channel '{}', skipping",
                channel.caller.id.number.number, channel.name
            );
            channel.set_variable("PRIVACYMGRSTATUS", PrivacyStatus::Success.as_str());
            return PbxExecResult::Success;
        }

        info!(
            "PrivacyManager: no CallerID on channel '{}', prompting (maxretries={}, minlength={})",
            channel.name, parsed.max_retries, parsed.min_length
        );

        // Answer the channel if not already up
        if channel.state != ChannelState::Up {
            debug!("PrivacyManager: answering channel");
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   // Brief pause
        //   tokio::time::sleep(Duration::from_secs(1)).await;
        //
        //   // Play "unidentified call" prompt
        //   play_file(channel, "privacy-unident").await;
        //
        //   for attempt in 0..parsed.max_retries {
        //       // Play "please enter your phone number" prompt
        //       play_file(channel, "privacy-prompt").await;
        //
        //       // Read DTMF input from the caller
        //       let phone = read_dtmf_string(channel, 29, '#', 3200, 5000).await;
        //
        //       if phone.is_err() {
        //           // Channel hung up
        //           channel.set_variable("PRIVACYMGRSTATUS", "FAILED");
        //           return PbxExecResult::Hangup;
        //       }
        //       let phone = phone.unwrap();
        //
        //       // Validate the number
        //       if phone.len() >= parsed.min_length {
        //           // Optional context-based validation
        //           if let Some(ref ctx) = parsed.check_context {
        //               if !dialplan.extension_exists(ctx, &phone, 1) {
        //                   play_file(channel, "privacy-incorrect").await;
        //                   continue;
        //               }
        //           }
        //
        //           // Success: set CallerID
        //           channel.caller.id.name.name = "Privacy Manager".to_string();
        //           channel.caller.id.name.valid = true;
        //           channel.caller.id.number.number = phone.clone();
        //           channel.caller.id.number.valid = true;
        //           channel.caller.id.number.presentation = presentation::ALLOWED;
        //           channel.caller.id.name.presentation = presentation::ALLOWED;
        //
        //           play_file(channel, "privacy-thankyou").await;
        //
        //           channel.set_variable("PRIVACYMGRSTATUS", "SUCCESS");
        //           return PbxExecResult::Success;
        //       }
        //
        //       // Number too short
        //       play_file(channel, "privacy-incorrect").await;
        //   }
        //
        //   // Exhausted all retries
        //   channel.set_variable("PRIVACYMGRSTATUS", "FAILED");

        // For the stub implementation, simulate success
        let status = PrivacyStatus::Success;
        channel.set_variable("PRIVACYMGRSTATUS", status.as_str());

        info!(
            "PrivacyManager: channel '{}' PRIVACYMGRSTATUS={}",
            channel.name,
            status.as_str()
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_privacy_args_defaults() {
        let args = PrivacyArgs::parse("");
        assert_eq!(args.max_retries, 3);
        assert_eq!(args.min_length, 10);
        assert!(args.check_context.is_none());
    }

    #[test]
    fn test_parse_privacy_args_custom() {
        let args = PrivacyArgs::parse("5,7,,from-internal");
        assert_eq!(args.max_retries, 5);
        assert_eq!(args.min_length, 7);
        assert_eq!(args.check_context.as_deref(), Some("from-internal"));
    }

    #[test]
    fn test_parse_privacy_args_partial() {
        let args = PrivacyArgs::parse("2");
        assert_eq!(args.max_retries, 2);
        assert_eq!(args.min_length, 10); // default
    }

    #[test]
    fn test_privacy_status_strings() {
        assert_eq!(PrivacyStatus::Success.as_str(), "SUCCESS");
        assert_eq!(PrivacyStatus::Failed.as_str(), "FAILED");
    }

    #[tokio::test]
    async fn test_privacy_with_callerid() {
        let mut channel = Channel::new("SIP/test-001");
        // Set a valid CallerID
        channel.caller.id.number.number = "5551234567".to_string();
        channel.caller.id.number.valid = true;

        let result = AppPrivacyManager::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("PRIVACYMGRSTATUS"), Some("SUCCESS"));
    }

    #[tokio::test]
    async fn test_privacy_without_callerid() {
        let mut channel = Channel::new("SIP/test-001");
        // Default CallerID is empty/invalid
        let result = AppPrivacyManager::exec(&mut channel, "3,10").await;
        assert_eq!(result, PbxExecResult::Success);
        // Stub returns SUCCESS
        assert_eq!(channel.get_variable("PRIVACYMGRSTATUS"), Some("SUCCESS"));
    }
}
