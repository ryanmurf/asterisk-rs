//! Realtime dialplan from database.
//!
//! Port of pbx/pbx_realtime.c from Asterisk C.
//!
//! Provides on-demand extension lookups from a realtime database backend
//! (ODBC, PostgreSQL, etc.) rather than loading the entire dialplan into
//! memory from extensions.conf.
//!
//! When an extension is dialed, the realtime engine queries the database
//! for matching rows and constructs temporary Extension/Priority objects.

use super::{Extension, Priority};
use std::collections::HashMap;
use tracing::debug;

/// A row returned from a realtime extension query.
#[derive(Debug, Clone)]
pub struct RealtimeExtensionRow {
    /// Context name
    pub context: String,
    /// Extension pattern
    pub exten: String,
    /// Priority number
    pub priority: i32,
    /// Application name
    pub app: String,
    /// Application data
    pub app_data: String,
    /// Priority label (optional)
    pub label: Option<String>,
}

/// Trait for realtime database backends.
///
/// Implementations query a database for extension data on demand.
pub trait RealtimeBackend: Send + Sync {
    /// Query extensions for a specific extension in a context.
    ///
    /// Returns all priority rows for the matching extension.
    fn lookup_extension(
        &self,
        context: &str,
        exten: &str,
    ) -> Result<Vec<RealtimeExtensionRow>, String>;

    /// Query all extensions in a context (for pattern matching).
    fn lookup_context(&self, context: &str) -> Result<Vec<RealtimeExtensionRow>, String>;
}

/// In-memory mock realtime backend for testing.
#[derive(Debug, Default)]
pub struct MockRealtimeBackend {
    /// Stored rows keyed by (context, exten)
    rows: parking_lot::RwLock<Vec<RealtimeExtensionRow>>,
}

impl MockRealtimeBackend {
    pub fn new() -> Self {
        Self {
            rows: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Add a row to the mock database.
    pub fn add_row(&self, row: RealtimeExtensionRow) {
        self.rows.write().push(row);
    }
}

impl RealtimeBackend for MockRealtimeBackend {
    fn lookup_extension(
        &self,
        context: &str,
        exten: &str,
    ) -> Result<Vec<RealtimeExtensionRow>, String> {
        let rows = self.rows.read();
        let matches: Vec<RealtimeExtensionRow> = rows
            .iter()
            .filter(|r| r.context == context && r.exten == exten)
            .cloned()
            .collect();
        Ok(matches)
    }

    fn lookup_context(&self, context: &str) -> Result<Vec<RealtimeExtensionRow>, String> {
        let rows = self.rows.read();
        let matches: Vec<RealtimeExtensionRow> = rows
            .iter()
            .filter(|r| r.context == context)
            .cloned()
            .collect();
        Ok(matches)
    }
}

/// Realtime dialplan engine.
///
/// Queries a realtime backend for extensions on demand rather than
/// loading everything at startup.
pub struct RealtimeDialplan {
    backend: Box<dyn RealtimeBackend>,
    /// Cache of recently looked-up extensions
    cache: parking_lot::RwLock<HashMap<(String, String), Vec<RealtimeExtensionRow>>>,
    /// Whether to cache lookups
    pub cache_enabled: bool,
    /// Cache TTL in seconds (0 = no expiry)
    pub cache_ttl: u64,
}

impl RealtimeDialplan {
    pub fn new(backend: Box<dyn RealtimeBackend>) -> Self {
        Self {
            backend,
            cache: parking_lot::RwLock::new(HashMap::new()),
            cache_enabled: true,
            cache_ttl: 0,
        }
    }

    /// Look up an extension, building a temporary Extension object.
    pub fn find_extension(&self, context: &str, exten: &str) -> Option<Extension> {
        // Check cache first
        if self.cache_enabled {
            let cache_key = (context.to_string(), exten.to_string());
            if let Some(rows) = self.cache.read().get(&cache_key) {
                return Self::rows_to_extension(rows);
            }
        }

        // Query backend
        let rows = match self.backend.lookup_extension(context, exten) {
            Ok(rows) => rows,
            Err(e) => {
                debug!("Realtime lookup failed for {}/{}@{}: {}", exten, context, context, e);
                return None;
            }
        };

        if rows.is_empty() {
            return None;
        }

        // Cache the result
        if self.cache_enabled {
            let cache_key = (context.to_string(), exten.to_string());
            self.cache.write().insert(cache_key, rows.clone());
        }

        Self::rows_to_extension(&rows)
    }

    /// Convert database rows into an Extension object.
    fn rows_to_extension(rows: &[RealtimeExtensionRow]) -> Option<Extension> {
        if rows.is_empty() {
            return None;
        }

        let mut ext = Extension::new(&rows[0].exten);
        for row in rows {
            ext.add_priority(Priority {
                priority: row.priority,
                app: row.app.clone(),
                app_data: row.app_data.clone(),
                label: row.label.clone(),
            });
        }
        Some(ext)
    }

    /// Clear the extension cache.
    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }

    /// Invalidate a specific cached extension.
    pub fn invalidate(&self, context: &str, exten: &str) {
        self.cache
            .write()
            .remove(&(context.to_string(), exten.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_backend() {
        let backend = MockRealtimeBackend::new();
        backend.add_row(RealtimeExtensionRow {
            context: "default".to_string(),
            exten: "100".to_string(),
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });
        backend.add_row(RealtimeExtensionRow {
            context: "default".to_string(),
            exten: "100".to_string(),
            priority: 2,
            app: "Hangup".to_string(),
            app_data: String::new(),
            label: None,
        });

        let rows = backend.lookup_extension("default", "100").unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_realtime_find_extension() {
        let backend = MockRealtimeBackend::new();
        backend.add_row(RealtimeExtensionRow {
            context: "default".to_string(),
            exten: "200".to_string(),
            priority: 1,
            app: "Dial".to_string(),
            app_data: "SIP/bob".to_string(),
            label: Some("start".to_string()),
        });

        let rt = RealtimeDialplan::new(Box::new(backend));
        let ext = rt.find_extension("default", "200").unwrap();
        assert_eq!(ext.name, "200");
        let prio = ext.get_priority(1).unwrap();
        assert_eq!(prio.app, "Dial");
        assert_eq!(prio.label.as_deref(), Some("start"));
    }

    #[test]
    fn test_realtime_not_found() {
        let backend = MockRealtimeBackend::new();
        let rt = RealtimeDialplan::new(Box::new(backend));
        assert!(rt.find_extension("default", "999").is_none());
    }

    #[test]
    fn test_cache_invalidation() {
        let backend = MockRealtimeBackend::new();
        backend.add_row(RealtimeExtensionRow {
            context: "default".to_string(),
            exten: "100".to_string(),
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });

        let rt = RealtimeDialplan::new(Box::new(backend));
        // First lookup populates cache
        assert!(rt.find_extension("default", "100").is_some());
        assert!(!rt.cache.read().is_empty());

        // Invalidate
        rt.invalidate("default", "100");
        assert!(!rt.cache.read().contains_key(&("default".to_string(), "100".to_string())));
    }
}
