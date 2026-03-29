//! AstDB functions - internal key-value database.
//!
//! Port of func_db.c from Asterisk C.
//!
//! Provides:
//! - DB(family/key) - read/write database entries
//! - DB_EXISTS(family/key) - check if key exists
//! - DB_KEYS(family) - list keys in a family
//! - DB_KEYCOUNT(family) - count keys in a family
//! - DB_DELETE(family/key) - delete an entry

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory AstDB store.
///
/// Keys are stored as "family/key" strings mapping to values.
/// In a production system this would persist to SQLite (like Asterisk)
/// or another backing store.
#[derive(Debug, Default)]
pub struct AstDb {
    entries: HashMap<String, String>,
}

impl AstDb {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Get a value by family/key.
    pub fn get(&self, family: &str, key: &str) -> Option<&String> {
        let full_key = format!("{}/{}", family, key);
        self.entries.get(&full_key)
    }

    /// Set a value.
    pub fn put(&mut self, family: &str, key: &str, value: &str) {
        let full_key = format!("{}/{}", family, key);
        self.entries.insert(full_key, value.to_string());
    }

    /// Delete a value. Returns the old value if it existed.
    pub fn delete(&mut self, family: &str, key: &str) -> Option<String> {
        let full_key = format!("{}/{}", family, key);
        self.entries.remove(&full_key)
    }

    /// Check if a key exists.
    pub fn exists(&self, family: &str, key: &str) -> bool {
        let full_key = format!("{}/{}", family, key);
        self.entries.contains_key(&full_key)
    }

    /// List all keys in a family.
    pub fn keys(&self, family: &str) -> Vec<String> {
        let prefix = format!("{}/", family);
        self.entries
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix).map(|s| s.to_string()))
            .collect()
    }

    /// Count keys in a family.
    pub fn key_count(&self, family: &str) -> usize {
        let prefix = format!("{}/", family);
        self.entries
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .count()
    }

    /// Delete all entries in a family. Returns number of deleted entries.
    pub fn delete_family(&mut self, family: &str) -> usize {
        let prefix = format!("{}/", family);
        let keys: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        let count = keys.len();
        for key in keys {
            self.entries.remove(&key);
        }
        count
    }
}

/// Thread-safe handle to a shared AstDB instance.
pub type SharedAstDb = Arc<RwLock<AstDb>>;

/// Parse a family/key argument string.
/// Returns (family, key). The key portion may be empty for family-level operations.
fn parse_family_key(args: &str) -> Result<(String, String), FuncError> {
    let args = args.trim();
    if args.is_empty() {
        return Err(FuncError::InvalidArgument(
            "DB: family/key argument is required".to_string(),
        ));
    }

    // Split on first '/'
    if let Some(slash) = args.find('/') {
        let family = args[..slash].trim().to_string();
        let key = args[slash + 1..].trim().to_string();
        if family.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB: family cannot be empty".to_string(),
            ));
        }
        Ok((family, key))
    } else {
        // No slash -- treat as family-only
        Ok((args.to_string(), String::new()))
    }
}

/// DB() function.
///
/// Reads or writes entries in the internal database.
///
/// Read usage:  DB(family/key) - returns the stored value
/// Write usage: Set(DB(family/key)=value)
pub struct FuncDb;

impl FuncDb {
    /// Read the DB value from the channel-variable-backed store.
    fn db_get(ctx: &FuncContext, family: &str, key: &str) -> Option<String> {
        let var_key = format!("__DB_{}/{}", family, key);
        ctx.get_variable(&var_key).cloned()
    }

    /// Write a DB value.
    fn db_put(ctx: &mut FuncContext, family: &str, key: &str, value: &str) {
        let var_key = format!("__DB_{}/{}", family, key);
        ctx.set_variable(&var_key, value);
        // Track key in the family list
        let list_key = format!("__DB_KEYS_{}", family);
        let mut keys = ctx
            .get_variable(&list_key)
            .map(|v| {
                v.split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let key_str = key.to_string();
        if !keys.contains(&key_str) {
            keys.push(key_str);
        }
        ctx.set_variable(&list_key, &keys.join(","));
    }

    fn db_delete(ctx: &mut FuncContext, family: &str, key: &str) -> Option<String> {
        let var_key = format!("__DB_{}/{}", family, key);
        let old = ctx.variables.remove(&var_key);
        // Remove from family key list
        let list_key = format!("__DB_KEYS_{}", family);
        if let Some(list) = ctx.variables.get(&list_key).cloned() {
            let keys: Vec<&str> = list
                .split(',')
                .filter(|s| !s.is_empty() && *s != key)
                .collect();
            ctx.set_variable(&list_key, &keys.join(","));
        }
        old
    }
}

impl DialplanFunc for FuncDb {
    fn name(&self) -> &str {
        "DB"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (family, key) = parse_family_key(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB: key is required for read (use DB(family/key))".to_string(),
            ));
        }
        Self::db_get(ctx, &family, &key).ok_or_else(|| {
            FuncError::DataNotAvailable(format!("DB: key '{}/{}' not found", family, key))
        })
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let (family, key) = parse_family_key(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB: key is required for write (use DB(family/key))".to_string(),
            ));
        }
        Self::db_put(ctx, &family, &key, value);
        Ok(())
    }
}

/// DB_EXISTS() function.
///
/// Checks whether a key exists in the database.
///
/// Usage: DB_EXISTS(family/key) -> "1" or "0"
pub struct FuncDbExists;

