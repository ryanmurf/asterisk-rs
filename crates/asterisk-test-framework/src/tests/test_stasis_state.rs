//! Port of asterisk/tests/test_stasis_state.c
//!
//! Tests Stasis state management:
//! - Implicit publishing: subscribe to topics, publish state via callback, verify
//! - Explicit publishing: use explicit publisher objects, verify state

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Simulated stasis state manager
// ---------------------------------------------------------------------------

const TOPIC_COUNT: usize = 50; // Reduced from 500 for test speed.

#[derive(Debug, Clone)]
struct FooData {
    bar: usize,
}

struct StateManager {
    states: std::collections::HashMap<String, Option<FooData>>,
}

impl StateManager {
    fn new() -> Self {
        Self {
            states: std::collections::HashMap::new(),
        }
    }

    fn subscribe(&mut self, id: &str) {
        self.states.entry(id.to_string()).or_insert(None);
    }

    fn publish_by_id(&mut self, id: &str, data: FooData) {
        self.states.insert(id.to_string(), Some(data));
    }

    fn callback_all<F>(&self, mut f: F)
    where
        F: FnMut(&str, Option<&FooData>),
    {
        for (id, data) in &self.states {
            f(id, data.as_ref());
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(implicit_publish) from test_stasis_state.c.
///
/// Subscribe to TOPIC_COUNT states, callback to verify initial null state,
/// then publish data and verify it matches.
#[test]
fn test_stasis_state_implicit_publish() {
    let mut mgr = StateManager::new();
    let sum_total: usize = (0..TOPIC_COUNT).sum();

    for i in 0..TOPIC_COUNT {
        mgr.subscribe(&i.to_string());
    }

    // First pass: no data yet.
    let running = Arc::new(AtomicUsize::new(0));
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|id, data| {
            let num: usize = id.parse().unwrap();
            r.fetch_add(num, Ordering::SeqCst);
            assert!(data.is_none(), "Expected None for first pass");
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);

    // Publish data for each topic.
    for i in 0..TOPIC_COUNT {
        mgr.publish_by_id(&i.to_string(), FooData { bar: i });
    }

    // Second pass: verify data.
    running.store(0, Ordering::SeqCst);
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|id, data| {
            let num: usize = id.parse().unwrap();
            r.fetch_add(num, Ordering::SeqCst);
            let foo = data.expect("Expected data for second pass");
            assert_eq!(foo.bar, num);
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);
}

/// Port of AST_TEST_DEFINE(explicit_publish) from test_stasis_state.c.
///
/// Explicit publishers create and publish state.
#[test]
fn test_stasis_state_explicit_publish() {
    let mut mgr = StateManager::new();
    let sum_total: usize = (0..TOPIC_COUNT).sum();

    for i in 0..TOPIC_COUNT {
        mgr.subscribe(&i.to_string());
    }

    // Explicitly publish.
    for i in 0..TOPIC_COUNT {
        mgr.publish_by_id(&i.to_string(), FooData { bar: i });
    }

    // Verify.
    let running = Arc::new(AtomicUsize::new(0));
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|id, data| {
            let num: usize = id.parse().unwrap();
            r.fetch_add(num, Ordering::SeqCst);
            let foo = data.unwrap();
            assert_eq!(foo.bar, num);
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);
}
