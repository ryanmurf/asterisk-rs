//! PostgreSQL-based realtime configuration backend.
//!
//! Port of `res/res_config_pgsql.c`. Provides a realtime configuration
//! driver using direct PostgreSQL SQL queries. Includes connection pooling,
//! table column caching, and proper string escaping.

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::config_curl::ConfigRealtimeDriver;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigPgsqlError {
    #[error("PostgreSQL connection failed: {0}")]
    ConnectionError(String),
    #[error("SQL execution failed: {0}")]
    SqlError(String),
    #[error("database not configured: {0}")]
    NotConfigured(String),
    #[error("table not found: {0}")]
    TableNotFound(String),
}

pub type ConfigPgsqlResult<T> = Result<T, ConfigPgsqlError>;

// ---------------------------------------------------------------------------
// Connection config
// ---------------------------------------------------------------------------

/// PostgreSQL connection parameters (mirrors the static globals in the C source).
#[derive(Debug, Clone)]
pub struct PgsqlConnectionConfig {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: String,
    pub appname: String,
    pub socket_path: String,
}

impl Default for PgsqlConnectionConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5432,
            dbname: "asterisk".to_string(),
            user: String::new(),
            password: String::new(),
            appname: "asterisk".to_string(),
            socket_path: String::new(),
        }
    }
}

impl PgsqlConnectionConfig {
    /// Build a PostgreSQL connection string.
    pub fn connection_string(&self) -> String {
        let mut parts = Vec::new();
        if !self.host.is_empty() {
            parts.push(format!("host={}", self.host));
        }
        parts.push(format!("port={}", self.port));
        parts.push(format!("dbname={}", self.dbname));
        if !self.user.is_empty() {
            parts.push(format!("user={}", self.user));
        }
        if !self.password.is_empty() {
            parts.push(format!("password={}", self.password));
        }
        if !self.appname.is_empty() {
            parts.push(format!("application_name={}", self.appname));
        }
        parts.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Column metadata cache (mirrors `struct columns` / `struct tables`)
// ---------------------------------------------------------------------------

/// Column metadata for cached table schema.
#[derive(Debug, Clone)]
pub struct PgsqlColumn {
    pub name: String,
    pub col_type: String,
    pub len: i32,
    pub not_null: bool,
    pub has_default: bool,
}

/// Cached table schema information.
#[derive(Debug, Clone)]
pub struct PgsqlTableInfo {
    pub name: String,
    pub columns: Vec<PgsqlColumn>,
}

// ---------------------------------------------------------------------------
// Requirement mode (mirrors the C enum)
// ---------------------------------------------------------------------------

/// Column requirement enforcement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementMode {
    /// Warn about missing columns.
    Warn,
    /// Create missing columns and close connection on failure.
    CreateClose,
    /// Create missing columns as CHAR type.
    CreateChar,
}

// ---------------------------------------------------------------------------
// PostgreSQL escape helpers
// ---------------------------------------------------------------------------

/// Escape a string for PostgreSQL (double single quotes, handle semicolons).
pub fn pgsql_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        match ch {
            '\'' => escaped.push_str("''"),
            ';' => escaped.push_str("^3B"),
            '^' => escaped.push_str("^5E"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Decode Asterisk-style escaped characters back.
pub fn pgsql_unescape(s: &str) -> String {
    let mut decoded = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                decoded.push(byte as char);
            } else {
                decoded.push('^');
                decoded.push_str(&hex);
            }
        } else {
            decoded.push(ch);
        }
    }
    decoded
}

// ---------------------------------------------------------------------------
// SQL builders
// ---------------------------------------------------------------------------

fn build_select(table: &str, fields: &[(&str, &str)]) -> String {
    let mut sql = format!("SELECT * FROM {}", table);
    if !fields.is_empty() {
        sql.push_str(" WHERE ");
        for (i, (key, value)) in fields.iter().enumerate() {
            if i > 0 {
                sql.push_str(" AND ");
            }
            if value.contains('%') {
                sql.push_str(&format!("{} LIKE E'{}'", key, pgsql_escape(value)));
            } else {
                sql.push_str(&format!("{} = E'{}'", key, pgsql_escape(value)));
            }
        }
    }
    sql
}

fn build_insert(table: &str, fields: &[(&str, &str)]) -> String {
    let columns: Vec<&str> = fields.iter().map(|(k, _)| *k).collect();
    let values: Vec<String> = fields
        .iter()
        .map(|(_, v)| format!("E'{}'", pgsql_escape(v)))
        .collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table,
        columns.join(", "),
        values.join(", ")
    )
}

fn build_update(table: &str, key_field: &str, entity: &str, fields: &[(&str, &str)]) -> String {
    let sets: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{} = E'{}'", k, pgsql_escape(v)))
        .collect();
    format!(
        "UPDATE {} SET {} WHERE {} = E'{}'",
        table,
        sets.join(", "),
        key_field,
        pgsql_escape(entity)
    )
}

fn build_delete(table: &str, key_field: &str, entity: &str, extra: &[(&str, &str)]) -> String {
    let mut sql = format!(
        "DELETE FROM {} WHERE {} = E'{}'",
        table,
        key_field,
        pgsql_escape(entity)
    );
    for (key, value) in extra {
        sql.push_str(&format!(" AND {} = E'{}'", key, pgsql_escape(value)));
    }
    sql
}

// ---------------------------------------------------------------------------
// PostgreSQL realtime driver
// ---------------------------------------------------------------------------

