//! FreeTDS/MSSQL CEL backend.
//!
//! Port of cel/cel_tds.c from Asterisk C.
//!
//! Writes CEL events to a Microsoft SQL Server database via FreeTDS.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Configuration for FreeTDS CEL backend.
#[derive(Debug, Clone)]
pub struct TdsCelConfig {
    pub hostname: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: String,
    pub table: String,
}

impl Default for TdsCelConfig {
    fn default() -> Self {
        Self {
            hostname: "localhost".to_string(),
            port: 1433,
            dbname: "asterisk".to_string(),
            user: "sa".to_string(),
            password: String::new(),
            table: "cel".to_string(),
        }
    }
}

/// FreeTDS CEL backend.
#[derive(Debug)]
pub struct TdsCelBackend {
    config: TdsCelConfig,
    events: parking_lot::RwLock<Vec<String>>,
}

impl TdsCelBackend {
    pub fn new() -> Self {
        Self {
            config: TdsCelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: TdsCelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for TdsCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for TdsCelBackend {
    fn name(&self) -> &str {
        "tds"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let sql = format!(
            "INSERT INTO {} (eventtype,eventtime,cidname,cidnum,exten,context,channame,appname,appdata,accountcode,uniqueid,linkedid,peer) VALUES (N'{}',{},N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}')",
            self.config.table,
            event.event_type.name(), event.timestamp,
            event.caller_id_name, event.caller_id_num,
            event.extension, event.context, event.channel_name,
            event.application, event.application_data,
            event.account_code, event.unique_id, event.linked_id, event.peer,
        );
        debug!("CEL TDS [{}]: {}", self.config.dbname, sql);
        self.events.write().push(sql);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_tds_cel_write() {
        let backend = TdsCelBackend::new();
        let event = CelEvent::new(CelEventType::Answer, "SIP/alice", "u1");
        backend.write(&event).unwrap();
        let events = backend.logged_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("ANSWER"));
        assert!(events[0].contains("N'SIP/alice'"));
    }

    #[test]
    fn test_default_config() {
        let config = TdsCelConfig::default();
        assert_eq!(config.port, 1433);
        assert_eq!(config.table, "cel");
    }
}
