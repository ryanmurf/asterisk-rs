//! Call file spooler.
//!
//! Port of pbx/pbx_spool.c from Asterisk C.
//!
//! Watches /var/spool/asterisk/outgoing/ for .call files, parses them,
//! and originates calls based on their contents.
//!
//! Call file format (one key: value per line):
//!   Channel: SIP/provider/18005551234
//!   CallerID: "Asterisk" <1001>
//!   MaxRetries: 3
//!   RetryTime: 60
//!   WaitTime: 30
//!   Context: default
//!   Extension: s
//!   Priority: 1
//!   Set: VARIABLE=value
//!   Account: billing_code
//!   Application: Playback
//!   Data: hello-world

use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// Default spool directory.
pub const DEFAULT_SPOOL_DIR: &str = "/var/spool/asterisk/outgoing";

/// A parsed .call file.
#[derive(Debug, Clone)]
pub struct CallFile {
    /// Source file path
    pub file_path: PathBuf,
    /// Channel to dial (required)
    pub channel: String,
    /// Caller ID string
    pub caller_id: String,
    /// Maximum retries (default 0)
    pub max_retries: u32,
    /// Time between retries in seconds (default 300)
    pub retry_time: u32,
    /// Wait time for answer in seconds (default 45)
    pub wait_time: u32,
    /// Context (for dialplan destination)
    pub context: String,
    /// Extension (for dialplan destination)
    pub extension: String,
    /// Priority (for dialplan destination)
    pub priority: i32,
    /// Account code
    pub account: String,
    /// Application to run (alternative to context/extension)
    pub application: String,
    /// Application data
    pub data: String,
    /// Channel variables to set
    pub variables: HashMap<String, String>,
    /// Whether to archive the file after processing
    pub archive: bool,
    /// Current retry count
    pub retries: u32,
    /// Earliest time to process (0 = now)
    pub earliest: u64,
}

impl CallFile {
    /// Create a new empty call file.
    pub fn new() -> Self {
        Self {
            file_path: PathBuf::new(),
            channel: String::new(),
            caller_id: String::new(),
            max_retries: 0,
            retry_time: 300,
            wait_time: 45,
            context: "default".to_string(),
            extension: "s".to_string(),
            priority: 1,
            account: String::new(),
            application: String::new(),
            data: String::new(),
            variables: HashMap::new(),
            archive: false,
            retries: 0,
            earliest: 0,
        }
    }

    /// Parse a .call file from its text content.
    pub fn parse(content: &str) -> Result<Self, String> {
        let mut cf = CallFile::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() < 2 {
                continue;
            }

            let key = parts[0].trim().to_lowercase();
            let value = parts[1].trim();

            match key.as_str() {
                "channel" => cf.channel = value.to_string(),
                "callerid" => cf.caller_id = value.to_string(),
                "maxretries" => cf.max_retries = value.parse().unwrap_or(0),
                "retrytime" => cf.retry_time = value.parse().unwrap_or(300),
                "waittime" => cf.wait_time = value.parse().unwrap_or(45),
                "context" => cf.context = value.to_string(),
                "extension" => cf.extension = value.to_string(),
                "priority" => cf.priority = value.parse().unwrap_or(1),
                "account" => cf.account = value.to_string(),
                "application" => cf.application = value.to_string(),
                "data" => cf.data = value.to_string(),
                "archive" => cf.archive = value == "yes" || value == "1" || value == "true",
                "set" => {
                    // Format: VARIABLE=value
                    if let Some(eq_pos) = value.find('=') {
                        let var_name = value[..eq_pos].trim().to_string();
                        let var_val = value[eq_pos + 1..].trim().to_string();
                        cf.variables.insert(var_name, var_val);
                    }
                }
                _ => {
                    debug!("call file: unknown directive '{}'", key);
                }
            }
        }

        if cf.channel.is_empty() {
            return Err("Call file missing required 'Channel' directive".to_string());
        }

        Ok(cf)
    }

    /// Check if this call file uses an application (vs context/exten/priority).
    pub fn uses_application(&self) -> bool {
        !self.application.is_empty()
    }

    /// Check if retries are exhausted.
    pub fn retries_exhausted(&self) -> bool {
        self.retries >= self.max_retries
    }

    /// Increment the retry count.
    pub fn increment_retries(&mut self) {
        self.retries += 1;
    }
}

impl Default for CallFile {
    fn default() -> Self {
        Self::new()
    }
}

/// Spool directory watcher configuration.
#[derive(Debug, Clone)]
pub struct SpoolConfig {
    /// Directory to watch
    pub spool_dir: PathBuf,
    /// Archive directory for processed files
    pub archive_dir: PathBuf,
    /// Polling interval in seconds
    pub poll_interval: u64,
}

impl Default for SpoolConfig {
    fn default() -> Self {
        Self {
            spool_dir: PathBuf::from(DEFAULT_SPOOL_DIR),
            archive_dir: PathBuf::from("/var/spool/asterisk/outgoing_done"),
            poll_interval: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_call_file() {
        let content = r#"
Channel: SIP/provider/18005551234
CallerID: "Test" <1001>
MaxRetries: 3
RetryTime: 60
WaitTime: 30
Context: outbound
Extension: s
Priority: 1
"#;
        let cf = CallFile::parse(content).unwrap();
        assert_eq!(cf.channel, "SIP/provider/18005551234");
        assert_eq!(cf.caller_id, "\"Test\" <1001>");
        assert_eq!(cf.max_retries, 3);
        assert_eq!(cf.retry_time, 60);
        assert_eq!(cf.wait_time, 30);
        assert_eq!(cf.context, "outbound");
        assert_eq!(cf.extension, "s");
        assert_eq!(cf.priority, 1);
    }

    #[test]
    fn test_parse_application_call_file() {
        let content = r#"
Channel: SIP/alice
Application: Playback
Data: hello-world
"#;
        let cf = CallFile::parse(content).unwrap();
        assert!(cf.uses_application());
        assert_eq!(cf.application, "Playback");
        assert_eq!(cf.data, "hello-world");
    }

    #[test]
    fn test_parse_with_variables() {
        let content = r#"
Channel: SIP/bob
Set: CUSTOMER_ID=12345
Set: CAMPAIGN=spring2024
Context: default
Extension: 100
Priority: 1
"#;
        let cf = CallFile::parse(content).unwrap();
        assert_eq!(cf.variables.get("CUSTOMER_ID").unwrap(), "12345");
        assert_eq!(cf.variables.get("CAMPAIGN").unwrap(), "spring2024");
    }

    #[test]
    fn test_parse_missing_channel() {
        let content = "Context: default\nExtension: 100\n";
        assert!(CallFile::parse(content).is_err());
    }

    #[test]
    fn test_retries() {
        let mut cf = CallFile::new();
        cf.channel = "SIP/test".to_string();
        cf.max_retries = 2;
        assert!(!cf.retries_exhausted());
        cf.increment_retries();
        assert!(!cf.retries_exhausted());
        cf.increment_retries();
        assert!(cf.retries_exhausted());
    }

    #[test]
    fn test_archive_flag() {
        let content = "Channel: SIP/test\nArchive: yes\n";
        let cf = CallFile::parse(content).unwrap();
        assert!(cf.archive);
    }
}
