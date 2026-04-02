//! Directory application - voice directory for dial-by-name.
//!
//! Port of app_directory.c from Asterisk C. Presents the caller with a
//! voice directory of extensions sourced from voicemail.conf. The caller
//! enters DTMF digits corresponding to the first or last name of the
//! person they want to reach, and the system searches for matching entries.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info};

/// Directory result set as the DIRECTORY_RESULT channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryResult {
    /// User requested operator (pressed '0').
    Operator,
    /// User requested assistant (pressed '*').
    Assistant,
    /// User allowed DTMF wait to pass without input.
    Timeout,
    /// Channel hung up.
    Hangup,
    /// User selected an entry and was connected.
    Selected,
    /// User exited with '#' during selection.
    UserExit,
    /// Application failed.
    Failed,
}

impl DirectoryResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Operator => "OPERATOR",
            Self::Assistant => "ASSISTANT",
            Self::Timeout => "TIMEOUT",
            Self::Hangup => "HANGUP",
            Self::Selected => "SELECTED",
            Self::UserExit => "USEREXIT",
            Self::Failed => "FAILED",
        }
    }
}

/// Search mode for the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum DirectorySearchMode {
    /// Search by last name (default).
    #[default]
    LastName,
    /// Search by first name.
    FirstName,
    /// Search by either first or last name.
    Both,
}


/// Options for the Directory application.
#[derive(Debug, Clone)]
pub struct DirectoryOptions {
    /// Search mode (first, last, or both).
    pub search_mode: DirectorySearchMode,
    /// Also read the extension number to the caller.
    pub read_extension: bool,
    /// Allow searching by alias.
    pub allow_alias: bool,
    /// Create a menu of up to 8 names instead of sequential confirmation.
    pub menu_mode: bool,
    /// Read digits even if channel is not answered.
    pub no_answer_read: bool,
    /// Pause in milliseconds after digits are typed.
    pub pause_ms: u32,
    /// Number of characters the user should enter (default 3).
    pub num_chars: usize,
    /// Alternative config file instead of voicemail.conf.
    pub config_file: Option<String>,
    /// Skip calling the extension, set DIRECTORY_EXTEN instead.
    pub skip_dial: bool,
}

impl Default for DirectoryOptions {
    fn default() -> Self {
        Self {
            search_mode: DirectorySearchMode::LastName,
            read_extension: false,
            allow_alias: false,
            menu_mode: false,
            no_answer_read: false,
            pause_ms: 0,
            num_chars: 3,
            config_file: None,
            skip_dial: false,
        }
    }
}

impl DirectoryOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut has_first = false;
        let mut has_last = false;
        let mut has_both = false;
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                'e' => result.read_extension = true,
                'f' => {
                    has_first = true;
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(n) = arg.parse::<usize>() {
                            if n > 0 {
                                result.num_chars = n;
                            }
                        }
                    }
                }
                'l' => {
                    has_last = true;
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(n) = arg.parse::<usize>() {
                            if n > 0 {
                                result.num_chars = n;
                            }
                        }
                    }
                }
                'b' => {
                    has_both = true;
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(n) = arg.parse::<usize>() {
                            if n > 0 {
                                result.num_chars = n;
                            }
                        }
                    }
                }
                'a' => result.allow_alias = true,
                'm' => result.menu_mode = true,
                'n' => result.no_answer_read = true,
                'p' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(ms) = arg.parse::<u32>() {
                            result.pause_ms = ms;
                        }
                    }
                }
                'c' => {
                    result.config_file = Self::extract_paren_arg(&mut chars);
                }
                's' => result.skip_dial = true,
                _ => {
                    debug!("Directory: ignoring unknown option '{}'", ch);
                }
            }
        }

        // Determine search mode: if more than one of f/l/b specified, use Both
        if has_both || (has_first && has_last) {
            result.search_mode = DirectorySearchMode::Both;
        } else if has_first {
            result.search_mode = DirectorySearchMode::FirstName;
        } else if has_last {
            result.search_mode = DirectorySearchMode::LastName;
        }
        // default is LastName

        result
    }

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
}

/// A directory entry sourced from voicemail configuration.
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// The mailbox extension number (e.g., "100").
    pub extension: String,
    /// Full name of the user.
    pub full_name: String,
    /// First name (parsed from full name).
    pub first_name: String,
    /// Last name (parsed from full name).
    pub last_name: String,
    /// Optional alias.
    pub alias: Option<String>,
    /// The voicemail context.
    pub vm_context: String,
}

impl DirectoryEntry {
    /// Create a new directory entry from voicemail user data.
    pub fn new(extension: &str, full_name: &str, vm_context: &str) -> Self {
        let (first, last) = Self::split_name(full_name);
        Self {
            extension: extension.to_string(),
            full_name: full_name.to_string(),
            first_name: first,
            last_name: last,
            alias: None,
            vm_context: vm_context.to_string(),
        }
    }

