//! HTTP/cURL-based realtime configuration backend.
//!
//! Port of `res/res_config_curl.c`. Provides a realtime configuration
//! driver that fetches and stores configuration via HTTP GET/POST to an
//! external configuration server. Field queries are URL-encoded and
//! responses are parsed as `key=value&key=value` pairs.

use std::collections::HashMap;
use std::fmt;

use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigCurlError {
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("URL not configured for table '{0}'")]
    UrlNotConfigured(String),
    #[error("config curl error: {0}")]
    Other(String),
}

pub type ConfigCurlResult<T> = Result<T, ConfigCurlError>;

// ---------------------------------------------------------------------------
// Realtime driver trait (shared across config backends)
// ---------------------------------------------------------------------------

/// Trait for realtime configuration drivers.
///
/// Mirrors the `ast_config_engine` callbacks: realtime_func, store_func,
/// update_func, destroy_func from the C source.
pub trait ConfigRealtimeDriver: Send + Sync + fmt::Debug {
    /// Driver name (e.g., "curl", "odbc", "pgsql").
    fn name(&self) -> &str;

    /// Load a single row matching the given field criteria.
    ///
    /// Returns key-value pairs from the first matching row.
    fn realtime_load(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<Vec<(String, String)>>;

    /// Load multiple rows matching the given field criteria.
    fn realtime_load_multi(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<Vec<Vec<(String, String)>>>;

    /// Store a new row with the given field values.
    fn realtime_store(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64>;

    /// Update rows matching `key_field=entity` with the given field values.
    fn realtime_update(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64>;

    /// Delete rows matching `key_field=entity` and optional extra field criteria.
    fn realtime_destroy(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64>;
}

// ---------------------------------------------------------------------------
// URL encoding helper
// ---------------------------------------------------------------------------

/// Percent-encode a string for use in URLs/form data.
fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

/// Decode a percent-encoded string.
fn url_decode(s: &str) -> String {
    let mut decoded = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                decoded.push(byte as char);
            } else {
                decoded.push('%');
                decoded.push_str(&hex);
            }
        } else if c == '+' {
            decoded.push(' ');
        } else {
            decoded.push(c);
        }
    }
    decoded
}

/// Parse a URL-encoded `key=value&key=value` response into pairs.
fn parse_response(body: &str) -> Vec<(String, String)> {
    let body = body.trim_end_matches(['\r', '\n']);
    let mut pairs = Vec::new();
    for pair_str in body.split('&') {
        if pair_str.is_empty() {
            continue;
        }
        let mut parts = pair_str.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        if !key.is_empty() {
            pairs.push((url_decode(key), url_decode(value)));
        }
    }
    pairs
}

/// Build a URL-encoded query string from field pairs.
fn build_query(fields: &[(&str, &str)]) -> String {
    let mut query = String::new();
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            query.push('&');
        }
        query.push_str(&url_encode(key));
        query.push('=');
        query.push_str(&url_encode(value));
    }
    query
}

// ---------------------------------------------------------------------------
// cURL configuration backend
// ---------------------------------------------------------------------------

/// Configuration for the cURL realtime driver.
#[derive(Debug, Clone)]
pub struct CurlRealtimeConfig {
    /// Base URL for the config server (from `extconfig.conf`).
    /// Requests are made to `{url}/single`, `{url}/multi`, `{url}/store`, etc.
    pub url: String,
}

/// HTTP-based realtime configuration driver.
///
/// Port of `res_config_curl.c`. Makes HTTP requests to an external
/// configuration server to load/store/update/delete configuration rows.
///
/// The URL endpoints follow the convention:
/// - `{base_url}/single?fields` -- load single row
/// - `{base_url}/multi?fields`  -- load multiple rows
/// - `{base_url}/store`  -- POST new row
/// - `{base_url}/update` -- POST update
/// - `{base_url}/destroy` -- POST delete
#[derive(Debug)]
pub struct CurlRealtimeDriver {
    /// URL configurations keyed by database/table.
    urls: parking_lot::RwLock<HashMap<String, String>>,
}

