//! SQLite3 CDR backend.
//!
//! Port of cdr/cdr_sqlite3_custom.c from Asterisk C.
//!
//! Writes CDR records to a SQLite3 database with configurable
//! table and column mappings. The actual SQLite calls are stubbed
//! out with a trait interface so a real SQLite implementation can
//! be plugged in.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// Column mapping: maps a column name to a CDR field.
#[derive(Debug, Clone)]
pub struct ColumnMapping {
    /// Database column name
    pub column: String,
    /// CDR field name (e.g., "src", "dst", "duration")
    pub cdr_field: String,
    /// SQL type for auto-create (e.g., "TEXT", "INTEGER")
    pub sql_type: String,
}

/// Configuration for the SQLite3 CDR backend.
#[derive(Debug, Clone)]
pub struct Sqlite3CdrConfig {
    /// Path to the SQLite database file
    pub db_path: String,
    /// Table name
    pub table: String,
    /// Column mappings
    pub columns: Vec<ColumnMapping>,
    /// Whether to auto-create the table if it doesn't exist
    pub auto_create: bool,
}

impl Default for Sqlite3CdrConfig {
    fn default() -> Self {
        Self {
            db_path: "/var/log/asterisk/cdr.db".to_string(),
            table: "cdr".to_string(),
            columns: vec![
                ColumnMapping {
                    column: "accountcode".to_string(),
                    cdr_field: "accountcode".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "src".to_string(),
                    cdr_field: "src".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "dst".to_string(),
                    cdr_field: "dst".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "dcontext".to_string(),
                    cdr_field: "dcontext".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "clid".to_string(),
                    cdr_field: "clid".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "channel".to_string(),
                    cdr_field: "channel".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "dstchannel".to_string(),
                    cdr_field: "dstchannel".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "lastapp".to_string(),
                    cdr_field: "lastapp".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "lastdata".to_string(),
                    cdr_field: "lastdata".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "duration".to_string(),
                    cdr_field: "duration".to_string(),
                    sql_type: "INTEGER".to_string(),
                },
                ColumnMapping {
                    column: "billsec".to_string(),
                    cdr_field: "billsec".to_string(),
                    sql_type: "INTEGER".to_string(),
                },
                ColumnMapping {
                    column: "disposition".to_string(),
                    cdr_field: "disposition".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "amaflags".to_string(),
                    cdr_field: "amaflags".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "uniqueid".to_string(),
                    cdr_field: "uniqueid".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "userfield".to_string(),
                    cdr_field: "userfield".to_string(),
                    sql_type: "TEXT".to_string(),
                },
            ],
            auto_create: true,
        }
    }
}

/// Trait for SQLite database operations.
///
/// This allows the backend to be used with different SQLite
/// implementations or mocked for testing.
pub trait SqliteConnection: Send + Sync {
    /// Execute a SQL statement with no return value.
    fn execute(&self, sql: &str, params: &[&str]) -> Result<(), String>;
}

/// In-memory mock SQLite connection for testing.
#[derive(Debug, Default)]
pub struct MockSqliteConnection {
    /// Log of executed SQL statements
    pub executed: parking_lot::Mutex<Vec<(String, Vec<String>)>>,
}

