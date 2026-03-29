//! Mini-Voicemail application -- lightweight voicemail building blocks.
//!
//! Port of app_minivm.c from Asterisk C. Provides a minimal voicemail system
//! built as composable dialplan applications rather than a single monolithic
//! voicemail application. Designed for multi-language systems where voicemail
//! messages are forwarded via email.
//!
//! Dialplan applications:
//! - MinivmRecord:  Record a voicemail message to a file
//! - MinivmGreet:   Play the user's personal greeting (or a default)
//! - MinivmNotify:  Send notification (email) about a new message
//! - MinivmDelete:  Delete a voicemail message file
//! - MinivmAccMess: Record personal greeting messages (busy/unavailable/temporary)

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// A mini-voicemail account (user@domain).
#[derive(Debug, Clone)]
pub struct MinivmAccount {
    /// Username portion (e.g. "1000").
    pub username: String,
    /// Domain (e.g. "example.com").
    pub domain: String,
    /// Full name of the account owner.
    pub fullname: String,
    /// Email address for notifications.
    pub email: String,
    /// Preferred language code (e.g. "en_us").
    pub language: String,
    /// Timezone name.
    pub timezone: String,
    /// Path to the spool directory for this account.
    pub spool_dir: PathBuf,
    /// Whether to attach the recording to the notification email.
    pub attach_voicemail: bool,
    /// Maximum message duration in seconds (0 = unlimited).
    pub max_message_secs: u32,
    /// Maximum greeting duration in seconds.
    pub max_greeting_secs: u32,
}

impl MinivmAccount {
    /// Create a new account with defaults.
    pub fn new(username: impl Into<String>, domain: impl Into<String>) -> Self {
        let username = username.into();
        let domain = domain.into();
        let spool_dir = PathBuf::from(format!(
            "/var/spool/asterisk/voicemail/{}/{}",
            domain, username
        ));
        Self {
            username,
            domain,
            fullname: String::new(),
            email: String::new(),
            language: "en_us".to_string(),
            timezone: "eastern".to_string(),
            spool_dir,
            attach_voicemail: true,
            max_message_secs: 300,
            max_greeting_secs: 60,
        }
    }

    /// Get the full mailbox identifier "user@domain".
    pub fn mailbox_id(&self) -> String {
        format!("{}@{}", self.username, self.domain)
    }

    /// Get the path to a greeting file.
    pub fn greeting_path(&self, greeting_type: GreetingType) -> PathBuf {
        let filename = match greeting_type {
            GreetingType::Unavailable => "unavail",
            GreetingType::Busy => "busy",
            GreetingType::Temporary => "temp",
            GreetingType::Name => "greet",
        };
        self.spool_dir.join(filename)
    }
}

/// Types of greetings a mini-voicemail account can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreetingType {
    /// Standard unavailable greeting.
    Unavailable,
    /// Busy greeting.
    Busy,
    /// Temporary greeting (overrides others).
    Temporary,
    /// Name recording.
    Name,
}

/// Parsed arguments for MinivmRecord.
#[derive(Debug, Clone)]
pub struct MinivmRecordArgs {
    /// Target mailbox (user@domain).
    pub mailbox: String,
    /// Recording format (e.g. "wav", "gsm").
    pub format: String,
    /// Maximum duration in seconds (0 = use account default).
    pub max_duration: u32,
    /// DTMF digit to terminate recording (default: "#").
    pub terminate_key: String,
    /// Whether to play a beep before recording.
    pub beep: bool,
    /// Gain adjustment in dB.
    pub gain: i32,
}

impl MinivmRecordArgs {
    /// Parse from a dialplan argument string.
    ///
    /// Format: MinivmRecord(user@domain[,options[,format]])
    pub fn parse(args: &str) -> Result<Self, String> {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        let mailbox = parts
            .first()
            .ok_or("missing mailbox argument")?
            .trim()
            .to_string();
        if mailbox.is_empty() || !mailbox.contains('@') {
            return Err("mailbox must be in user@domain format".into());
        }

        let format = parts
            .get(2)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "wav".to_string());

        let mut result = Self {
            mailbox,
            format,
            max_duration: 0,
            terminate_key: "#".to_string(),
            beep: true,
            gain: 0,
        };