impl CurlRealtimeDriver {
    /// Create a new cURL realtime driver.
    pub fn new() -> Self {
        Self {
            urls: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Register a base URL for a database name (from extconfig.conf mapping).
    pub fn add_url_mapping(&self, database: &str, url: &str) {
        self.urls
            .write()
            .insert(database.to_string(), url.to_string());
    }

    /// Get the base URL for a database.
    fn get_url(&self, database: &str) -> ConfigCurlResult<String> {
        self.urls
            .read()
            .get(database)
            .cloned()
            .ok_or_else(|| ConfigCurlError::UrlNotConfigured(database.to_string()))
    }

    /// Simulate an HTTP GET request.
    ///
    /// In a full implementation this would use an HTTP client (reqwest, hyper, etc.).
    /// Returns the response body as a string.
    fn http_get(&self, url: &str) -> ConfigCurlResult<String> {
        debug!(url = url, "HTTP GET (stub)");
        // Stub: real implementation would perform the HTTP request
        Err(ConfigCurlError::HttpError(format!(
            "HTTP client not connected (url: {})",
            url
        )))
    }

    /// Simulate an HTTP POST request.
    fn http_post(&self, url: &str, body: &str) -> ConfigCurlResult<String> {
        debug!(url = url, body_len = body.len(), "HTTP POST (stub)");
        Err(ConfigCurlError::HttpError(format!(
            "HTTP client not connected (url: {})",
            url
        )))
    }
}

impl Default for CurlRealtimeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigRealtimeDriver for CurlRealtimeDriver {
    fn name(&self) -> &str {
        "curl"
    }

    fn realtime_load(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<Vec<(String, String)>> {
        let base_url = self.get_url(database)?;
        let query = build_query(fields);
        let url = format!("{}/single,{}?{}", base_url, table, query);
        let body = self.http_get(&url)?;
        Ok(parse_response(&body))
    }

    fn realtime_load_multi(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<Vec<Vec<(String, String)>>> {
        let base_url = self.get_url(database)?;
        let query = build_query(fields);
        let url = format!("{}/multi,{}?{}", base_url, table, query);
        let body = self.http_get(&url)?;

        // Multi-row responses are separated by blank lines.
        let mut rows = Vec::new();
        for line in body.split("\n\n") {
            let line = line.trim();
            if !line.is_empty() {
                rows.push(parse_response(line));
            }
        }
        Ok(rows)
    }

    fn realtime_store(
        &self,
        database: &str,
        table: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64> {
        let base_url = self.get_url(database)?;
        let url = format!("{}/store,{}", base_url, table);
        let body = build_query(fields);
        let response = self.http_post(&url, &body)?;
        response
            .trim()
            .parse::<u64>()
            .map_err(|_| ConfigCurlError::InvalidResponse(response))
    }

    fn realtime_update(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64> {
        let base_url = self.get_url(database)?;
        let url = format!("{}/update,{}", base_url, table);
        let mut body_fields = vec![(key_field, entity)];
        body_fields.extend_from_slice(fields);
        let body = build_query(&body_fields);
        let response = self.http_post(&url, &body)?;
        response
            .trim()
            .parse::<u64>()
            .map_err(|_| ConfigCurlError::InvalidResponse(response))
    }

    fn realtime_destroy(
        &self,
        database: &str,
        table: &str,
        key_field: &str,
        entity: &str,
        fields: &[(&str, &str)],
    ) -> ConfigCurlResult<u64> {
        let base_url = self.get_url(database)?;
        let url = format!("{}/destroy,{}", base_url, table);
        let mut body_fields = vec![(key_field, entity)];
        body_fields.extend_from_slice(fields);
        let body = build_query(&body_fields);
        let response = self.http_post(&url, &body)?;
        response
            .trim()
            .parse::<u64>()
            .map_err(|_| ConfigCurlError::InvalidResponse(response))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_decode() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("key=value&foo"), "key%3Dvalue%26foo");
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("key%3Dvalue"), "key=value");
    }

    #[test]
    fn test_parse_response() {
        let pairs = parse_response("name=Alice&ext=100&context=default");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("name".to_string(), "Alice".to_string()));
        assert_eq!(pairs[1], ("ext".to_string(), "100".to_string()));
    }

    #[test]
    fn test_build_query() {
        let query = build_query(&[("name", "Alice"), ("ext", "100")]);
        assert_eq!(query, "name=Alice&ext=100");
    }

    #[test]
    fn test_build_query_encoding() {
        let query = build_query(&[("key field", "value&stuff")]);
        assert_eq!(query, "key%20field=value%26stuff");
    }

    #[test]
    fn test_driver_url_mapping() {
        let driver = CurlRealtimeDriver::new();
        assert!(driver.get_url("mydb").is_err());
        driver.add_url_mapping("mydb", "http://localhost:8080");
        assert_eq!(
            driver.get_url("mydb").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn test_driver_name() {
        let driver = CurlRealtimeDriver::new();
        assert_eq!(ConfigRealtimeDriver::name(&driver), "curl");
    }
}
