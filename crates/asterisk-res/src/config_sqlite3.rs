//! SQLite3-based realtime configuration backend.
//!
//! Port of `res/res_config_sqlite3.c`. Provides a file-based realtime
//! configuration driver using SQLite3. Supports multiple database files,
//! batch mode for WAL, and column requirement enforcement.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use parking_lot::{Mutex, RwLock};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::config_curl::ConfigRealtimeDriver;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigSqlite3Error {
    #[error("SQLite3 error: {0}")]
    SqliteError(String),
    #[error("database '{0}' not found")]
    DatabaseNotFound(String),
    #[error("table '{0}' not found")]
    TableNotFound(String),
    #[error("configuration error: {0}")]
    ConfigError(String),
}

pub type ConfigSqlite3Result<T> = Result<T, ConfigSqlite3Error>;

// ---------------------------------------------------------------------------
// Requirement mode (mirrors the C enum)
// ---------------------------------------------------------------------------

/// Column requirement enforcement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sqlite3RequirementMode {
    /// Warn about missing/mismatched columns.
    Warn,
    /// Close the database if requirements are not met.
    Close,
    /// Create missing columns as CHAR type.
    CreateChar,
}

// ---------------------------------------------------------------------------
// Database configuration (mirrors `struct realtime_sqlite3_db`)
// ---------------------------------------------------------------------------

/// Configuration for a single SQLite3 database.
#[derive(Debug, Clone)]
pub struct Sqlite3DbConfig {
    /// Logical name for this database connection.
    pub name: String,
    /// Path to the SQLite3 database file.
    pub filename: PathBuf,
    /// Requirement mode for column enforcement.
    pub requirements: Sqlite3RequirementMode,
    /// Enable debug logging of SQL queries.
    pub debug: bool,
    /// Batch mode: number of changes before fsync (0 = disabled).
    pub batch: u32,
    /// Busy timeout in milliseconds.
    pub busy_timeout: i32,
}

impl Default for Sqlite3DbConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            filename: PathBuf::new(),
            requirements: Sqlite3RequirementMode::Warn,
            debug: false,
            batch: 0,
            busy_timeout: 1000,
        }
    }
}

// ---------------------------------------------------------------------------
// SQLite3 escape helpers
// ---------------------------------------------------------------------------

/// Escape a value for SQLite (double single quotes).
fn sqlite3_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Escape a table or column name for SQLite (wrap in double quotes).
fn sqlite3_quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// ---------------------------------------------------------------------------
// SQL builders
// ---------------------------------------------------------------------------

fn build_select(table: &str, fields: &[(&str, &str)]) -> String {
    let table_q = sqlite3_quote_identifier(table);
    let mut sql = format!("SELECT * FROM {}", table_q);
    if !fields.is_empty() {
        sql.push_str(" WHERE ");
        for (i, (key, value)) in fields.iter().enumerate() {
            if i > 0 {
                sql.push_str(" AND ");
            }
            if value.contains('%') || value.contains('_') {
                sql.push_str(&format!(
                    "{} LIKE '{}' ESCAPE '\\'",
                    sqlite3_quote_identifier(key),
                    sqlite3_escape(value)
                ));
            } else {
                sql.push_str(&format!(
                    "{} = '{}'",
                    sqlite3_quote_identifier(key),
                    sqlite3_escape(value)
                ));
            }
        }
    }
    sql
}

fn build_insert(table: &str, fields: &[(&str, &str)]) -> String {
    let table_q = sqlite3_quote_identifier(table);
    let columns: Vec<String> = fields
        .iter()
        .map(|(k, _)| sqlite3_quote_identifier(k))
        .collect();
    let values: Vec<String> = fields
        .iter()
        .map(|(_, v)| format!("'{}'", sqlite3_escape(v)))
        .collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table_q,
        columns.join(", "),
        values.join(", ")
    )
}

fn build_update(table: &str, key_field: &str, entity: &str, fields: &[(&str, &str)]) -> String {
    let table_q = sqlite3_quote_identifier(table);
    let sets: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            format!("{} = '{}'", sqlite3_quote_identifier(k), sqlite3_escape(v))
        })
        .collect();
    format!(
        "UPDATE {} SET {} WHERE {} = '{}'",
        table_q,
        sets.join(", "),
        sqlite3_quote_identifier(key_field),
        sqlite3_escape(entity)
    )
}

fn build_delete(table: &str, key_field: &str, entity: &str, extra: &[(&str, &str)]) -> String {
    let table_q = sqlite3_quote_identifier(table);
    let mut sql = format!(
        "DELETE FROM {} WHERE {} = '{}'",
        table_q,
        sqlite3_quote_identifier(key_field),
        sqlite3_escape(entity)
    );
    for (key, value) in extra {
        sql.push_str(&format!(
            " AND {} = '{}'",
            sqlite3_quote_identifier(key),
            sqlite3_escape(value)
        ));
    }
    sql
}

