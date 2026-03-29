//! Port of asterisk/tests/test_optional_api.c
//!
//! Tests optional API binding:
//! - Provide implementation before use (provide first)
//! - Use before provide (stub first, then provide)
//! - Unprovide reverts to stub
//! - Available/unavailable API detection
//! - Graceful degradation when API not available
//!
//! In Rust, this maps to Option<fn()> or trait objects with default
//! implementations.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Optional API system mirroring Asterisk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum WasCalled {
    None = 0,
    Stub = 1,
    Impl = 2,
}

impl From<u8> for WasCalled {
    fn from(v: u8) -> Self {
        match v {
            0 => WasCalled::None,
            1 => WasCalled::Stub,
            2 => WasCalled::Impl,
            _ => WasCalled::None,
        }
    }
}

/// An optional API binding. Can be in one of three states:
/// - No binding (calls do nothing / return None)
/// - Stub binding (fallback behavior when implementation is not loaded)
/// - Implementation binding (actual implementation)
struct OptionalApi {
    func_ref: Option<Box<dyn Fn() -> WasCalled + Send + Sync>>,
    stub: Option<Box<dyn Fn() -> WasCalled + Send + Sync>>,
}

impl OptionalApi {
    fn new() -> Self {
        Self {
            func_ref: None,
            stub: None,
        }
    }

    /// Register a stub (fallback) function.
    fn use_api<F: Fn() -> WasCalled + Send + Sync + 'static>(&mut self, stub: F) {
        self.stub = Some(Box::new(stub));
        // If no implementation is provided, use the stub.
        if self.func_ref.is_none() {
            // Don't set func_ref to stub -- we'll check in call()
        }
    }

    /// Provide the actual implementation.
    fn provide<F: Fn() -> WasCalled + Send + Sync + 'static>(&mut self, implementation: F) {
        self.func_ref = Some(Box::new(implementation));
    }

    /// Remove the implementation (revert to stub).
    fn unprovide(&mut self) {
        self.func_ref = None;
    }

    /// Call the optional API. Uses implementation if available, stub if not.
    fn call(&self) -> WasCalled {
        if let Some(ref func) = self.func_ref {
            func()
        } else if let Some(ref stub) = self.stub {
            stub()
        } else {
            WasCalled::None
        }
    }

    /// Check if an implementation is available.
    fn is_available(&self) -> bool {
        self.func_ref.is_some()
    }
}

// ---------------------------------------------------------------------------
// Tests: Provide first (port of test_provide_first)
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(test_provide_first) from test_optional_api.c.
///
/// Provide implementation first, then register a user. The user should
/// get the implementation, not the stub.
#[test]
fn test_provide_first() {
    let mut api = OptionalApi::new();

    // Provide implementation first.
    api.provide(|| WasCalled::Impl);

    // Then register a user with a stub.
    api.use_api(|| WasCalled::Stub);

    // Call should use the implementation.
    assert_eq!(api.call(), WasCalled::Impl);
}

// ---------------------------------------------------------------------------
// Tests: Provide last (port of test_provide_last)
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(test_provide_last) from test_optional_api.c.
///
/// Register a user with a stub first, then provide implementation.
/// Before provide: stub should be called.
/// After provide: implementation should be called.
/// After unprovide: stub should be called again.
#[test]
fn test_provide_last() {
    let mut api = OptionalApi::new();

    // Register user with stub first.
    api.use_api(|| WasCalled::Stub);

    // Before implementation is provided, stub should be called.
    assert_eq!(api.call(), WasCalled::Stub);

    // Provide implementation.
    api.provide(|| WasCalled::Impl);

    // Now implementation should be called.
    assert_eq!(api.call(), WasCalled::Impl);

    // Unprovide implementation.
    api.unprovide();

    // Should revert to stub.
    assert_eq!(api.call(), WasCalled::Stub);
}

// ---------------------------------------------------------------------------
// Tests: No binding
// ---------------------------------------------------------------------------

#[test]
fn test_no_binding() {
    let api = OptionalApi::new();
    assert_eq!(api.call(), WasCalled::None);
    assert!(!api.is_available());
}

// ---------------------------------------------------------------------------
// Tests: Availability detection
// ---------------------------------------------------------------------------

#[test]
fn test_api_available_after_provide() {
    let mut api = OptionalApi::new();
    assert!(!api.is_available());

    api.provide(|| WasCalled::Impl);
    assert!(api.is_available());
}

#[test]
fn test_api_unavailable_after_unprovide() {
    let mut api = OptionalApi::new();
    api.provide(|| WasCalled::Impl);
    assert!(api.is_available());

    api.unprovide();
    assert!(!api.is_available());
}

// ---------------------------------------------------------------------------
// Tests: Stub-only usage
// ---------------------------------------------------------------------------

#[test]
fn test_stub_only() {
    let mut api = OptionalApi::new();
    api.use_api(|| WasCalled::Stub);

    assert_eq!(api.call(), WasCalled::Stub);
    assert!(!api.is_available());
}

// ---------------------------------------------------------------------------
// Tests: Re-provide after unprovide
// ---------------------------------------------------------------------------

#[test]
fn test_reprovide() {
    let mut api = OptionalApi::new();
    api.use_api(|| WasCalled::Stub);

    // Provide, unprovide, provide again.
    api.provide(|| WasCalled::Impl);
    assert_eq!(api.call(), WasCalled::Impl);

    api.unprovide();
    assert_eq!(api.call(), WasCalled::Stub);

    api.provide(|| WasCalled::Impl);
    assert_eq!(api.call(), WasCalled::Impl);
}

// ---------------------------------------------------------------------------
// Tests: Thread safety with Arc+Mutex pattern
// ---------------------------------------------------------------------------

#[test]
fn test_optional_api_thread_safe() {
    let result = Arc::new(AtomicU8::new(0));
    let result_clone = Arc::clone(&result);

    // Simulate thread-safe optional API.
    let api = Arc::new(parking_lot::Mutex::new(OptionalApi::new()));

    // Provide from one "module".
    {
        let mut api_guard = api.lock();
        api_guard.provide(move || {
            result_clone.store(WasCalled::Impl as u8, Ordering::SeqCst);
            WasCalled::Impl
        });
    }

    // Call from another "module".
    {
        let api_guard = api.lock();
        let called = api_guard.call();
        assert_eq!(called, WasCalled::Impl);
    }

    assert_eq!(
        WasCalled::from(result.load(Ordering::SeqCst)),
        WasCalled::Impl
    );
}

// ---------------------------------------------------------------------------
// Tests: Graceful degradation
// ---------------------------------------------------------------------------

/// Test that calling an unimplemented optional API does not panic.
#[test]
fn test_graceful_degradation() {
    let api = OptionalApi::new();
    // Should return WasCalled::None without panicking.
    let result = api.call();
    assert_eq!(result, WasCalled::None);
}

/// Test Rust-native Option<fn> pattern for optional API.
#[test]
fn test_option_fn_pattern() {
    let mut optional_fn: Option<fn() -> i32> = None;

    // API not available.
    assert!(optional_fn.is_none());
    let result = optional_fn.map(|f| f()).unwrap_or(-1);
    assert_eq!(result, -1);

    // API becomes available.
    optional_fn = Some(|| 42);
    assert!(optional_fn.is_some());
    let result = optional_fn.map(|f| f()).unwrap_or(-1);
    assert_eq!(result, 42);

    // API removed.
    optional_fn = None;
    let result = optional_fn.map(|f| f()).unwrap_or(-1);
    assert_eq!(result, -1);
}