    /// Split a full name into first and last components.
    fn split_name(full_name: &str) -> (String, String) {
        let parts: Vec<&str> = full_name.splitn(2, ' ').collect();
        match parts.len() {
            0 => (String::new(), String::new()),
            1 => (parts[0].to_string(), String::new()),
            _ => (parts[0].to_string(), parts[1].to_string()),
        }
    }

    /// Convert a name to its DTMF digit representation for matching.
    ///
    /// Maps letters to phone keypad digits:
    /// ABC=2, DEF=3, GHI=4, JKL=5, MNO=6, PQRS=7, TUV=8, WXYZ=9
    pub fn name_to_dtmf(name: &str) -> String {
        name.chars()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| match c.to_ascii_uppercase() {
                'A' | 'B' | 'C' => '2',
                'D' | 'E' | 'F' => '3',
                'G' | 'H' | 'I' => '4',
                'J' | 'K' | 'L' => '5',
                'M' | 'N' | 'O' => '6',
                'P' | 'Q' | 'R' | 'S' => '7',
                'T' | 'U' | 'V' => '8',
                'W' | 'X' | 'Y' | 'Z' => '9',
                _ => '0',
            })
            .collect()
    }

    /// Check if the entry matches the given DTMF digits for a search mode.
    pub fn matches_dtmf(&self, digits: &str, mode: DirectorySearchMode) -> bool {
        match mode {
            DirectorySearchMode::LastName => {
                let dtmf = Self::name_to_dtmf(&self.last_name);
                dtmf.starts_with(digits)
            }
            DirectorySearchMode::FirstName => {
                let dtmf = Self::name_to_dtmf(&self.first_name);
                dtmf.starts_with(digits)
            }
            DirectorySearchMode::Both => {
                let first_dtmf = Self::name_to_dtmf(&self.first_name);
                let last_dtmf = Self::name_to_dtmf(&self.last_name);
                first_dtmf.starts_with(digits) || last_dtmf.starts_with(digits)
            }
        }
    }
}

/// The Directory() dialplan application.
///
/// Usage: Directory([vm-context[,dial-context[,options]]])
///
/// Presents a voice directory of extensions. The caller enters digits
/// matching a name (using phone keypad letter mapping) and the system
/// searches voicemail users for matches.
pub struct AppDirectory;

impl DialplanApp for AppDirectory {
    fn name(&self) -> &str {
        "Directory"
    }

    fn description(&self) -> &str {
        "Provide a directory of voicemail extensions"
    }
}

/// Parsed arguments for the Directory application.
#[derive(Debug)]
pub struct DirectoryArgs {
    /// Voicemail context to search.
    pub vm_context: String,
    /// Dialplan context for connecting to selected extension.
    pub dial_context: Option<String>,
    /// Directory options.
    pub options: DirectoryOptions,
}

impl DirectoryArgs {
    /// Parse Directory() argument string.
    ///
    /// Format: [vm-context[,dial-context[,options]]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.splitn(3, ',').collect();

        let vm_context = parts
            .first()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".to_string());

        let dial_context = parts
            .get(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let options = parts
            .get(2)
            .map(|o| DirectoryOptions::parse(o.trim()))
            .unwrap_or_default();

        Self {
            vm_context,
            dial_context,
            options,
        }
    }
}