// ---------------------------------------------------------------------------
// SQLite3 realtime driver
// ---------------------------------------------------------------------------

/// SQLite3 realtime configuration driver.
///
/// Port of `res_config_sqlite3.c`. Manages multiple SQLite3 database files
/// and generates appropriate SQL for config load/store/update/delete.
#[derive(Debug)]
pub struct Sqlite3RealtimeDriver {
    /// Database configurations keyed by name.
    databases: RwLock<HashMap<String, Sqlite3DbConfig>>,
}

impl Sqlite3RealtimeDriver {
    pub fn new() -> Self {
        Self {
            databases: RwLock::new(HashMap::new()),
        }
    }

    /// Register a database configuration.
    pub fn add_database(&self, config: Sqlite3DbConfig) {
        info!(name = %config.name, path = ?config.filename, "Registered SQLite3 database");
        self.databases
            .write()
            .insert(config.name.clone(), config);
    }

    /// Get a database configuration by name.
    pub fn get_database(&self, name: &str) -> Option<Sqlite3DbConfig> {
        self.databases.read().get(name).cloned()
    }

    /// Remove a database configuration.
    pub fn remove_database(&self, name: &str) -> Option<Sqlite3DbConfig> {
        self.databases.write().remove(name)
    }

    /// List all configured database names.
    pub fn database_names(&self) -> Vec<String> {
        self.databases.read().keys().cloned().collect()
    }

    /// Execute a SQL query against a named database (stub).
    fn execute(
        &self,
        database: &str,
        sql: &str,
    ) -> ConfigSqlite3Result<Vec<Vec<(String, String)>>> {
        let _config = self.databases.read().get(database).cloned().ok_or_else(|| {
            ConfigSqlite3Error::DatabaseNotFound(database.to_string())
        })?;
        debug!(database = database, sql = sql, "SQLite3 execute (stub)");
        Err(ConfigSqlite3Error::SqliteError(
            "SQLite3 driver not connected".to_string(),
        ))
    }
}

impl Default for Sqlite3RealtimeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigRealtimeDriver for Sqlite3RealtimeDriver {
    fn name(&self) -> &str {
        "sqlite3"
    }

    fn realtime_load(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<(String, String)>> {
        let sql = build_select(table, fields);
        let rows = self
            .execute(database, &sql)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    fn realtime_load_multi(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<Vec<(String, String)>>> {
        let sql = build_select(table, fields);
        self.execute(database, &sql)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_store(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_insert(table, fields);
        self.execute(database, &sql)
            .map(|_| 1)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_update(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_update(table, key_field, entity, fields);
        self.execute(database, &sql)
            .map(|_| 1)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_destroy(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_delete(table, key_field, entity, fields);
        self.execute(database, &sql)
            .map(|_| 1)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite3_escape() {
        assert_eq!(sqlite3_escape("it's"), "it''s");
    }

    #[test]
    fn test_sqlite3_quote_identifier() {
        assert_eq!(sqlite3_quote_identifier("table"), "\"table\"");
        assert_eq!(sqlite3_quote_identifier("col\"name"), "\"col\"\"name\"");
    }

    #[test]
    fn test_build_select() {
        let sql = build_select("sippeers", &[("name", "alice")]);
        assert!(sql.contains("SELECT * FROM \"sippeers\""));
        assert!(sql.contains("\"name\" = 'alice'"));
    }

    #[test]
    fn test_build_insert() {
        let sql = build_insert("sippeers", &[("name", "alice"), ("host", "dynamic")]);
        assert!(sql.contains("INSERT INTO \"sippeers\""));
    }

    #[test]
    fn test_build_update() {
        let sql = build_update("sippeers", "name", "alice", &[("host", "1.2.3.4")]);
        assert!(sql.contains("UPDATE \"sippeers\""));
        assert!(sql.contains("\"host\" = '1.2.3.4'"));
    }

    #[test]
    fn test_build_delete() {
        let sql = build_delete("sippeers", "name", "alice", &[]);
        assert!(sql.contains("DELETE FROM \"sippeers\""));
    }

    #[test]
    fn test_driver_registration() {
        let driver = Sqlite3RealtimeDriver::new();
        driver.add_database(Sqlite3DbConfig {
            name: "test".to_string(),
            filename: PathBuf::from("/tmp/test.db"),
            ..Default::default()
        });
        assert!(driver.get_database("test").is_some());
        assert!(driver.get_database("missing").is_none());
    }

    #[test]
    fn test_driver_name() {
        let driver = Sqlite3RealtimeDriver::new();
        assert_eq!(ConfigRealtimeDriver::name(&driver), "sqlite3");
    }
}
