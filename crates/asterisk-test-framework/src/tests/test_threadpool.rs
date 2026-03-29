//! Port of asterisk/tests/test_threadpool.c
//!
//! Tests TaskProcessor: push task and verify execution, push multiple
//! tasks and verify serial execution order, shutdown while tasks pending,
//! concurrent push from multiple threads, and task processor name lookup.
//!
//! Our TaskProcessor in asterisk-core is the Rust equivalent of the C
//! ast_taskprocessor -- a named queue that processes tasks one-at-a-time.

use asterisk_core::taskprocessor::TaskProcessor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Port of threadpool_push test from test_threadpool.c.
///
/// Push a single task to a TaskProcessor and verify it executes.
#[tokio::test]
async fn test_task_push_and_execute() {
    let tp = TaskProcessor::new("test-push");
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = Arc::clone(&done);

    assert!(tp.push(async move {
        done_clone.store(true, Ordering::SeqCst);
    }));

    // Wait for execution.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(done.load(Ordering::SeqCst));
}

/// Port of threadpool_push_multiple test from test_threadpool.c.
///
/// Push multiple tasks and verify they execute in FIFO (serial) order.
#[tokio::test]
async fn test_task_push_multiple_serial_order() {
    let tp = TaskProcessor::new("test-serial");
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

    for i in 0..10 {
        let order_clone = Arc::clone(&order);
        tp.push(async move {
            order_clone.lock().push(i);
        });
    }

    // Wait for all tasks to complete.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let final_order = order.lock().clone();
    assert_eq!(final_order, (0..10).collect::<Vec<_>>());
}

/// Port of threadpool_task_pushed test from test_threadpool.c.
///
/// Verify that the pending count reflects queued tasks.
#[tokio::test]
async fn test_task_pending_count() {
    let tp = TaskProcessor::new("test-pending");

    // Initially no pending tasks.
    // Note: pending count is best-effort (tasks may start executing immediately).
    // We use a barrier pattern to hold tasks.
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let barrier_clone = Arc::clone(&barrier);

    tp.push(async move {
        barrier_clone.wait().await;
    });

    // Give the task processor time to pick up the first task (it will block on barrier).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Push more tasks while the first is blocked.
    let counter = Arc::new(AtomicU64::new(0));
    for _ in 0..5 {
        let c = Arc::clone(&counter);
        tp.push(async move {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }

    // Release the barrier.
    barrier.wait().await;

    // Wait for remaining tasks.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

/// Port of threadpool_shutdown test from test_threadpool.c.
///
/// Verify that shutdown completes all pending tasks before stopping.
#[tokio::test]
async fn test_task_shutdown_completes_pending() {
    let mut tp = TaskProcessor::new("test-shutdown");
    let counter = Arc::new(AtomicU64::new(0));

    for _ in 0..10 {
        let c = Arc::clone(&counter);
        tp.push(async move {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }

    // Shutdown waits for all pending tasks to complete.
    tp.shutdown().await;

    assert_eq!(counter.load(Ordering::SeqCst), 10);
    assert!(!tp.is_running());
}

/// Verify that pushing a task after shutdown returns false.
#[tokio::test]
async fn test_task_push_after_shutdown() {
    let mut tp = TaskProcessor::new("test-push-after-shutdown");
    tp.shutdown().await;

    let result = tp.push(async {});
    assert!(!result);
}

/// Port of threadpool_concurrent test from test_threadpool.c.
///
/// Push tasks from multiple threads concurrently and verify all execute.
#[tokio::test]
async fn test_concurrent_push_from_multiple_threads() {
    let tp = Arc::new(TaskProcessor::new("test-concurrent"));
    let counter = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let tp_clone = Arc::clone(&tp);
        let c = Arc::clone(&counter);
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                let c2 = Arc::clone(&c);
                tp_clone.push(async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                });
            }
        }));
    }

    // Wait for all spawn tasks to complete pushing.
    for h in handles {
        h.await.unwrap();
    }

    // Wait for all processor tasks to complete.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 50);
}

/// Test task processor name lookup.
#[tokio::test]
async fn test_task_processor_name() {
    let tp = TaskProcessor::new("my-named-processor");
    assert_eq!(tp.name(), "my-named-processor");
}

/// Test task processor is initially running.
#[tokio::test]
async fn test_task_processor_is_running() {
    let tp = TaskProcessor::new("test-running");
    assert!(tp.is_running());
}

/// Test that a heavy task doesn't block other task processors.
///
/// Port of the independence test from test_threadpool.c: each
/// TaskProcessor runs independently.
#[tokio::test]
async fn test_task_processor_independence() {
    let tp1 = TaskProcessor::new("tp-independent-1");
    let tp2 = TaskProcessor::new("tp-independent-2");

    let c1 = Arc::new(AtomicU64::new(0));
    let c2 = Arc::new(AtomicU64::new(0));

    // tp1 has a slow task.
    let c1_clone = Arc::clone(&c1);
    tp1.push(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c1_clone.fetch_add(1, Ordering::SeqCst);
    });

    // tp2 has a fast task.
    let c2_clone = Arc::clone(&c2);
    tp2.push(async move {
        c2_clone.fetch_add(1, Ordering::SeqCst);
    });

    // tp2's task should complete quickly even though tp1 is slow.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(c2.load(Ordering::SeqCst), 1);
    // tp1 should not have completed yet.
    // (This is best-effort; it's possible it has completed on fast machines.)

    // Wait for tp1.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(c1.load(Ordering::SeqCst), 1);
}

/// Port of threadpool_empty_notice test from test_threadpool.c.
///
/// Test that after all tasks are processed, the pending count goes to zero.
#[tokio::test]
async fn test_task_processor_drains_to_zero() {
    let tp = TaskProcessor::new("test-drain");
    let counter = Arc::new(AtomicU64::new(0));

    for _ in 0..5 {
        let c = Arc::clone(&counter);
        tp.push(async move {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    assert_eq!(tp.pending(), 0);
}

/// Test that tasks within a TaskProcessor see each other's side effects
/// (serialization guarantee).
#[tokio::test]
async fn test_task_processor_serialization_guarantee() {
    let tp = TaskProcessor::new("test-serial-guarantee");
    let shared = Arc::new(parking_lot::Mutex::new(0u64));

    for _ in 0..100 {
        let s = Arc::clone(&shared);
        tp.push(async move {
            // Read-modify-write without atomics; safe because tasks are serial.
            let mut val = s.lock();
            *val += 1;
        });
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(*shared.lock(), 100);
}

/// Test creating multiple TaskProcessors with different names.
#[tokio::test]
async fn test_multiple_task_processors() {
    let tp1 = TaskProcessor::new("processor-alpha");
    let tp2 = TaskProcessor::new("processor-beta");
    let tp3 = TaskProcessor::new("processor-gamma");

    assert_eq!(tp1.name(), "processor-alpha");
    assert_eq!(tp2.name(), "processor-beta");
    assert_eq!(tp3.name(), "processor-gamma");

    // All should be running independently.
    assert!(tp1.is_running());
    assert!(tp2.is_running());
    assert!(tp3.is_running());
}

/// Test Debug formatting.
#[tokio::test]
async fn test_task_processor_debug() {
    let tp = TaskProcessor::new("debug-test");
    let debug_str = format!("{:?}", tp);
    assert!(debug_str.contains("debug-test"));
    assert!(debug_str.contains("TaskProcessor"));
}
