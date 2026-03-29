//! LDAP-based realtime configuration backend.
//!
//! Port of `res/res_config_ldap.c`. Provides a realtime configuration
//! driver that queries an LDAP directory for configuration data.
//! Supports attribute name mapping and LDAP search filters.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::config_curl::ConfigRealtimeDriver;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigLdapError {
    #[error("LDAP connection failed: {0}")]
    ConnectionError(String),
    #[error("LDAP search failed: {0}")]
    SearchError(String),
    #[error("LDAP modify failed: {0}")]
    ModifyError(String),
    #[error("table not configured: {0}")]
    TableNotConfigured(String),
}

pub type ConfigLdapResult<T> = Result<T, ConfigLdapError>;

// ---------------------------------------------------------------------------
// LDAP connection config
// ---------------------------------------------------------------------------

/// LDAP connection parameters (mirrors static globals in the C source).
#[derive(Debug, Clone)]
pub struct LdapConnectionConfig {
    /// LDAP server URL (e.g., `ldap://ldap.example.com`).
    pub url: String,
    /// Bind DN (user for authentication).
    pub bind_dn: String,
    /// Bind password.
    pub bind_password: String,
    /// Base DN for searches.
    pub base_dn: String,
    /// LDAP protocol version (2 or 3).
    pub version: i32,
}

