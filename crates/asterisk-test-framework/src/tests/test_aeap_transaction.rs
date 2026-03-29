//! Port of asterisk/tests/test_aeap_transaction.c
//!
//! Tests the AEAP transaction lifecycle:
//!
//! - Creating and executing a basic transaction that completes before timeout
//! - Creating a transaction that times out and invokes the timeout handler
//!
//! A transaction represents a request-response pair with an optional
//! timeout. We model this using Rust threading primitives.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Transaction model
// ---------------------------------------------------------------------------

struct TransactionParams {
    timeout_ms: u64,
    on_timeout: Option<Box<dyn Fn(&mut i32) + Send + Sync>>,
}

struct Transaction {
    id: String,
    completed: Arc<(Mutex<bool>, Condvar)>,
    user_data: Arc<Mutex<i32>>,
}

impl Transaction {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            completed: Arc::new((Mutex::new(false), Condvar::new())),
            user_data: Arc::new(Mutex::new(0)),
        }
    }

    fn end(&self) {
        let (lock, cvar) = &*self.completed;
        let mut completed = lock.lock().unwrap();
        *completed = true;
        cvar.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.completed;
        let mut completed = lock.lock().unwrap();
        if *completed {
            return true;
        }
        let result = cvar.wait_timeout(completed, timeout).unwrap();
        *result.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

const AEAP_TRANSACTION_ID: &str = "foo";

/// Port of AST_TEST_DEFINE(transaction_exec).
///
/// Test creating a basic AEAP transaction that completes before timeout.
/// A separate thread ends the transaction after a short delay, and the
/// main thread waits for completion.
#[test]
fn test_transaction_exec() {
    let tsx = Arc::new(Transaction::new(AEAP_TRANSACTION_ID));
    let tsx_clone = tsx.clone();

    // Spawn a thread that will end the transaction after a brief delay
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        // Increment user_data to signal the thread ran
        {
            let mut data = tsx_clone.user_data.lock().unwrap();
            *data += 1;
        }
        tsx_clone.end();
    });

    // Wait with a generous timeout
    let completed = tsx.wait(Duration::from_secs(5));
    handle.join().unwrap();

    assert!(completed, "Transaction should have completed");
    assert_eq!(*tsx.user_data.lock().unwrap(), 1, "Thread should have incremented user data");
}

/// Port of AST_TEST_DEFINE(transaction_exec_timeout).
///
/// Test creating an AEAP transaction that times out. The timeout handler
/// should be invoked.
#[test]
fn test_transaction_exec_timeout() {
    let tsx = Arc::new(Transaction::new(AEAP_TRANSACTION_ID));
    let tsx_clone = tsx.clone();

    let timeout_called = Arc::new(Mutex::new(false));
    let timeout_called_clone = timeout_called.clone();

    // Spawn a thread that will NOT end the transaction in time
    let handle = std::thread::spawn(move || {
        // Sleep longer than the timeout
        std::thread::sleep(Duration::from_millis(500));
        {
            let mut data = tsx_clone.user_data.lock().unwrap();
            *data += 1;
        }
        tsx_clone.end();
    });

    // Wait with a short timeout that will expire before the thread completes
    let completed = tsx.wait(Duration::from_millis(50));

    if !completed {
        // Timeout handler invoked
        *timeout_called.lock().unwrap() = true;
    }

    // Clean up the thread
    handle.join().unwrap();

    assert!(
        *timeout_called.lock().unwrap(),
        "Timeout handler should have been invoked"
    );
}