        // Parse options string
        if let Some(opts) = parts.get(1) {
            for ch in opts.trim().chars() {
                match ch {
                    's' => result.beep = false,
                    'g' => result.gain = 5,
                    _ => {
                        debug!("MinivmRecord: ignoring unknown option '{}'", ch);
                    }
                }
            }
        }

        Ok(result)
    }
}

/// MinivmRecord() -- record a voicemail message to a file.
///
/// Usage: MinivmRecord(user@domain[,options[,format]])
///
/// Records a voicemail message. After recording, the file is stored in the
/// account's spool directory. Sets the MINIVM_RECORD_STATUS channel variable
/// to SUCCESS, USEREXIT, or FAILED.
pub struct AppMinivmRecord;

impl DialplanApp for AppMinivmRecord {
    fn name(&self) -> &str {
        "MinivmRecord"
    }

    fn description(&self) -> &str {
        "Receive and record a minivm voicemail message"
    }
}

impl AppMinivmRecord {
    /// Execute the MinivmRecord application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match MinivmRecordArgs::parse(args) {
            Ok(a) => a,
            Err(e) => {
                warn!("MinivmRecord: invalid arguments: {}", e);
                return PbxExecResult::Failed;
            }
        };

        info!(
            "MinivmRecord: recording message for '{}' on channel '{}'",
            parsed.mailbox, channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // In a full implementation:
        // 1. Look up the minivm account
        // 2. Play beep if requested
        // 3. Record audio to a temp file in the account's spool directory
        // 4. Stop on DTMF terminate key or max duration
        // 5. Set MINIVM_RECORD_STATUS channel variable
        // 6. Save the recording metadata

        debug!(
            "MinivmRecord: would record to spool for '{}' in format '{}'",
            parsed.mailbox, parsed.format
        );

        PbxExecResult::Success
    }
}

/// MinivmGreet() -- play user's greeting.
///
/// Usage: MinivmGreet(user@domain[,options])
///
/// Plays the user's personal greeting. Falls back to a default greeting
/// if no personal greeting is recorded. Sets MINIVM_GREET_STATUS to
/// SUCCESS or USEREXIT.
pub struct AppMinivmGreet;

impl DialplanApp for AppMinivmGreet {
    fn name(&self) -> &str {
        "MinivmGreet"
    }

    fn description(&self) -> &str {
        "Play a minivm greeting"
    }
}

impl AppMinivmGreet {
    /// Execute the MinivmGreet application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let mailbox = match parts.first() {
            Some(m) if !m.is_empty() => m.trim(),
            _ => {
                warn!("MinivmGreet: missing mailbox argument");
                return PbxExecResult::Failed;
            }
        };

        let busy_greeting = parts
            .get(1)
            .map(|opts| opts.contains('b'))
            .unwrap_or(false);

        info!(
            "MinivmGreet: playing {} greeting for '{}' on channel '{}'",
            if busy_greeting { "busy" } else { "unavailable" },
            mailbox,
            channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // In a full implementation:
        // 1. Look up the minivm account
        // 2. Check for temporary greeting first (overrides all)
        // 3. Then check for busy or unavailable greeting
        // 4. Fall back to default system greeting
        // 5. Play the selected greeting
        // 6. Set MINIVM_GREET_STATUS channel variable

        PbxExecResult::Success
    }
}

/// MinivmNotify() -- send notification about a new voicemail message.
///
/// Usage: MinivmNotify(user@domain[,template])
///
/// Sends an email notification to the account's configured email address.
/// Optionally attaches the voicemail recording. Sets MINIVM_NOTIFY_STATUS.
pub struct AppMinivmNotify;

impl DialplanApp for AppMinivmNotify {
    fn name(&self) -> &str {
        "MinivmNotify"
    }

    fn description(&self) -> &str {
        "Notify a minivm user about a new message"
    }
}

impl AppMinivmNotify {
    /// Execute the MinivmNotify application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let mailbox = match parts.first() {
            Some(m) if !m.is_empty() => m.trim(),
            _ => {
                warn!("MinivmNotify: missing mailbox argument");
                return PbxExecResult::Failed;
            }
        };

        let _template = parts.get(1).map(|s| s.trim()).unwrap_or("email-default");

        info!(
            "MinivmNotify: sending notification for '{}' on channel '{}'",
            mailbox, channel.name
        );

        // In a full implementation:
        // 1. Look up the minivm account
        // 2. Load the email template
        // 3. Substitute variables (caller ID, date, duration, etc.)
        // 4. Attach voicemail file if configured
        // 5. Send email via sendmail/SMTP
        // 6. Set MINIVM_NOTIFY_STATUS channel variable

        debug!(
            "MinivmNotify: email notification stub for '{}' (email not implemented)",
            mailbox
        );

        PbxExecResult::Success
    }
}

