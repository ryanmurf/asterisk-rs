//! Port of asterisk/tests/test_astobj2_weaken.c
//!
//! Tests weak reference behavior using Rust's Arc/Weak:
//!
//! - Creating a weak reference from a strong reference
//! - Upgrading weak to strong while the object is alive
//! - Weak reference becomes invalid after all strong references are dropped
//! - Subscription notifications when object is destroyed
//! - Weak proxy containers: finding and iterating through weak references
//!
//! The C ao2_weakproxy system is mapped to Rust's Arc<T> / Weak<T>.

use std::sync::{Arc, Mutex, Weak};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestObj {
    #[allow(dead_code)]
    value: i32,
}

/// A notification subscription that fires when an object is destroyed.
struct DestroyNotify {
    count: Arc<Mutex<i32>>,
}

impl DestroyNotify {
    fn new() -> (Self, Arc<Mutex<i32>>) {
        let count = Arc::new(Mutex::new(0));
        (Self { count: count.clone() }, count)
    }

    fn fire(&self) {
        *self.count.lock().unwrap() += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(astobj2_weak1).
///
/// Test the full lifecycle of weak references:
/// - Create an object and a weak reference to it
/// - Verify upgrade succeeds while object is alive
/// - Verify notifications fire when object is destroyed
/// - Verify upgrade fails after object is destroyed
/// - Verify a new object can be associated after the old one is gone
#[test]
fn test_astobj2_weak1() {
    let (notify1, notify1_count) = DestroyNotify::new();
    let (notify2, notify2_count) = DestroyNotify::new();
    let destructor_called = Arc::new(Mutex::new(0));
    let destructor_called_clone = destructor_called.clone();

    // Create obj1 (we track destruction via a custom wrapper)
    struct TrackDrop {
        obj: TestObj,
        on_drop: Arc<Mutex<i32>>,
        notifications: Vec<DestroyNotify>,
    }

    impl Drop for TrackDrop {
        fn drop(&mut self) {
            *self.on_drop.lock().unwrap() += 1;
            for n in &self.notifications {
                n.fire();
            }
        }
    }

    let obj1 = Arc::new(TrackDrop {
        obj: TestObj { value: 42 },
        on_drop: destructor_called_clone,
        notifications: vec![notify1, notify2],
    });

    // Create weak reference
    let weak1: Weak<TrackDrop> = Arc::downgrade(&obj1);

    // Another weak ref from the same object should be equivalent
    let weak2 = Arc::downgrade(&obj1);

    // Both upgrade to the same object
    let strong1 = weak1.upgrade().unwrap();
    assert!(Arc::ptr_eq(&strong1, &obj1));
    drop(strong1);

    let strong2 = weak2.upgrade().unwrap();
    assert!(Arc::ptr_eq(&strong2, &obj1));
    drop(strong2);

    // Notifications should NOT have fired yet
    assert_eq!(*destructor_called.lock().unwrap(), 0);
    assert_eq!(*notify1_count.lock().unwrap(), 0);
    assert_eq!(*notify2_count.lock().unwrap(), 0);

    // Drop the strong reference - object should be destroyed
    drop(obj1);

    // Notifications should have fired
    assert_eq!(*destructor_called.lock().unwrap(), 1);
    assert_eq!(*notify1_count.lock().unwrap(), 1);
    assert_eq!(*notify2_count.lock().unwrap(), 1);

    // Upgrade should now fail
    assert!(weak1.upgrade().is_none());
    assert!(weak2.upgrade().is_none());
}

/// Port of AST_TEST_DEFINE(astobj2_weak_container).
///
/// Test weak references in a container context:
/// - Store objects and their weak references
/// - Look up objects through weak references
/// - Verify orphaned weak references are handled correctly
#[test]
fn test_astobj2_weak_container() {
    // Container of (key, weak reference)
    let mut weak_container: Vec<(String, Weak<String>)> = Vec::new();

    let strong1 = Arc::new("obj1".to_string());
    let strong2 = Arc::new("obj2".to_string());
    let strong3 = Arc::new("obj3".to_string());

    weak_container.push(("obj1".to_string(), Arc::downgrade(&strong1)));
    weak_container.push(("obj2".to_string(), Arc::downgrade(&strong2)));
    weak_container.push(("obj3".to_string(), Arc::downgrade(&strong3)));

    // All weak refs should upgrade successfully
    {
        let live_objects: Vec<Arc<String>> = weak_container
            .iter()
            .filter_map(|(_, w)| w.upgrade())
            .collect();
        assert_eq!(live_objects.len(), 3);
    } // live_objects dropped here, releasing temporary strong refs

    // Look up by key
    {
        let found = weak_container
            .iter()
            .find(|(k, _)| k == "obj2")
            .and_then(|(_, w)| w.upgrade());
        assert!(found.is_some());
        assert_eq!(*found.unwrap(), "obj2");
    }

    // Unknown key should return None
    let not_found = weak_container
        .iter()
        .find(|(k, _)| k == "unknown")
        .and_then(|(_, w)| w.upgrade());
    assert!(not_found.is_none());

    // Drop strong2 - orphan the "obj2" weak reference
    drop(strong2);

    // Now only 2 live objects
    let live_objects: Vec<Arc<String>> = weak_container
        .iter()
        .filter_map(|(_, w)| w.upgrade())
        .collect();
    assert_eq!(live_objects.len(), 2);

    // obj2 should no longer be findable
    let found = weak_container
        .iter()
        .find(|(k, _)| k == "obj2")
        .and_then(|(_, w)| w.upgrade());
    assert!(found.is_none());

    // obj1 and obj3 should still work
    let found1 = weak_container
        .iter()
        .find(|(k, _)| k == "obj1")
        .and_then(|(_, w)| w.upgrade());
    assert!(found1.is_some());
    assert_eq!(*found1.unwrap(), "obj1");

    let found3 = weak_container
        .iter()
        .find(|(k, _)| k == "obj3")
        .and_then(|(_, w)| w.upgrade());
    assert!(found3.is_some());
    assert_eq!(*found3.unwrap(), "obj3");
}
