//! AstDB key-value store.
//!
//! Port of `main/db.c`. Provides a simple hierarchical key-value database
//! organized by "family" and "key" pairs. Data persists to a JSON file on
//! disk and is protected by a read-write lock for thread safety.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AstDbError {
    #[error("key not found: family={0}, key={1}")]
    KeyNotFound(String, String),
    #[error("family not found: {0}")]
    FamilyNotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type AstDbResult<T> = Result<T, AstDbError>;

// ---------------------------------------------------------------------------
// AstDB
// ---------------------------------------------------------------------------

/// A hierarchical key-value store organized by family.
///
/// Each family is a namespace containing key-value string pairs. The store
/// persists to JSON on disk and is protected by an `RwLock` for safe
/// concurrent access from multiple threads.
pub struct AstDb {
    /// family -> (key -> value)
    data: RwLock<HashMap<String, HashMap<String, String>>>,
    /// Path for persistence. `None` means in-memory only.
    db_path: Option<PathBuf>,
}

impl AstDb {
    /// Create a new in-memory database (no file persistence).
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            db_path: None,
        }
    }

    /// Create a database backed by the given file path.
    ///
    /// If the file exists it will be loaded; otherwise an empty database is
    /// created and the file will be written on the first mutation.
    pub fn with_file(path: impl Into<PathBuf>) -> AstDbResult<Self> {
        let path = path.into();
        let data = if path.exists() {
            let contents = fs::read_to_string(&path)?;
            let parsed: HashMap<String, HashMap<String, String>> =
                serde_json::from_str(&contents)?;
            info!(path = %path.display(), families = parsed.len(), "Loaded AstDB");
            parsed
        } else {
            HashMap::new()
        };

        Ok(Self {
            data: RwLock::new(data),
            db_path: Some(path),
        })
    }

    /// Store a value under the given family and key.
    pub fn put(&self, family: &str, key: &str, value: &str) -> AstDbResult<()> {
        {
            let mut data = self.data.write();
            data.entry(family.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        debug!(family, key, "AstDB put");
        self.persist()
    }

    /// Retrieve a value by family and key.
    pub fn get(&self, family: &str, key: &str) -> AstDbResult<String> {
        let data = self.data.read();
        data.get(family)
            .and_then(|fam| fam.get(key))
            .cloned()
            .ok_or_else(|| AstDbError::KeyNotFound(family.to_string(), key.to_string()))
    }

    /// Delete a single key from a family.
    pub fn del(&self, family: &str, key: &str) -> AstDbResult<()> {
        {
            let mut data = self.data.write();
            let fam = data
                .get_mut(family)
                .ok_or_else(|| AstDbError::KeyNotFound(family.to_string(), key.to_string()))?;
            if fam.remove(key).is_none() {
                return Err(AstDbError::KeyNotFound(
                    family.to_string(),
                    key.to_string(),
                ));
            }
            // Remove empty families to keep the store tidy.
            if fam.is_empty() {
                data.remove(family);
            }
        }
        debug!(family, key, "AstDB del");
        self.persist()
    }

    /// Delete an entire family tree.
    pub fn deltree(&self, family: &str) -> AstDbResult<()> {
        {
            let mut data = self.data.write();
            // Delete the exact family and any family that starts with `family/`.
            let prefix = format!("{}/", family);
            let keys_to_remove: Vec<String> = data
                .keys()
                .filter(|k| *k == family || k.starts_with(&prefix))
                .cloned()
                .collect();
            if keys_to_remove.is_empty() {
                return Err(AstDbError::FamilyNotFound(family.to_string()));
            }
            for k in keys_to_remove {
                data.remove(&k);
            }
        }
        debug!(family, "AstDB deltree");
        self.persist()
    }

    /// Get all key-value pairs in a family.
    pub fn gettree(&self, family: &str) -> AstDbResult<Vec<(String, String)>> {
        let data = self.data.read();
        let prefix = format!("{}/", family);
        let mut results: Vec<(String, String)> = Vec::new();

        for (fam_name, entries) in data.iter() {
            if fam_name == family || fam_name.starts_with(&prefix) {
                for (key, value) in entries {
                    let full_key = format!("/{}/{}", fam_name, key);
                    results.push((full_key, value.clone()));
                }
            }
        }

        if results.is_empty() {
            return Err(AstDbError::FamilyNotFound(family.to_string()));
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    /// Show a single key (returns the formatted path and value).
    pub fn show_key(&self, family: &str, key: &str) -> AstDbResult<(String, String)> {
        let value = self.get(family, key)?;
        let path = format!("/{}/{}", family, key);
        Ok((path, value))
    }

    /// Return all families present in the database.
    pub fn families(&self) -> Vec<String> {
        let data = self.data.read();
        let mut fams: Vec<String> = data.keys().cloned().collect();
        fams.sort();
        fams
    }

    /// Return the total number of key-value entries across all families.
    pub fn entry_count(&self) -> usize {
        let data = self.data.read();
        data.values().map(|fam| fam.len()).sum()
    }

    /// Save the database to its configured path (if any).
    pub fn save(&self) -> AstDbResult<()> {
        self.persist()
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn persist(&self) -> AstDbResult<()> {
        if let Some(ref path) = self.db_path {
            let data = self.data.read();
            let json = serde_json::to_string_pretty(&*data)?;
            // Write atomically via a temp file.
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, json)?;
            fs::rename(&tmp, path)?;
            debug!(path = %path.display(), "AstDB persisted");
        }
        Ok(())
    }
}

impl Default for AstDb {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AstDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.data.read();
        f.debug_struct("AstDb")
            .field("families", &data.len())
            .field(
                "entries",
                &data.values().map(|fam| fam.len()).sum::<usize>(),
            )
            .field("path", &self.db_path)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let db = AstDb::new();
        db.put("cidname", "12025551234", "Alice").unwrap();
        assert_eq!(db.get("cidname", "12025551234").unwrap(), "Alice");
    }

    #[test]
    fn test_get_missing() {
        let db = AstDb::new();
        assert!(db.get("none", "nothing").is_err());
    }

    #[test]
    fn test_del() {
        let db = AstDb::new();
        db.put("test", "key1", "val1").unwrap();
        db.del("test", "key1").unwrap();
        assert!(db.get("test", "key1").is_err());
    }

    #[test]
    fn test_deltree() {
        let db = AstDb::new();
        db.put("myapp", "k1", "v1").unwrap();
        db.put("myapp", "k2", "v2").unwrap();
        db.put("myapp/sub", "k3", "v3").unwrap();
        db.deltree("myapp").unwrap();
        assert!(db.get("myapp", "k1").is_err());
        assert!(db.get("myapp/sub", "k3").is_err());
    }

    #[test]
    fn test_gettree() {
        let db = AstDb::new();
        db.put("cidname", "100", "Alice").unwrap();
        db.put("cidname", "101", "Bob").unwrap();
        let tree = db.gettree("cidname").unwrap();
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn test_show_key() {
        let db = AstDb::new();
        db.put("test", "mykey", "myval").unwrap();
        let (path, val) = db.show_key("test", "mykey").unwrap();
        assert_eq!(path, "/test/mykey");
        assert_eq!(val, "myval");
    }

    #[test]
    fn test_families() {
        let db = AstDb::new();
        db.put("a", "k", "v").unwrap();
        db.put("b", "k", "v").unwrap();
        let fams = db.families();
        assert!(fams.contains(&"a".to_string()));
        assert!(fams.contains(&"b".to_string()));
    }

    #[test]
    fn test_entry_count() {
        let db = AstDb::new();
        db.put("f1", "k1", "v1").unwrap();
        db.put("f1", "k2", "v2").unwrap();
        db.put("f2", "k1", "v1").unwrap();
        assert_eq!(db.entry_count(), 3);
    }

    #[test]
    fn test_file_persistence() {
        let dir = std::env::temp_dir().join("astdb_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.json");
        let _ = fs::remove_file(&path);

        {
            let db = AstDb::with_file(&path).unwrap();
            db.put("persist", "hello", "world").unwrap();
        }

        // Reload
        let db2 = AstDb::with_file(&path).unwrap();
        assert_eq!(db2.get("persist", "hello").unwrap(), "world");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}