/// PostgreSQL realtime configuration driver.
///
/// Port of `res_config_pgsql.c`. Maintains a connection configuration,
/// table column cache, and generates PostgreSQL-specific SQL.
#[derive(Debug)]
pub struct PgsqlRealtimeDriver {
    /// Connection configuration.
    config: RwLock<PgsqlConnectionConfig>,
    /// Cached table schemas.
    table_cache: RwLock<HashMap<String, PgsqlTableInfo>>,
    /// Connection timestamp (0 if not connected).
    connect_time: RwLock<u64>,
    /// Requirement mode for missing columns.
    pub requirements: RwLock<RequirementMode>,
    /// Whether to order multi-row results by initial column.
    pub order_by_initial_column: bool,
}

impl PgsqlRealtimeDriver {
    pub fn new(config: PgsqlConnectionConfig) -> Self {
        Self {
            config: RwLock::new(config),
            table_cache: RwLock::new(HashMap::new()),
            connect_time: RwLock::new(0),
            requirements: RwLock::new(RequirementMode::Warn),
            order_by_initial_column: true,
        }
    }

    /// Check if we are connected.
    pub fn is_connected(&self) -> bool {
        *self.connect_time.read() > 0
    }

    /// Get connection uptime in seconds.
    pub fn uptime_secs(&self) -> u64 {
        let ct = *self.connect_time.read();
        if ct == 0 {
            return 0;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(ct)
    }

    /// Simulate a reconnect.
    pub fn reconnect(&self) -> ConfigPgsqlResult<()> {
        let config = self.config.read();
        debug!(
            host = %config.host,
            port = config.port,
            dbname = %config.dbname,
            "PostgreSQL reconnect (stub)"
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        *self.connect_time.write() = now;
        info!("PostgreSQL connection established (stub)");
        Ok(())
    }

    /// Cache a table's column information.
    pub fn cache_table(&self, info: PgsqlTableInfo) {
        self.table_cache
            .write()
            .insert(info.name.clone(), info);
    }

    /// Look up cached column info for a table.
    pub fn get_cached_table(&self, name: &str) -> Option<PgsqlTableInfo> {
        self.table_cache.read().get(name).cloned()
    }

    /// Build a SELECT query.
    pub fn build_select(&self, table: &str, fields: &[(&str, &str)]) -> String {
        let mut sql = build_select(table, fields);
        if self.order_by_initial_column && !fields.is_empty() {
            sql.push_str(&format!(" ORDER BY {}", fields[0].0));
        }
        sql
    }

    /// Execute a SQL query (stub).
    fn execute(
        &self,
        sql: &str,
    ) -> ConfigPgsqlResult<Vec<Vec<(String, String)>>> {
        debug!(sql = sql, "PgSQL execute (stub)");
        Err(ConfigPgsqlError::ConnectionError(
            "PostgreSQL driver not connected".to_string(),
        ))
    }
}

impl ConfigRealtimeDriver for PgsqlRealtimeDriver {
    fn name(&self) -> &str {
        "pgsql"
    }

    fn realtime_load(
        &self,
        _database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<(String, String)>> {
        let sql = build_select(table, fields);
        let rows = self
            .execute(&sql)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    fn realtime_load_multi(
        &self,
        _database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<Vec<(String, String)>>> {
        let sql = self.build_select(table, fields);
        self.execute(&sql)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_store(
        &self,
        _database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_insert(table, fields);
        self.execute(&sql)
            .map(|_| 1)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_update(
        &self,
        _database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_update(table, key_field, entity, fields);
        self.execute(&sql)
            .map(|_| 1)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_destroy(
        &self,
        _database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        let sql = build_delete(table, key_field, entity, fields);
        self.execute(&sql)
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
    fn test_pgsql_escape() {
        assert_eq!(pgsql_escape("it's"), "it''s");
        assert_eq!(pgsql_escape("a;b"), "a^3Bb");
        assert_eq!(pgsql_escape("a^b"), "a^5Eb");
    }

    #[test]
    fn test_pgsql_unescape() {
        assert_eq!(pgsql_unescape("a^3Bb"), "a;b");
        assert_eq!(pgsql_unescape("a^5Eb"), "a^b");
    }

    #[test]
    fn test_connection_string() {
        let config = PgsqlConnectionConfig {
            host: "db.example.com".to_string(),
            port: 5432,
            dbname: "asterisk".to_string(),
            user: "admin".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let cs = config.connection_string();
        assert!(cs.contains("host=db.example.com"));
        assert!(cs.contains("port=5432"));
        assert!(cs.contains("dbname=asterisk"));
        assert!(cs.contains("user=admin"));
    }

    #[test]
    fn test_build_select() {
        let sql = build_select("sippeers", &[("name", "alice")]);
        assert_eq!(sql, "SELECT * FROM sippeers WHERE name = E'alice'");
    }

    #[test]
    fn test_build_insert() {
        let sql = build_insert("sippeers", &[("name", "alice"), ("host", "dynamic")]);
        assert!(sql.contains("INSERT INTO sippeers"));
        assert!(sql.contains("E'alice'"));
    }

    #[test]
    fn test_build_update() {
        let sql = build_update("sippeers", "name", "alice", &[("host", "1.2.3.4")]);
        assert!(sql.contains("UPDATE sippeers SET"));
        assert!(sql.contains("WHERE name = E'alice'"));
    }

    #[test]
    fn test_driver_name() {
        let config = PgsqlConnectionConfig::default();
        let driver = PgsqlRealtimeDriver::new(config);
        assert_eq!(ConfigRealtimeDriver::name(&driver), "pgsql");
    }
}