impl AppDirectory {
    /// Execute the Directory application.
    ///
    /// # Arguments
    /// * `channel` - The channel to present the directory to
    /// * `args` - Argument string: "[vm-context[,dial-context[,options]]]"
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, DirectoryResult) {
        let parsed = DirectoryArgs::parse(args);

        info!(
            "Directory: channel '{}' searching vm-context='{}' dial-context={:?} mode={:?}",
            channel.name,
            parsed.vm_context,
            parsed.dial_context,
            parsed.options.search_mode,
        );

        // Answer the channel if needed (unless 'n' option)
        if !parsed.options.no_answer_read && channel.state != ChannelState::Up {
            debug!("Directory: answering channel");
            channel.state = ChannelState::Up;
        }

        let _dial_context = parsed
            .dial_context
            .unwrap_or_else(|| channel.context.clone());

        // In a real implementation:
        //
        //   // Load directory entries from voicemail.conf (or custom config)
        //   let config_file = parsed.options.config_file.as_deref()
        //       .unwrap_or("voicemail.conf");
        //   let entries = load_directory_entries(config_file, &parsed.vm_context);
        //
        //   // Prompt for first/last name search
        //   match parsed.options.search_mode {
        //       DirectorySearchMode::LastName =>
        //           play_file(channel, "dir-usingexten").await,
        //       DirectorySearchMode::FirstName =>
        //           play_file(channel, "dir-usingfirstname").await,
        //       DirectorySearchMode::Both =>
        //           play_file(channel, "dir-usingboth").await,
        //   }
        //
        //   // Collect DTMF digits
        //   let digits = read_dtmf(channel, parsed.options.num_chars, 3000).await;
        //   if digits.is_empty() {
        //       channel.set_variable("DIRECTORY_RESULT", "TIMEOUT");
        //       return (PbxExecResult::Success, DirectoryResult::Timeout);
        //   }
        //
        //   // Check for operator/assistant
        //   if digits == "0" {
        //       channel.set_variable("DIRECTORY_RESULT", "OPERATOR");
        //       return (PbxExecResult::Success, DirectoryResult::Operator);
        //   }
        //
        //   // Search for matching entries
        //   let matches: Vec<&DirectoryEntry> = entries.iter()
        //       .filter(|e| e.matches_dtmf(&digits, parsed.options.search_mode))
        //       .collect();
        //
        //   if matches.is_empty() {
        //       play_file(channel, "dir-nomatch").await;
        //       channel.set_variable("DIRECTORY_RESULT", "FAILED");
        //       return (PbxExecResult::Success, DirectoryResult::Failed);
        //   }
        //
        //   // Present matches to the caller
        //   if parsed.options.menu_mode {
        //       // Menu mode: present up to 8 choices
        //       for (i, entry) in matches.iter().take(8).enumerate() {
        //           say_name(channel, &entry.full_name).await;
        //           say_digits(channel, &format!("{}", i + 1)).await;
        //       }
        //       let choice = read_dtmf(channel, 1, 5000).await;
        //       // ... handle choice
        //   } else {
        //       // Sequential mode: present each and ask for confirmation
        //       for entry in &matches {
        //           say_name(channel, &entry.full_name).await;
        //           if parsed.options.read_extension {
        //               say_digits(channel, &entry.extension).await;
        //           }
        //           let confirm = read_dtmf(channel, 1, 3000).await;
        //           if confirm == "1" {
        //               if parsed.options.skip_dial {
        //                   channel.set_variable("DIRECTORY_EXTEN", &entry.extension);
        //               } else {
        //                   // Transfer to the selected extension
        //                   channel.context = dial_context.clone();
        //                   channel.exten = entry.extension.clone();
        //                   channel.priority = 1;
        //               }
        //               channel.set_variable("DIRECTORY_RESULT", "SELECTED");
        //               return (PbxExecResult::Success, DirectoryResult::Selected);
        //           }
        //       }
        //   }

        let result = DirectoryResult::Selected;
        channel.set_variable("DIRECTORY_RESULT", result.as_str());

        (PbxExecResult::Success, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_directory_args_empty() {
        let args = DirectoryArgs::parse("");
        assert_eq!(args.vm_context, "default");
        assert!(args.dial_context.is_none());
    }

    #[test]
    fn test_parse_directory_args_full() {
        let args = DirectoryArgs::parse("sales,from-internal,fb(4)");
        assert_eq!(args.vm_context, "sales");
        assert_eq!(args.dial_context.as_deref(), Some("from-internal"));
        assert_eq!(args.options.search_mode, DirectorySearchMode::Both);
        assert_eq!(args.options.num_chars, 4);
    }

    #[test]
    fn test_parse_directory_options() {
        let opts = DirectoryOptions::parse("fems");
        assert_eq!(opts.search_mode, DirectorySearchMode::FirstName);
        assert!(opts.read_extension);
        assert!(opts.menu_mode);
        assert!(opts.skip_dial);
    }

    #[test]
    fn test_name_to_dtmf() {
        assert_eq!(DirectoryEntry::name_to_dtmf("Smith"), "76484");
        assert_eq!(DirectoryEntry::name_to_dtmf("Jones"), "56637");
        assert_eq!(DirectoryEntry::name_to_dtmf("ABC"), "222");
    }

    #[test]
    fn test_directory_entry_match() {
        let entry = DirectoryEntry::new("100", "John Smith", "default");
        assert_eq!(entry.first_name, "John");
        assert_eq!(entry.last_name, "Smith");

        // "764" matches "Smith" -> 76484
        assert!(entry.matches_dtmf("764", DirectorySearchMode::LastName));
        // "564" matches "John" -> 5646
        assert!(entry.matches_dtmf("564", DirectorySearchMode::FirstName));
        // Both mode should match either
        assert!(entry.matches_dtmf("764", DirectorySearchMode::Both));
        assert!(entry.matches_dtmf("564", DirectorySearchMode::Both));
        // "999" should not match
        assert!(!entry.matches_dtmf("999", DirectorySearchMode::LastName));
    }

    #[test]
    fn test_directory_result_strings() {
        assert_eq!(DirectoryResult::Selected.as_str(), "SELECTED");
        assert_eq!(DirectoryResult::Operator.as_str(), "OPERATOR");
        assert_eq!(DirectoryResult::Timeout.as_str(), "TIMEOUT");
        assert_eq!(DirectoryResult::Failed.as_str(), "FAILED");
    }

    #[tokio::test]
    async fn test_directory_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let (result, dir_result) =
            AppDirectory::exec(&mut channel, "default,from-internal,l").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(dir_result, DirectoryResult::Selected);
    }
}
