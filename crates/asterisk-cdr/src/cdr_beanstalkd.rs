//! Beanstalkd queue CDR backend.
//!
//! Port of cdr/cdr_beanstalkd.c from Asterisk C.
//!
//! Publishes CDR records as JSON messages to a Beanstalkd work queue
//! for asynchronous processing by external consumers.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// Configuration for the Beanstalkd CDR backend.
#[derive(Debug, Clone)]
pub struct BeanstalkdCdrConfig {
    /// Beanstalkd server host
    pub host: String,
    /// Beanstalkd server port
    pub port: u16,
    /// Tube name to publish to
    pub tube: String,
    /// Job priority (lower = higher priority)
    pub priority: u32,
    /// Job delay in seconds
    pub delay: u32,
    /// Job time-to-run in seconds
    pub ttr: u32,
}

impl Default for BeanstalkdCdrConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 11300,
            tube: "asterisk-cdr".to_string(),
            priority: 99,
            delay: 0,
            ttr: 60,
        }
    }
}

/// Beanstalkd CDR backend.
///
/// Publishes CDR records as JSON to a Beanstalkd tube. The actual
/// network connection is stubbed; this provides the JSON serialization
/// and interface.
pub struct BeanstalkdCdrBackend {
    config: BeanstalkdCdrConfig,
    /// Last published message (for testing)
    last_message: parking_lot::Mutex<Option<String>>,
}

impl BeanstalkdCdrBackend {
    pub fn new() -> Self {
        Self {
            config: BeanstalkdCdrConfig::default(),
            last_message: parking_lot::Mutex::new(None),
        }
    }

    pub fn with_config(config: BeanstalkdCdrConfig) -> Self {
        Self {
            config,
            last_message: parking_lot::Mutex::new(None),
        }
    }

    /// Format CDR as JSON string.
    pub fn cdr_to_json(cdr: &Cdr) -> String {
        format!(
            r#"{{"src":"{}","dst":"{}","channel":"{}","dstchannel":"{}","lastapp":"{}","lastdata":"{}","duration":{},"billsec":{},"disposition":"{}","amaflags":"{}","accountcode":"{}","uniqueid":"{}","linkedid":"{}","userfield":"{}"}}"#,
            cdr.src, cdr.dst, cdr.channel, cdr.dst_channel,
            cdr.last_app, cdr.last_data, cdr.duration, cdr.billsec,
            cdr.disposition.as_str(), cdr.ama_flags.as_str(),
            cdr.account_code, cdr.unique_id, cdr.linked_id, cdr.user_field,
        )
    }

    pub fn last_message(&self) -> Option<String> {
        self.last_message.lock().clone()
    }
}

impl Default for BeanstalkdCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for BeanstalkdCdrBackend {
    fn name(&self) -> &str {
        "beanstalkd"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let json = Self::cdr_to_json(cdr);
        debug!(
            "CDR Beanstalkd: put to tube '{}' at {}:{} - {}",
            self.config.tube, self.config.host, self.config.port, json
        );
        *self.last_message.lock() = Some(json);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CdrDisposition;

    #[test]
    fn test_cdr_to_json() {
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid-1".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.disposition = CdrDisposition::Answered;
        cdr.duration = 60;
        cdr.billsec = 45;

        let json = BeanstalkdCdrBackend::cdr_to_json(&cdr);
        assert!(json.contains("\"src\":\"5551234\""));
        assert!(json.contains("\"duration\":60"));
        assert!(json.contains("\"disposition\":\"ANSWERED\""));
    }

    #[test]
    fn test_beanstalkd_log() {
        let backend = BeanstalkdCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        backend.log(&cdr).unwrap();
        assert!(backend.last_message().is_some());
    }

    #[test]
    fn test_default_config() {
        let config = BeanstalkdCdrConfig::default();
        assert_eq!(config.port, 11300);
        assert_eq!(config.tube, "asterisk-cdr");
    }
}
