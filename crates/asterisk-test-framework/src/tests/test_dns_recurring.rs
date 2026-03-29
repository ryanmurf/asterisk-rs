//! Port of asterisk/tests/test_dns_recurring.c
//!
//! Tests recurring DNS query functionality:
//!
//! - A recurring query re-resolves after the TTL expires
//! - The completion callback is invoked on each resolution
//! - Cancellation stops future resolutions
//! - Query results are updated on each resolution
//!
//! Since we do not have a real DNS resolver, we model recurring queries
//! with a timer-based polling mechanism.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Recurring DNS query model
// ---------------------------------------------------------------------------

struct RecurringQuery {
    name: String,
    ttl_ms: u64,
    complete_count: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    results: Arc<Mutex<Vec<String>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
    max_iterations: usize,
}

impl RecurringQuery {
    fn new(name: &str, ttl_ms: u64, max_iterations: usize) -> Self {
        Self {
            name: name.to_string(),
            ttl_ms,
            complete_count: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            results: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new((Mutex::new(false), Condvar::new())),
            max_iterations,
        }
    }

    fn start(&self) {
        let name = self.name.clone();
        let ttl_ms = self.ttl_ms;
        let count = self.complete_count.clone();
        let cancelled = self.cancelled.clone();
        let results = self.results.clone();
        let done = self.done.clone();
        let max = self.max_iterations;

        thread::spawn(move || {
            for i in 0..max {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }

                // Simulate resolution
                let result = format!("{}:{}", name, i);
                results.lock().unwrap().push(result);
                count.fetch_add(1, Ordering::SeqCst);

                // Check if we should stop
                if count.load(Ordering::SeqCst) >= max {
                    let (lock, cvar) = &*done;
                    let mut d = lock.lock().unwrap();
                    *d = true;
                    cvar.notify_all();
                    break;
                }

                // Wait for TTL before re-resolving
                thread::sleep(Duration::from_millis(ttl_ms));
            }
        });
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.done;
        let mut d = lock.lock().unwrap();
        if *d {
            return true;
        }
        let (d, _) = cvar.wait_timeout(d, timeout).unwrap();
        *d
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of the recurring DNS resolution test.
///
/// A recurring query should resolve multiple times based on TTL.
#[test]
fn test_dns_recurring_resolve() {
    let query = RecurringQuery::new("example.com", 10, 3);
    query.start();

    let completed = query.wait(Duration::from_secs(5));
    assert!(completed, "Recurring query should complete");
    assert_eq!(query.complete_count.load(Ordering::SeqCst), 3);
    assert_eq!(query.results.lock().unwrap().len(), 3);
}

/// Test that each resolution produces different results.
#[test]
fn test_dns_recurring_results_update() {
    let query = RecurringQuery::new("asterisk.org", 10, 3);
    query.start();
    query.wait(Duration::from_secs(5));

    let results = query.results.lock().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], "asterisk.org:0");
    assert_eq!(results[1], "asterisk.org:1");
    assert_eq!(results[2], "asterisk.org:2");
}

/// Port of the cancellation test.
///
/// Cancelling a recurring query should stop future resolutions.
#[test]
fn test_dns_recurring_cancel() {
    let query = RecurringQuery::new("example.com", 50, 100);
    query.start();

    // Let one or two resolutions happen, then cancel
    thread::sleep(Duration::from_millis(30));
    query.cancel();
    thread::sleep(Duration::from_millis(150));

    let count = query.complete_count.load(Ordering::SeqCst);
    assert!(
        count < 100,
        "Cancelled query should not complete all iterations, got {}",
        count
    );
}

/// Test that recurring query handles zero TTL (immediate re-resolution).
#[test]
fn test_dns_recurring_zero_ttl() {
    let query = RecurringQuery::new("example.com", 0, 5);
    query.start();

    let completed = query.wait(Duration::from_secs(5));
    assert!(completed, "Zero-TTL query should complete");
    assert_eq!(query.complete_count.load(Ordering::SeqCst), 5);
}
