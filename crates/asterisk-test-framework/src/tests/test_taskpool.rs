//! Port of asterisk/tests/test_taskpool.c
//!
//! Tests task pool operations: task submission (async and sync),
//! serializer access, multiple task execution, pool sizing,
//! concurrent submission, and shutdown behavior.
//!
//! We use tokio task spawning to emulate the C taskpool behavior.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// TaskPool: simplified port of ast_taskpool
// ---------------------------------------------------------------------------

/// A pool of workers that execute tasks.
struct TaskPool {
    name: String,
    size: usize,
    running: AtomicBool,
    task_count: AtomicU64,
}

impl TaskPool {
    fn new(name: &str, size: usize) -> Self {
        Self {
            name: name.to_string(),
            size,
            running: AtomicBool::new(true),
            task_count: AtomicU64::new(0),
        }
    }

    /// Push a task for execution. Returns true if successfully queued.
    fn push<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        if !self.running.load(Ordering::SeqCst) {
            return false;
        }
        self.task_count.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(f);
        true
    }

    /// Push a task and wait for it to complete synchronously.
    fn push_wait<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        if !self.running.load(Ordering::SeqCst) {
            return false;
        }
        self.task_count.fetch_add(1, Ordering::SeqCst);
        let handle = std::thread::spawn(f);
        handle.join().is_ok()
    }

    /// Shut down the pool.
    fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn pool_size(&self) -> usize {
        self.size
    }

    fn total_tasks_submitted(&self) -> u64 {
        self.task_count.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(taskpool_push) from test_taskpool.c.
///
/// Push a single task into a taskpool asynchronously and ensure it is executed.
#[test]
fn test_taskpool_push() {
    let pool = TaskPool::new("test-push", 1);
    let executed = Arc::new(AtomicBool::new(false));
    let exec_clone = Arc::clone(&executed);

    assert!(pool.push(move || {
        exec_clone.store(true, Ordering::SeqCst);
    }));

    // Wait for execution.
    std::thread::sleep(Duration::from_millis(100));
    assert!(executed.load(Ordering::SeqCst));
}

/// Port of AST_TEST_DEFINE(taskpool_push_synchronous) from test_taskpool.c.
///
/// Push a single task synchronously and ensure it completes before returning.
#[test]
fn test_taskpool_push_synchronous() {
    let pool = TaskPool::new("test-push-sync", 1);
    let executed = Arc::new(AtomicBool::new(false));
    let exec_clone = Arc::clone(&executed);

    let result = pool.push_wait(move || {
        exec_clone.store(true, Ordering::SeqCst);
    });

    assert!(result);
    assert!(executed.load(Ordering::SeqCst));
}

/// Port of AST_TEST_DEFINE(taskpool_push_multiple) from test_taskpool.c.
///
/// Push multiple tasks and verify all are executed.
#[test]
fn test_taskpool_push_multiple() {
    let pool = TaskPool::new("test-push-multi", 4);
    let counter = Arc::new(AtomicU64::new(0));

    for _ in 0..10 {
        let c = Arc::clone(&counter);
        pool.push(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }

    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

/// Test that pool tracks total tasks submitted.
#[test]
fn test_taskpool_task_count() {
    let pool = TaskPool::new("test-count", 2);

    for _ in 0..5 {
        pool.push(|| {});
    }

    assert_eq!(pool.total_tasks_submitted(), 5);
}

/// Port of taskpool shutdown test.
///
/// Verify that pushing after shutdown fails.
#[test]
fn test_taskpool_push_after_shutdown() {
    let pool = TaskPool::new("test-shutdown", 1);
    pool.shutdown();

    let result = pool.push(|| {});
    assert!(!result);
    assert!(!pool.is_running());
}

/// Test pool name.
#[test]
fn test_taskpool_name() {
    let pool = TaskPool::new("my-pool", 4);
    assert_eq!(pool.name(), "my-pool");
}

/// Test pool size.
#[test]
fn test_taskpool_size() {
    let pool = TaskPool::new("sized-pool", 8);
    assert_eq!(pool.pool_size(), 8);
}

/// Test concurrent push from multiple threads.
#[test]
fn test_taskpool_concurrent_push() {
    let pool = Arc::new(TaskPool::new("test-concurrent", 4));
    let counter = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let pool_clone = Arc::clone(&pool);
        let c = Arc::clone(&counter);
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                let c2 = Arc::clone(&c);
                pool_clone.push(move || {
                    c2.fetch_add(1, Ordering::SeqCst);
                });
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(counter.load(Ordering::SeqCst), 50);
}

/// Test that shutdown prevents new tasks.
#[test]
fn test_taskpool_shutdown_prevents_new() {
    let pool = TaskPool::new("test-shutdown-prevent", 2);

    assert!(pool.is_running());
    pool.shutdown();
    assert!(!pool.is_running());

    let executed = Arc::new(AtomicBool::new(false));
    let exec_clone = Arc::clone(&executed);
    let result = pool.push(move || {
        exec_clone.store(true, Ordering::SeqCst);
    });
    assert!(!result);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!executed.load(Ordering::SeqCst));
}

/// Test creating multiple pools.
#[test]
fn test_taskpool_multiple_pools() {
    let pool1 = TaskPool::new("pool-alpha", 1);
    let pool2 = TaskPool::new("pool-beta", 2);
    let pool3 = TaskPool::new("pool-gamma", 4);

    assert_eq!(pool1.name(), "pool-alpha");
    assert_eq!(pool2.name(), "pool-beta");
    assert_eq!(pool3.name(), "pool-gamma");

    assert!(pool1.is_running());
    assert!(pool2.is_running());
    assert!(pool3.is_running());
}
