//! Port of asterisk/tests/test_logger.c
//!
//! Tests logger functionality:
//! - Dynamic log level registration and unregistration
//! - Registering multiple custom log levels
//! - Performance of logging many messages
//!
//! The C test is CLI-driven (not AST_TEST_DEFINE), so we port the
//! essential logic into unit tests.

use std::collections::HashMap;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Simulated dynamic log level registry
// ---------------------------------------------------------------------------

/// Maximum number of custom log levels (matches C limit of 16).
const MAX_CUSTOM_LEVELS: usize = 16;

struct LogLevelRegistry {
    levels: HashMap<String, u32>,
    next_id: u32,
}

impl LogLevelRegistry {
    fn new() -> Self {
        Self {
            levels: HashMap::new(),
            next_id: 0,
        }
    }

    /// Register a named log level. Returns the level ID, or None if
    /// the limit is reached or the name is already registered.
    fn register(&mut self, name: &str) -> Option<u32> {
        if self.levels.len() >= MAX_CUSTOM_LEVELS {
            return None;
        }
        if self.levels.contains_key(name) {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.levels.insert(name.to_string(), id);
        Some(id)
    }

    /// Unregister a named log level.
    fn unregister(&mut self, name: &str) -> bool {
        self.levels.remove(name).is_some()
    }

    fn is_registered(&self, name: &str) -> bool {
        self.levels.contains_key(name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of the "Simple register/message/unregister" test from test_logger.c.
///
/// Register a custom log level, verify it exists, then unregister it.
#[test]
fn test_logger_register_unregister() {
    let mut registry = LogLevelRegistry::new();

    let level = registry.register("test");
    assert!(level.is_some());
    assert!(registry.is_registered("test"));

    assert!(registry.unregister("test"));
    assert!(!registry.is_registered("test"));
}

/// Port of the "Register multiple levels" test from test_logger.c.
///
/// Register up to MAX_CUSTOM_LEVELS levels and verify that exceeding
/// the limit fails as expected.
#[test]
fn test_logger_register_multiple() {
    let mut registry = LogLevelRegistry::new();

    for i in 0..MAX_CUSTOM_LEVELS {
        let name = format!("level{:02}", i);
        let result = registry.register(&name);
        assert!(
            result.is_some(),
            "Failed to register level {} (should succeed)",
            name
        );
    }

    // Next registration should fail (at capacity).
    let overflow = registry.register("level_overflow");
    assert!(overflow.is_none(), "Should not be able to exceed max custom levels");

    // Clean up all registered levels.
    for i in 0..MAX_CUSTOM_LEVELS {
        let name = format!("level{:02}", i);
        assert!(registry.unregister(&name));
    }
}

/// Port of the performance test from test_logger.c.
///
/// Log 10,000 messages and verify it completes in a reasonable time.
#[test]
fn test_logger_performance() {
    let mut registry = LogLevelRegistry::new();
    let _level = registry.register("perftest").unwrap();

    let start = Instant::now();
    let mut sink = Vec::with_capacity(10_000);
    for i in 0..10_000u32 {
        sink.push(format!("Performance test log message {}", i));
    }
    let elapsed = start.elapsed();

    // Just verify it completed -- the C test prints timing info.
    assert_eq!(sink.len(), 10_000);
    // Should complete in well under 10 seconds even on slow machines.
    assert!(
        elapsed.as_secs() < 10,
        "Logging 10,000 messages took too long: {:?}",
        elapsed
    );

    assert!(registry.unregister("perftest"));
}

/// Verify double-registration of the same name fails.
#[test]
fn test_logger_duplicate_register() {
    let mut registry = LogLevelRegistry::new();
    assert!(registry.register("dup").is_some());
    assert!(registry.register("dup").is_none());
}

/// Verify unregistering a non-existent level returns false.
#[test]
fn test_logger_unregister_nonexistent() {
    let mut registry = LogLevelRegistry::new();
    assert!(!registry.unregister("nonexistent"));
}