impl MockSqliteConnection {
    pub fn new() -> Self {
        Self {
            executed: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Get all executed statements.
    pub fn statements(&self) -> Vec<(String, Vec<String>)> {
        self.executed.lock().clone()
    }
}

impl SqliteConnection for MockSqliteConnection {
    fn execute(&self, sql: &str, params: &[&str]) -> Result<(), String> {
        self.executed.lock().push((
            sql.to_string(),
            params.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(())
    }
}

/// SQLite3 CDR backend.
///
/// Writes CDR records to a SQLite database using configurable
/// column mappings. Supports auto-creating the table schema.
pub struct Sqlite3CdrBackend {
    config: Sqlite3CdrConfig,
    connection: Box<dyn SqliteConnection>,
    initialized: parking_lot::Mutex<bool>,
}

impl Sqlite3CdrBackend {
    /// Create a new SQLite3 CDR backend with a mock connection.
    pub fn new_mock() -> Self {
        Self {
            config: Sqlite3CdrConfig::default(),
            connection: Box::new(MockSqliteConnection::new()),
            initialized: parking_lot::Mutex::new(false),
        }
    }

    /// Create with a specific config and connection.
    pub fn with_config(config: Sqlite3CdrConfig, connection: Box<dyn SqliteConnection>) -> Self {
        Self {
            config,
            connection,
            initialized: parking_lot::Mutex::new(false),
        }
    }

    /// Generate CREATE TABLE SQL.
    pub fn create_table_sql(&self) -> String {
        let columns: Vec<String> = self
            .config
            .columns
            .iter()
            .map(|c| format!("{} {}", c.column, c.sql_type))
            .collect();
        format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            self.config.table,
            columns.join(", ")
        )
    }

    /// Generate INSERT SQL.
    fn insert_sql(&self) -> String {
        let columns: Vec<&str> = self.config.columns.iter().map(|c| c.column.as_str()).collect();
        let placeholders: Vec<&str> = (0..columns.len()).map(|_| "?").collect();
        format!(
            "INSERT INTO {} ({}) VALUES ({})",
            self.config.table,
            columns.join(", "),
            placeholders.join(", ")
        )
    }

    /// Get CDR field value by name.
    fn get_cdr_field(cdr: &Cdr, field: &str) -> String {
        match field.to_lowercase().as_str() {
            "accountcode" => cdr.account_code.clone(),
            "src" | "source" => cdr.src.clone(),
            "dst" | "destination" => cdr.dst.clone(),
            "dcontext" | "dstcontext" => cdr.dst_context.clone(),
            "clid" | "callerid" => cdr.caller_id.clone(),
            "channel" => cdr.channel.clone(),
            "dstchannel" => cdr.dst_channel.clone(),
            "lastapp" => cdr.last_app.clone(),
            "lastdata" => cdr.last_data.clone(),
            "duration" => cdr.duration.to_string(),
            "billsec" => cdr.billsec.to_string(),
            "disposition" => cdr.disposition.as_str().to_string(),
            "amaflags" => cdr.ama_flags.as_str().to_string(),
            "uniqueid" => cdr.unique_id.clone(),
            "userfield" => cdr.user_field.clone(),
            "linkedid" => cdr.linked_id.clone(),
            "peeraccount" => cdr.peer_account.clone(),
            "sequence" => cdr.sequence.to_string(),
            _ => cdr.variables.get(field).cloned().unwrap_or_default(),
        }
    }

    /// Ensure table exists (called on first log).
    fn ensure_table(&self) -> Result<(), CdrError> {
        let mut initialized = self.initialized.lock();
        if !*initialized && self.config.auto_create {
            let create_sql = self.create_table_sql();
            self.connection
                .execute(&create_sql, &[])
                .map_err(|e| CdrError::Backend(format!("Failed to create table: {}", e)))?;
            *initialized = true;
        }
        Ok(())
    }
}

impl CdrBackend for Sqlite3CdrBackend {
    fn name(&self) -> &str {
        "sqlite3_custom"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        self.ensure_table()?;

        let insert = self.insert_sql();
        let values: Vec<String> = self
            .config
            .columns
            .iter()
            .map(|c| Self::get_cdr_field(cdr, &c.cdr_field))
            .collect();
        let value_refs: Vec<&str> = values.iter().map(|s| s.as_str()).collect();

        debug!(
            "CDR SQLite3: inserting record for '{}' into '{}'",
            cdr.channel, self.config.table
        );

        self.connection
            .execute(&insert, &value_refs)
            .map_err(|e| CdrError::Backend(format!("SQLite insert failed: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_table_sql() {
        let backend = Sqlite3CdrBackend::new_mock();
        let sql = backend.create_table_sql();
        assert!(sql.starts_with("CREATE TABLE IF NOT EXISTS cdr"));
        assert!(sql.contains("src TEXT"));
        assert!(sql.contains("duration INTEGER"));
        assert!(sql.contains("uniqueid TEXT"));
    }

    #[test]
    fn test_insert_sql() {
        let backend = Sqlite3CdrBackend::new_mock();
        let sql = backend.insert_sql();
        assert!(sql.starts_with("INSERT INTO cdr"));
        assert!(sql.contains("src"));
        assert!(sql.contains("?"));
    }

    #[test]
    fn test_sqlite3_log() {
        let config = Sqlite3CdrConfig {
            columns: vec![
                ColumnMapping {
                    column: "src".to_string(),
                    cdr_field: "src".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "dst".to_string(),
                    cdr_field: "dst".to_string(),
                    sql_type: "TEXT".to_string(),
                },
                ColumnMapping {
                    column: "duration".to_string(),
                    cdr_field: "duration".to_string(),
                    sql_type: "INTEGER".to_string(),
                },
            ],
            ..Default::default()
        };

        // We need to create a new mock for the backend since it takes Box<dyn>
        let mock_for_backend = MockSqliteConnection::new();
        let backend = Sqlite3CdrBackend::with_config(config, Box::new(mock_for_backend));

        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid-1".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.duration = 60;

        backend.log(&cdr).unwrap();
    }

    #[test]
    fn test_default_config_columns() {
        let config = Sqlite3CdrConfig::default();
        assert_eq!(config.table, "cdr");
        assert!(!config.columns.is_empty());
        // Should have standard CDR columns
        let col_names: Vec<&str> = config.columns.iter().map(|c| c.column.as_str()).collect();
        assert!(col_names.contains(&"src"));
        assert!(col_names.contains(&"dst"));
        assert!(col_names.contains(&"duration"));
        assert!(col_names.contains(&"uniqueid"));
    }
}
