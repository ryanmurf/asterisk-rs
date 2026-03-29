//! Port of asterisk/tests/test_sched.c
//!
//! Tests scheduler operations: schedule with delay, cancel,
//! schedule with zero delay (immediate), and pending count.

use asterisk_core::scheduler::Scheduler;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Port of sched_test_order: schedule and cancel callbacks.
///
/// In C, this tests that ast_sched_wait() returns the correct value
/// as entries are added and removed. In Rust we test the equivalent
/// using the Scheduler's pending count and cancel behavior.
#[tokio::test]
async fn test_sched_add_and_cancel() {
    let sched = Scheduler::new();

    // Initially no pending tasks
    assert_eq!(sched.pending(), 0);

    // Schedule three tasks with different delays
    let id1 = sched.schedule_ms(100000, async {});
    assert_eq!(sched.pending(), 1);

    let id2 = sched.schedule_ms(10000, async {});
    assert_eq!(sched.pending(), 2);

    let id3 = sched.schedule_ms(1000, async {});
    assert_eq!(sched.pending(), 3);

    // Cancel in reverse order
    assert!(sched.cancel(id3));
    assert_eq!(sched.pending(), 2);

    assert!(sched.cancel(id2));
    assert_eq!(sched.pending(), 1);

    assert!(sched.cancel(id1));
    assert_eq!(sched.pending(), 0);
}

/// Test that cancelling a non-existent task returns false.
#[tokio::test]
async fn test_sched_cancel_nonexistent() {
    let sched = Scheduler::new();
    let fake_id = asterisk_core::scheduler::SchedId(99999);
    assert!(!sched.cancel(fake_id));
}

/// Test that double-cancelling returns false the second time.
#[tokio::test]
async fn test_sched_double_cancel() {
    let sched = Scheduler::new();
    let id = sched.schedule_ms(100000, async {});

    assert!(sched.cancel(id));
    assert!(!sched.cancel(id)); // already cancelled
}

/// Port of zero-delay (immediate) scheduling from test_sched.c.
///
/// Test that scheduling with zero delay causes the callback to execute
/// quickly without blocking.
#[tokio::test]
async fn test_sched_zero_delay() {
    let sched = Scheduler::new();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    sched.schedule_ms(0, async move {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    // Wait a bit for the task to execute
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Test that a scheduled task actually executes after the delay.
#[tokio::test]
async fn test_sched_delayed_execution() {
    let sched = Scheduler::new();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    sched.schedule_ms(50, async move {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    // Should not have run yet
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Wait for it
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// Test that a cancelled task does not execute.
#[tokio::test]
async fn test_sched_cancelled_does_not_execute() {
    let sched = Scheduler::new();
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let id = sched.schedule_ms(50, async move {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });

    // Cancel before it runs
    assert!(sched.cancel(id));

    // Wait well past the delay
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0); // should not have run
}

/// Test multiple tasks executing in order.
///
/// Port of the ordering checks from test_sched.c where tasks scheduled
/// at different times should execute in the correct order.
#[tokio::test]
async fn test_sched_ordering() {
    let sched = Scheduler::new();
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

    let o1 = Arc::clone(&order);
    sched.schedule_ms(10, async move {
        o1.lock().push(1);
    });

    let o2 = Arc::clone(&order);
    sched.schedule_ms(50, async move {
        o2.lock().push(2);
    });

    let o3 = Arc::clone(&order);
    sched.schedule_ms(100, async move {
        o3.lock().push(3);
    });

    // Wait for all tasks to complete
    tokio::time::sleep(Duration::from_millis(200)).await;

    let final_order = order.lock().clone();
    assert_eq!(final_order, vec![1, 2, 3]);
}

/// Test cancel_all clears all pending tasks.
#[tokio::test]
async fn test_sched_cancel_all() {
    let sched = Scheduler::new();

    sched.schedule_ms(100000, async {});
    sched.schedule_ms(100000, async {});
    sched.schedule_ms(100000, async {});
    assert_eq!(sched.pending(), 3);

    sched.cancel_all();
    assert_eq!(sched.pending(), 0);
}

/// Test scheduler drop cancels pending tasks.
#[tokio::test]
async fn test_sched_drop_cleanup() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    {
        let sched = Scheduler::new();
        sched.schedule_ms(100, async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });
        // Scheduler is dropped here
    }

    // Wait past the delay
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The task should have been cancelled by drop
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

/// Test scheduling from within a scheduled task (reschedule pattern).
#[tokio::test]
async fn test_sched_reschedule() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = Arc::clone(&counter);

    let sched = Arc::new(Scheduler::new());
    let sched_clone = Arc::clone(&sched);

    sched.schedule_ms(10, async move {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        // Note: in practice, rescheduling requires access to the scheduler.
        // This is valid because Scheduler is Send + Sync.
        let c2 = Arc::clone(&counter_clone);
        sched_clone.schedule_ms(10, async move {
            c2.fetch_add(1, Ordering::SeqCst);
        });
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
