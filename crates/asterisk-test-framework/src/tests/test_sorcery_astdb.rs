//! Port of asterisk/tests/test_sorcery_astdb.c
//!
//! Tests Sorcery persistence with an AstDB-like backend:
//! - Object creation
//! - Retrieve by ID
//! - Retrieve by field value
//! - Retrieve multiple (all)
//! - Retrieve multiple by field
//! - Retrieve by regex
//! - Object update
//! - Update of uncreated object
//! - Object deletion
//! - Delete of uncreated object

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Simulated AstDB-backed sorcery wizard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TestObject {
    id: String,
    bob: u32,
    joe: u32,
}

impl TestObject {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            bob: 5,
            joe: 10,
        }
    }
}

/// A sorcery wizard backed by an in-memory HashMap (simulating AstDB).
struct AstDbWizard {
    db: HashMap<String, TestObject>,
}

impl AstDbWizard {
    fn new() -> Self {
        Self {
            db: HashMap::new(),
        }
    }

    fn create(&mut self, obj: &TestObject) -> Result<(), &'static str> {
        if self.db.contains_key(&obj.id) {
            return Err("Object already exists");
        }
        self.db.insert(obj.id.clone(), obj.clone());
        Ok(())
    }

    fn retrieve_by_id(&self, id: &str) -> Option<&TestObject> {
        self.db.get(id)
    }

    fn retrieve_by_field(&self, field: &str, value: &str) -> Option<&TestObject> {
        self.db.values().find(|obj| {
            let v = match field {
                "bob" => obj.bob.to_string(),
                "joe" => obj.joe.to_string(),
                _ => return false,
            };
            v == value
        })
    }

    fn retrieve_all(&self) -> Vec<&TestObject> {
        self.db.values().collect()
    }

    fn retrieve_multiple_by_field(&self, field: &str, op: &str, value: u32) -> Vec<&TestObject> {
        self.db
            .values()
            .filter(|obj| {
                let v = match field {
                    "bob" => obj.bob,
                    "joe" => obj.joe,
                    _ => return false,
                };
                match op {
                    ">=" => v >= value,
                    "<" => v < value,
                    "=" => v == value,
                    _ => false,
                }
            })
            .collect()
    }

    fn retrieve_by_regex(&self, pattern: &str) -> Vec<&TestObject> {
        let re = regex::Regex::new(pattern).unwrap();
        self.db
            .values()
            .filter(|obj| re.is_match(&obj.id))
            .collect()
    }

    fn update(&mut self, obj: &TestObject) -> Result<(), &'static str> {
        if !self.db.contains_key(&obj.id) {
            return Err("Object not found");
        }
        self.db.insert(obj.id.clone(), obj.clone());
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), &'static str> {
        if self.db.remove(id).is_some() {
            Ok(())
        } else {
            Err("Object not found")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(object_create) from test_sorcery_astdb.c.
#[test]
fn test_object_create() {
    let mut wizard = AstDbWizard::new();
    let obj = TestObject::new("blah");

    assert!(wizard.create(&obj).is_ok());
    assert!(wizard.retrieve_by_id("blah").is_some());
}

/// Port of AST_TEST_DEFINE(object_retrieve_id) from test_sorcery_astdb.c.
#[test]
fn test_object_retrieve_id() {
    let mut wizard = AstDbWizard::new();

    wizard.create(&TestObject::new("blah")).unwrap();
    wizard.create(&TestObject::new("blah2")).unwrap();

    let obj = wizard.retrieve_by_id("blah").unwrap();
    assert_eq!(obj.id, "blah");
}

/// Port of AST_TEST_DEFINE(object_retrieve_field) from test_sorcery_astdb.c.
#[test]
fn test_object_retrieve_field() {
    let mut wizard = AstDbWizard::new();
    let mut obj = TestObject::new("blah");
    obj.joe = 42;
    wizard.create(&obj).unwrap();

    // Should find by field.
    let found = wizard.retrieve_by_field("joe", "42");
    assert!(found.is_some());

    // Should not find with wrong value.
    let not_found = wizard.retrieve_by_field("joe", "49");
    assert!(not_found.is_none());
}

/// Port of AST_TEST_DEFINE(object_retrieve_multiple_all) from test_sorcery_astdb.c.
#[test]
fn test_object_retrieve_multiple_all() {
    let mut wizard = AstDbWizard::new();
    wizard.create(&TestObject::new("blah")).unwrap();
    wizard.create(&TestObject::new("blah2")).unwrap();

    let all = wizard.retrieve_all();
    assert_eq!(all.len(), 2);
}

/// Port of AST_TEST_DEFINE(object_retrieve_multiple_field) from test_sorcery_astdb.c.
#[test]
fn test_object_retrieve_multiple_field() {
    let mut wizard = AstDbWizard::new();
    let mut obj = TestObject::new("blah");
    obj.joe = 6;
    wizard.create(&obj).unwrap();

    let found = wizard.retrieve_multiple_by_field("joe", ">=", 6);
    assert_eq!(found.len(), 1);

    let not_found = wizard.retrieve_multiple_by_field("joe", "<", 6);
    assert_eq!(not_found.len(), 0);
}

/// Port of AST_TEST_DEFINE(object_retrieve_regex) from test_sorcery_astdb.c.
#[test]
fn test_object_retrieve_regex() {
    let mut wizard = AstDbWizard::new();
    wizard.create(&TestObject::new("blah-98joe")).unwrap();
    wizard.create(&TestObject::new("blah-93joe")).unwrap();
    wizard.create(&TestObject::new("neener-93joe")).unwrap();

    let found = wizard.retrieve_by_regex("^blah-");
    assert_eq!(found.len(), 2);
}

/// Port of AST_TEST_DEFINE(object_update) from test_sorcery_astdb.c.
#[test]
fn test_object_update() {
    let mut wizard = AstDbWizard::new();
    wizard.create(&TestObject::new("blah")).unwrap();

    let mut updated = TestObject::new("blah");
    updated.bob = 1000;
    updated.joe = 2000;
    assert!(wizard.update(&updated).is_ok());

    let obj = wizard.retrieve_by_id("blah").unwrap();
    assert_eq!(obj.bob, 1000);
    assert_eq!(obj.joe, 2000);
}

/// Port of AST_TEST_DEFINE(object_update_uncreated) from test_sorcery_astdb.c.
#[test]
fn test_object_update_uncreated() {
    let mut wizard = AstDbWizard::new();
    let obj = TestObject::new("blah");

    // Updating a non-existent object should fail.
    assert!(wizard.update(&obj).is_err());
}

/// Port of AST_TEST_DEFINE(object_delete) from test_sorcery_astdb.c.
#[test]
fn test_object_delete() {
    let mut wizard = AstDbWizard::new();
    wizard.create(&TestObject::new("blah")).unwrap();

    assert!(wizard.delete("blah").is_ok());
    assert!(wizard.retrieve_by_id("blah").is_none());
}

/// Port of AST_TEST_DEFINE(object_delete_uncreated) from test_sorcery_astdb.c.
#[test]
fn test_object_delete_uncreated() {
    let mut wizard = AstDbWizard::new();

    // Deleting a non-existent object should fail.
    assert!(wizard.delete("blah").is_err());
}
