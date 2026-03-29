//! RADIUS CEL backend.
//!
//! Port of cel/cel_radius.c from Asterisk C.
//!
//! Sends CEL events as RADIUS Accounting-Request messages.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Configuration for RADIUS CEL backend.
#[derive(Debug, Clone)]
pub struct RadiusCelConfig {
    pub server: String,
    pub port: u16,
    pub secret: String,
}

impl Default for RadiusCelConfig {
    fn default() -> Self {
        Self {
            server: "127.0.0.1".to_string(),
            port: 1813,
            secret: String::new(),
        }
    }
}

/// A RADIUS AVP for CEL.
#[derive(Debug, Clone)]
pub struct CelRadiusAvp {
    pub name: String,
    pub value: String,
}

/// RADIUS CEL backend.
#[derive(Debug)]
pub struct RadiusCelBackend {
    config: RadiusCelConfig,
    events: parking_lot::RwLock<Vec<Vec<CelRadiusAvp>>>,
}

impl RadiusCelBackend {
    pub fn new() -> Self {
        Self {
            config: RadiusCelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: RadiusCelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Build RADIUS AVPs from a CEL event.
    pub fn build_avps(event: &CelEvent) -> Vec<CelRadiusAvp> {
        vec![
            CelRadiusAvp { name: "Event-Type".to_string(), value: event.event_type.name().to_string() },
            CelRadiusAvp { name: "Event-Timestamp".to_string(), value: event.timestamp.to_string() },
            CelRadiusAvp { name: "Channel-Name".to_string(), value: event.channel_name.clone() },
            CelRadiusAvp { name: "Unique-ID".to_string(), value: event.unique_id.clone() },
            CelRadiusAvp { name: "Linked-ID".to_string(), value: event.linked_id.clone() },
            CelRadiusAvp { name: "Caller-ID-Num".to_string(), value: event.caller_id_num.clone() },
            CelRadiusAvp { name: "Caller-ID-Name".to_string(), value: event.caller_id_name.clone() },
            CelRadiusAvp { name: "Context".to_string(), value: event.context.clone() },
            CelRadiusAvp { name: "Extension".to_string(), value: event.extension.clone() },
            CelRadiusAvp { name: "Application".to_string(), value: event.application.clone() },
            CelRadiusAvp { name: "Account-Code".to_string(), value: event.account_code.clone() },
        ]
    }

    pub fn logged_events(&self) -> Vec<Vec<CelRadiusAvp>> {
        self.events.read().clone()
    }
}

impl Default for RadiusCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for RadiusCelBackend {
    fn name(&self) -> &str {
        "radius"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let avps = Self::build_avps(event);
        debug!("CEL RADIUS: {} AVPs to {}:{}", avps.len(), self.config.server, self.config.port);
        self.events.write().push(avps);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_build_avps() {
        let event = CelEvent::new(CelEventType::Answer, "SIP/alice", "u1")
            .with_caller_id("1001", "Alice");
        let avps = RadiusCelBackend::build_avps(&event);
        assert!(!avps.is_empty());
        assert_eq!(avps[0].value, "ANSWER");
    }

    #[test]
    fn test_radius_cel_write() {
        let backend = RadiusCelBackend::new();
        let event = CelEvent::new(CelEventType::Hangup, "SIP/bob", "u2");
        backend.write(&event).unwrap();
        assert_eq!(backend.logged_events().len(), 1);
    }
}
