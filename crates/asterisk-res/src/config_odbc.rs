//! ODBC-based realtime configuration backend.
//!
//! Port of `res/res_config_odbc.c`. Provides a realtime configuration
//! driver that executes SQL queries via ODBC to load, store, update,
//! and delete configuration rows from a database.

use std::collections::HashMap;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::debug;

use crate::config_curl::ConfigRealtimeDriver;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigOdbcError {
    #[error("ODBC connection not available: {0}")]
    ConnectionError(String),
    #[error("SQL execution failed: {0}")]
    SqlError(String),
    #[error("DSN not configured for database '{0}'")]
    DsnNotConfigured(String),
    #[error("config ODBC error: {0}")]
    Other(String),
}

pub type ConfigOdbcResult<T> = Result<T, ConfigOdbcError>;

// ---------------------------------------------------------------------------
// SQL query builder helpers
// ---------------------------------------------------------------------------

/// Escape a value for safe inclusion in SQL (basic quoting).
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Build a WHERE clause from field pairs.
fn build_where_clause(fields: &[(&str, &str)]) -> String {
    let mut clause = String::new();
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            clause.push_str(" AND ");
        }
        // Handle LIKE operators (field LIKE 'value%')
        if value.contains('%') {
            clause.push_str(&format!("{} LIKE '{}'", key, sql_escape(value)));
        } else {
            clause.push_str(&format!("{} = '{}'", key, sql_escape(value)));
        }
    }
    clause
}

/// Build an INSERT statement.
fn build_insert(table: &str, fields: &[(&str, &str)]) -> String {
    let columns: Vec<&str> = fields.iter().map(|(k, _)| *k).collect();
    let values: Vec<String> = fields
        .iter()
        .map(|(_, v)| format!("'{}'", sql_escape(v)))
        .collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table,
        columns.join(", "),
        values.join(", ")
    )
}

/// Build an UPDATE statement.
fn build_update(
    table: &str,
    key_field: &str,
    entity: &str,
    fields: &[(&str, &str)],
) -> String {
    let sets: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{} = '{}'", k, sql_escape(v)))
        .collect();
    format!(
        "UPDATE {} SET {} WHERE {} = '{}'",
        table,
        sets.join(", "),
        key_field,
        sql_escape(entity)
    )
}

/// Build a DELETE statement.
fn build_delete(
    table: &str,
    key_field: &str,
    entity: &str,
    extra_fields: &[(&str, &str)],
) -> String {
    let mut where_clause = format!("{} = '{}'", key_field, sql_escape(entity));
    if !extra_fields.is_empty() {
        where_clause.push_str(" AND ");
        where_clause.push_str(&build_where_clause(extra_fields));
    }
    format!("DELETE FROM {} WHERE {}", table, where_clause)
}

// ---------------------------------------------------------------------------
// Prepared statement cache
// ---------------------------------------------------------------------------

/// Represents a cached prepared statement (placeholder for actual ODBC stmt).
#[derive(Debug, Clone)]
pub struct PreparedStatement {
    /// The SQL template with `?` placeholders.
    pub sql: String,
    /// Number of parameters expected.
    pub param_count: usize,
}

impl PreparedStatement {
    pub fn new(sql: &str) -> Self {
        let param_count = sql.chars().filter(|c| *c == '?').count();
        Self {
            sql: sql.to_string(),
            param_count,
        }
    }
}

// ---------------------------------------------------------------------------
// ODBC connection config
// ---------------------------------------------------------------------------

/// Configuration for an ODBC connection.
#[derive(Debug, Clone)]
pub struct OdbcConnectionConfig {
    /// DSN name (Data Source Name).
    pub dsn: String,
    /// Username for connection.
    pub username: String,
    /// Password for connection.
    pub password: String,
    /// Whether to pre-connect at startup.
    pub pre_connect: bool,
    /// Maximum number of connections in the pool.
    pub max_connections: u32,
    /// Idle timeout in seconds.
    pub idle_timeout: u64,
}

impl Default for OdbcConnectionConfig {
    fn default() -> Self {
        Self {
            dsn: String::new(),
            username: String::new(),
            password: String::new(),
            pre_connect: true,
            max_connections: 1,
            idle_timeout: 600,
        }
    }
}

// ---------------------------------------------------------------------------
// ODBC realtime driver
// ---------------------------------------------------------------------------

/// Column metadata from the ODBC cache (mirrors `odbc_cache_columns`).
#[derive(Debug, Clone)]
pub struct OdbcColumnInfo {
    pub name: String,
    pub sql_type: i32,
    pub size: usize,
    pub nullable: bool,
}

/// ODBC-based realtime configuration driver.
///
/// Port of `res_config_odbc.c`. SQL queries are built from the table name
/// and field criteria, executed via ODBC. In this Rust port the actual ODBC
/// connection is stubbed; the SQL generation and interface are complete.
#[derive(Debug)]
#[allow(dead_code)]
pub struct OdbcRealtimeDriver {
    /// DSN configurations keyed by database name.
    dsn_map: RwLock<HashMap<String, OdbcConnectionConfig>>,
    /// Whether to order multi-row results by the initial column.
    pub order_by_initial_column: bool,
    /// Prepared statement cache.
    stmt_cache: RwLock<HashMap<String, PreparedStatement>>,
}

