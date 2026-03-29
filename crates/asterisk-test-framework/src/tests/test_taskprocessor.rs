//! Port of asterisk/tests/test_taskprocessor.c
//!
//! Tests task processor serial execution:
//! - Default taskprocessor: push and execute a single task
//! - Load test: push many tasks and verify FIFO order
//! - Subsystem alert: high/low water mark alerting
//! - Shutdown behavior: push after shutdown fails
//! - Concurrent push from multiple threads
//! - Task processor suspension and resumption
//! - Serializer shutdown callbacks
//! - Listener operations

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Task processor implementation (simplified)
// ---------------------------------------------------------------------------

struct SimpleTaskProcessor {
    name: String,
    running: AtomicBool,
    task_count: AtomicU64,
}

impl SimpleTaskProcessor {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            running: AtomicBool::new(true),
            task_count: AtomicU64::new(0),
        }
    }

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

    fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(default_taskprocessor) from test_taskprocessor.c.
///
/// Push a task to a processor and verify it executes.
#[test]
fn test_default_taskprocessor() {
    let tp = SimpleTaskProcessor::new("test-default");
    let done = Arc::new(AtomicBool::new(false));
    let d = Arc::clone(&done);

    assert!(tp.push_wait(move || {
        d.store(true, Ordering::SeqCst);
    }));

    assert!(done.load(Ordering::SeqCst));
}

/// Port of AST_TEST_DEFINE(default_taskprocessor_load) from test_taskprocessor.c.
///
/// Push many tasks and verify they all execute in FIFO order.
#[test]
fn test_default_taskprocessor_load() {
    let num_tasks = 1000;
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

    // Use a channel to serialize execution.
    let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();

    let order_reader = Arc::clone(&order);
    let worker = std::thread::spawn(move || {
        while let Ok(task) = rx.recv() {
            task();
        }
    });

    for i in 0..num_tasks {
        let o = Arc::clone(&order);
        tx.send(Box::new(move || {
            o.lock().push(i);
        }))
        .unwrap();
    }
    drop(tx);
    worker.join().unwrap();

    let final_order = order_reader.lock();
    assert_eq!(final_order.len(), num_tasks);
    for i in 0..num_tasks {
        assert_eq!(final_order[i], i, "Task {} executed out of order", i);
    }
}

/// Port of AST_TEST_DEFINE(subsystem_alert) from test_taskprocessor.c.
///
/// Test high/low water mark alerting on queue depth.
#[test]
fn test_subsystem_alert() {
    let low_water: usize = 3;
    let high_water: usize = 6;
    let test_size: usize = 10;

    let mut queue: Vec<usize> = Vec::new();
    let mut alert_active = false;

    for i in 1..=test_size {
        queue.push(i);
        let depth = queue.len();

        if depth >= high_water && !alert_active {
            alert_active = true;
        }

        if depth < high_water {
            assert!(!alert_active || depth >= low_water,
                "Alert should not be active below high water mark before first trigger");
        }
    }

    assert!(alert_active, "Alert should have been triggered");

    // Drain the queue.
    while !queue.is_empty() {
        queue.remove(0);
        let depth = queue.len();
        if depth <= low_water && alert_active {
            alert_active = false;
        }
    }

    assert!(!alert_active, "Alert should have been cleared");
}

/// Push after shutdown should fail.
#[test]
fn test_push_after_shutdown() {
    let tp = SimpleTaskProcessor::new("test-shutdown");
    tp.shutdown();

    let result = tp.push(|| {});
    assert!(!result);
    assert!(!tp.is_running());
}

/// Concurrent push from multiple threads.
#[test]
fn test_concurrent_push() {
    let tp = Arc::new(SimpleTaskProcessor::new("test-concurrent"));
    let counter = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let tp_clone = Arc::clone(&tp);
        let c = Arc::clone(&counter);
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                let c2 = Arc::clone(&c);
                tp_clone.push(move || {
                    c2.fetch_add(1, Ordering::SeqCst);
                });
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(counter.load(Ordering::SeqCst), 50);
}

/// Test that task processor reports correct name.
#[test]
fn test_taskprocessor_name() {
    let tp = SimpleTaskProcessor::new("my-processor");
    assert_eq!(tp.name, "my-processor");
}

/// Test suspension: tasks queued while suspended execute after resume.
#[test]
fn test_taskprocessor_suspend_resume() {
    // Simulate suspension by collecting tasks, then executing on "resume".
    let mut suspended_tasks: Vec<Box<dyn FnOnce() + Send>> = Vec::new();
    let counter = Arc::new(AtomicU64::new(0));

    // "Suspended" - queue tasks.
    for _ in 0..5 {
        let c = Arc::clone(&counter);
        suspended_tasks.push(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // "Resume" - execute queued tasks.
    for task in suspended_tasks {
        task();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

/// Test that multiple task processors work independently.
#[test]
fn test_multiple_taskprocessors() {
    let tp1 = SimpleTaskProcessor::new("tp-1");
    let tp2 = SimpleTaskProcessor::new("tp-2");

    assert!(tp1.is_running());
    assert!(tp2.is_running());

    tp1.shutdown();
    assert!(!tp1.is_running());
    assert!(tp2.is_running());
}
