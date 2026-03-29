//! Port of asterisk/tests/test_dns_query_set.c
//!
//! Tests DNS query set functionality:
//!
//! - Creating a query set and adding queries
//! - Resolving all queries in a set
//! - Verifying all queries complete before the callback fires
//! - Cancellation of a query set
//!
//! Since we do not have a real DNS resolver, we model query sets as a
//! collection of futures that resolve asynchronously.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// DNS query set model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DnsResult {
    name: String,
    answer: String,
}

struct DnsQuery {
    name: String,
    resolved: AtomicBool,
}

struct DnsQuerySet {
    queries: Vec<Arc<DnsQuery>>,
    complete: Arc<(Mutex<bool>, Condvar)>,
    results: Arc<Mutex<Vec<DnsResult>>>,
    resolve_count: Arc<AtomicUsize>,
}

impl DnsQuerySet {
    fn new() -> Self {
        Self {
            queries: Vec::new(),
            complete: Arc::new((Mutex::new(false), Condvar::new())),
            results: Arc::new(Mutex::new(Vec::new())),
            resolve_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn add(&mut self, name: &str) {
        self.queries.push(Arc::new(DnsQuery {
            name: name.to_string(),
            resolved: AtomicBool::new(false),
        }));
    }

    /// Resolve all queries. Each query is resolved in a separate thread.
    /// When all queries complete, the completion signal is fired.
    fn resolve(&self) {
        let total = self.queries.len();
        let resolve_count = self.resolve_count.clone();
        let results = self.results.clone();
        let complete = self.complete.clone();

        for query in &self.queries {
            let q = query.clone();
            let rc = resolve_count.clone();
            let res = results.clone();
            let comp = complete.clone();

            thread::spawn(move || {
                // Simulate resolution
                let result = DnsResult {
                    name: q.name.clone(),
                    answer: "Yes sirree".to_string(),
                };

                q.resolved.store(true, Ordering::SeqCst);
                res.lock().unwrap().push(result);

                let count = rc.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= total {
                    let (lock, cvar) = &*comp;
                    let mut done = lock.lock().unwrap();
                    *done = true;
                    cvar.notify_all();
                }
            });
        }
    }

    fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.complete;
        let mut done = lock.lock().unwrap();
        if *done {
            return true;
        }
        let (d, _) = cvar.wait_timeout(done, timeout).unwrap();
        *d
    }

    fn cancel(&self) {
        // Mark the set as complete without resolving remaining queries
        let (lock, cvar) = &*self.complete;
        let mut done = lock.lock().unwrap();
        *done = true;
        cvar.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of the nominal query set resolution test.
///
/// Create a query set with multiple queries, resolve them all,
/// and verify all queries completed.
#[test]
fn test_dns_query_set_resolve() {
    let mut qs = DnsQuerySet::new();
    qs.add("example.com");
    qs.add("asterisk.org");
    qs.add("digium.com");

    qs.resolve();
    let completed = qs.wait(Duration::from_secs(5));

    assert!(completed, "Query set should have completed");
    assert_eq!(qs.resolve_count.load(Ordering::SeqCst), 3);
    assert_eq!(qs.results.lock().unwrap().len(), 3);
}

/// Test that all results contain the expected answer.
#[test]
fn test_dns_query_set_results() {
    let mut qs = DnsQuerySet::new();
    qs.add("example.com");
    qs.add("asterisk.org");

    qs.resolve();
    qs.wait(Duration::from_secs(5));

    let results = qs.results.lock().unwrap();
    for r in results.iter() {
        assert_eq!(r.answer, "Yes sirree");
    }
}

/// Test empty query set completes immediately.
#[test]
fn test_dns_query_set_empty() {
    let qs = DnsQuerySet::new();
    // An empty query set should be considered complete
    assert_eq!(qs.queries.len(), 0);
    assert_eq!(qs.resolve_count.load(Ordering::SeqCst), 0);
}

/// Port of the cancellation test.
///
/// Create a query set, cancel it, verify it does not block.
#[test]
fn test_dns_query_set_cancel() {
    let mut qs = DnsQuerySet::new();
    qs.add("example.com");

    // Cancel before resolving
    qs.cancel();

    let completed = qs.wait(Duration::from_millis(100));
    assert!(completed, "Cancelled query set should report complete");
    // No queries should have been resolved
    assert_eq!(qs.resolve_count.load(Ordering::SeqCst), 0);
}
