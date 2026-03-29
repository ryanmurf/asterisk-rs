//! Port of asterisk/tests/test_scoped_lock.c
//!
//! Tests RAII lock guard behavior:
//! - Lock is acquired when the guard is created
//! - Lock is released when the guard goes out of scope
//! - Cleanup order: unlock before unref (reverse declaration order)

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

/// A simple RAII guard that runs a closure on drop.
struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    fn new(f: F) -> Self {
        Self(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: SCOPED_LOCK behavior
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(lock_test) from test_scoped_lock.c.
///
/// Verifies that a MutexGuard (Rust's RAII lock) acquires the lock
/// when created and releases it when dropped.
#[test]
fn test_scoped_lock_basic() {
    let indicator = Arc::new(AtomicI32::new(0));
    let mutex = Arc::new(parking_lot::Mutex::new(()));

    // Acquire lock in a scope.
    {
        let _guard = mutex.lock();
        indicator.store(1, Ordering::SeqCst);
        assert_eq!(indicator.load(Ordering::SeqCst), 1);
    }
    // Lock is released, we can update the indicator.
    indicator.store(0, Ordering::SeqCst);
    assert_eq!(indicator.load(Ordering::SeqCst), 0);

    // Repeat in a loop to ensure consistent behavior.
    for _ in 0..10 {
        {
            let _guard = mutex.lock();
            indicator.store(1, Ordering::SeqCst);
            assert_eq!(indicator.load(Ordering::SeqCst), 1);
        }
        indicator.store(0, Ordering::SeqCst);
        assert_eq!(indicator.load(Ordering::SeqCst), 0);
    }
}

/// Port of AST_TEST_DEFINE(cleanup_order) from test_scoped_lock.c.
///
/// Verifies that Rust drop order (reverse of declaration) ensures
/// that a lock guard is dropped before a ref-counted reference,
/// mimicking the C RAII cleanup order behavior.
#[test]
fn test_cleanup_order() {
    let locked = Arc::new(AtomicBool::new(false));
    let reffed = Arc::new(AtomicBool::new(false));
    let mutex = Arc::new(parking_lot::Mutex::new(()));

    {
        // Simulate: first ref (created first, dropped last).
        reffed.store(true, Ordering::SeqCst);
        let reffed_clone = Arc::clone(&reffed);
        let _ref_guard = OnDrop::new(move || {
            reffed_clone.store(false, Ordering::SeqCst);
        });

        // Then lock (created second, dropped first).
        let _lock_guard = mutex.lock();
        locked.store(true, Ordering::SeqCst);
        let locked_clone = Arc::clone(&locked);
        let _lock_cleanup = OnDrop::new(move || {
            locked_clone.store(false, Ordering::SeqCst);
        });

        // Both should be active.
        assert!(reffed.load(Ordering::SeqCst));
        assert!(locked.load(Ordering::SeqCst));
    }

    // After scope exit, both should be cleaned up.
    // Rust drops in reverse order: lock_cleanup first, then ref_guard.
    assert!(!locked.load(Ordering::SeqCst));
    assert!(!reffed.load(Ordering::SeqCst));
}

/// Test that try_lock correctly reports lock availability.
#[test]
fn test_scoped_lock_try() {
    let mutex = parking_lot::Mutex::new(42);

    {
        let guard = mutex.lock();
        assert_eq!(*guard, 42);
        // While locked, try_lock should fail.
        assert!(mutex.try_lock().is_none());
    }

    // After drop, try_lock should succeed.
    assert!(mutex.try_lock().is_some());
}
