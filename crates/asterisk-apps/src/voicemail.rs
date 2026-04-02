//! VoiceMail application - voicemail system.
//!
//! Port of app_voicemail.c from Asterisk C. Provides a voicemail system
//! with mailboxes, message recording, playback, IMAP-style folders,
//! greeting management, silence detection, DTMF controls, email/pager
//! notifications, storage backends (file, IMAP, ODBC), MWI integration,
//! and voicemail.conf configuration parsing.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Global registry of mailboxes keyed by "mailbox@context".
static MAILBOXES: once_cell::sync::Lazy<DashMap<String, Arc<RwLock<Mailbox>>>> =
    once_cell::sync::Lazy::new(DashMap::new);

// ---------------------------------------------------------------------------
// VoicemailFolder
// ---------------------------------------------------------------------------

/// Standard IMAP-style voicemail folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoicemailFolder {
    /// New messages (INBOX)
    Inbox,
    /// Old (read) messages
    Old,
    /// Work messages
    Work,
    /// Family messages
    Family,
    /// Friends messages
    Friends,
}

impl VoicemailFolder {
    /// All standard folders.
    pub const ALL: [VoicemailFolder; 5] = [
        Self::Inbox,
        Self::Old,
        Self::Work,
        Self::Family,
        Self::Friends,
    ];

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Old => "Old",
            Self::Work => "Work",
            Self::Family => "Family",
            Self::Friends => "Friends",
        }
    }

    /// Folder number (0-based, for DTMF menu).
    pub fn number(&self) -> u32 {
        match self {
            Self::Inbox => 0,
            Self::Old => 1,
            Self::Work => 2,
            Self::Family => 3,
            Self::Friends => 4,
        }
    }

    /// Parse from folder number.
    pub fn from_number(n: u32) -> Option<Self> {
        match n {
            0 => Some(Self::Inbox),
            1 => Some(Self::Old),
            2 => Some(Self::Work),
            3 => Some(Self::Family),
            4 => Some(Self::Friends),
            _ => None,
        }
    }

    /// Directory name for file storage.
    pub fn dir_name(&self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Old => "Old",
            Self::Work => "Work",
            Self::Family => "Family",
            Self::Friends => "Friends",
        }
    }
}

impl std::fmt::Display for VoicemailFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Greeting Management
// ---------------------------------------------------------------------------

/// Types of greetings a mailbox can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GreetingType {
    /// Standard unavailable greeting
    #[default]
    Unavailable,
    /// Busy greeting
    Busy,
    /// Name recording
    Name,
    /// Temporary greeting (overrides all others when set)
    Temp,
}

impl GreetingType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unavailable => "unavail",
            Self::Busy => "busy",
            Self::Name => "greet",
            Self::Temp => "temp",
        }
    }

    pub fn file_name(&self) -> &'static str {
        match self {
            Self::Unavailable => "unavail",
            Self::Busy => "busy",
            Self::Name => "greet",
            Self::Temp => "temp",
        }
    }
}

/// Per-mailbox greeting state: which greetings have been recorded.
#[derive(Debug, Clone, Default)]
pub struct GreetingState {
    /// Whether an unavailable greeting has been recorded
    pub has_unavailable: bool,
    /// Whether a busy greeting has been recorded
    pub has_busy: bool,
    /// Whether a name recording exists
    pub has_name: bool,
    /// Whether a temporary greeting is set
    pub has_temp: bool,
}

impl GreetingState {
    /// Check if a specific greeting type has been recorded.
    pub fn has_greeting(&self, gtype: GreetingType) -> bool {
        match gtype {
            GreetingType::Unavailable => self.has_unavailable,
            GreetingType::Busy => self.has_busy,
            GreetingType::Name => self.has_name,
            GreetingType::Temp => self.has_temp,
        }
    }

    /// Determine which greeting to play, with fallback logic:
    /// 1. If temp greeting exists, always use it
    /// 2. If requested type (busy/unavail) exists, use it
    /// 3. If unavailable greeting exists (and requested was something else), use it
    /// 4. If name recording exists, use "greet" + default message
    /// 5. Fall back to system default (Unavailable -- the system ships a default)
    pub fn resolve_greeting(&self, requested: GreetingType) -> GreetingType {
        if self.has_temp {
            return GreetingType::Temp;
        }
        if self.has_greeting(requested) {
            return requested;
        }
        // If the requested greeting doesn't exist, try unavailable as fallback
        if requested != GreetingType::Unavailable && self.has_unavailable {
            return GreetingType::Unavailable;
        }
        if self.has_name {
            return GreetingType::Name;
        }
        // Fall back to system default unavailable greeting.
        // Even if has_unavailable is false, the system ships a built-in
        // "The person at extension X is unavailable" greeting.
        GreetingType::Unavailable
    }

    /// Get the file path for a greeting within a mailbox directory.
    pub fn greeting_path(mailbox_dir: &Path, gtype: GreetingType) -> PathBuf {
        mailbox_dir.join(gtype.file_name())
    }
}

// ---------------------------------------------------------------------------
// Recording Control
// ---------------------------------------------------------------------------

/// DTMF controls available during message recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingDtmfAction {
    /// '#' - End recording and save
    EndAndSave,
    /// '*' - Restart recording (discard and start over)
    Restart,
    /// '0' - Transfer to operator
    TransferToOperator,
}

impl RecordingDtmfAction {
    pub fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '#' => Some(Self::EndAndSave),
            '*' => Some(Self::Restart),
            '0' => Some(Self::TransferToOperator),
            _ => None,
        }
    }
}

/// Configuration for message recording.
#[derive(Debug, Clone)]
pub struct RecordingConfig {
    /// Maximum message length in seconds (0 = unlimited)
    pub max_message_secs: u32,
    /// Minimum message length in seconds. Messages shorter are deleted.
    pub min_message_secs: u32,
    /// Silence threshold in dB for silence detection
    pub silence_threshold: i32,
    /// Maximum silence duration before auto-stop recording (seconds). 0 = disabled.
    pub max_silence_secs: u32,
    /// Whether to skip the greeting playback
    pub skip_greeting: bool,
    /// Whether to allow review after recording
    pub review: bool,
    /// Whether to allow the caller to mark as urgent
    pub allow_mark_urgent: bool,
    /// Audio format(s) for recording
    pub format: String,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            max_message_secs: 300,
            min_message_secs: 3,
            silence_threshold: 128,
            max_silence_secs: 10,
            skip_greeting: false,
            review: false,
            allow_mark_urgent: false,
            format: "wav".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Playback Navigation
// ---------------------------------------------------------------------------

/// DTMF actions during message playback in VoiceMailMain.
///
/// Full navigation per Asterisk:
///   1 - Listen to old/saved messages (from main menu)
///   2 - Change folders
///   3 - Advanced options (reply, callback, envelope)
///   4 - Previous message
///   5 - Repeat current message
///   6 - Next message
///   7 - Delete current message
///   8 - Forward current message to another mailbox
///   9 - Save message to a folder
///   * - Help
///   # - Exit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAction {
    /// Listen to old/saved messages
    OldMessages,
    /// Change to a different folder
    ChangeFolders,
    /// Advanced options (reply, callback, envelope)
    AdvancedOptions,
    /// Go to previous message
    PreviousMessage,
    /// Repeat current message
    RepeatMessage,
    /// Go to next message
    NextMessage,
    /// Delete current message
    DeleteMessage,
    /// Forward message to another mailbox
    ForwardMessage,
    /// Save message to a specific folder
    SaveMessage,
    /// Help prompt
    Help,
    /// Exit voicemail
    Exit,
}

impl PlaybackAction {
    /// Map a DTMF digit to a playback action.
    pub fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '1' => Some(Self::OldMessages),
            '2' => Some(Self::ChangeFolders),
            '3' => Some(Self::AdvancedOptions),
            '4' => Some(Self::PreviousMessage),
            '5' => Some(Self::RepeatMessage),
            '6' => Some(Self::NextMessage),
            '7' => Some(Self::DeleteMessage),
            '8' => Some(Self::ForwardMessage),
            '9' => Some(Self::SaveMessage),
            '*' => Some(Self::Help),
            '#' => Some(Self::Exit),
            _ => None,
        }
    }
}

/// State tracker for message playback within VoiceMailMain.
#[derive(Debug)]
pub struct PlaybackState {
    /// Current folder being viewed
    pub current_folder: VoicemailFolder,
    /// Index of the current message within the folder
    pub current_message_index: usize,
    /// Total number of messages in current folder
    pub total_messages: usize,
    /// Whether the current message has been heard (auto-move to Old)
    pub heard: bool,
    /// Messages marked for deletion (msg_id)
    pub deleted_messages: Vec<String>,
    /// Messages marked for saving to another folder (msg_id -> target_folder)
    pub saved_messages: Vec<(String, VoicemailFolder)>,
}

impl PlaybackState {
    pub fn new(folder: VoicemailFolder, total: usize) -> Self {
        Self {
            current_folder: folder,
            current_message_index: 0,
            total_messages: total,
            heard: false,
            deleted_messages: Vec::new(),
            saved_messages: Vec::new(),
        }
    }

    /// Move to the next message. Returns false if at end.
    pub fn advance(&mut self) -> bool {
        if self.current_message_index + 1 < self.total_messages {
            self.current_message_index += 1;
            self.heard = false;
            true
        } else {
            false
        }
    }

    /// Move to the previous message. Returns false if at beginning.
    pub fn previous(&mut self) -> bool {
        if self.current_message_index > 0 {
            self.current_message_index -= 1;
            self.heard = false;
            true
        } else {
            false
        }
    }

    /// Mark current message as deleted.
    pub fn mark_deleted(&mut self, msg_id: &str) {
        if !self.deleted_messages.contains(&msg_id.to_string()) {
            self.deleted_messages.push(msg_id.to_string());
        }
    }

