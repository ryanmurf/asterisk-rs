//! Syslog CDR backend.
//!
//! Port of cdr/cdr_syslog.c from Asterisk C.
//!
//! Writes CDR records to the system syslog facility using a
//! configurable template format and syslog priority level.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::{debug, info, warn};

/// Syslog priority levels (matching standard syslog severities).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyslogPriority {
    Emergency,
    Alert,
    Critical,
    Error,
    Warning,
    Notice,
    Info,
    Debug,
}

impl SyslogPriority {
    /// Parse from string.
    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "emerg" | "emergency" => Self::Emergency,
            "alert" => Self::Alert,
            "crit" | "critical" => Self::Critical,
            "err" | "error" => Self::Error,
            "warning" | "warn" => Self::Warning,
            "notice" => Self::Notice,
            "info" => Self::Info,
            "debug" => Self::Debug,
            _ => Self::Info,
        }
    }

    /// String representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Emergency => "emerg",
            Self::Alert => "alert",
            Self::Critical => "crit",
            Self::Error => "err",
            Self::Warning => "warning",
            Self::Notice => "notice",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// Syslog facility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyslogFacility {
    User,
    Local0,
    Local1,
    Local2,
    Local3,
    Local4,
    Local5,
    Local6,
    Local7,
}

impl SyslogFacility {
    /// Parse from string.
    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "user" => Self::User,
            "local0" => Self::Local0,
            "local1" => Self::Local1,
            "local2" => Self::Local2,
            "local3" => Self::Local3,
            "local4" => Self::Local4,
            "local5" => Self::Local5,
            "local6" => Self::Local6,
            "local7" => Self::Local7,
            _ => Self::Local0,
        }
    }
}

/// Configuration for the Syslog CDR backend.
#[derive(Debug, Clone)]
pub struct SyslogCdrConfig {
    /// Syslog facility
    pub facility: SyslogFacility,
    /// Syslog priority/severity
    pub priority: SyslogPriority,
    /// Template format string with ${CDR(field)} placeholders
    pub template: String,
    /// Ident string for syslog
    pub ident: String,
}

impl Default for SyslogCdrConfig {
    fn default() -> Self {
        Self {
            facility: SyslogFacility::Local0,
            priority: SyslogPriority::Info,
            template: "\"${CDR(src)}\",\"${CDR(dst)}\",\"${CDR(channel)}\",${CDR(duration)},${CDR(billsec)},\"${CDR(disposition)}\"".to_string(),
            ident: "asterisk-cdr".to_string(),
        }
    }
}

/// Syslog CDR backend.
///
/// Writes CDR records to syslog using a configurable template.
/// In this Rust port, we use the `tracing` crate for logging
/// since direct syslog access varies by platform.
///
/// In production, this could be connected to a real syslog
/// sender via `syslog` crate or similar.
pub struct SyslogCdrBackend {
    config: SyslogCdrConfig,
    /// Last logged message (for testing)
    last_message: parking_lot::Mutex<Option<String>>,
}

impl SyslogCdrBackend {
    /// Create a new syslog CDR backend with default configuration.
    pub fn new() -> Self {
        Self {
            config: SyslogCdrConfig::default(),
            last_message: parking_lot::Mutex::new(None),
        }
    }

    /// Create with specific configuration.
    pub fn with_config(config: SyslogCdrConfig) -> Self {
        Self {
            config,
            last_message: parking_lot::Mutex::new(None),
        }
    }

    /// Get the last logged message (for testing).
    pub fn last_message(&self) -> Option<String> {
        self.last_message.lock().clone()
    }

    /// Expand template variables.
    fn expand_template(template: &str, cdr: &Cdr) -> String {
        let mut result = template.to_string();

        let replacements = [
            ("${CDR(src)}", cdr.src.as_str()),
            ("${CDR(dst)}", cdr.dst.as_str()),
            ("${CDR(dcontext)}", cdr.dst_context.as_str()),
            ("${CDR(channel)}", cdr.channel.as_str()),
            ("${CDR(dstchannel)}", cdr.dst_channel.as_str()),
            ("${CDR(lastapp)}", cdr.last_app.as_str()),
            ("${CDR(lastdata)}", cdr.last_data.as_str()),
            ("${CDR(disposition)}", cdr.disposition.as_str()),
            ("${CDR(amaflags)}", cdr.ama_flags.as_str()),
            ("${CDR(accountcode)}", cdr.account_code.as_str()),
            ("${CDR(uniqueid)}", cdr.unique_id.as_str()),
            ("${CDR(userfield)}", cdr.user_field.as_str()),
            ("${CDR(linkedid)}", cdr.linked_id.as_str()),
            ("${CDR(clid)}", cdr.caller_id.as_str()),
        ];

        for (var, val) in &replacements {
            result = result.replace(var, val);
        }

        result = result.replace("${CDR(duration)}", &cdr.duration.to_string());
        result = result.replace("${CDR(billsec)}", &cdr.billsec.to_string());
        result = result.replace("${CDR(sequence)}", &cdr.sequence.to_string());

        result
    }
}