impl OdbcRealtimeDriver {
    /// Create a new ODBC realtime driver.
    pub fn new() -> Self {
        Self {
            dsn_map: RwLock::new(HashMap::new()),
            order_by_initial_column: true,
            stmt_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Register a DSN mapping for a database name.
    pub fn add_dsn_mapping(&self, database: &str, config: OdbcConnectionConfig) {
        self.dsn_map
            .write()
            .insert(database.to_string(), config);
    }

    /// Get the DSN for a database.
    fn get_dsn(&self, database: &str) -> Result<OdbcConnectionConfig, ConfigOdbcError> {
        self.dsn_map
            .read()
            .get(database)
            .cloned()
            .ok_or_else(|| ConfigOdbcError::DsnNotConfigured(database.to_string()))
    }

    /// Build a SELECT query for single-row retrieval.
    pub fn build_select_query(
        &self,
        table: &str,
        fields: &[(&str, &str)],
    ) -> String {
        let where_clause = build_where_clause(fields);
        if where_clause.is_empty() {
            format!("SELECT * FROM {}", table)
        } else {
            format!("SELECT * FROM {} WHERE {}", table, where_clause)
        }
    }

    /// Build a SELECT query for multi-row retrieval.
    pub fn build_select_multi_query(
        &self,
        table: &str,
        fields: &[(&str, &str)],
    ) -> String {
        let base = self.build_select_query(table, fields);
        if self.order_by_initial_column && !fields.is_empty() {
            format!("{} ORDER BY {}", base, fields[0].0)
        } else {
            base
        }
    }

    /// Execute a SQL query (stub).
    ///
    /// In a full implementation this would use an ODBC driver manager.
    fn execute_query(
        &self,
        _dsn: &OdbcConnectionConfig,
        sql: &str,
    ) -> Result<Vec<Vec<(String, String)>>, ConfigOdbcError> {
        debug!(sql = sql, "ODBC execute (stub)");
        Err(ConfigOdbcError::ConnectionError(
            "ODBC driver not connected".to_string(),
        ))
    }
}

impl Default for OdbcRealtimeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigRealtimeDriver for OdbcRealtimeDriver {
    fn name(&self) -> &str {
        "odbc"
    }

    fn realtime_load(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<(String, String)>> {
        let dsn = self.get_dsn(database).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        let sql = self.build_select_query(table, fields);
        let rows = self.execute_query(&dsn, &sql).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    fn realtime_load_multi(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<Vec<(String, String)>>> {
        let dsn = self.get_dsn(database).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        let sql = self.build_select_multi_query(table, fields);
        self.execute_query(&dsn, &sql).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })
    }

    fn realtime_store(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let dsn = self.get_dsn(database).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        let sql = build_insert(table, fields);
        self.execute_query(&dsn, &sql)
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
        let dsn = self.get_dsn(database).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        let sql = build_update(table, key_field, entity, fields);
        self.execute_query(&dsn, &sql)
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
        let dsn = self.get_dsn(database).map_err(|e| {
            crate::config_curl::ConfigCurlError::Other(e.to_string())
        })?;
        let sql = build_delete(table, key_field, entity, fields);
        self.execute_query(&dsn, &sql)
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
    fn test_sql_escape() {
        assert_eq!(sql_escape("it's"), "it''s");
        assert_eq!(sql_escape("normal"), "normal");
    }

    #[test]
    fn test_build_where_clause() {
        let clause = build_where_clause(&[("name", "Alice"), ("context", "default")]);
        assert_eq!(clause, "name = 'Alice' AND context = 'default'");
    }

    #[test]
    fn test_build_where_clause_like() {
        let clause = build_where_clause(&[("name", "%Ali%")]);
        assert_eq!(clause, "name LIKE '%Ali%'");
    }

    #[test]
    fn test_build_insert() {
        let sql = build_insert("sippeers", &[("name", "alice"), ("host", "dynamic")]);
        assert_eq!(
            sql,
            "INSERT INTO sippeers (name, host) VALUES ('alice', 'dynamic')"
        );
    }

    #[test]
    fn test_build_update() {
        let sql = build_update("sippeers", "name", "alice", &[("host", "192.168.1.1")]);
        assert_eq!(
            sql,
            "UPDATE sippeers SET host = '192.168.1.1' WHERE name = 'alice'"
        );
    }

    #[test]
    fn test_build_delete() {
        let sql = build_delete("sippeers", "name", "alice", &[]);
        assert_eq!(sql, "DELETE FROM sippeers WHERE name = 'alice'");
    }

    #[test]
    fn test_build_select_query() {
        let driver = OdbcRealtimeDriver::new();
        let sql = driver.build_select_query("sippeers", &[("name", "alice")]);
        assert_eq!(sql, "SELECT * FROM sippeers WHERE name = 'alice'");
    }

    #[test]
    fn test_build_select_multi_query_ordering() {
        let driver = OdbcRealtimeDriver::new();
        let sql = driver.build_select_multi_query("sippeers", &[("context", "default")]);
        assert!(sql.contains("ORDER BY context"));
    }

    #[test]
    fn test_prepared_statement() {
        let stmt = PreparedStatement::new("SELECT * FROM ? WHERE name = ?");
        assert_eq!(stmt.param_count, 2);
    }

    #[test]
    fn test_driver_name() {
        let driver = OdbcRealtimeDriver::new();
        assert_eq!(ConfigRealtimeDriver::name(&driver), "odbc");
    }
}
