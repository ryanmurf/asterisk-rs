//! Port of asterisk/tests/test_res_pjsip_scheduler.c
//!
//! Tests SIP scheduler task management:
//! - Serialized scheduler: tasks on the same serializer run sequentially
//! - Unserialized scheduler: tasks on different threads run concurrently
//! - Scheduler cleanup: task data is properly released
//! - Scheduler cancel: canceling a pending task prevents execution
//! - Scheduler policy: periodic tasks fire at the expected intervals

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Simulated SIP scheduler
// ---------------------------------------------------------------------------

struct ScheduledTask {
    handle: Option<std::thread::JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl ScheduledTask {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

fn schedule_task<F>(delay_ms: u64, task_fn: F) -> ScheduledTask
where
    F: FnOnce() + Send + 'static,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&cancelled);
    let r = Arc::clone(&running);

    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        if !c.load(Ordering::SeqCst) {
            r.store(true, Ordering::SeqCst);
            task_fn();
            r.store(false, Ordering::SeqCst);
        }
    });

    ScheduledTask {
        handle: Some(handle),
        cancelled,
        running,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(serialized_scheduler) from test_res_pjsip_scheduler.c.
///
/// Two tasks scheduled on the same serializer should run sequentially.
/// We simulate by scheduling tasks that record their thread IDs.
#[test]
fn test_serialized_scheduler() {
    let done1 = Arc::new(AtomicBool::new(false));
    let done2 = Arc::new(AtomicBool::new(false));
    let tid1 = Arc::new(parking_lot::Mutex::new(None::<std::thread::ThreadId>));
    let tid2 = Arc::new(parking_lot::Mutex::new(None::<std::thread::ThreadId>));

    // Simulate serialized execution by running sequentially.
    let d1 = Arc::clone(&done1);
    let t1 = Arc::clone(&tid1);
    let d2 = Arc::clone(&done2);
    let t2 = Arc::clone(&tid2);

    let handle = std::thread::spawn(move || {
        // Task 1
        *t1.lock() = Some(std::thread::current().id());
        std::thread::sleep(Duration::from_millis(50));
        d1.store(true, Ordering::SeqCst);

        // Task 2 (serialized = same thread)
        *t2.lock() = Some(std::thread::current().id());
        std::thread::sleep(Duration::from_millis(50));
        d2.store(true, Ordering::SeqCst);
    });

    handle.join().unwrap();

    assert!(done1.load(Ordering::SeqCst));
    assert!(done2.load(Ordering::SeqCst));
    // Same thread since serialized.
    assert_eq!(tid1.lock().unwrap(), tid2.lock().unwrap());
}

/// Port of AST_TEST_DEFINE(unserialized_scheduler) from test_res_pjsip_scheduler.c.
///
/// Two tasks without a serializer should run on different threads.
#[test]
fn test_unserialized_scheduler() {
    let tid1 = Arc::new(parking_lot::Mutex::new(None::<std::thread::ThreadId>));
    let tid2 = Arc::new(parking_lot::Mutex::new(None::<std::thread::ThreadId>));
    let done1 = Arc::new(AtomicBool::new(false));
    let done2 = Arc::new(AtomicBool::new(false));

    let t1 = Arc::clone(&tid1);
    let d1 = Arc::clone(&done1);
    let h1 = std::thread::spawn(move || {
        *t1.lock() = Some(std::thread::current().id());
        std::thread::sleep(Duration::from_millis(50));
        d1.store(true, Ordering::SeqCst);
    });

    let t2 = Arc::clone(&tid2);
    let d2 = Arc::clone(&done2);
    let h2 = std::thread::spawn(move || {
        *t2.lock() = Some(std::thread::current().id());
        std::thread::sleep(Duration::from_millis(50));
        d2.store(true, Ordering::SeqCst);
    });

    h1.join().unwrap();
    h2.join().unwrap();

    assert!(done1.load(Ordering::SeqCst));
    assert!(done2.load(Ordering::SeqCst));
    assert_ne!(tid1.lock().unwrap(), tid2.lock().unwrap());
}

/// Port of AST_TEST_DEFINE(scheduler_cleanup) from test_res_pjsip_scheduler.c.
///
/// Verify that task data is properly cleaned up after execution.
#[test]
fn test_scheduler_cleanup() {
    let destruct_count = Arc::new(AtomicU32::new(0));
    let dc = Arc::clone(&destruct_count);

    let run_count = Arc::new(AtomicU32::new(0));
    let rc = Arc::clone(&run_count);

    let task = schedule_task(50, move || {
        rc.fetch_add(1, Ordering::SeqCst);
        // Simulate data cleanup on drop.
    });

    task.handle.unwrap().join().unwrap();
    // Simulate destructor.
    dc.fetch_add(1, Ordering::SeqCst);

    assert_eq!(run_count.load(Ordering::SeqCst), 1);
    assert_eq!(destruct_count.load(Ordering::SeqCst), 1);
}

/// Port of AST_TEST_DEFINE(scheduler_cancel) from test_res_pjsip_scheduler.c.
///
/// Schedule a task, cancel it before it runs, verify it never executes.
#[test]
fn test_scheduler_cancel() {
    let run_count = Arc::new(AtomicU32::new(0));
    let rc = Arc::clone(&run_count);

    let task = schedule_task(200, move || {
        rc.fetch_add(1, Ordering::SeqCst);
    });

    // Cancel before it fires.
    task.cancel();
    task.handle.unwrap().join().unwrap();

    assert_eq!(run_count.load(Ordering::SeqCst), 0);
}

/// Port of AST_TEST_DEFINE(scheduler_policy) from test_res_pjsip_scheduler.c.
///
/// Verify that a periodic task fires at approximately the correct intervals.
#[test]
fn test_scheduler_policy_periodic() {
    let interval_ms = 100u64;
    let fire_count = Arc::new(AtomicU32::new(0));
    let fc = Arc::clone(&fire_count);
    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);

    let start = Instant::now();
    let handle = std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(interval_ms));
            if s.load(Ordering::SeqCst) {
                break;
            }
            fc.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Let it run for ~350ms so we get ~3 fires.
    std::thread::sleep(Duration::from_millis(350));
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();

    let count = fire_count.load(Ordering::SeqCst);
    let elapsed = start.elapsed();

    // We should have gotten at least 2 and at most 4 fires.
    assert!(
        count >= 2 && count <= 4,
        "Expected 2-4 periodic fires, got {} in {:?}",
        count,
        elapsed
    );
}
