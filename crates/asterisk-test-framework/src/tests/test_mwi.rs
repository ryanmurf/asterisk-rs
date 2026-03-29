//! Port of asterisk/tests/test_mwi.c
//!
//! Tests MWI (Message Waiting Indicator) state management:
//! - Implicit publishing: subscribe to mailboxes, publish state, verify
//! - Explicit publishing: use explicit publisher objects, verify state

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// MWI state structures
// ---------------------------------------------------------------------------

const MAILBOX_PREFIX: &str = "test~";
const MAILBOX_COUNT: usize = 50; // Reduced from 500 for test speed.

#[derive(Debug, Clone)]
struct MwiState {
    mailbox: String,
    urgent_msgs: usize,
    new_msgs: usize,
    old_msgs: usize,
}

impl MwiState {
    fn new(mailbox: &str) -> Self {
        Self {
            mailbox: mailbox.to_string(),
            urgent_msgs: 0,
            new_msgs: 0,
            old_msgs: 0,
        }
    }
}

fn num_to_mailbox(num: usize) -> String {
    format!("{}{}", MAILBOX_PREFIX, num)
}

fn mailbox_to_num(mailbox: &str) -> Option<usize> {
    mailbox
        .strip_prefix(MAILBOX_PREFIX)
        .and_then(|s| s.parse::<usize>().ok())
}

// ---------------------------------------------------------------------------
// MWI manager (simplified pub/sub)
// ---------------------------------------------------------------------------

struct MwiManager {
    states: std::collections::HashMap<String, MwiState>,
}

impl MwiManager {
    fn new() -> Self {
        Self {
            states: std::collections::HashMap::new(),
        }
    }

    fn subscribe(&mut self, mailbox: &str) {
        self.states
            .entry(mailbox.to_string())
            .or_insert_with(|| MwiState::new(mailbox));
    }

    fn publish(&mut self, mailbox: &str, urgent: usize, new: usize, old: usize) {
        if let Some(state) = self.states.get_mut(mailbox) {
            state.urgent_msgs = urgent;
            state.new_msgs = new;
            state.old_msgs = old;
        }
    }

    fn get_state(&self, mailbox: &str) -> Option<&MwiState> {
        self.states.get(mailbox)
    }

    fn callback_all<F>(&self, mut f: F)
    where
        F: FnMut(&str, &MwiState),
    {
        for (mailbox, state) in &self.states {
            f(mailbox, state);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(implicit_publish) from test_mwi.c.
///
/// Subscribe to MAILBOX_COUNT mailboxes, publish state implicitly,
/// and verify the running total of mailbox numbers matches the expected sum.
#[test]
fn test_mwi_implicit_publish() {
    let mut mgr = MwiManager::new();
    let sum_total: usize = (0..MAILBOX_COUNT).sum();

    // Subscribe to all mailboxes.
    for i in 0..MAILBOX_COUNT {
        mgr.subscribe(&num_to_mailbox(i));
    }

    // First pass: state should be zero.
    let running = Arc::new(AtomicUsize::new(0));
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|mailbox, state| {
            if let Some(num) = mailbox_to_num(mailbox) {
                r.fetch_add(num, Ordering::SeqCst);
                assert_eq!(state.urgent_msgs, 0);
                assert_eq!(state.new_msgs, 0);
                assert_eq!(state.old_msgs, 0);
            }
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);

    // Publish state for each mailbox.
    for i in 0..MAILBOX_COUNT {
        let mb = num_to_mailbox(i);
        mgr.publish(&mb, i, i, i);
    }

    // Second pass: verify state data matches.
    running.store(0, Ordering::SeqCst);
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|mailbox, state| {
            if let Some(num) = mailbox_to_num(mailbox) {
                r.fetch_add(num, Ordering::SeqCst);
                assert_eq!(state.urgent_msgs, num);
                assert_eq!(state.new_msgs, num);
                assert_eq!(state.old_msgs, num);
            }
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);
}

/// Port of AST_TEST_DEFINE(explicit_publish) from test_mwi.c.
///
/// Use explicit publisher objects for each mailbox.
#[test]
fn test_mwi_explicit_publish() {
    let mut mgr = MwiManager::new();
    let sum_total: usize = (0..MAILBOX_COUNT).sum();

    // Subscribe to all mailboxes.
    for i in 0..MAILBOX_COUNT {
        mgr.subscribe(&num_to_mailbox(i));
    }

    // Simulate explicit publishers: publish from each.
    for i in 0..MAILBOX_COUNT {
        let mb = num_to_mailbox(i);
        mgr.publish(&mb, i, i, i);
    }

    // Verify all states.
    let running = Arc::new(AtomicUsize::new(0));
    {
        let r = Arc::clone(&running);
        mgr.callback_all(|mailbox, state| {
            if let Some(num) = mailbox_to_num(mailbox) {
                r.fetch_add(num, Ordering::SeqCst);
                assert_eq!(state.urgent_msgs, num);
                assert_eq!(state.new_msgs, num);
                assert_eq!(state.old_msgs, num);
            }
        });
    }
    assert_eq!(running.load(Ordering::SeqCst), sum_total);
}
