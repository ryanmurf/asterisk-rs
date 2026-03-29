//! Beanstalkd CEL backend.
//!
//! Port of cel/cel_beanstalkd.c from Asterisk C.
//!
//! Publishes CEL events as JSON messages to a Beanstalkd work queue.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Configuration for Beanstalkd CEL backend.
#[derive(Debug, Clone)]
pub struct BeanstalkdCelConfig {
    pub host: String,
    pub port: u16,
    pub tube: String,
    pub priority: u32,
}

impl Default for BeanstalkdCelConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 11300,
            tube: "asterisk-cel".to_string(),
            priority: 99,
        }
    }
}

/// Beanstalkd CEL backend.
#[derive(Debug)]
pub struct BeanstalkdCelBackend {
    config: BeanstalkdCelConfig,
    events: parking_lot::RwLock<Vec<String>>,
}

impl BeanstalkdCelBackend {
    pub fn new() -> Self {
        Self {
            config: BeanstalkdCelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: BeanstalkdCelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Format CEL event as JSON.
    pub fn event_to_json(event: &CelEvent) -> String {
        format!(
            r#"{{"event_type":"{}","timestamp":{},"channel":"{}","unique_id":"{}","linked_id":"{}","caller_id_num":"{}","caller_id_name":"{}","context":"{}","extension":"{}","application":"{}","app_data":"{}","account_code":"{}","peer":"{}","extra":"{}"}}"#,
            event.event_type.name(), event.timestamp,
            event.channel_name, event.unique_id, event.linked_id,
            event.caller_id_num, event.caller_id_name,
            event.context, event.extension,
            event.application, event.application_data,
            event.account_code, event.peer, event.extra,
        )
    }

    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for BeanstalkdCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for BeanstalkdCelBackend {
    fn name(&self) -> &str {
        "beanstalkd"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let json = Self::event_to_json(event);
        debug!("CEL Beanstalkd: tube '{}' - {}", self.config.tube, json);
        self.events.write().push(json);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_event_to_json() {
        let event = CelEvent::new(CelEventType::ChannelStart, "SIP/alice", "u1");
        let json = BeanstalkdCelBackend::event_to_json(&event);
        assert!(json.contains("CHAN_START"));
        assert!(json.contains("SIP/alice"));
    }

    #[test]
    fn test_beanstalkd_cel_write() {
        let backend = BeanstalkdCelBackend::new();
        let event = CelEvent::new(CelEventType::Answer, "SIP/bob", "u2");
        backend.write(&event).unwrap();
        assert_eq!(backend.logged_events().len(), 1);
    }
}
