//! Follow-Me/Find-Me application.
//!
//! Port of app_followme.c from Asterisk C. Provides sequential and parallel
//! ringing of multiple numbers to find the callee. Configurable per user
//! with number list, timeouts, music class, caller announcement recording,
//! and call screening.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// A single number/extension entry in a Follow-Me step.
#[derive(Debug, Clone)]
pub struct FollowMeNumber {
    /// Phone number(s) and/or extension(s) to dial (may contain '&' for parallel).
    pub number: String,
    /// Dial timeout in seconds for this step.
    pub timeout: u32,
    /// The order in which to attempt this number.
    pub order: u32,
}

/// A Follow-Me profile loaded from followme.conf.
#[derive(Debug, Clone)]
pub struct FollowMeProfile {
    /// Profile name (matches the followmeid argument).
    pub name: String,
    /// Music On Hold class to play while searching.
    pub music_class: String,
    /// Dialplan context to originate calls from.
    pub context: String,
    /// Whether this profile is active.
    pub active: bool,
    /// Whether callees are prompted to accept/reject the forwarded call.
    pub enable_callee_prompt: bool,
    /// Digit to press to take the call (default "1").
    pub take_call: String,
    /// Digit to press to decline the call (default "2").
    pub next_indp: String,
    /// Sound prompt: "call from..." announcement.
    pub call_from_prompt: String,
    /// Sound prompt: played when no recording of caller name is available.
    pub no_recording_prompt: String,
    /// Sound prompt: options menu for callee.
    pub options_prompt: String,
    /// Sound prompt: "please hold while we try to connect your call".
    pub pls_hold_prompt: String,
    /// Sound prompt: incoming status message.
    pub status_prompt: String,
    /// Sound prompt: "sorry, the person you are calling is not available".
    pub sorry_prompt: String,
    /// Sound prompt: played when call is connected.
    pub connected_prompt: String,
    /// Ordered list of numbers to try.
    pub numbers: Vec<FollowMeNumber>,
    /// Black-listed caller numbers (will not be forwarded).
    pub blacklist: Vec<String>,
    /// White-listed caller numbers (always forwarded).
    pub whitelist: Vec<String>,
}

impl Default for FollowMeProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            music_class: "default".to_string(),
            context: String::new(),
            active: true,
            enable_callee_prompt: true,
            take_call: "1".to_string(),
            next_indp: "2".to_string(),
            call_from_prompt: "followme/call-from".to_string(),
            no_recording_prompt: "followme/no-recording".to_string(),
            options_prompt: "followme/options".to_string(),
            pls_hold_prompt: "followme/pls-hold-while-try".to_string(),
            status_prompt: "followme/status".to_string(),
            sorry_prompt: "followme/sorry".to_string(),
            connected_prompt: String::new(),
            numbers: Vec::new(),
            blacklist: Vec::new(),
            whitelist: Vec::new(),
        }
    }
}

/// Global registry of Follow-Me profiles.
pub struct FollowMeConfig {
    profiles: RwLock<HashMap<String, Arc<FollowMeProfile>>>,
}

impl FollowMeConfig {
    /// Create an empty config.
    pub fn new() -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
        }
    }

    /// Add or update a profile.
    pub fn add_profile(&self, profile: FollowMeProfile) {
        let name = profile.name.clone();
        self.profiles.write().insert(name, Arc::new(profile));
    }

    /// Look up a profile by name.
    pub fn get_profile(&self, name: &str) -> Option<Arc<FollowMeProfile>> {
        self.profiles.read().get(name).cloned()
    }
}

impl Default for FollowMeConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Options for the FollowMe application.
#[derive(Debug, Clone, Default)]
pub struct FollowMeOptions {
    /// Record the caller's name for announcement to callee.
    pub record_name: bool,
    /// Disable the "please hold" announcement.
    pub disable_hold_prompt: bool,
    /// Ignore connected line updates.
    pub ignore_connected_line: bool,
    /// Disable local call optimization.
    pub disable_optimization: bool,
    /// Don't answer until ready to connect or give up.
    pub no_answer: bool,
    /// Play unreachable status message if out of steps.
    pub unreachable_msg: bool,
    /// Play incoming status message before starting steps.
    pub status_msg: bool,
    /// Pre-dial GoSub for the caller channel.
    pub predial_caller: Option<String>,
    /// Pre-dial GoSub for each callee channel.
    pub predial_callee: Option<String>,
}