    /// Mark current message for saving to a folder.
    pub fn mark_saved(&mut self, msg_id: &str, folder: VoicemailFolder) {
        self.saved_messages.push((msg_id.to_string(), folder));
    }
}

// ---------------------------------------------------------------------------
// Email / Pager Notification
// ---------------------------------------------------------------------------

/// Email notification configuration for voicemail.
///
/// Template variables:
///   ${VM_NAME}     - Mailbox owner's name
///   ${VM_MAILBOX}  - Mailbox number
///   ${VM_CIDNAME}  - Caller ID name of the person who left the message
///   ${VM_CIDNUM}   - Caller ID number
///   ${VM_DATE}     - Date/time the message was left
///   ${VM_DURATION} - Duration of the message in seconds
///   ${VM_MSGNUM}   - Message number
#[derive(Debug, Clone)]
pub struct EmailNotification {
    /// Recipient email address
    pub to: String,
    /// Sender email address
    pub from: String,
    /// Email subject template
    pub subject: String,
    /// Email body template (supports template variables above)
    pub body_template: String,
    /// Attachment format (e.g., "wav", "mp3"). Empty = no attachment.
    pub attachment_format: String,
}

impl Default for EmailNotification {
    fn default() -> Self {
        Self {
            to: String::new(),
            from: "asterisk@localhost".to_string(),
            subject: "New voicemail from ${VM_CIDNAME} <${VM_CIDNUM}>".to_string(),
            body_template: concat!(
                "Dear ${VM_NAME},\n\n",
                "You have a new voicemail message from ${VM_CIDNAME} (${VM_CIDNUM})\n",
                "left on ${VM_DATE}.\n",
                "The message is ${VM_DURATION} seconds long.\n\n",
                "Message number: ${VM_MSGNUM}\n"
            )
            .to_string(),
            attachment_format: "wav".to_string(),
        }
    }
}

impl EmailNotification {
    /// Render the subject with template variable substitution.
    pub fn render_subject(&self, vars: &NotificationVars) -> String {
        self.substitute_vars(&self.subject, vars)
    }

    /// Render the body with template variable substitution.
    pub fn render_body(&self, vars: &NotificationVars) -> String {
        self.substitute_vars(&self.body_template, vars)
    }

    fn substitute_vars(&self, template: &str, vars: &NotificationVars) -> String {
        template
            .replace("${VM_NAME}", &vars.vm_name)
            .replace("${VM_MAILBOX}", &vars.vm_mailbox)
            .replace("${VM_CIDNAME}", &vars.vm_cidname)
            .replace("${VM_CIDNUM}", &vars.vm_cidnum)
            .replace("${VM_DATE}", &vars.vm_date)
            .replace("${VM_DURATION}", &vars.vm_duration.to_string())
            .replace("${VM_MSGNUM}", &vars.vm_msgnum.to_string())
    }

    /// Stub: send the email notification.
    /// In production, this would invoke sendmail or an SMTP client.
    pub fn send(&self, vars: &NotificationVars) -> bool {
        let subject = self.render_subject(vars);
        let body = self.render_body(vars);
        info!(
            "EmailNotification: would send to='{}' subject='{}'",
            self.to, subject
        );
        debug!("EmailNotification: body:\n{}", body);
        // Stub: always succeeds
        true
    }
}

/// Pager notification (shorter body, no attachment).
#[derive(Debug, Clone)]
pub struct PagerNotification {
    /// Pager email address
    pub to: String,
    /// Sender email address
    pub from: String,
    /// Subject template
    pub subject: String,
    /// Body template (short, suitable for pager)
    pub body_template: String,
}

impl Default for PagerNotification {
    fn default() -> Self {
        Self {
            to: String::new(),
            from: "asterisk@localhost".to_string(),
            subject: "VM: ${VM_CIDNUM}".to_string(),
            body_template: "New msg from ${VM_CIDNAME} (${VM_CIDNUM}), ${VM_DURATION}s".to_string(),
        }
    }
}

impl PagerNotification {
    /// Render the subject.
    pub fn render_subject(&self, vars: &NotificationVars) -> String {
        self.subject
            .replace("${VM_NAME}", &vars.vm_name)
            .replace("${VM_MAILBOX}", &vars.vm_mailbox)
            .replace("${VM_CIDNAME}", &vars.vm_cidname)
            .replace("${VM_CIDNUM}", &vars.vm_cidnum)
            .replace("${VM_DATE}", &vars.vm_date)
            .replace("${VM_DURATION}", &vars.vm_duration.to_string())
            .replace("${VM_MSGNUM}", &vars.vm_msgnum.to_string())
    }

    /// Render the body.
    pub fn render_body(&self, vars: &NotificationVars) -> String {
        self.body_template
            .replace("${VM_NAME}", &vars.vm_name)
            .replace("${VM_MAILBOX}", &vars.vm_mailbox)
            .replace("${VM_CIDNAME}", &vars.vm_cidname)
            .replace("${VM_CIDNUM}", &vars.vm_cidnum)
            .replace("${VM_DATE}", &vars.vm_date)
            .replace("${VM_DURATION}", &vars.vm_duration.to_string())
            .replace("${VM_MSGNUM}", &vars.vm_msgnum.to_string())
    }

    /// Stub: send the pager notification.
    pub fn send(&self, vars: &NotificationVars) -> bool {
        info!(
            "PagerNotification: would send to='{}' body='{}'",
            self.to,
            self.render_body(vars)
        );
        true
    }
}

/// Variables for notification template substitution.
#[derive(Debug, Clone)]
pub struct NotificationVars {
    pub vm_name: String,
    pub vm_mailbox: String,
    pub vm_cidname: String,
    pub vm_cidnum: String,
    pub vm_date: String,
    pub vm_duration: u32,
    pub vm_msgnum: u32,
}

// ---------------------------------------------------------------------------
// VoicemailStorage trait + implementations
// ---------------------------------------------------------------------------

/// Trait for voicemail message storage backends.
///
/// Three implementations:
/// - FileStorage (default): stores messages on the filesystem
/// - ImapStorage (stub): stores messages in an IMAP mailbox
/// - OdbcStorage (stub): stores messages in an ODBC database
pub trait VoicemailStorage: Send + Sync + std::fmt::Debug {
    /// Save a message to storage. Returns the storage path/key.
    fn save_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg: &VoiceMessage,
    ) -> Result<String, String>;

    /// Load a message from storage by ID.
    fn load_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<VoiceMessage, String>;

    /// Delete a message from storage.
    fn delete_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String>;

    /// Move a message between folders.
    fn move_msg(
        &self,
        mailbox: &str,
        context: &str,
        from: VoicemailFolder,
        to: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String>;

    /// Count messages in a folder.
    fn count_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<usize, String>;

    /// List all message IDs in a folder.
    fn list_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<Vec<String>, String>;
}

/// File-based voicemail storage (default).
///
/// Stores messages in /var/spool/asterisk/voicemail/<context>/<mailbox>/<folder>/
#[derive(Debug)]
pub struct FileStorage {
    /// Base spool directory
    pub spool_dir: PathBuf,
}

impl Default for FileStorage {
    fn default() -> Self {
        Self {
            spool_dir: PathBuf::from("/var/spool/asterisk/voicemail"),
        }
    }
}

impl FileStorage {
    fn folder_path(&self, mailbox: &str, context: &str, folder: VoicemailFolder) -> PathBuf {
        self.spool_dir
            .join(context)
            .join(mailbox)
            .join(folder.dir_name())
    }
}

impl VoicemailStorage for FileStorage {
    fn save_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg: &VoiceMessage,
    ) -> Result<String, String> {
        let dir = self.folder_path(mailbox, context, folder);
        let path = dir.join(format!("msg{}.wav", msg.msg_id));
        info!("FileStorage: would save message to {:?}", path);
        Ok(path.to_string_lossy().to_string())
    }

    fn load_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<VoiceMessage, String> {
        let dir = self.folder_path(mailbox, context, folder);
        let path = dir.join(format!("msg{}.wav", msg_id));
        info!("FileStorage: would load message from {:?}", path);
        // Stub: return a placeholder message
        Ok(VoiceMessage {
            msg_id: msg_id.to_string(),
            caller_id: String::new(),
            caller_number: String::new(),
            timestamp: SystemTime::now(),
            duration: 0,
            folder,
            file_path: path,
            urgent: false,
            orig_context: context.to_string(),
            called_extension: String::new(),
        })
    }

    fn delete_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        let dir = self.folder_path(mailbox, context, folder);
        let path = dir.join(format!("msg{}.wav", msg_id));
        info!("FileStorage: would delete {:?}", path);
        Ok(())
    }

    fn move_msg(
        &self,
        mailbox: &str,
        context: &str,
        from: VoicemailFolder,
        to: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        let from_dir = self.folder_path(mailbox, context, from);
        let to_dir = self.folder_path(mailbox, context, to);
        info!(
            "FileStorage: would move msg {} from {:?} to {:?}",
            msg_id, from_dir, to_dir
        );
        Ok(())
    }

    fn count_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<usize, String> {
        let dir = self.folder_path(mailbox, context, folder);
        info!("FileStorage: would count messages in {:?}", dir);
        Ok(0)
    }

    fn list_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<Vec<String>, String> {
        let dir = self.folder_path(mailbox, context, folder);
        info!("FileStorage: would list messages in {:?}", dir);
        Ok(Vec::new())
    }
}

/// IMAP-based voicemail storage (stub).
///
/// In Asterisk, IMAP storage saves voicemail messages as email messages
/// in an IMAP mailbox, using the c-client library.
#[derive(Debug)]
pub struct ImapStorage {
    pub server: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub folder: String,
    pub flags: String,
}

