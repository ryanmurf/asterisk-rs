//! Realtime CLI commands.
//!
//! Port of `res/res_realtime.c`. Provides CLI commands for interacting
//! with the Realtime configuration engine: load, update, store, and
//! destroy operations against realtime backends (ODBC, PostgreSQL, etc.).


use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum RealtimeError {
    #[error("no rows found")]
    NotFound,
    #[error("update failed: affected {0} rows")]
    UpdateFailed(i32),
    #[error("driver not connected")]
    NotConnected,
    #[error("realtime error: {0}")]
    Other(String),
}

pub type RealtimeResult<T> = Result<T, RealtimeError>;

// ---------------------------------------------------------------------------
// CLI operation types
// ---------------------------------------------------------------------------

/// A realtime load operation: query a single row by column match.
#[derive(Debug, Clone)]
pub struct RealtimeLoadRequest {
    /// Family (table) name.
    pub family: String,
    /// Column to match on.
    pub match_column: String,
    /// Value to match.
    pub match_value: String,
}

/// A realtime update operation: update rows matching criteria.
#[derive(Debug, Clone)]
pub struct RealtimeUpdateRequest {
    /// Family (table) name.
    pub family: String,
    /// Column to match on.
    pub match_column: String,
    /// Value to match.
    pub match_value: String,
    /// Column to update.
    pub update_column: String,
    /// New value.
    pub update_value: String,
}

/// A realtime store (insert) operation.
#[derive(Debug, Clone)]
pub struct RealtimeStoreRequest {
    /// Family (table) name.
    pub family: String,
    /// Column-value pairs to insert.
    pub fields: Vec<(String, String)>,
}

/// A realtime destroy (delete) operation.
#[derive(Debug, Clone)]
pub struct RealtimeDestroyRequest {
    /// Family (table) name.
    pub family: String,
    /// Column to match on.
    pub match_column: String,
    /// Value to match.
    pub match_value: String,
}

// ---------------------------------------------------------------------------
// Realtime result row
// ---------------------------------------------------------------------------

/// A row returned from a realtime query.
#[derive(Debug, Clone)]
pub struct RealtimeRow {
    /// Column-value pairs.
    pub fields: Vec<(String, String)>,
}

impl RealtimeRow {
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    pub fn add(&mut self, column: &str, value: &str) {
        self.fields.push((column.to_string(), value.to_string()));
    }

    pub fn get(&self, column: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(c, _)| c == column)
            .map(|(_, v)| v.as_str())
    }

    /// Format for CLI display.
    pub fn format_cli(&self) -> String {
        let mut output = String::new();
        for (col, val) in &self.fields {
            output.push_str(&format!("{:>30}  {}\n", col, val));
        }
        output
    }
}

impl Default for RealtimeRow {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Realtime CLI handler
// ---------------------------------------------------------------------------

/// Process a realtime load command.
///
/// Mirrors `cli_realtime_load()` from the C source. In the full
/// implementation this would delegate to `ast_load_realtime_all()`.
pub fn cli_load(request: &RealtimeLoadRequest) -> RealtimeResult<Vec<RealtimeRow>> {
    debug!(
        family = %request.family,
        column = %request.match_column,
        value = %request.match_value,
        "Realtime load (stub)"
    );
    Err(RealtimeError::NotConnected)
}

/// Process a realtime update command.
///
/// Mirrors `cli_realtime_update()` from the C source.
pub fn cli_update(request: &RealtimeUpdateRequest) -> RealtimeResult<i32> {
    debug!(
        family = %request.family,
        match_col = %request.match_column,
        update_col = %request.update_column,
        "Realtime update (stub)"
    );
    Err(RealtimeError::NotConnected)
}

/// Process a realtime store command.
///
/// Mirrors `cli_realtime_store()` from the C source.
pub fn cli_store(request: &RealtimeStoreRequest) -> RealtimeResult<()> {
    debug!(
        family = %request.family,
        fields = request.fields.len(),
        "Realtime store (stub)"
    );
    Err(RealtimeError::NotConnected)
}

/// Process a realtime destroy command.
///
/// Mirrors `cli_realtime_destroy()` from the C source.
pub fn cli_destroy(request: &RealtimeDestroyRequest) -> RealtimeResult<i32> {
    debug!(
        family = %request.family,
        column = %request.match_column,
        value = %request.match_value,
        "Realtime destroy (stub)"
    );
    Err(RealtimeError::NotConnected)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_realtime_row() {
        let mut row = RealtimeRow::new();
        row.add("name", "alice");
        row.add("port", "5060");
        assert_eq!(row.get("name"), Some("alice"));
        assert_eq!(row.get("port"), Some("5060"));
        assert_eq!(row.get("missing"), None);
    }

    #[test]
    fn test_row_cli_format() {
        let mut row = RealtimeRow::new();
        row.add("name", "alice");
        let formatted = row.format_cli();
        assert!(formatted.contains("name"));
        assert!(formatted.contains("alice"));
    }

    #[test]
    fn test_stub_operations() {
        let load = RealtimeLoadRequest {
            family: "sippeers".to_string(),
            match_column: "name".to_string(),
            match_value: "alice".to_string(),
        };
        assert!(cli_load(&load).is_err());

        let update = RealtimeUpdateRequest {
            family: "sippeers".to_string(),
            match_column: "name".to_string(),
            match_value: "alice".to_string(),
            update_column: "port".to_string(),
            update_value: "5061".to_string(),
        };
        assert!(cli_update(&update).is_err());
    }
}