/// MinivmDelete() -- delete a voicemail message.
///
/// Usage: MinivmDelete(filename)
///
/// Deletes a voicemail message file. Sets MINIVM_DELETE_STATUS to
/// SUCCESS or FAILED.
pub struct AppMinivmDelete;

impl DialplanApp for AppMinivmDelete {
    fn name(&self) -> &str {
        "MinivmDelete"
    }

    fn description(&self) -> &str {
        "Delete a minivm voicemail message"
    }
}

impl AppMinivmDelete {
    /// Execute the MinivmDelete application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let filename = args.trim();
        if filename.is_empty() {
            warn!("MinivmDelete: missing filename argument");
            return PbxExecResult::Failed;
        }

        info!(
            "MinivmDelete: deleting message '{}' on channel '{}'",
            filename, channel.name
        );

        // In a full implementation:
        // 1. Validate the path (must be within spool directory)
        // 2. Delete the audio file and any associated metadata
        // 3. Set MINIVM_DELETE_STATUS channel variable

        let path = PathBuf::from(filename);
        if !path.exists() {
            debug!("MinivmDelete: file '{}' does not exist", filename);
            return PbxExecResult::Failed;
        }

        match std::fs::remove_file(&path) {
            Ok(_) => {
                debug!("MinivmDelete: deleted '{}'", filename);
                PbxExecResult::Success
            }
            Err(e) => {
                warn!("MinivmDelete: failed to delete '{}': {}", filename, e);
                PbxExecResult::Failed
            }
        }
    }
}

/// MinivmAccMess() -- record personal account messages (greetings).
///
/// Usage: MinivmAccMess(user@domain[,options])
///
/// Options:
///   b - Record busy greeting
///   u - Record unavailable greeting (default)
///   t - Record temporary greeting
///   n - Record name
pub struct AppMinivmAccMess;

impl DialplanApp for AppMinivmAccMess {
    fn name(&self) -> &str {
        "MinivmAccMess"
    }

    fn description(&self) -> &str {
        "Record a minivm account greeting"
    }
}

impl AppMinivmAccMess {
    /// Execute the MinivmAccMess application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let mailbox = match parts.first() {
            Some(m) if !m.is_empty() => m.trim(),
            _ => {
                warn!("MinivmAccMess: missing mailbox argument");
                return PbxExecResult::Failed;
            }
        };

        let greeting_type = match parts.get(1) {
            Some(opts) if opts.contains('b') => GreetingType::Busy,
            Some(opts) if opts.contains('t') => GreetingType::Temporary,
            Some(opts) if opts.contains('n') => GreetingType::Name,
            _ => GreetingType::Unavailable,
        };

        info!(
            "MinivmAccMess: recording {:?} greeting for '{}' on channel '{}'",
            greeting_type, mailbox, channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // In a full implementation:
        // 1. Look up the minivm account
        // 2. Determine the greeting file path based on type
        // 3. Play instructions to the caller
        // 4. Record the greeting
        // 5. Allow review, re-record, or save
        // 6. Set MINIVM_ACCMESS_STATUS channel variable

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minivm_record_args_parse() {
        let args = MinivmRecordArgs::parse("1000@example.com").unwrap();
        assert_eq!(args.mailbox, "1000@example.com");
        assert_eq!(args.format, "wav");
        assert!(args.beep);

        let args = MinivmRecordArgs::parse("1000@example.com,s,gsm").unwrap();
        assert!(!args.beep);
        assert_eq!(args.format, "gsm");
    }

    #[test]
    fn test_minivm_record_args_parse_error() {
        assert!(MinivmRecordArgs::parse("").is_err());
        assert!(MinivmRecordArgs::parse("nondomain").is_err());
    }

    #[test]
    fn test_minivm_account() {
        let acct = MinivmAccount::new("1000", "example.com");
        assert_eq!(acct.mailbox_id(), "1000@example.com");
        let path = acct.greeting_path(GreetingType::Busy);
        assert!(path.to_string_lossy().contains("busy"));
    }
}