impl DialplanFunc for FuncDbExists {
    fn name(&self) -> &str {
        "DB_EXISTS"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (family, key) = parse_family_key(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB_EXISTS: key is required".to_string(),
            ));
        }
        if FuncDb::db_get(ctx, &family, &key).is_some() {
            Ok("1".to_string())
        } else {
            Ok("0".to_string())
        }
    }
}

/// DB_KEYS() function.
///
/// Lists all keys in a database family.
///
/// Usage: DB_KEYS(family) -> comma-separated list of keys
pub struct FuncDbKeys;

impl DialplanFunc for FuncDbKeys {
    fn name(&self) -> &str {
        "DB_KEYS"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let family = args.trim();
        if family.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB_KEYS: family argument is required".to_string(),
            ));
        }
        let list_key = format!("__DB_KEYS_{}", family);
        Ok(ctx.get_variable(&list_key).cloned().unwrap_or_default())
    }
}

/// DB_KEYCOUNT() function.
///
/// Counts the number of keys in a database family.
///
/// Usage: DB_KEYCOUNT(family) -> count as string
pub struct FuncDbKeyCount;

impl DialplanFunc for FuncDbKeyCount {
    fn name(&self) -> &str {
        "DB_KEYCOUNT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let family = args.trim();
        if family.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB_KEYCOUNT: family argument is required".to_string(),
            ));
        }
        let list_key = format!("__DB_KEYS_{}", family);
        let count = ctx
            .get_variable(&list_key)
            .map(|v| v.split(',').filter(|s| !s.is_empty()).count())
            .unwrap_or(0);
        Ok(count.to_string())
    }
}

/// DB_DELETE() function.
///
/// Deletes a key from the database and returns its former value.
///
/// Usage: DB_DELETE(family/key) -> the deleted value, or ""
pub struct FuncDbDelete;

impl DialplanFunc for FuncDbDelete {
    fn name(&self) -> &str {
        "DB_DELETE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        // DB_DELETE is a read function that has the side-effect of deleting.
        // We cannot mutate ctx from a read call, so we return the value
        // and the caller must use write semantics.
        let (family, key) = parse_family_key(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB_DELETE: key is required".to_string(),
            ));
        }
        // Return current value (actual deletion handled by write)
        Ok(FuncDb::db_get(ctx, &family, &key).unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, _value: &str) -> Result<(), FuncError> {
        let (family, key) = parse_family_key(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DB_DELETE: key is required".to_string(),
            ));
        }
        FuncDb::db_delete(ctx, &family, &key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_put_and_get() {
        let mut ctx = FuncContext::new();
        let func = FuncDb;
        func.write(&mut ctx, "cidname/1234", "John Doe").unwrap();
        assert_eq!(func.read(&ctx, "cidname/1234").unwrap(), "John Doe");
    }

    #[test]
    fn test_db_not_found() {
        let ctx = FuncContext::new();
        let func = FuncDb;
        assert!(func.read(&ctx, "cidname/9999").is_err());
    }

    #[test]
    fn test_db_exists() {
        let mut ctx = FuncContext::new();
        let db = FuncDb;
        let exists = FuncDbExists;
        db.write(&mut ctx, "test/key1", "val1").unwrap();
        assert_eq!(exists.read(&ctx, "test/key1").unwrap(), "1");
        assert_eq!(exists.read(&ctx, "test/key2").unwrap(), "0");
    }

    #[test]
    fn test_db_keys() {
        let mut ctx = FuncContext::new();
        let db = FuncDb;
        let keys = FuncDbKeys;
        db.write(&mut ctx, "family/key1", "a").unwrap();
        db.write(&mut ctx, "family/key2", "b").unwrap();
        let result = keys.read(&ctx, "family").unwrap();
        assert!(result.contains("key1"));
        assert!(result.contains("key2"));
    }

    #[test]
    fn test_db_keycount() {
        let mut ctx = FuncContext::new();
        let db = FuncDb;
        let count = FuncDbKeyCount;
        db.write(&mut ctx, "fam/k1", "a").unwrap();
        db.write(&mut ctx, "fam/k2", "b").unwrap();
        db.write(&mut ctx, "fam/k3", "c").unwrap();
        assert_eq!(count.read(&ctx, "fam").unwrap(), "3");
    }

    #[test]
    fn test_db_delete() {
        let mut ctx = FuncContext::new();
        let db = FuncDb;
        let del = FuncDbDelete;
        let exists = FuncDbExists;
        db.write(&mut ctx, "test/delme", "hello").unwrap();
        assert_eq!(exists.read(&ctx, "test/delme").unwrap(), "1");
        let old_val = del.read(&ctx, "test/delme").unwrap();
        assert_eq!(old_val, "hello");
        del.write(&mut ctx, "test/delme", "").unwrap();
        assert_eq!(exists.read(&ctx, "test/delme").unwrap(), "0");
    }

    #[test]
    fn test_astdb_standalone() {
        let mut db = AstDb::new();
        db.put("cid", "100", "Alice");
        db.put("cid", "200", "Bob");
        assert_eq!(db.get("cid", "100"), Some(&"Alice".to_string()));
        assert!(db.exists("cid", "200"));
        assert!(!db.exists("cid", "300"));
        assert_eq!(db.key_count("cid"), 2);
        let keys = db.keys("cid");
        assert!(keys.contains(&"100".to_string()));
        assert!(keys.contains(&"200".to_string()));
        db.delete("cid", "100");
        assert!(!db.exists("cid", "100"));
        assert_eq!(db.key_count("cid"), 1);
    }
}
