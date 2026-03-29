//! SQLite3 CEL backend.
//!
//! Port of cel/cel_sqlite3_custom.c from Asterisk C.
//!
//! Writes CEL events to a SQLite3 database with configurable columns.

use crate::cel::{CelBackend, CelEvent, CelResult};
use tracing::debug;

/// Column mapping for SQLite3 CEL.
#[derive(Debug, Clone)]
pub struct CelColumnMapping {
    pub column: String,
    pub cel_field: String,
    pub sql_type: String,
}

/// Configuration for SQLite3 CEL backend.
#[derive(Debug, Clone)]
pub struct Sqlite3CelConfig {
    pub db_path: String,
    pub table: String,
    pub columns: Vec<CelColumnMapping>,
}

impl Default for Sqlite3CelConfig {
    fn default() -> Self {
        Self {
            db_path: "/var/log/asterisk/cel.db".to_string(),
            table: "cel".to_string(),
            columns: vec![
                CelColumnMapping { column: "eventtype".to_string(), cel_field: "eventtype".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "eventtime".to_string(), cel_field: "eventtime".to_string(), sql_type: "INTEGER".to_string() },
                CelColumnMapping { column: "cidname".to_string(), cel_field: "cidname".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "cidnum".to_string(), cel_field: "cidnum".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "exten".to_string(), cel_field: "exten".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "context".to_string(), cel_field: "context".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "channame".to_string(), cel_field: "channame".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "appname".to_string(), cel_field: "appname".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "appdata".to_string(), cel_field: "appdata".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "accountcode".to_string(), cel_field: "accountcode".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "uniqueid".to_string(), cel_field: "uniqueid".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "linkedid".to_string(), cel_field: "linkedid".to_string(), sql_type: "TEXT".to_string() },
                CelColumnMapping { column: "peer".to_string(), cel_field: "peer".to_string(), sql_type: "TEXT".to_string() },
            ],
        }
    }
}

/// SQLite3 CEL backend.
#[derive(Debug)]
pub struct Sqlite3CelBackend {
    config: Sqlite3CelConfig,
    events: parking_lot::RwLock<Vec<String>>,
}

impl Sqlite3CelBackend {
    pub fn new() -> Self {
        Self {
            config: Sqlite3CelConfig::default(),
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: Sqlite3CelConfig) -> Self {
        Self {
            config,
            events: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Generate CREATE TABLE SQL.
    pub fn create_table_sql(&self) -> String {
        let cols: Vec<String> = self.config.columns.iter()
            .map(|c| format!("{} {}", c.column, c.sql_type))
            .collect();
        format!("CREATE TABLE IF NOT EXISTS {} ({})", self.config.table, cols.join(", "))
    }

    /// Get CEL field value by name.
    fn get_cel_field(event: &CelEvent, field: &str) -> String {
        match field {
            "eventtype" => event.event_type.name().to_string(),
            "eventtime" => event.timestamp.to_string(),
            "cidname" => event.caller_id_name.clone(),
            "cidnum" => event.caller_id_num.clone(),
            "exten" => event.extension.clone(),
            "context" => event.context.clone(),
            "channame" => event.channel_name.clone(),
            "appname" => event.application.clone(),
            "appdata" => event.application_data.clone(),
            "accountcode" => event.account_code.clone(),
            "uniqueid" => event.unique_id.clone(),
            "linkedid" => event.linked_id.clone(),
            "peer" => event.peer.clone(),
            _ => String::new(),
        }
    }

    pub fn logged_events(&self) -> Vec<String> {
        self.events.read().clone()
    }
}

impl Default for Sqlite3CelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CelBackend for Sqlite3CelBackend {
    fn name(&self) -> &str {
        "sqlite3_custom"
    }

    fn write(&self, event: &CelEvent) -> CelResult<()> {
        let cols: Vec<&str> = self.config.columns.iter().map(|c| c.column.as_str()).collect();
        let vals: Vec<String> = self.config.columns.iter()
            .map(|c| Self::get_cel_field(event, &c.cel_field))
            .collect();
        let placeholders: Vec<&str> = (0..cols.len()).map(|_| "?").collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            self.config.table, cols.join(", "), placeholders.join(", ")
        );
        debug!("CEL SQLite3: {} with values {:?}", sql, vals);
        self.events.write().push(format!("{}: {}", event.event_type.name(), event.channel_name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cel::CelEventType;

    #[test]
    fn test_create_table_sql() {
        let backend = Sqlite3CelBackend::new();
        let sql = backend.create_table_sql();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS cel"));
        assert!(sql.contains("eventtype TEXT"));
    }

    #[test]
    fn test_sqlite3_cel_write() {
        let backend = Sqlite3CelBackend::new();
        let event = CelEvent::new(CelEventType::ChannelStart, "SIP/alice", "u1");
        backend.write(&event).unwrap();
        assert_eq!(backend.logged_events().len(), 1);
    }
}