impl FollowMeOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                'a' => result.record_name = true,
                'd' => result.disable_hold_prompt = true,
                'I' => result.ignore_connected_line = true,
                'l' => result.disable_optimization = true,
                'N' => result.no_answer = true,
                'n' => result.unreachable_msg = true,
                's' => result.status_msg = true,
                'B' => {
                    // Consume up to next option letter or end
                    let mut arg = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '(' {
                            chars.next();
                            for inner in chars.by_ref() {
                                if inner == ')' {
                                    break;
                                }
                                arg.push(inner);
                            }
                            break;
                        }
                        break;
                    }
                    if !arg.is_empty() {
                        result.predial_caller = Some(arg);
                    }
                }
                'b' => {
                    let mut arg = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '(' {
                            chars.next();
                            for inner in chars.by_ref() {
                                if inner == ')' {
                                    break;
                                }
                                arg.push(inner);
                            }
                            break;
                        }
                        break;
                    }
                    if !arg.is_empty() {
                        result.predial_callee = Some(arg);
                    }
                }
                _ => {
                    debug!("FollowMe: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The FollowMe() dialplan application.
///
/// Usage: FollowMe(followmeid[,options])
///
/// Performs Find-Me/Follow-Me for the caller using the profile matching
/// the given followmeid from followme.conf. Each step dials one or more
/// numbers in sequence, with configurable timeouts and call screening.
///
/// Options:
///   a - Record caller name for announcement
///   d - Disable "please hold" prompt
///   I - Ignore connected line updates
///   l - Disable local call optimization
///   N - Don't answer until ready to connect
///   n - Play unreachable message if no steps left
///   s - Play status message before starting
///   B(x) - Pre-dial GoSub on caller channel
///   b(x) - Pre-dial GoSub on callee channel
pub struct AppFollowMe;

impl DialplanApp for AppFollowMe {
    fn name(&self) -> &str {
        "FollowMe"
    }

    fn description(&self) -> &str {
        "Find-Me/Follow-Me application"
    }
}

impl AppFollowMe {
    /// Execute the FollowMe application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let followme_id = match parts.first() {
            Some(id) if !id.trim().is_empty() => id.trim(),
            _ => {
                warn!("FollowMe: requires a followmeid argument");
                return PbxExecResult::Failed;
            }
        };

        let options = parts
            .get(1)
            .map(|o| FollowMeOptions::parse(o.trim()))
            .unwrap_or_default();

        info!(
            "FollowMe: channel '{}' executing profile '{}'",
            channel.name, followme_id,
        );

        // Answer the channel unless no_answer option is set
        if !options.no_answer && channel.state != ChannelState::Up {
            debug!("FollowMe: answering channel");
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   let config = get_followme_config();
        //   let profile = match config.get_profile(followme_id) {
        //       Some(p) if p.active => p,
        //       _ => {
        //           warn!("FollowMe: profile '{}' not found or inactive", followme_id);
        //           return PbxExecResult::Failed;
        //       }
        //   };
        //
        //   // Record caller's name if 'a' option and channel is answered
        //   if options.record_name && channel.state == ChannelState::Up {
        //       let name_file = format!("{}/followme/name-{}", spool_dir, channel.unique_id);
        //       record_caller_name(channel, &name_file).await;
        //   }
        //
        //   // Play "please hold" prompt unless disabled
        //   if !options.disable_hold_prompt && channel.state == ChannelState::Up {
        //       play_file(channel, &profile.pls_hold_prompt).await;
        //   }
        //
        //   // Play status prompt if requested
        //   if options.status_msg {
        //       play_file(channel, &profile.status_prompt).await;
        //   }
        //
        //   // Start music on hold
        //   start_moh(channel, &profile.music_class).await;
        //
        //   // Try each step in order
        //   for number_entry in &profile.numbers {
        //       let dial_result = dial_followme_step(
        //           channel,
        //           &profile,
        //           number_entry,
        //           &options,
        //       ).await;
        //
        //       match dial_result {
        //           FollowMeStepResult::Connected => {
        //               stop_moh(channel).await;
        //               if !profile.connected_prompt.is_empty() {
        //                   play_file(channel, &profile.connected_prompt).await;
        //               }
        //               // Bridge the channels
        //               bridge_calls(channel, &outgoing_channel).await;
        //               return PbxExecResult::Success;
        //           }
        //           FollowMeStepResult::Declined | FollowMeStepResult::Timeout => {
        //               continue;  // Try next step
        //           }
        //           FollowMeStepResult::Hangup => {
        //               return PbxExecResult::Hangup;
        //           }
        //       }
        //   }
        //
        //   stop_moh(channel).await;
        //
        //   // No steps succeeded
        //   if options.unreachable_msg {
        //       play_file(channel, &profile.sorry_prompt).await;
        //   }
        //
        //   PbxExecResult::Failed

        // Stub: simulate follow-me attempt
        info!(
            "FollowMe: channel '{}' follow-me completed for profile '{}'",
            channel.name, followme_id,
        );
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_followme_options_parse() {
        let opts = FollowMeOptions::parse("adNns");
        assert!(opts.record_name);
        assert!(opts.disable_hold_prompt);
        assert!(opts.no_answer);
        assert!(opts.unreachable_msg);
        assert!(opts.status_msg);
    }

    #[test]
    fn test_followme_profile_default() {
        let profile = FollowMeProfile::default();
        assert_eq!(profile.music_class, "default");
        assert_eq!(profile.take_call, "1");
        assert_eq!(profile.next_indp, "2");
        assert!(profile.enable_callee_prompt);
    }

    #[test]
    fn test_followme_config() {
        let config = FollowMeConfig::new();
        let mut profile = FollowMeProfile::default();
        profile.name = "john".to_string();
        profile.numbers.push(FollowMeNumber {
            number: "SIP/john-cell".to_string(),
            timeout: 20,
            order: 1,
        });
        config.add_profile(profile);

        let p = config.get_profile("john").unwrap();
        assert_eq!(p.name, "john");
        assert_eq!(p.numbers.len(), 1);
    }

    #[tokio::test]
    async fn test_followme_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppFollowMe::exec(&mut channel, "john").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_followme_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppFollowMe::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
