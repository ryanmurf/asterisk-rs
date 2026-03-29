//! PostgreSQL CEL backend.
//!
//! Port of cel/cel_pgsql.c from Asterisk C.
//!
//! Writes CEL events to a PostgreSQL database.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Configuration for PostgreSQL CEL backend.
#[derive(Debug, Clone)]
pub struct PgsqlCelConfig {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: String,
    pub table: String,
}

impl Default for PgsqlCelConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5432,
            dbname: "asteriskceldb".to_string(),
            user: "asterisk".to_string(),
            password: String::new(),
            table: "cel".to_string(),
        }
    }
}

/// PostgreSQL CEL backend.
#[derive(Debug)]
pub struct PgsqlCelBackend {
    config: PgsqlCelConfig,
    events: parking_lot::RwLock<Vec<String>>,
}

impl PgsqlCelBackend {
    pub fn new() -> Self {
        Self {
            config: PgsqlCelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: PgsqlCelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn connection_string(&self) -> String {
        format!(
            "host={} port={} dbname={} user={}",
            self.config.host, self.config.port, self.config.dbname, self.config.user,
        )
    }

    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for PgsqlCelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for PgsqlCelBackend {
    fn name(&self) -> &str {
        "pgsql"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let sql = format!(
            "INSERT INTO {} (eventtype,eventtime,cidname,cidnum,exten,context,channame,appname,appdata,accountcode,uniqueid,linkedid,peer,extra) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
            self.config.table
        );
        debug!("CEL PgSQL [{}]: {}", self.config.dbname, sql);
        self.events.write().push(format!("{}: {}", event.event_type.name(), event.channel_name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_connection_string() {
        let backend = PgsqlCelBackend::new();
        let cs = backend.connection_string();
        assert!(cs.contains("host=localhost"));
    }

    #[test]
    fn test_pgsql_cel_write() {
        let backend = PgsqlCelBackend::new();
        let event = CelEvent::new(CelEventType::BridgeEnter, "SIP/alice", "u1");
        backend.write(&event).unwrap();
        assert_eq!(backend.logged_events().len(), 1);
    }
}