impl Default for SyslogCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for SyslogCdrBackend {
    fn name(&self) -> &str {
        "syslog"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let message = Self::expand_template(&self.config.template, cdr);

        debug!("CDR Syslog [{}]: {}", self.config.ident, message);

        // Use tracing at the appropriate level to simulate syslog
        match self.config.priority {
            SyslogPriority::Emergency | SyslogPriority::Alert | SyslogPriority::Critical => {
                tracing::error!(target: "cdr_syslog", "{}: {}", self.config.ident, message);
            }
            SyslogPriority::Error => {
                tracing::error!(target: "cdr_syslog", "{}: {}", self.config.ident, message);
            }
            SyslogPriority::Warning => {
                warn!(target: "cdr_syslog", "{}: {}", self.config.ident, message);
            }
            SyslogPriority::Notice | SyslogPriority::Info => {
                info!(target: "cdr_syslog", "{}: {}", self.config.ident, message);
            }
            SyslogPriority::Debug => {
                debug!(target: "cdr_syslog", "{}: {}", self.config.ident, message);
            }
        }

        *self.last_message.lock() = Some(message);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CdrDisposition;

    #[test]
    fn test_syslog_expand_template() {
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid-1".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.disposition = CdrDisposition::Answered;
        cdr.duration = 60;
        cdr.billsec = 45;

        let template = "${CDR(src)} -> ${CDR(dst)} [${CDR(disposition)}] ${CDR(duration)}s";
        let result = SyslogCdrBackend::expand_template(template, &cdr);

        assert_eq!(result, "5551234 -> 100 [ANSWERED] 60s");
    }

    #[test]
    fn test_syslog_log() {
        let backend = SyslogCdrBackend::new();
        let mut cdr = Cdr::new("SIP/bob".to_string(), "uid-2".to_string());
        cdr.src = "5559876".to_string();
        cdr.dst = "200".to_string();
        cdr.disposition = CdrDisposition::NoAnswer;
        cdr.duration = 30;
        cdr.billsec = 0;

        backend.log(&cdr).unwrap();

        let msg = backend.last_message().unwrap();
        assert!(msg.contains("5559876"));
        assert!(msg.contains("200"));
        assert!(msg.contains("NO ANSWER"));
    }

    #[test]
    fn test_syslog_priority_parsing() {
        assert_eq!(SyslogPriority::from_str_name("info"), SyslogPriority::Info);
        assert_eq!(
            SyslogPriority::from_str_name("warning"),
            SyslogPriority::Warning
        );
        assert_eq!(
            SyslogPriority::from_str_name("error"),
            SyslogPriority::Error
        );
        assert_eq!(
            SyslogPriority::from_str_name("debug"),
            SyslogPriority::Debug
        );
    }

    #[test]
    fn test_syslog_facility_parsing() {
        assert_eq!(
            SyslogFacility::from_str_name("local0"),
            SyslogFacility::Local0
        );
        assert_eq!(
            SyslogFacility::from_str_name("user"),
            SyslogFacility::User
        );
        assert_eq!(
            SyslogFacility::from_str_name("local7"),
            SyslogFacility::Local7
        );
    }

    #[test]
    fn test_syslog_custom_config() {
        let config = SyslogCdrConfig {
            priority: SyslogPriority::Warning,
            template: "CDR: ${CDR(src)}->${CDR(dst)}".to_string(),
            ..Default::default()
        };
        let backend = SyslogCdrBackend::with_config(config);

        let mut cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        cdr.src = "1234".to_string();
        cdr.dst = "5678".to_string();

        backend.log(&cdr).unwrap();
        let msg = backend.last_message().unwrap();
        assert_eq!(msg, "CDR: 1234->5678");
    }
}
