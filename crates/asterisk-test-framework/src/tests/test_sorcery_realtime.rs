//! Port of asterisk/tests/test_sorcery_realtime.c
//!
//! Tests Sorcery with a realtime (dynamic configuration) backend:
//! - Object creation
//! - Retrieve by ID
//! - Retrieve by field value
//! - Retrieve multiple (all)
//! - Retrieve multiple by field
//! - Retrieve by regex (prefix)
//! - Object update
//! - Update of uncreated object
//! - Object deletion
//! - Delete of uncreated object
//! - Field defaults
//! - Object copy
//! - Diff between objects

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Simulated realtime backend
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RealtimeObject {
    id: String,
    fields: HashMap<String, String>,
}

impl RealtimeObject {
    fn new(id: &str) -> Self {
        let mut fields = HashMap::new();
        fields.insert("bob".to_string(), "5".to_string());
        fields.insert("joe".to_string(), "10".to_string());
        Self {
            id: id.to_string(),
            fields,
        }
    }

    fn get_u32(&self, field: &str) -> u32 {
        self.fields
            .get(field)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    fn set_u32(&mut self, field: &str, value: u32) {
        self.fields.insert(field.to_string(), value.to_string());
    }
}

struct RealtimeWizard {
    objects: HashMap<String, RealtimeObject>,
}

impl RealtimeWizard {
    fn new() -> Self {
        Self {
            objects: HashMap::new(),
        }
    }

    fn create(&mut self, obj: &RealtimeObject) -> Result<(), &'static str> {
        if self.objects.contains_key(&obj.id) {
            return Err("Already exists");
        }
        self.objects.insert(obj.id.clone(), obj.clone());
        Ok(())
    }

    fn retrieve_by_id(&self, id: &str) -> Option<&RealtimeObject> {
        self.objects.get(id)
    }

    fn retrieve_by_field(&self, field: &str, value: &str) -> Option<&RealtimeObject> {
        self.objects.values().find(|obj| {
            obj.fields.get(field).map(|v| v.as_str()) == Some(value)
        })
    }

    fn retrieve_all(&self) -> Vec<&RealtimeObject> {
        self.objects.values().collect()
    }

    fn retrieve_multiple_by_field(&self, field: &str, op: &str, value: u32) -> Vec<&RealtimeObject> {
        self.objects.values().filter(|obj| {
            let v = obj.get_u32(field);
            match op {
                ">=" => v >= value,
                "<" => v < value,
                "=" => v == value,
                _ => false,
            }
        }).collect()
    }

    fn retrieve_by_regex(&self, pattern: &str) -> Vec<&RealtimeObject> {
        let re = regex::Regex::new(pattern).unwrap();
        self.objects.values().filter(|obj| re.is_match(&obj.id)).collect()
    }

    fn update(&mut self, obj: &RealtimeObject) -> Result<(), &'static str> {
        if !self.objects.contains_key(&obj.id) {
            return Err("Not found");
        }
        self.objects.insert(obj.id.clone(), obj.clone());
        Ok(())
    }

    fn delete(&mut self, id: &str) -> Result<(), &'static str> {
        self.objects.remove(id).map(|_| ()).ok_or("Not found")
    }
}

fn diff_objects(a: &RealtimeObject, b: &RealtimeObject) -> Vec<(String, String, String)> {
    let mut diffs = Vec::new();
    for (key, val_a) in &a.fields {
        if let Some(val_b) = b.fields.get(key) {
            if val_a != val_b {
                diffs.push((key.clone(), val_a.clone(), val_b.clone()));
            }
        }
    }
    diffs
}

