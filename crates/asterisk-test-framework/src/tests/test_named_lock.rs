//! Port of asterisk/tests/test_named_lock.c
//!
//! Tests named mutex/lock operations:
//! - Named lock creation and lookup
//! - Lock/unlock operations
//! - Try-lock behavior (success and failure)
//! - Independent named locks don't block each other
//! - Same-named locks are the same lock
//! - Lock contention between threads

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Named lock registry mirroring Asterisk's ast_named_lock
// ---------------------------------------------------------------------------

/// A registry of named locks.
struct NamedLockRegistry {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl NamedLockRegistry {
    fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create a named lock.
    fn get(&self, keyspace: &str, key: &str) -> Arc<Mutex<()>> {
        let full_key = format!("{}:{}", keyspace, key);
        let mut map = self.locks.lock().unwrap();
        map.entry(full_key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Tests: Named lock creation
// ---------------------------------------------------------------------------

#[test]
fn test_named_lock_create() {
    let registry = NamedLockRegistry::new();
    let lock = registry.get("test", "lock_1");
    // Should be able to lock it immediately.
    let guard = lock.try_lock();
    assert!(guard.is_ok(), "Should be able to lock a newly created lock");
}

#[test]
fn test_named_lock_same_name_same_lock() {
    let registry = NamedLockRegistry::new();
    let lock1 = registry.get("test", "same_name");
    let lock2 = registry.get("test", "same_name");

    // They should be the same Arc (same underlying mutex).
    assert!(Arc::ptr_eq(&lock1, &lock2));
}

#[test]
fn test_named_lock_different_name_different_lock() {
    let registry = NamedLockRegistry::new();
    let lock1 = registry.get("test", "lock_a");
    let lock2 = registry.get("test", "lock_b");

    // Different names should produce different locks.
    assert!(!Arc::ptr_eq(&lock1, &lock2));
}

// ---------------------------------------------------------------------------
// Tests: Lock/unlock operations
// ---------------------------------------------------------------------------

#[test]
fn test_named_lock_lock_unlock() {
    let registry = NamedLockRegistry::new();
    let lock = registry.get("test", "lock_1");

    {
        let _guard = lock.lock().unwrap();
        // Lock is held.
    }
    // Lock is released (guard dropped).

    // Should be able to lock again.
    let guard = lock.try_lock();
    assert!(guard.is_ok());
}

// ---------------------------------------------------------------------------
// Tests: Try-lock behavior
// ---------------------------------------------------------------------------

#[test]
fn test_named_lock_try_lock_success() {
    let registry = NamedLockRegistry::new();
    let lock = registry.get("test", "lock_1");

    let result = lock.try_lock();
    assert!(result.is_ok());
}

#[test]
fn test_named_lock_try_lock_failure() {
    let registry = NamedLockRegistry::new();
    let lock = registry.get("test", "lock_1");

    let _guard = lock.lock().unwrap();
    // Try to lock from the same thread -- should fail (Mutex is not reentrant).
    // Note: std Mutex will deadlock on same thread, so we use try_lock.
    let result = lock.try_lock();
    assert!(result.is_err(), "try_lock should fail when lock is held");
}

// ---------------------------------------------------------------------------
// Tests: Independent named locks
// ---------------------------------------------------------------------------

/// Port of named_lock_test from test_named_lock.c.
///
/// Two independent named locks should not block each other.
#[test]
fn test_named_locks_independent() {
    let registry = Arc::new(NamedLockRegistry::new());

    let lock1 = registry.get("test", "lock_1");
    let lock2 = registry.get("test", "lock_2");

    // Lock both -- they should not interfere.
    let _g1 = lock1.lock().unwrap();
    let g2 = lock2.try_lock();
    assert!(g2.is_ok(), "Independent locks should not block each other");
}

// ---------------------------------------------------------------------------
// Tests: Lock contention between threads
// ---------------------------------------------------------------------------

/// Port of the threaded named_lock_test from test_named_lock.c.
///
/// Two threads contend for the same named lock. The second thread
/// should block until the first releases.
#[test]
fn test_named_lock_contention() {
    let registry = Arc::new(NamedLockRegistry::new());
    let lock = registry.get("test", "contended");

    let lock_clone = Arc::clone(&lock);

    // Thread 1: hold the lock for 200ms.
    let handle = std::thread::spawn(move || {
        let _guard = lock_clone.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });

    // Give thread 1 time to acquire the lock.
    std::thread::sleep(Duration::from_millis(50));

    // Try-lock should fail while thread 1 holds it.
    let try_result = lock.try_lock();
    assert!(try_result.is_err(), "Lock should be held by thread 1");

    let start = Instant::now();
    // Blocking lock should succeed after thread 1 releases.
    let _guard = lock.lock().unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(100),
        "Expected to wait at least 100ms, waited {:?}",
        elapsed
    );

    handle.join().unwrap();
}

/// Test that two threads with different named locks proceed independently.
#[test]
fn test_named_lock_two_threads_independent() {
    let registry = Arc::new(NamedLockRegistry::new());

    let reg1 = Arc::clone(&registry);
    let reg2 = Arc::clone(&registry);

    let start = Instant::now();

    let h1 = std::thread::spawn(move || {
        let lock = reg1.get("test", "independent_1");
        let _guard = lock.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });

    let h2 = std::thread::spawn(move || {
        let lock = reg2.get("test", "independent_2");
        let _guard = lock.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });

    h1.join().unwrap();
    h2.join().unwrap();

    let elapsed = start.elapsed();
    // Both threads should run in parallel (~200ms total, not ~400ms).
    assert!(
        elapsed < Duration::from_millis(400),
        "Independent locks should run in parallel, took {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Tests: RwLock named lock variant
// ---------------------------------------------------------------------------

#[test]
fn test_named_rwlock() {
    let lock = Arc::new(RwLock::new(42));

    // Multiple readers.
    let r1 = lock.read().unwrap();
    let r2 = lock.read().unwrap();
    assert_eq!(*r1, 42);
    assert_eq!(*r2, 42);
    drop(r1);
    drop(r2);

    // Single writer.
    {
        let mut w = lock.write().unwrap();
        *w = 100;
    }

    let r3 = lock.read().unwrap();
    assert_eq!(*r3, 100);
}