impl Default for ImapStorage {
    fn default() -> Self {
        Self {
            server: "localhost".to_string(),
            port: 143,
            user: String::new(),
            password: String::new(),
            folder: "INBOX".to_string(),
            flags: String::new(),
        }
    }
}

impl VoicemailStorage for ImapStorage {
    fn save_msg(
        &self,
        mailbox: &str,
        _context: &str,
        folder: VoicemailFolder,
        msg: &VoiceMessage,
    ) -> Result<String, String> {
        info!(
            "ImapStorage: stub - would save msg {} to {}:{}/{} for mailbox {}",
            msg.msg_id,
            self.server,
            self.port,
            folder.name(),
            mailbox
        );
        Ok(format!("imap://{}:{}/{}", self.server, self.port, msg.msg_id))
    }

    fn load_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<VoiceMessage, String> {
        info!(
            "ImapStorage: stub - would load msg {} from {}/{}",
            msg_id,
            folder.name(),
            mailbox
        );
        Ok(VoiceMessage {
            msg_id: msg_id.to_string(),
            caller_id: String::new(),
            caller_number: String::new(),
            timestamp: SystemTime::now(),
            duration: 0,
            folder,
            file_path: PathBuf::new(),
            urgent: false,
            orig_context: context.to_string(),
            called_extension: String::new(),
        })
    }

    fn delete_msg(
        &self,
        mailbox: &str,
        _context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        info!(
            "ImapStorage: stub - would delete msg {} from {}/{}",
            msg_id,
            folder.name(),
            mailbox
        );
        Ok(())
    }

    fn move_msg(
        &self,
        mailbox: &str,
        _context: &str,
        from: VoicemailFolder,
        to: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        info!(
            "ImapStorage: stub - would move msg {} from {} to {} for {}",
            msg_id,
            from.name(),
            to.name(),
            mailbox
        );
        Ok(())
    }

    fn count_msgs(
        &self,
        mailbox: &str,
        _context: &str,
        folder: VoicemailFolder,
    ) -> Result<usize, String> {
        info!(
            "ImapStorage: stub - would count msgs in {}/{}",
            folder.name(),
            mailbox
        );
        Ok(0)
    }

    fn list_msgs(
        &self,
        mailbox: &str,
        _context: &str,
        folder: VoicemailFolder,
    ) -> Result<Vec<String>, String> {
        info!(
            "ImapStorage: stub - would list msgs in {}/{}",
            folder.name(),
            mailbox
        );
        Ok(Vec::new())
    }
}

/// ODBC-based voicemail storage (stub).
///
/// In Asterisk, ODBC storage saves voicemail messages in a database table
/// using the res_odbc module. The recording data is stored as a BLOB.
#[derive(Debug)]
pub struct OdbcStorage {
    pub dsn: String,
    pub table: String,
}

impl Default for OdbcStorage {
    fn default() -> Self {
        Self {
            dsn: "asterisk".to_string(),
            table: "voicemessages".to_string(),
        }
    }
}