fn copy_object(obj: &RealtimeObject) -> RealtimeObject {
    obj.clone()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of object_create from test_sorcery_realtime.c.
#[test]
fn test_realtime_object_create() {
    let mut wizard = RealtimeWizard::new();
    let obj = RealtimeObject::new("blah");
    assert!(wizard.create(&obj).is_ok());
    assert!(wizard.retrieve_by_id("blah").is_some());
}

/// Port of object_retrieve_id.
#[test]
fn test_realtime_object_retrieve_id() {
    let mut wizard = RealtimeWizard::new();
    wizard.create(&RealtimeObject::new("blah")).unwrap();
    wizard.create(&RealtimeObject::new("blah2")).unwrap();

    let obj = wizard.retrieve_by_id("blah").unwrap();
    assert_eq!(obj.id, "blah");
}

/// Port of object_retrieve_field.
#[test]
fn test_realtime_object_retrieve_field() {
    let mut wizard = RealtimeWizard::new();
    let mut obj = RealtimeObject::new("blah");
    obj.set_u32("joe", 42);
    wizard.create(&obj).unwrap();

    assert!(wizard.retrieve_by_field("joe", "42").is_some());
    assert!(wizard.retrieve_by_field("joe", "49").is_none());
}

/// Port of object_retrieve_multiple_all.
#[test]
fn test_realtime_object_retrieve_all() {
    let mut wizard = RealtimeWizard::new();
    wizard.create(&RealtimeObject::new("blah")).unwrap();
    wizard.create(&RealtimeObject::new("blah2")).unwrap();

    assert_eq!(wizard.retrieve_all().len(), 2);
}

/// Port of object_retrieve_multiple_field.
#[test]
fn test_realtime_object_retrieve_multiple_field() {
    let mut wizard = RealtimeWizard::new();
    let mut obj = RealtimeObject::new("blah");
    obj.set_u32("joe", 6);
    wizard.create(&obj).unwrap();

    let found = wizard.retrieve_multiple_by_field("joe", ">=", 6);
    assert_eq!(found.len(), 1);

    let not_found = wizard.retrieve_multiple_by_field("joe", "<", 6);
    assert_eq!(not_found.len(), 0);
}

/// Port of object_retrieve_regex.
#[test]
fn test_realtime_object_retrieve_regex() {
    let mut wizard = RealtimeWizard::new();
    wizard.create(&RealtimeObject::new("blah-98joe")).unwrap();
    wizard.create(&RealtimeObject::new("blah-93joe")).unwrap();
    wizard.create(&RealtimeObject::new("neener-93joe")).unwrap();

    let found = wizard.retrieve_by_regex("^blah-");
    assert_eq!(found.len(), 2);
}

/// Port of object_update.
#[test]
fn test_realtime_object_update() {
    let mut wizard = RealtimeWizard::new();
    wizard.create(&RealtimeObject::new("blah")).unwrap();

    let mut updated = RealtimeObject::new("blah");
    updated.set_u32("bob", 1000);
    updated.set_u32("joe", 2000);
    assert!(wizard.update(&updated).is_ok());

    let obj = wizard.retrieve_by_id("blah").unwrap();
    assert_eq!(obj.get_u32("bob"), 1000);
    assert_eq!(obj.get_u32("joe"), 2000);
}

/// Port of object_update_uncreated.
#[test]
fn test_realtime_object_update_uncreated() {
    let mut wizard = RealtimeWizard::new();
    assert!(wizard.update(&RealtimeObject::new("blah")).is_err());
}

/// Port of object_delete.
#[test]
fn test_realtime_object_delete() {
    let mut wizard = RealtimeWizard::new();
    wizard.create(&RealtimeObject::new("blah")).unwrap();

    assert!(wizard.delete("blah").is_ok());
    assert!(wizard.retrieve_by_id("blah").is_none());
}

/// Port of object_delete_uncreated.
#[test]
fn test_realtime_object_delete_uncreated() {
    let mut wizard = RealtimeWizard::new();
    assert!(wizard.delete("blah").is_err());
}

/// Test field defaults.
#[test]
fn test_realtime_field_defaults() {
    let obj = RealtimeObject::new("test");
    assert_eq!(obj.get_u32("bob"), 5);
    assert_eq!(obj.get_u32("joe"), 10);
}

/// Test object copy.
#[test]
fn test_realtime_object_copy() {
    let obj = RealtimeObject::new("original");
    let copy = copy_object(&obj);
    assert_eq!(copy.id, "original");
    assert_eq!(copy.get_u32("bob"), obj.get_u32("bob"));
}

/// Test object diff.
#[test]
fn test_realtime_object_diff() {
    let mut a = RealtimeObject::new("test");
    let mut b = RealtimeObject::new("test");
    b.set_u32("bob", 999);

    let diffs = diff_objects(&a, &b);
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0].0, "bob");
    assert_eq!(diffs[0].1, "5");
    assert_eq!(diffs[0].2, "999");
}