impl Default for LdapConnectionConfig {
    fn default() -> Self {
        Self {
            url: "ldap://localhost".to_string(),
            bind_dn: String::new(),
            bind_password: String::new(),
            base_dn: "asterisk".to_string(),
            version: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Table configuration (mirrors `struct ldap_table_config`)
// ---------------------------------------------------------------------------

/// Configuration for how a realtime table maps to LDAP.
#[derive(Debug, Clone)]
pub struct LdapTableConfig {
    /// Table name (logical name used in extconfig.conf).
    pub table_name: String,
    /// Additional LDAP filter to append to searches.
    pub additional_filter: String,
    /// Attribute name mapping: realtime column name -> LDAP attribute name.
    pub attribute_map: HashMap<String, String>,
}

impl LdapTableConfig {
    pub fn new(table_name: &str) -> Self {
        Self {
            table_name: table_name.to_string(),
            additional_filter: String::new(),
            attribute_map: HashMap::new(),
        }
    }

    /// Map a realtime column name to an LDAP attribute name.
    pub fn map_attribute(&mut self, column: &str, ldap_attr: &str) {
        self.attribute_map
            .insert(column.to_string(), ldap_attr.to_string());
    }

    /// Get the LDAP attribute name for a realtime column.
    pub fn get_ldap_attr<'a>(&'a self, column: &'a str) -> &'a str {
        self.attribute_map
            .get(column)
            .map(|s| s.as_str())
            .unwrap_or(column)
    }
}

// ---------------------------------------------------------------------------
// LDAP filter builder
// ---------------------------------------------------------------------------

/// Escape a value for use in an LDAP search filter (RFC 4515).
pub fn ldap_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        match ch {
            '*' => escaped.push_str("\\2a"),
            '(' => escaped.push_str("\\28"),
            ')' => escaped.push_str("\\29"),
            '\\' => escaped.push_str("\\5c"),
            '\0' => escaped.push_str("\\00"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Build an LDAP search filter from field criteria.
fn build_ldap_filter(
    table_config: &LdapTableConfig,
    fields: &[(&str, &str)],
) -> String {
    let mut filter = String::from("(&");

    // Add per-field conditions.
    for (key, value) in fields {
        let attr = table_config.get_ldap_attr(key);
        filter.push_str(&format!("({}={})", attr, ldap_escape(value)));
    }

    // Add any additional filter from config.
    if !table_config.additional_filter.is_empty() {
        filter.push_str(&table_config.additional_filter);
    }

    filter.push(')');
    filter
}

// ---------------------------------------------------------------------------
// LDAP realtime driver
// ---------------------------------------------------------------------------

/// LDAP-based realtime configuration driver.
///
/// Port of `res_config_ldap.c`. Translates realtime queries into
/// LDAP searches with appropriate filter construction and attribute mapping.
#[derive(Debug)]
pub struct LdapRealtimeDriver {
    /// Connection configuration.
    config: RwLock<LdapConnectionConfig>,
    /// Table configurations keyed by table name.
    tables: RwLock<HashMap<String, LdapTableConfig>>,
    /// Whether currently connected.
    connected: RwLock<bool>,
}

impl LdapRealtimeDriver {
    pub fn new(config: LdapConnectionConfig) -> Self {
        Self {
            config: RwLock::new(config),
            tables: RwLock::new(HashMap::new()),
            connected: RwLock::new(false),
        }
    }

    /// Register a table configuration.
    pub fn add_table_config(&self, table_config: LdapTableConfig) {
        self.tables
            .write()
            .insert(table_config.table_name.clone(), table_config);
    }

    /// Get the table config, or create a default one.
    fn get_table_config(&self, table: &str) -> LdapTableConfig {
        self.tables
            .read()
            .get(table)
            .cloned()
            .unwrap_or_else(|| LdapTableConfig::new(table))
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        *self.connected.read()
    }

    /// Attempt to connect (stub).
    pub fn connect(&self) -> ConfigLdapResult<()> {
        let config = self.config.read();
        debug!(url = %config.url, base_dn = %config.base_dn, "LDAP connect (stub)");
        *self.connected.write() = true;
        info!("LDAP connection established (stub)");
        Ok(())
    }

    /// Build the LDAP search filter for a query.
    pub fn build_filter(&self, table: &str, fields: &[(&str, &str)]) -> String {
        let table_config = self.get_table_config(table);
        build_ldap_filter(&table_config, fields)
    }

    /// Execute an LDAP search (stub).
    fn ldap_search(
        &self,
        _filter: &str,
    ) -> ConfigLdapResult<Vec<Vec<(String, String)>>> {
        Err(ConfigLdapError::ConnectionError(
            "LDAP driver not connected".to_string(),
        ))
    }
}

impl ConfigRealtimeDriver for LdapRealtimeDriver {
    fn name(&self) -> &str {
        "ldap"
    }

    fn realtime_load(
        &self,
        _database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<(String, String)>> {
        let filter = self.build_filter(table, fields);
        debug!(filter = %filter, "LDAP realtime load");
        let rows = self
            .ldap_search(&filter)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    fn realtime_load_multi(
        &self,
        _database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<Vec<Vec<(String, String)>>> {
        let filter = self.build_filter(table, fields);
        self.ldap_search(&filter)
            .map_err(|e| crate::config_curl::ConfigCurlError::Other(e.to_string()))
    }

    fn realtime_store(
        &self,
        _database: &str,
        _table: &str,
        _fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        // LDAP store = ldap_add
        Err(crate::config_curl::ConfigCurlError::Other(
            "LDAP store not connected".to_string(),
        ))
    }

    fn realtime_update(
        &self,
        _database: &str,
        _table: &str,
        _key_field: &str,
        _entity: &str,
        _fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        // LDAP update = ldap_modify
        Err(crate::config_curl::ConfigCurlError::Other(
            "LDAP update not connected".to_string(),
        ))
    }

    fn realtime_destroy(
        &self,
        _database: &str,
        _table: &str,
        _key_field: &str,
        _entity: &str,
        _fields: &[(&str, &str)],
    ) -> crate::config_curl::ConfigCurlResult<u64> {
        // LDAP destroy = ldap_delete
        Err(crate::config_curl::ConfigCurlError::Other(
            "LDAP delete not connected".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ldap_escape() {
        assert_eq!(ldap_escape("hello*world"), "hello\\2aworld");
        assert_eq!(ldap_escape("(test)"), "\\28test\\29");
        assert_eq!(ldap_escape("back\\slash"), "back\\5cslash");
    }

    #[test]
    fn test_build_ldap_filter() {
        let table = LdapTableConfig::new("sippeers");
        let filter = build_ldap_filter(&table, &[("name", "alice"), ("context", "default")]);
        assert_eq!(filter, "(&(name=alice)(context=default))");
    }

    #[test]
    fn test_build_ldap_filter_with_mapping() {
        let mut table = LdapTableConfig::new("sippeers");
        table.map_attribute("name", "cn");
        table.map_attribute("context", "ou");
        let filter = build_ldap_filter(&table, &[("name", "alice")]);
        assert_eq!(filter, "(&(cn=alice))");
    }

    #[test]
    fn test_build_ldap_filter_with_additional() {
        let mut table = LdapTableConfig::new("sippeers");
        table.additional_filter = "(objectClass=asteriskSIPUser)".to_string();
        let filter = build_ldap_filter(&table, &[("name", "alice")]);
        assert_eq!(filter, "(&(name=alice)(objectClass=asteriskSIPUser))");
    }

    #[test]
    fn test_table_config() {
        let mut tc = LdapTableConfig::new("test");
        tc.map_attribute("username", "uid");
        assert_eq!(tc.get_ldap_attr("username"), "uid");
        assert_eq!(tc.get_ldap_attr("unmapped"), "unmapped");
    }

    #[test]
    fn test_driver_name() {
        let driver = LdapRealtimeDriver::new(LdapConnectionConfig::default());
        assert_eq!(ConfigRealtimeDriver::name(&driver), "ldap");
    }
}