impl VoicemailStorage for OdbcStorage {
    fn save_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg: &VoiceMessage,
    ) -> Result<String, String> {
        info!(
            "OdbcStorage: stub - would INSERT msg {} into {}.{} for {}@{}/{}",
            msg.msg_id,
            self.dsn,
            self.table,
            mailbox,
            context,
            folder.name()
        );
        Ok(msg.msg_id.clone())
    }

    fn load_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<VoiceMessage, String> {
        info!(
            "OdbcStorage: stub - would SELECT msg {} from {}.{} for {}@{}/{}",
            msg_id,
            self.dsn,
            self.table,
            mailbox,
            context,
            folder.name()
        );
        Ok(VoiceMessage {
            msg_id: msg_id.to_string(),
            caller_id: String::new(),
            caller_number: String::new(),
            timestamp: SystemTime::now(),
            duration: 0,
            folder,
            file_path: PathBuf::new(),
            urgent: false,
            orig_context: context.to_string(),
            called_extension: String::new(),
        })
    }

    fn delete_msg(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        info!(
            "OdbcStorage: stub - would DELETE msg {} from {}.{} for {}@{}/{}",
            msg_id,
            self.dsn,
            self.table,
            mailbox,
            context,
            folder.name()
        );
        Ok(())
    }

    fn move_msg(
        &self,
        mailbox: &str,
        context: &str,
        from: VoicemailFolder,
        to: VoicemailFolder,
        msg_id: &str,
    ) -> Result<(), String> {
        info!(
            "OdbcStorage: stub - would UPDATE msg {} from {} to {} in {}.{} for {}@{}",
            msg_id,
            from.name(),
            to.name(),
            self.dsn,
            self.table,
            mailbox,
            context
        );
        Ok(())
    }

    fn count_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<usize, String> {
        info!(
            "OdbcStorage: stub - would COUNT msgs in {}.{} for {}@{}/{}",
            self.dsn,
            self.table,
            mailbox,
            context,
            folder.name()
        );
        Ok(0)
    }

    fn list_msgs(
        &self,
        mailbox: &str,
        context: &str,
        folder: VoicemailFolder,
    ) -> Result<Vec<String>, String> {
        info!(
            "OdbcStorage: stub - would SELECT msg_ids from {}.{} for {}@{}/{}",
            self.dsn,
            self.table,
            mailbox,
            context,
            folder.name()
        );
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// MWI (Message Waiting Indication)
// ---------------------------------------------------------------------------

/// MWI state for a mailbox.
#[derive(Debug, Clone)]
pub struct MwiState {
    pub mailbox: String,
    pub context: String,
    pub new_messages: usize,
    pub old_messages: usize,
}

/// Stub function to publish MWI state change.
/// In production, this would publish via Stasis and/or send SIP NOTIFY.
pub fn publish_mwi(state: &MwiState) {
    info!(
        "MWI: {}@{} new={} old={}",
        state.mailbox, state.context, state.new_messages, state.old_messages
    );
    // In production:
    // 1. Publish stasis message for MWI subscribers
    // 2. For SIP endpoints, generate a NOTIFY with message-summary body
    // 3. For SMDI, send MWI indicator
}

// ---------------------------------------------------------------------------
// voicemail.conf Parsing
// ---------------------------------------------------------------------------

/// A parsed mailbox definition from voicemail.conf.
///
/// Format: mailbox => password,name,email,pager,options
#[derive(Debug, Clone)]
pub struct MailboxConfig {
    pub mailbox_number: String,
    pub context: String,
    pub password: String,
    pub fullname: String,
    pub email: Option<String>,
    pub pager: Option<String>,
    pub options: HashMap<String, String>,
}

impl MailboxConfig {
    /// Parse a mailbox definition line.
    /// Format: "password,name,email,pager,opt1=val1|opt2=val2"
    pub fn parse(mailbox_number: &str, context: &str, value: &str) -> Self {
        let parts: Vec<&str> = value.splitn(5, ',').collect();
        let password = parts.first().map(|s| s.trim().to_string()).unwrap_or_default();
        let fullname = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
        let email = parts
            .get(2)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let pager = parts
            .get(3)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let options = if let Some(opts_str) = parts.get(4) {
            opts_str
                .split('|')
                .filter_map(|kv| {
                    let kv = kv.trim();
                    if let Some(eq_pos) = kv.find('=') {
                        Some((kv[..eq_pos].to_string(), kv[eq_pos + 1..].to_string()))
                    } else if !kv.is_empty() {
                        Some((kv.to_string(), "yes".to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            HashMap::new()
        };

        Self {
            mailbox_number: mailbox_number.to_string(),
            context: context.to_string(),
            password,
            fullname,
            email,
            pager,
            options,
        }
    }

    /// Convert this config into a Mailbox.
    pub fn to_mailbox(&self) -> Mailbox {
        let mut mailbox = Mailbox::new(
            self.mailbox_number.clone(),
            self.context.clone(),
            self.password.clone(),
            self.fullname.clone(),
        );
        mailbox.email = self.email.clone();
        mailbox.pager = self.pager.clone();

        // Apply options
        if let Some(tz) = self.options.get("tz") {
            mailbox.timezone = tz.clone();
        }
        if let Some(attach) = self.options.get("attach") {
            mailbox.attach_voicemail = attach == "yes" || attach == "true";
        }
        if let Some(max_msg) = self.options.get("maxmsg") {
            if let Ok(n) = max_msg.parse() {
                mailbox.max_messages = n;
            }
        }
        if let Some(max_secs) = self.options.get("maxsecs") {
            if let Ok(n) = max_secs.parse() {
                mailbox.recording_config.max_message_secs = n;
            }
        }
        if let Some(min_secs) = self.options.get("minsecs") {
            if let Ok(n) = min_secs.parse() {
                mailbox.recording_config.min_message_secs = n;
            }
        }

        mailbox
    }
}

/// Parse a voicemail.conf style config block.
/// Returns a list of MailboxConfig entries.
pub fn parse_voicemail_conf(config_text: &str) -> Vec<MailboxConfig> {
    let mut results = Vec::new();
    let mut current_context = "default".to_string();

    for line in config_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        // Context header
        if line.starts_with('[') && line.ends_with(']') {
            let ctx = &line[1..line.len() - 1];
            if ctx != "general" && ctx != "zonemessages" {
                current_context = ctx.to_string();
            }
            continue;
        }
        // Mailbox definition: number => value
        if let Some(arrow_pos) = line.find("=>") {
            let mailbox_num = line[..arrow_pos].trim();
            let value = line[arrow_pos + 2..].trim();
            if !mailbox_num.is_empty() {
                results.push(MailboxConfig::parse(mailbox_num, &current_context, value));
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Mailbox
// ---------------------------------------------------------------------------

/// A voicemail mailbox.
#[derive(Debug)]
pub struct Mailbox {
    /// Mailbox number (e.g., "100")
    pub mailbox_number: String,
    /// Voicemail context (e.g., "default")
    pub context: String,
    /// Password for access
    pub password: String,
    /// Full name of the mailbox owner
    pub fullname: String,
    /// Email address for notifications
    pub email: Option<String>,
    /// Pager email for urgent notifications
    pub pager: Option<String>,
    /// Per-mailbox greeting state
    pub greetings: GreetingState,
    /// Maximum number of messages per folder
    pub max_messages: u32,
    /// Messages organized by folder
    pub folders: HashMap<VoicemailFolder, Vec<VoiceMessage>>,
    /// Whether to attach voicemail to email
    pub attach_voicemail: bool,
    /// Time zone for date/time announcements
    pub timezone: String,
    /// Recording configuration
    pub recording_config: RecordingConfig,
    /// Email notification settings
    pub email_notification: EmailNotification,
    /// Pager notification settings
    pub pager_notification: PagerNotification,
    /// Whether to delete messages after emailing them
    pub delete_after_email: bool,
    /// Whether to say CallerID during envelope playback
    pub say_caller_id: bool,
    /// Whether to play envelope (who/when) info
    pub envelope: bool,
    /// Whether to say message duration
    pub say_duration: bool,
    /// Minimum duration to say (seconds)
    pub say_duration_min: u32,
    /// Whether to allow message review before saving
    pub review: bool,
    /// Whether to allow operator transfer (press 0)
    pub operator: bool,
    /// Language for prompts
    pub language: String,
    /// Number of login attempts before lockout
    pub max_login_attempts: u32,
}

impl Mailbox {
    /// Create a new mailbox with default settings.
    pub fn new(
        mailbox_number: String,
        context: String,
        password: String,
        fullname: String,
    ) -> Self {
        let mut folders = HashMap::new();
        for folder in VoicemailFolder::ALL.iter() {
            folders.insert(*folder, Vec::new());
        }

        Self {
            mailbox_number,
            context,
            password,
            fullname,
            email: None,
            pager: None,
            greetings: GreetingState::default(),
            max_messages: 100,
            folders,
            attach_voicemail: false,
            timezone: "America/New_York".to_string(),
            recording_config: RecordingConfig::default(),
            email_notification: EmailNotification::default(),
            pager_notification: PagerNotification::default(),
            delete_after_email: false,
            say_caller_id: true,
            envelope: true,
            say_duration: true,
            say_duration_min: 2,
            review: false,
            operator: false,
            language: "en".to_string(),
            max_login_attempts: 3,
        }
    }

    /// Full mailbox identifier ("number@context").
    pub fn full_id(&self) -> String {
        format!("{}@{}", self.mailbox_number, self.context)
    }

    /// Count messages in a specific folder.
    pub fn message_count(&self, folder: VoicemailFolder) -> usize {
        self.folders.get(&folder).map(|v| v.len()).unwrap_or(0)
    }

    /// Count new (INBOX) messages.
    pub fn new_message_count(&self) -> usize {
        self.message_count(VoicemailFolder::Inbox)
    }

    /// Count old (read) messages.
    pub fn old_message_count(&self) -> usize {
        self.message_count(VoicemailFolder::Old)
    }

    /// Total messages across all folders.
    pub fn total_message_count(&self) -> usize {
        self.folders.values().map(|v| v.len()).sum()
    }

    /// Add a message to a folder.
    pub fn add_message(&mut self, folder: VoicemailFolder, msg: VoiceMessage) -> bool {
        let messages = self.folders.entry(folder).or_default();
        if messages.len() >= self.max_messages as usize {
            warn!(
                "Mailbox {}: folder {:?} is full ({} messages)",
                self.full_id(),
                folder,
                self.max_messages
            );
            return false;
        }

        // Check minimum message length
        if msg.duration < self.recording_config.min_message_secs {
            info!(
                "Mailbox {}: message too short ({}s < {}s minimum), discarding",
                self.full_id(),
                msg.duration,
                self.recording_config.min_message_secs
            );
            return false;
        }

        messages.push(msg);
        true
    }

    /// Add a message and send notifications + MWI update.
    pub fn add_message_with_notify(&mut self, folder: VoicemailFolder, msg: VoiceMessage) -> bool {
        let duration = msg.duration;
        let caller_id = msg.caller_id.clone();
        let caller_num = msg.caller_number.clone();
        let msg_count = self.message_count(folder) as u32;

        if !self.add_message(folder, msg) {
            return false;
        }

        // Send email notification
        if let Some(email) = &self.email {
            let vars = NotificationVars {
                vm_name: self.fullname.clone(),
                vm_mailbox: self.mailbox_number.clone(),
                vm_cidname: caller_id.clone(),
                vm_cidnum: caller_num.clone(),
                vm_date: format!("{:?}", SystemTime::now()),
                vm_duration: duration,
                vm_msgnum: msg_count + 1,
            };
            let mut notif = self.email_notification.clone();
            notif.to = email.clone();
            notif.send(&vars);
        }

        // Send pager notification
        if let Some(pager) = &self.pager {
            let vars = NotificationVars {
                vm_name: self.fullname.clone(),
                vm_mailbox: self.mailbox_number.clone(),
                vm_cidname: caller_id,
                vm_cidnum: caller_num,
                vm_date: format!("{:?}", SystemTime::now()),
                vm_duration: duration,
                vm_msgnum: msg_count + 1,
            };
            let mut notif = self.pager_notification.clone();
            notif.to = pager.clone();
            notif.send(&vars);
        }

        // Update MWI
        self.update_mwi();

        true
    }

    /// Move a message between folders.
    pub fn move_message(
        &mut self,
        msg_id: &str,
        from: VoicemailFolder,
        to: VoicemailFolder,
    ) -> bool {
        if let Some(from_folder) = self.folders.get_mut(&from) {
            if let Some(pos) = from_folder.iter().position(|m| m.msg_id == msg_id) {
                let mut msg = from_folder.remove(pos);
                msg.folder = to;
                let to_folder = self.folders.entry(to).or_default();
                to_folder.push(msg);
                self.update_mwi();
                return true;
            }
        }
        false
    }

    /// Delete a message from a folder.
    pub fn delete_message(&mut self, msg_id: &str, folder: VoicemailFolder) -> bool {
        if let Some(folder_msgs) = self.folders.get_mut(&folder) {
            let before = folder_msgs.len();
            folder_msgs.retain(|m| m.msg_id != msg_id);
            let removed = folder_msgs.len() < before;
            if removed {
                self.update_mwi();
            }
            return removed;
        }
        false
    }

    /// Forward a message to another mailbox.
    pub fn forward_message(
        &self,
        msg_id: &str,
        folder: VoicemailFolder,
        target_mailbox: &str,
        target_context: &str,
    ) -> bool {
        let msg = self
            .folders
            .get(&folder)
            .and_then(|msgs| msgs.iter().find(|m| m.msg_id == msg_id))
            .cloned();

        if let Some(msg) = msg {
            let target_key = format!("{}@{}", target_mailbox, target_context);
            if let Some(target) = MAILBOXES.get(&target_key) {
                let mut target = target.write();
                let mut forwarded = msg;
                forwarded.msg_id = Uuid::new_v4().to_string();
                forwarded.folder = VoicemailFolder::Inbox;
                return target.add_message_with_notify(VoicemailFolder::Inbox, forwarded);
            }
        }
        false
    }

    /// Get messages in a specific folder.
    pub fn get_messages(&self, folder: VoicemailFolder) -> &[VoiceMessage] {
        self.folders
            .get(&folder)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Verify the password.
    ///
    /// If the mailbox has no password set (empty string), authentication
    /// always fails to prevent unauthorized access. Callers who want to
    /// allow password-less access should check `self.password.is_empty()`
    /// explicitly and skip authentication.
    pub fn verify_password(&self, password: &str) -> bool {
        if self.password.is_empty() {
            // Empty password on the mailbox means "no password set".
            // Reject all attempts to prevent unauthorized access.
            return false;
        }
        self.password == password
    }

    /// Change the password.
    pub fn change_password(&mut self, new_password: &str) {
        self.password = new_password.to_string();
        info!(
            "Mailbox {}: password changed",
            self.full_id()
        );
    }

    /// Get the base directory for this mailbox's files.
    pub fn base_dir(&self) -> PathBuf {
        PathBuf::from(format!(
            "/var/spool/asterisk/voicemail/{}/{}",
            self.context, self.mailbox_number
        ))
    }

    /// Update MWI state based on current message counts.
    pub fn update_mwi(&self) {
        let mwi = MwiState {
            mailbox: self.mailbox_number.clone(),
            context: self.context.clone(),
            new_messages: self.new_message_count(),
            old_messages: self.old_message_count(),
        };
        publish_mwi(&mwi);
    }

    /// Determine the greeting file to play for a caller.
    pub fn resolve_greeting_path(&self, requested: GreetingType) -> PathBuf {
        let actual = self.greetings.resolve_greeting(requested);
        GreetingState::greeting_path(&self.base_dir(), actual)
    }
}

// ---------------------------------------------------------------------------
// VoiceMessage
// ---------------------------------------------------------------------------

/// A voice message in a mailbox.
#[derive(Debug, Clone)]
pub struct VoiceMessage {
    /// Unique message ID
    pub msg_id: String,
    /// CallerID of the caller who left the message
    pub caller_id: String,
    /// CallerID number
    pub caller_number: String,
    /// Timestamp when the message was recorded
    pub timestamp: SystemTime,
    /// Duration of the message in seconds
    pub duration: u32,
    /// Which folder this message is in
    pub folder: VoicemailFolder,
    /// Path to the audio file
    pub file_path: PathBuf,
    /// Whether the message has been marked as urgent
    pub urgent: bool,
    /// The originating channel's context
    pub orig_context: String,
    /// The extension that was called
    pub called_extension: String,
}

impl VoiceMessage {
    /// Create a new voice message.
    pub fn new(
        caller_id: String,
        caller_number: String,
        duration: u32,
        file_path: PathBuf,
    ) -> Self {
        Self {
            msg_id: Uuid::new_v4().to_string(),
            caller_id,
            caller_number,
            timestamp: SystemTime::now(),
            duration,
            folder: VoicemailFolder::Inbox,
            file_path,
            urgent: false,
            orig_context: "default".to_string(),
            called_extension: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// VoicemailStatus
// ---------------------------------------------------------------------------

/// Status of the VoiceMail application execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicemailStatus {
    /// Successfully recorded a message
    Success,
    /// User exited (pressed * or 0)
    UserExit,
    /// An error occurred
    Failed,
}

impl VoicemailStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::UserExit => "USEREXIT",
            Self::Failed => "FAILED",
        }
    }
}

// ---------------------------------------------------------------------------
// VoicemailOptions
// ---------------------------------------------------------------------------

/// Options for VoiceMail().
#[derive(Debug, Clone, Default)]
pub struct VoicemailOptions {
    /// Play the busy greeting
    pub busy_greeting: bool,
    /// Play the unavailable greeting
    pub unavailable_greeting: bool,
    /// Skip instructions
    pub skip_instructions: bool,
    /// Mark message as urgent
    pub urgent: bool,
    /// Mark message as priority
    pub priority: bool,
    /// Suppress beep
    pub no_beep: bool,
    /// Early media greeting
    pub early_media: bool,
    /// Skip instructions only if a greeting was recorded
    pub skip_if_greeting: bool,
}

impl VoicemailOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'b' => result.busy_greeting = true,
                'u' => result.unavailable_greeting = true,
                's' => result.skip_instructions = true,
                'S' => result.skip_if_greeting = true,
                'U' => result.urgent = true,
                'P' => result.priority = true,
                'e' => result.early_media = true,
                _ => {}
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// AppVoiceMail
// ---------------------------------------------------------------------------

/// The VoiceMail() dialplan application - leave a voicemail message.
///
/// Usage: VoiceMail(mailbox[@context][&mailbox2[@context2]...][,options])
///
/// Records a voicemail message for the specified mailbox(es).
pub struct AppVoiceMail;

impl DialplanApp for AppVoiceMail {
    fn name(&self) -> &str {
        "VoiceMail"
    }

    fn description(&self) -> &str {
        "Leave a Voicemail message"
    }
}

impl AppVoiceMail {
    /// Execute the VoiceMail application (leave a message).
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, VoicemailStatus) {
        let (mailbox_specs, options) = Self::parse_args(args);

        if mailbox_specs.is_empty() {
            warn!("VoiceMail: no mailbox specified");
            channel
                .variables
                .insert("VMSTATUS".to_string(), "FAILED".to_string());
            return (PbxExecResult::Failed, VoicemailStatus::Failed);
        }

        // Parse all mailbox targets
        let targets: Vec<(String, String)> = mailbox_specs
            .split('&')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                if let Some(at_pos) = s.find('@') {
                    Some((s[..at_pos].to_string(), s[at_pos + 1..].to_string()))
                } else {
                    Some((s.to_string(), "default".to_string()))
                }
            })
            .collect();

        if targets.is_empty() {
            channel
                .variables
                .insert("VMSTATUS".to_string(), "FAILED".to_string());
            return (PbxExecResult::Failed, VoicemailStatus::Failed);
        }

        // Use the first mailbox for the greeting
        let (primary_box, primary_ctx) = &targets[0];
        let mailbox_key = format!("{}@{}", primary_box, primary_ctx);

        info!(
            "VoiceMail: channel '{}' leaving message for mailbox '{}'",
            channel.name, mailbox_key
        );

        // Get or create the mailbox
        let mailbox = Self::get_or_create_mailbox(primary_box, primary_ctx);

        // Answer the channel if not already answered (unless early_media)
        if !options.early_media && channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // Resolve and play greeting
        {
            let mbox = mailbox.read();
            let requested_greeting = if options.busy_greeting {
                GreetingType::Busy
            } else {
                GreetingType::Unavailable
            };
            let actual_greeting = mbox.greetings.resolve_greeting(requested_greeting);
            let greeting_path = mbox.resolve_greeting_path(requested_greeting);
            info!(
                "VoiceMail: playing {} greeting for mailbox '{}' (path: {:?})",
                actual_greeting.as_str(),
                mbox.full_id(),
                greeting_path
            );
            // In production: play the greeting file, check for DTMF exit
        }

        // Play instructions (unless 's' option or 'S' option with existing greeting)
        let skip_instructions = options.skip_instructions
            || (options.skip_if_greeting && {
                let mbox = mailbox.read();
                mbox.greetings.has_unavailable || mbox.greetings.has_busy
            });
        if !skip_instructions {
            debug!("VoiceMail: playing recording instructions");
            // In production: play "Leave your message after the tone..."
        }

        // Play beep (unless suppressed)
        if !options.no_beep {
            debug!("VoiceMail: playing beep");
            // In production: play_file(channel, "beep").await;
        }

        // Answer the channel if early media was requested
        if options.early_media && channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // Record the message
        let recording_config = {
            let mbox = mailbox.read();
            mbox.recording_config.clone()
        };
        let max_duration = Duration::from_secs(recording_config.max_message_secs as u64);

        let recording_path = {
            let mbox = mailbox.read();
            let mut path = mbox.base_dir();
            path.push(VoicemailFolder::Inbox.dir_name());
            path.push(format!("msg{:04}.{}", mbox.new_message_count(), recording_config.format));
            path
        };

        info!(
            "VoiceMail: recording message to {:?} (max {:?}, silence_detect: {}s, min_length: {}s)",
            recording_path,
            max_duration,
            recording_config.max_silence_secs,
            recording_config.min_message_secs
        );

        // Simulate recording
        // In production, this would:
        // 1. Start recording audio from the channel
        // 2. Monitor for silence (using silence_threshold and max_silence_secs)
        // 3. Monitor for DTMF: '#' = end, '*' = restart
        // 4. Stop recording when max_duration reached, silence detected, or DTMF received
        let duration = 10u32; // simulated duration

        // Create the voice message
        let caller_name = channel.caller.id.name.name.clone();
        let caller_number = channel.caller.id.number.number.clone();

        let mut message = VoiceMessage::new(caller_name, caller_number, duration, recording_path);
        message.urgent = options.urgent;
        message.orig_context = channel.context.clone();
        message.called_extension = channel.exten.clone();

        // Save message to all target mailboxes
        for (box_num, ctx) in &targets {
            let mbox = Self::get_or_create_mailbox(box_num, ctx);
            let mut mbox = mbox.write();
            if mbox.add_message_with_notify(VoicemailFolder::Inbox, message.clone()) {
                info!(
                    "VoiceMail: saved message to mailbox '{}@{}' ({}s)",
                    box_num, ctx, duration
                );
            } else {
                warn!("VoiceMail: mailbox '{}@{}' is full or message too short", box_num, ctx);
            }
        }

        // Set VMSTATUS channel variable
        channel
            .variables
            .insert("VMSTATUS".to_string(), "SUCCESS".to_string());

        (PbxExecResult::Success, VoicemailStatus::Success)
    }

    /// Execute the VoiceMailMain application (check voicemail).
    ///
    /// Usage: VoiceMailMain([mailbox][@context][,options])
    pub async fn exec_check(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let mailbox_spec = parts.first().map(|s| s.trim()).unwrap_or("");
        let options_str = parts.get(1).map(|s| s.trim()).unwrap_or("");

        let skip_password = options_str.contains('s');

        let (box_num, context) = if let Some(at_pos) = mailbox_spec.find('@') {
            (&mailbox_spec[..at_pos], &mailbox_spec[at_pos + 1..])
        } else {
            (mailbox_spec, "default")
        };

        info!(
            "VoiceMailMain: channel '{}' checking mailbox '{}@{}'",
            channel.name, box_num, context
        );

        // Answer the channel
        if channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        let mailbox_key = format!("{}@{}", box_num, context);
        let mailbox = match MAILBOXES.get(&mailbox_key) {
            Some(mbox) => mbox.value().clone(),
            None => {
                warn!("VoiceMailMain: mailbox '{}' not found", mailbox_key);
                return PbxExecResult::Failed;
            }
        };

        // Authenticate (prompt for password)
        if !skip_password {
            let mbox = mailbox.read();
            // In production: prompt user for password via DTMF, with max_login_attempts
            info!(
                "VoiceMailMain: would authenticate mailbox '{}' (max {} attempts)",
                mbox.full_id(),
                mbox.max_login_attempts
            );
        }

        {
            let mbox = mailbox.read();
            info!(
                "VoiceMailMain: mailbox '{}' has {} new and {} old messages",
                mbox.full_id(),
                mbox.new_message_count(),
                mbox.old_message_count()
            );
        }

        // Main voicemail menu loop
        // In production, this would be an interactive DTMF menu:
        //   1 - Listen to new messages (navigating with PlaybackAction keys)
        //   2 - Change folders
        //   3 - Advanced options (reply, callback, envelope)
        //   0 - Mailbox options (record greetings, change password, name recording)
        //   * - Help
        //   # - Exit

        // Simulate listening to messages
        {
            let mbox = mailbox.read();
            let new_msgs = mbox.get_messages(VoicemailFolder::Inbox);
            for (i, msg) in new_msgs.iter().enumerate() {
                info!(
                    "VoiceMailMain: message {} from '{}' <{}> ({}s, urgent: {})",
                    i + 1,
                    msg.caller_id,
                    msg.caller_number,
                    msg.duration,
                    msg.urgent
                );
                // In production: play envelope info, then play message audio
                // Then accept DTMF for navigation
            }
        }

        // Move listened messages from INBOX to Old (simulating "heard" behavior)
        {
            let mut mbox = mailbox.write();
            let inbox_msg_ids: Vec<String> = mbox
                .get_messages(VoicemailFolder::Inbox)
                .iter()
                .map(|m| m.msg_id.clone())
                .collect();
            for msg_id in inbox_msg_ids {
                mbox.move_message(&msg_id, VoicemailFolder::Inbox, VoicemailFolder::Old);
            }
        }

        PbxExecResult::Success
    }

    /// Parse VoiceMail() arguments.
    fn parse_args(args: &str) -> (&str, VoicemailOptions) {
        if let Some(comma_pos) = args.rfind(',') {
            let potential_opts = &args[comma_pos + 1..];
            let trimmed = potential_opts.trim();
            if !trimmed.is_empty()
                && trimmed.len() < 10
                && trimmed.chars().all(|c| c.is_alphabetic())
            {
                return (
                    &args[..comma_pos],
                    VoicemailOptions::parse(trimmed),
                );
            }
        }
        (args, VoicemailOptions::default())
    }

    /// Get an existing mailbox or create one with defaults.
    fn get_or_create_mailbox(box_num: &str, context: &str) -> Arc<RwLock<Mailbox>> {
        let key = format!("{}@{}", box_num, context);
        if let Some(mbox) = MAILBOXES.get(&key) {
            return mbox.value().clone();
        }

        let mailbox = Mailbox::new(
            box_num.to_string(),
            context.to_string(),
            "1234".to_string(),
            format!("Mailbox {}", box_num),
        );
        let mbox = Arc::new(RwLock::new(mailbox));
        MAILBOXES.insert(key.clone(), mbox.clone());
        info!("VoiceMail: created mailbox '{}'", key);
        mbox
    }

    /// Register a mailbox in the global registry.
    pub fn register_mailbox(
        box_num: &str,
        context: &str,
        password: &str,
        fullname: &str,
        email: Option<&str>,
    ) -> Arc<RwLock<Mailbox>> {
        let key = format!("{}@{}", box_num, context);
        let mut mailbox = Mailbox::new(
            box_num.to_string(),
            context.to_string(),
            password.to_string(),
            fullname.to_string(),
        );
        mailbox.email = email.map(|s| s.to_string());
        let mbox = Arc::new(RwLock::new(mailbox));
        MAILBOXES.insert(key, mbox.clone());
        mbox
    }

    /// Register a mailbox from a MailboxConfig.
    pub fn register_mailbox_from_config(config: &MailboxConfig) -> Arc<RwLock<Mailbox>> {
        let key = format!("{}@{}", config.mailbox_number, config.context);
        let mailbox = config.to_mailbox();
        let mbox = Arc::new(RwLock::new(mailbox));
        MAILBOXES.insert(key, mbox.clone());
        mbox
    }

    /// Check message waiting indication (MWI) for a mailbox.
    pub fn check_mwi(box_num: &str, context: &str) -> (usize, usize) {
        let key = format!("{}@{}", box_num, context);
        if let Some(mbox) = MAILBOXES.get(&key) {
            let mbox = mbox.read();
            (mbox.new_message_count(), mbox.old_message_count())
        } else {
            (0, 0)
        }
    }

    /// Load mailbox configurations from a voicemail.conf style string.
    pub fn load_config(config_text: &str) -> usize {
        let configs = parse_voicemail_conf(config_text);
        let count = configs.len();
        for config in &configs {
            Self::register_mailbox_from_config(config);
            info!(
                "VoiceMail: loaded mailbox '{}@{}' ({})",
                config.mailbox_number, config.context, config.fullname
            );
        }
        count
    }
}

// Simple once_cell implementation
mod once_cell {
    pub mod sync {
        pub struct Lazy<T> {
            inner: std::sync::OnceLock<T>,
            init: fn() -> T,
        }

        impl<T> Lazy<T> {
            pub const fn new(init: fn() -> T) -> Self {
                Self {
                    inner: std::sync::OnceLock::new(),
                    init,
                }
            }
        }

        impl<T> std::ops::Deref for Lazy<T> {
            type Target = T;

            fn deref(&self) -> &T {
                self.inner.get_or_init(self.init)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_creation() {
        let mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );
        assert_eq!(mbox.full_id(), "100@default");
        assert_eq!(mbox.new_message_count(), 0);
        assert_eq!(mbox.total_message_count(), 0);
        assert_eq!(mbox.language, "en");
        assert_eq!(mbox.max_login_attempts, 3);
    }

    #[test]
    fn test_add_message() {
        let mut mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );

        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            30,
            PathBuf::from("/tmp/msg.wav"),
        );

        assert!(mbox.add_message(VoicemailFolder::Inbox, msg));
        assert_eq!(mbox.new_message_count(), 1);
    }

    #[test]
    fn test_min_message_length() {
        let mut mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );
        mbox.recording_config.min_message_secs = 5;

        // Too short - should be rejected
        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            2, // Only 2 seconds
            PathBuf::from("/tmp/msg.wav"),
        );
        assert!(!mbox.add_message(VoicemailFolder::Inbox, msg));
        assert_eq!(mbox.new_message_count(), 0);

        // Long enough
        let msg2 = VoiceMessage::new(
            "Bob".to_string(),
            "5555678".to_string(),
            10,
            PathBuf::from("/tmp/msg2.wav"),
        );
        assert!(mbox.add_message(VoicemailFolder::Inbox, msg2));
        assert_eq!(mbox.new_message_count(), 1);
    }

    #[test]
    fn test_move_message() {
        let mut mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );

        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            30,
            PathBuf::from("/tmp/msg.wav"),
        );
        let msg_id = msg.msg_id.clone();
        mbox.add_message(VoicemailFolder::Inbox, msg);

        assert!(mbox.move_message(&msg_id, VoicemailFolder::Inbox, VoicemailFolder::Old));
        assert_eq!(mbox.new_message_count(), 0);
        assert_eq!(mbox.old_message_count(), 1);
    }

    #[test]
    fn test_delete_message() {
        let mut mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );

        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            30,
            PathBuf::from("/tmp/msg.wav"),
        );
        let msg_id = msg.msg_id.clone();
        mbox.add_message(VoicemailFolder::Inbox, msg);

        assert!(mbox.delete_message(&msg_id, VoicemailFolder::Inbox));
        assert_eq!(mbox.new_message_count(), 0);
    }

    #[test]
    fn test_verify_password() {
        let mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "5678".to_string(),
            "Test User".to_string(),
        );
        assert!(mbox.verify_password("5678"));
        assert!(!mbox.verify_password("1234"));
    }

    #[test]
    fn test_change_password() {
        let mut mbox = Mailbox::new(
            "100".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test User".to_string(),
        );
        mbox.change_password("9999");
        assert!(mbox.verify_password("9999"));
        assert!(!mbox.verify_password("1234"));
    }

    #[test]
    fn test_folder_names() {
        assert_eq!(VoicemailFolder::Inbox.name(), "INBOX");
        assert_eq!(VoicemailFolder::Old.name(), "Old");
        assert_eq!(
            VoicemailFolder::from_number(0),
            Some(VoicemailFolder::Inbox)
        );
        assert_eq!(VoicemailFolder::from_number(5), None);
    }

    #[test]
    fn test_voicemail_status() {
        assert_eq!(VoicemailStatus::Success.as_str(), "SUCCESS");
        assert_eq!(VoicemailStatus::UserExit.as_str(), "USEREXIT");
        assert_eq!(VoicemailStatus::Failed.as_str(), "FAILED");
    }

    // --- Greeting tests ---

    #[test]
    fn test_greeting_resolve_temp_override() {
        let mut gs = GreetingState::default();
        gs.has_temp = true;
        gs.has_unavailable = true;

        // Temp always wins
        assert_eq!(gs.resolve_greeting(GreetingType::Unavailable), GreetingType::Temp);
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Temp);
    }

    #[test]
    fn test_greeting_resolve_fallback() {
        let gs = GreetingState::default();
        // No greetings recorded -> falls back to system default Unavailable
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Unavailable);
    }

    #[test]
    fn test_greeting_resolve_busy_falls_to_unavail() {
        let mut gs = GreetingState::default();
        gs.has_unavailable = true;
        // Busy not recorded, but unavailable is -> falls back to unavailable
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Unavailable);
    }

    #[test]
    fn test_greeting_resolve_name_fallback() {
        let mut gs = GreetingState::default();
        gs.has_name = true;

        // No busy greeting -> falls back to name
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Name);
    }

    #[test]
    fn test_greeting_resolve_specific() {
        let mut gs = GreetingState::default();
        gs.has_busy = true;
        gs.has_unavailable = true;

        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Busy);
        assert_eq!(
            gs.resolve_greeting(GreetingType::Unavailable),
            GreetingType::Unavailable
        );
    }

    // --- Recording DTMF control tests ---

    #[test]
    fn test_recording_dtmf_actions() {
        assert_eq!(
            RecordingDtmfAction::from_digit('#'),
            Some(RecordingDtmfAction::EndAndSave)
        );
        assert_eq!(
            RecordingDtmfAction::from_digit('*'),
            Some(RecordingDtmfAction::Restart)
        );
        assert_eq!(
            RecordingDtmfAction::from_digit('0'),
            Some(RecordingDtmfAction::TransferToOperator)
        );
        assert_eq!(RecordingDtmfAction::from_digit('5'), None);
    }

    // --- Playback navigation tests ---

    #[test]
    fn test_playback_actions() {
        assert_eq!(
            PlaybackAction::from_digit('1'),
            Some(PlaybackAction::OldMessages)
        );
        assert_eq!(
            PlaybackAction::from_digit('4'),
            Some(PlaybackAction::PreviousMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('5'),
            Some(PlaybackAction::RepeatMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('6'),
            Some(PlaybackAction::NextMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('7'),
            Some(PlaybackAction::DeleteMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('8'),
            Some(PlaybackAction::ForwardMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('9'),
            Some(PlaybackAction::SaveMessage)
        );
        assert_eq!(
            PlaybackAction::from_digit('*'),
            Some(PlaybackAction::Help)
        );
        assert_eq!(
            PlaybackAction::from_digit('#'),
            Some(PlaybackAction::Exit)
        );
    }

    #[test]
    fn test_playback_state_navigation() {
        let mut state = PlaybackState::new(VoicemailFolder::Inbox, 3);
        assert_eq!(state.current_message_index, 0);

        assert!(state.advance());
        assert_eq!(state.current_message_index, 1);

        assert!(state.advance());
        assert_eq!(state.current_message_index, 2);

        // At end, can't go further
        assert!(!state.advance());
        assert_eq!(state.current_message_index, 2);

        // Go back
        assert!(state.previous());
        assert_eq!(state.current_message_index, 1);

        assert!(state.previous());
        assert_eq!(state.current_message_index, 0);

        // At beginning, can't go back
        assert!(!state.previous());
    }

    #[test]
    fn test_playback_state_delete_save() {
        let mut state = PlaybackState::new(VoicemailFolder::Inbox, 3);

        state.mark_deleted("msg001");
        assert_eq!(state.deleted_messages.len(), 1);

        // Duplicate delete is ok
        state.mark_deleted("msg001");
        assert_eq!(state.deleted_messages.len(), 1);

        state.mark_saved("msg002", VoicemailFolder::Work);
        assert_eq!(state.saved_messages.len(), 1);
        assert_eq!(state.saved_messages[0].1, VoicemailFolder::Work);
    }

    // --- Email notification tests ---

    #[test]
    fn test_email_notification_render() {
        let notif = EmailNotification::default();
        let vars = NotificationVars {
            vm_name: "John Smith".to_string(),
            vm_mailbox: "100".to_string(),
            vm_cidname: "Jane Doe".to_string(),
            vm_cidnum: "5551234".to_string(),
            vm_date: "2024-01-01 12:00".to_string(),
            vm_duration: 30,
            vm_msgnum: 1,
        };

        let subject = notif.render_subject(&vars);
        assert!(subject.contains("Jane Doe"));
        assert!(subject.contains("5551234"));

        let body = notif.render_body(&vars);
        assert!(body.contains("John Smith"));
        assert!(body.contains("Jane Doe"));
        assert!(body.contains("5551234"));
        assert!(body.contains("30"));
    }

    #[test]
    fn test_pager_notification_render() {
        let notif = PagerNotification::default();
        let vars = NotificationVars {
            vm_name: "John".to_string(),
            vm_mailbox: "100".to_string(),
            vm_cidname: "Jane".to_string(),
            vm_cidnum: "5551234".to_string(),
            vm_date: "now".to_string(),
            vm_duration: 15,
            vm_msgnum: 2,
        };

        let body = notif.render_body(&vars);
        assert!(body.contains("Jane"));
        assert!(body.contains("5551234"));
        assert!(body.contains("15"));
    }

    // --- Storage backend tests ---

    #[test]
    fn test_file_storage() {
        let storage = FileStorage::default();
        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            10,
            PathBuf::from("/tmp/test.wav"),
        );

        assert!(storage
            .save_msg("100", "default", VoicemailFolder::Inbox, &msg)
            .is_ok());
        assert!(storage
            .load_msg("100", "default", VoicemailFolder::Inbox, &msg.msg_id)
            .is_ok());
        assert!(storage
            .delete_msg("100", "default", VoicemailFolder::Inbox, &msg.msg_id)
            .is_ok());
        assert!(storage
            .move_msg(
                "100",
                "default",
                VoicemailFolder::Inbox,
                VoicemailFolder::Old,
                &msg.msg_id
            )
            .is_ok());
        assert!(storage
            .count_msgs("100", "default", VoicemailFolder::Inbox)
            .is_ok());
        assert!(storage
            .list_msgs("100", "default", VoicemailFolder::Inbox)
            .is_ok());
    }

    #[test]
    fn test_imap_storage() {
        let storage = ImapStorage::default();
        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            10,
            PathBuf::from("/tmp/test.wav"),
        );

        assert!(storage
            .save_msg("100", "default", VoicemailFolder::Inbox, &msg)
            .is_ok());
        assert!(storage
            .load_msg("100", "default", VoicemailFolder::Inbox, &msg.msg_id)
            .is_ok());
    }

    #[test]
    fn test_odbc_storage() {
        let storage = OdbcStorage::default();
        let msg = VoiceMessage::new(
            "Alice".to_string(),
            "5551234".to_string(),
            10,
            PathBuf::from("/tmp/test.wav"),
        );

        assert!(storage
            .save_msg("100", "default", VoicemailFolder::Inbox, &msg)
            .is_ok());
        assert!(storage
            .count_msgs("100", "default", VoicemailFolder::Inbox)
            .is_ok());
    }

    // --- voicemail.conf parsing tests ---

    #[test]
    fn test_parse_voicemail_conf() {
        let config = r#"
[general]
format=wav

[default]
100 => 1234,John Smith,john@example.com,pager@example.com,tz=US/Eastern|attach=yes
101 => 5678,Jane Doe,jane@example.com,,maxmsg=50
102 => 9999,Bob Builder

[sales]
200 => 1111,Sales Team,sales@example.com
"#;

        let configs = parse_voicemail_conf(config);
        assert_eq!(configs.len(), 4);

        assert_eq!(configs[0].mailbox_number, "100");
        assert_eq!(configs[0].context, "default");
        assert_eq!(configs[0].password, "1234");
        assert_eq!(configs[0].fullname, "John Smith");
        assert_eq!(configs[0].email, Some("john@example.com".to_string()));
        assert_eq!(configs[0].pager, Some("pager@example.com".to_string()));
        assert_eq!(configs[0].options.get("tz"), Some(&"US/Eastern".to_string()));
        assert_eq!(configs[0].options.get("attach"), Some(&"yes".to_string()));

        assert_eq!(configs[1].mailbox_number, "101");
        assert_eq!(
            configs[1].options.get("maxmsg"),
            Some(&"50".to_string())
        );

        assert_eq!(configs[2].mailbox_number, "102");
        assert_eq!(configs[2].email, None);

        assert_eq!(configs[3].context, "sales");
        assert_eq!(configs[3].mailbox_number, "200");
    }

    #[test]
    fn test_mailbox_config_to_mailbox() {
        let config = MailboxConfig::parse(
            "100",
            "default",
            "1234,John Smith,john@example.com,pager@example.com,tz=US/Eastern|attach=yes|maxmsg=50",
        );
        let mbox = config.to_mailbox();
        assert_eq!(mbox.mailbox_number, "100");
        assert_eq!(mbox.password, "1234");
        assert_eq!(mbox.fullname, "John Smith");
        assert_eq!(mbox.email, Some("john@example.com".to_string()));
        assert_eq!(mbox.pager, Some("pager@example.com".to_string()));
        assert_eq!(mbox.timezone, "US/Eastern");
        assert!(mbox.attach_voicemail);
        assert_eq!(mbox.max_messages, 50);
    }

    // --- VoicemailOptions tests ---

    #[test]
    fn test_voicemail_options_parse() {
        let opts = VoicemailOptions::parse("bsUP");
        assert!(opts.busy_greeting);
        assert!(opts.skip_instructions);
        assert!(opts.urgent);
        assert!(opts.priority);
        assert!(!opts.unavailable_greeting);
    }

    #[test]
    fn test_voicemail_options_early_media() {
        let opts = VoicemailOptions::parse("eu");
        assert!(opts.early_media);
        assert!(opts.unavailable_greeting);
    }

    // --- Recording config test ---

    #[test]
    fn test_recording_config_defaults() {
        let config = RecordingConfig::default();
        assert_eq!(config.max_message_secs, 300);
        assert_eq!(config.min_message_secs, 3);
        assert_eq!(config.max_silence_secs, 10);
        assert_eq!(config.format, "wav");
    }

    // --- MWI test ---

    #[test]
    fn test_mwi_state() {
        let state = MwiState {
            mailbox: "100".to_string(),
            context: "default".to_string(),
            new_messages: 3,
            old_messages: 5,
        };
        // Just ensure it doesn't panic
        publish_mwi(&state);
    }

    // -----------------------------------------------------------------------
    // Adversarial tests -- edge cases and attack vectors
    // -----------------------------------------------------------------------

    // --- Mailbox at max messages -> new message rejected ---
    #[test]
    fn test_adversarial_max_messages() {
        let mut mbox = Mailbox::new(
            "200".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        mbox.max_messages = 2;

        let msg1 = VoiceMessage::new("A".to_string(), "1".to_string(), 10, PathBuf::from("/tmp/1.wav"));
        let msg2 = VoiceMessage::new("B".to_string(), "2".to_string(), 10, PathBuf::from("/tmp/2.wav"));
        let msg3 = VoiceMessage::new("C".to_string(), "3".to_string(), 10, PathBuf::from("/tmp/3.wav"));

        assert!(mbox.add_message(VoicemailFolder::Inbox, msg1));
        assert!(mbox.add_message(VoicemailFolder::Inbox, msg2));
        // Third message should be rejected
        assert!(!mbox.add_message(VoicemailFolder::Inbox, msg3));
        assert_eq!(mbox.new_message_count(), 2);
    }

    // --- Message with duration < min_message_length -> deleted ---
    #[test]
    fn test_adversarial_min_message_length_zero() {
        let mut mbox = Mailbox::new(
            "201".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        mbox.recording_config.min_message_secs = 5;

        // Duration 0 -> rejected
        let msg = VoiceMessage::new("A".to_string(), "1".to_string(), 0, PathBuf::from("/tmp/0.wav"));
        assert!(!mbox.add_message(VoicemailFolder::Inbox, msg));

        // Duration exactly at min -> rejected (< not <=)
        let msg = VoiceMessage::new("A".to_string(), "1".to_string(), 4, PathBuf::from("/tmp/4.wav"));
        assert!(!mbox.add_message(VoicemailFolder::Inbox, msg));

        // Duration at min -> accepted
        let msg = VoiceMessage::new("A".to_string(), "1".to_string(), 5, PathBuf::from("/tmp/5.wav"));
        assert!(mbox.add_message(VoicemailFolder::Inbox, msg));
    }

    // --- Move message to same folder -> should work ---
    #[test]
    fn test_adversarial_move_to_same_folder() {
        let mut mbox = Mailbox::new(
            "202".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        let msg = VoiceMessage::new("A".to_string(), "1".to_string(), 10, PathBuf::from("/tmp/1.wav"));
        let msg_id = msg.msg_id.clone();
        mbox.add_message(VoicemailFolder::Inbox, msg);

        // Moving to same folder: removes from Inbox, adds back to Inbox
        assert!(mbox.move_message(&msg_id, VoicemailFolder::Inbox, VoicemailFolder::Inbox));
        assert_eq!(mbox.new_message_count(), 1);
    }

    // --- Move non-existent message -> false, no panic ---
    #[test]
    fn test_adversarial_move_nonexistent_message() {
        let mut mbox = Mailbox::new(
            "203".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        assert!(!mbox.move_message("ghost_id", VoicemailFolder::Inbox, VoicemailFolder::Old));
    }

    // --- Delete non-existent message -> false, no panic ---
    #[test]
    fn test_adversarial_delete_nonexistent_message() {
        let mut mbox = Mailbox::new(
            "204".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        assert!(!mbox.delete_message("ghost_id", VoicemailFolder::Inbox));
    }

    // --- Empty password -> authentication should fail ---
    #[test]
    fn test_adversarial_empty_password() {
        let mbox = Mailbox::new(
            "205".to_string(),
            "default".to_string(),
            "".to_string(), // Empty password
            "Test".to_string(),
        );
        // Empty password mailbox should reject all auth attempts
        assert!(!mbox.verify_password(""));
        assert!(!mbox.verify_password("1234"));
        assert!(!mbox.verify_password("anything"));
    }

    // --- Greeting fallback chain: no greetings at all -> system default ---
    #[test]
    fn test_adversarial_no_greetings_at_all() {
        let gs = GreetingState::default();
        // No greetings at all -> system default (Unavailable type, system ships default)
        let resolved = gs.resolve_greeting(GreetingType::Unavailable);
        assert_eq!(resolved, GreetingType::Unavailable);
        let resolved = gs.resolve_greeting(GreetingType::Busy);
        assert_eq!(resolved, GreetingType::Unavailable);
        let resolved = gs.resolve_greeting(GreetingType::Name);
        assert_eq!(resolved, GreetingType::Unavailable);
    }

    // --- Greeting fallback: busy requested, only unavail exists ---
    #[test]
    fn test_adversarial_greeting_busy_fallback_to_unavail() {
        let mut gs = GreetingState::default();
        gs.has_unavailable = true;
        // Busy not recorded -> falls back to unavailable
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Unavailable);
    }

    // --- Greeting fallback: busy requested, only name exists ---
    #[test]
    fn test_adversarial_greeting_busy_fallback_to_name() {
        let mut gs = GreetingState::default();
        gs.has_name = true;
        // No busy, no unavailable -> falls back to name
        assert_eq!(gs.resolve_greeting(GreetingType::Busy), GreetingType::Name);
    }

    // --- Template with missing variables -> empty substitution, no panic ---
    #[test]
    fn test_adversarial_template_missing_vars() {
        let notif = EmailNotification {
            to: "test@example.com".to_string(),
            from: "ast@localhost".to_string(),
            subject: "Custom: ${UNKNOWN_VAR} and ${VM_NAME}".to_string(),
            body_template: "Hello ${UNKNOWN_VAR}, ${VM_NAME}".to_string(),
            attachment_format: "wav".to_string(),
        };
        let vars = NotificationVars {
            vm_name: "John".to_string(),
            vm_mailbox: "100".to_string(),
            vm_cidname: "Jane".to_string(),
            vm_cidnum: "555".to_string(),
            vm_date: "today".to_string(),
            vm_duration: 30,
            vm_msgnum: 1,
        };
        // Unknown vars should remain as-is (not panic)
        let subject = notif.render_subject(&vars);
        assert!(subject.contains("${UNKNOWN_VAR}"));
        assert!(subject.contains("John"));
        let body = notif.render_body(&vars);
        assert!(body.contains("${UNKNOWN_VAR}"));
        assert!(body.contains("John"));
    }

    // --- Very large message count ---
    #[test]
    fn test_adversarial_many_messages() {
        let mut mbox = Mailbox::new(
            "206".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        mbox.max_messages = 1000;
        for i in 0..1000 {
            let msg = VoiceMessage::new(
                format!("Caller{}", i),
                format!("{}", i),
                10,
                PathBuf::from(format!("/tmp/msg{}.wav", i)),
            );
            assert!(mbox.add_message(VoicemailFolder::Inbox, msg));
        }
        assert_eq!(mbox.new_message_count(), 1000);
        assert_eq!(mbox.total_message_count(), 1000);
        // 1001st should be rejected
        let msg = VoiceMessage::new("X".to_string(), "X".to_string(), 10, PathBuf::from("/tmp/x.wav"));
        assert!(!mbox.add_message(VoicemailFolder::Inbox, msg));
    }

    // --- Playback state with 0 messages ---
    #[test]
    fn test_adversarial_playback_zero_messages() {
        let mut state = PlaybackState::new(VoicemailFolder::Inbox, 0);
        assert!(!state.advance());
        assert!(!state.previous());
        assert_eq!(state.current_message_index, 0);
    }

    // --- Playback state with 1 message ---
    #[test]
    fn test_adversarial_playback_one_message() {
        let mut state = PlaybackState::new(VoicemailFolder::Inbox, 1);
        assert!(!state.advance()); // Only one message, can't go forward
        assert!(!state.previous()); // Can't go back from 0
        assert_eq!(state.current_message_index, 0);
    }

    // --- VoicemailFolder from_number boundary ---
    #[test]
    fn test_adversarial_folder_from_number_boundary() {
        assert!(VoicemailFolder::from_number(0).is_some());
        assert!(VoicemailFolder::from_number(4).is_some());
        assert!(VoicemailFolder::from_number(5).is_none());
        assert!(VoicemailFolder::from_number(u32::MAX).is_none());
    }

    // --- Parse args with empty mailbox ---
    #[test]
    fn test_adversarial_parse_args_empty() {
        let (mailbox_specs, _opts) = AppVoiceMail::parse_args("");
        assert!(mailbox_specs.is_empty());
    }

    // --- Parse voicemail.conf with garbage ---
    #[test]
    fn test_adversarial_parse_conf_garbage() {
        let config = r#"
garbage line with no meaning
[default]
=> no_mailbox_number
100 =>
101 => ,,,,,
"#;
        let configs = parse_voicemail_conf(config);
        // "=> no_mailbox_number" has empty mailbox number, skipped
        // "100 =>" has empty value but is still parsed
        // "101 => ,,,,," has empty fields
        assert!(!configs.is_empty());
    }

    // --- MailboxConfig with all empty fields ---
    #[test]
    fn test_adversarial_mailbox_config_empty() {
        let config = MailboxConfig::parse("999", "default", "");
        assert_eq!(config.password, "");
        assert_eq!(config.fullname, "");
        assert!(config.email.is_none());
        let mbox = config.to_mailbox();
        assert_eq!(mbox.mailbox_number, "999");
    }

    // --- Recording DTMF: all non-standard digits ---
    #[test]
    fn test_adversarial_recording_dtmf_all_digits() {
        for c in '0'..='9' {
            let _ = RecordingDtmfAction::from_digit(c);
        }
        let _ = RecordingDtmfAction::from_digit('A');
        let _ = RecordingDtmfAction::from_digit('D');
        // None of these should panic
    }

    // --- Voicemail options with unknown chars -> ignored ---
    #[test]
    fn test_adversarial_voicemail_options_unknown() {
        let opts = VoicemailOptions::parse("bXYZ!");
        assert!(opts.busy_greeting);
        // Unknown chars are silently ignored
    }

    // --- Message with max u32 duration ---
    #[test]
    fn test_adversarial_huge_duration_message() {
        let mut mbox = Mailbox::new(
            "207".to_string(),
            "default".to_string(),
            "1234".to_string(),
            "Test".to_string(),
        );
        let msg = VoiceMessage::new(
            "A".to_string(),
            "1".to_string(),
            u32::MAX, // ~136 years
            PathBuf::from("/tmp/huge.wav"),
        );
        assert!(mbox.add_message(VoicemailFolder::Inbox, msg));
    }

    // --- Verify password with correct password ---
    #[test]
    fn test_adversarial_verify_password_correct() {
        let mbox = Mailbox::new(
            "208".to_string(),
            "default".to_string(),
            "secret".to_string(),
            "Test".to_string(),
        );
        assert!(mbox.verify_password("secret"));
        assert!(!mbox.verify_password("wrong"));
        assert!(!mbox.verify_password(""));
    }

    // --- No-beep option ---
    #[test]
    fn test_adversarial_nobeep_option() {
        // 'n' is not a valid voicemail option, ensure no crash
        let opts = VoicemailOptions::parse("n");
        assert!(!opts.busy_greeting);
    }
}
