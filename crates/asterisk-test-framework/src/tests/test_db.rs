//! Port of asterisk/tests/test_db.c
//!
//! Tests AstDB (Asterisk internal key-value database) operations:
//!
//! - put/get/del: basic CRUD with various key/value combinations
//! - gettree/deltree: hierarchical key operations
//! - perftest: bulk insert/delete performance
//! - put_get_long: large value storage and retrieval
//!
//! Since we do not have the Asterisk SQLite-based AstDB, we model it
//! with a HashMap using "family/key" as the compound key.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// AstDB model
// ---------------------------------------------------------------------------

struct AstDb {
    data: HashMap<String, String>,
}

impl AstDb {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    fn put(&mut self, family: &str, key: &str, value: &str) -> Result<(), String> {
        let compound = format!("/{}/{}", family, key);
        self.data.insert(compound, value.to_string());
        Ok(())
    }

    fn get(&self, family: &str, key: &str) -> Option<String> {
        let compound = format!("/{}/{}", family, key);
        self.data.get(&compound).cloned()
    }

    fn del(&mut self, family: &str, key: &str) -> Result<(), String> {
        let compound = format!("/{}/{}", family, key);
        self.data
            .remove(&compound)
            .map(|_| ())
            .ok_or_else(|| "Key not found".to_string())
    }

    /// Get all entries under a family prefix.
    fn gettree(&self, family: &str, subfamily: Option<&str>) -> Vec<(String, String)> {
        let prefix = match subfamily {
            Some(sub) => format!("/{}/{}/", family, sub),
            None => format!("/{}/", family),
        };
        self.data
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Delete all entries under a family prefix. Returns count of deleted entries.
    fn deltree(&mut self, family: &str, subfamily: Option<&str>) -> usize {
        let prefix = match subfamily {
            Some(sub) => format!("/{}/{}/", family, sub),
            None => format!("/{}/", family),
        };
        let keys: Vec<String> = self
            .data
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        let count = keys.len();
        for k in keys {
            self.data.remove(&k);
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(put_get_del).
///
/// Test basic put/get/del with various key/value combinations.
#[test]
fn test_put_get_del() {
    let long_val = "x".repeat(244);

    let inputs: Vec<(&str, &str, &str)> = vec![
        ("family", "key", "value"),
        ("astdbtest", "a", "b"),
        ("astdbtest", "a", "a"),
        ("astdbtest", "b", "a"),
        ("astdbtest", "b", "b"),
        ("astdbtest", "b", "!@#$%^&*()|+-<>?"),
        ("astdbtest", &long_val, "b"),
        ("astdbtest", "b", &long_val),
        ("astdbtest", "!@#$%^&*()|+-<>?", "b"),
    ];

    let mut db = AstDb::new();

    for (family, key, value) in &inputs {
        assert!(
            db.put(family, key, value).is_ok(),
            "Failed to put {}/{}/{}",
            family,
            key,
            value
        );

        let got = db.get(family, key);
        assert!(got.is_some(), "Failed to get {}/{}", family, key);
        assert_eq!(
            got.unwrap(),
            *value,
            "Value mismatch for {}/{}",
            family,
            key
        );

        assert!(
            db.del(family, key).is_ok(),
            "Failed to del {}/{}",
            family,
            key
        );

        assert!(
            db.get(family, key).is_none(),
            "Key {}/{} should be deleted",
            family,
            key
        );
    }
}

/// Port of AST_TEST_DEFINE(gettree_deltree).
///
/// Test hierarchical operations with family/subfamily structure.
#[test]
fn test_gettree_deltree() {
    let mut db = AstDb::new();

    let inputs = [
        ("astdbtest/one", "one", "blah"),
        ("astdbtest/one", "two", "bling"),
        ("astdbtest/one", "three", "blast"),
        ("astdbtest/two", "one", "blah"),
        ("astdbtest/two", "two", "bling"),
        ("astdbtest/two", "three", "blast"),
    ];

    for (family, key, value) in &inputs {
        db.put(family, key, value).unwrap();
    }

    // Get all entries under "astdbtest"
    let all = db.gettree("astdbtest", None);
    assert_eq!(all.len(), 6, "Expected 6 entries under astdbtest");

    // Verify all entries are present
    for (family, key, value) in &inputs {
        let compound = format!("/{}/{}", family, key);
        let found = all.iter().any(|(k, v)| k == &compound && v == value);
        assert!(found, "Missing entry {}/{}", family, key);
    }

    // Get entries under "astdbtest/one" subfam
    let sub1 = db.gettree("astdbtest", Some("one"));
    assert_eq!(sub1.len(), 3, "Expected 3 entries under astdbtest/one");

    // Delete "astdbtest/two" subtree
    let deleted = db.deltree("astdbtest", Some("two"));
    assert_eq!(deleted, 3, "Expected 3 deletions from astdbtest/two");

    // Delete remaining "astdbtest" subtree
    let deleted = db.deltree("astdbtest", None);
    assert_eq!(deleted, 3, "Expected 3 deletions from remaining astdbtest");

    assert!(db.data.is_empty());
}

/// Port of AST_TEST_DEFINE(perftest).
///
/// Test bulk insert/delete performance.
#[test]
fn test_db_perftest() {
    let mut db = AstDb::new();

    for x in 0..10_000u64 {
        let key = x.to_string();
        db.put("astdbtest", &key, &key).unwrap();
    }

    assert_eq!(db.gettree("astdbtest", None).len(), 10_000);

    let deleted = db.deltree("astdbtest", None);
    assert_eq!(deleted, 10_000);
}

/// Port of AST_TEST_DEFINE(put_get_long).
///
/// Test storing and retrieving large values.
#[test]
fn test_put_get_long() {
    let mut db = AstDb::new();
    let fill = "abcdefghijklmnopqrstuvwxyz123456";

    let mut size = 1024;
    while size <= 1024 * 1024 {
        let value: String = fill.repeat(size / fill.len() + 1);
        let value = &value[..size];

        db.put("astdbtest", "long_key", value).unwrap();

        let got = db.get("astdbtest", "long_key");
        assert!(got.is_some(), "Failed to get value of {} bytes", size);
        assert_eq!(
            got.unwrap(),
            value,
            "Value mismatch at {} bytes",
            size
        );

        db.del("astdbtest", "long_key").unwrap();
        size *= 2;
    }
}
