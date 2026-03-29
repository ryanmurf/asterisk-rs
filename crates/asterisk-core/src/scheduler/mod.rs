//! Scheduler -- timed callback execution.
//!
//! Provides scheduling of delayed and periodic tasks, modeled after
//! Asterisk's `ast_sched` from sched.h.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use parking_lot::Mutex;

/// A unique identifier for a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchedId(pub u64);

impl std::fmt::Display for SchedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SchedId({})", self.0)
    }
}

/// Handle for a scheduled task that can be used to cancel it.
struct SchedEntry {
    /// The abort handle for cancellation
    abort_handle: tokio::task::AbortHandle,
}

/// Scheduler for timed events.
///
/// Tasks can be scheduled to run after a delay. Each scheduled task
/// gets a unique SchedId that can be used to cancel it.
pub struct Scheduler {
    next_id: AtomicU64,
    entries: Arc<Mutex<HashMap<u64, SchedEntry>>>,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new() -> Self {
        Scheduler {
            next_id: AtomicU64::new(1),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Schedule a task to run after the given delay.
    ///
    /// Returns a SchedId that can be used to cancel the task.
    pub fn schedule<F>(&self, delay: Duration, callback: F) -> SchedId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entries = Arc::clone(&self.entries);

        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            callback.await;
            // Remove self from entries after completion
            entries.lock().remove(&id);
        });

        let entry = SchedEntry {
            abort_handle: handle.abort_handle(),
        };

        self.entries.lock().insert(id, entry);
        SchedId(id)
    }

    /// Schedule a task to run after the given delay in milliseconds.
    pub fn schedule_ms<F>(&self, delay_ms: u64, callback: F) -> SchedId
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.schedule(Duration::from_millis(delay_ms), callback)
    }

    /// Cancel a scheduled task.
    ///
    /// Returns true if the task was found and cancelled,
    /// false if it was already executed or not found.
    pub fn cancel(&self, id: SchedId) -> bool {
        if let Some(entry) = self.entries.lock().remove(&id.0) {
            entry.abort_handle.abort();
            true
        } else {
            false
        }
    }

    /// Get the number of pending scheduled tasks.
    pub fn pending(&self) -> usize {
        self.entries.lock().len()
    }

    /// Cancel all pending tasks.
    pub fn cancel_all(&self) {
        let mut entries = self.entries.lock();
        for (_, entry) in entries.drain() {
            entry.abort_handle.abort();
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("pending", &self.entries.lock().len())
            .finish()
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.cancel_all();
    }
}
