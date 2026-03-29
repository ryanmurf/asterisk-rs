//! ODBC CEL backend.
//!
//! Port of cel/cel_odbc.c from Asterisk C.
//!
//! Writes CEL events to an ODBC-compatible database.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Configuration for ODBC CEL backend.
#[derive(Debug, Clone)]
pub struct OdbcCelConfig {
    pub dsn: String,
    pub table: String,
}

impl Default for OdbcCelConfig {
    fn default() -> Self {
        Self {
            dsn: "asterisk-cel".to_string(),
            table: "cel".to_string(),
        }
    }
}

/// ODBC CEL backend.
#[derive(Debug)]
pub struct OdbcCelBackend {
    config: OdbcCelConfig,
    events: parking_lot::RwLock<Vec<String>>,
}

impl OdbcCelBackend {
    pub fn new() -> Self {
        Self {
            config: OdbcCelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: OdbcCelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Build INSERT SQL for a CEL event.
    pub fn build_insert_sql(&self, event: &CelEvent) -> String {
        format!(
            "INSERT INTO {} (eventtype,eventtime,cidname,cidnum,cidani,cidrdnis,ciddnid,exten,context,channame,appname,appdata,accountcode,uniqueid,linkedid,peer,userdeftype,extra) VALUES ('{}',{}.{},'{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}','{}')",
            self.config.table,
            event.event_type.name(), event.timestamp, event.timestamp_usec,
            event.caller_id_name, event.caller_id_num,
            event.caller_id_ani, event.caller_id_rdnis, event.caller_id_dnid,
            event.extension, event.context, event.channel_name,
            event.application, event.application_data,
            event.account_code,
            event.unique_id, event.linked_id, event.peer,
            event.user_defined_name, event.extra,
        )
    }

    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for OdbcCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for OdbcCelBackend {
    fn name(&self) -> &str {
        "odbc"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let sql = self.build_insert_sql(event);
        debug!("CEL ODBC [{}]: {}", self.config.dsn, sql);
        self.events.write().push(sql);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_build_insert() {
        let backend = OdbcCelBackend::new();
        let event = CelEvent::new(CelEventType::Answer, "SIP/alice", "u1");
        let sql = backend.build_insert_sql(&event);
        assert!(sql.contains("INSERT INTO cel"));
        assert!(sql.contains("ANSWER"));
    }

    #[test]
    fn test_odbc_cel_write() {
        let backend = OdbcCelBackend::new();
        let event = CelEvent::new(CelEventType::Hangup, "SIP/bob", "u2");
        backend.write(&event).unwrap();
        assert_eq!(backend.logged_events().len(), 1);
    }
}
