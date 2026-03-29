//! Task processor -- serialized async task execution.
//!
//! A TaskProcessor runs tasks one at a time in sequence on a dedicated
//! tokio task, ensuring serialization of operations that must not be
//! concurrent (like channel operations).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;

/// A boxed future that can be sent across threads.
type BoxedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A task processor that executes tasks sequentially.
///
/// Modeled after Asterisk's `ast_taskprocessor` from taskprocessor.h.
/// Each task processor has a name and processes tasks in FIFO order.
pub struct TaskProcessor {
    name: String,
    sender: Option<mpsc::UnboundedSender<BoxedTask>>,
    task_count: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl TaskProcessor {
    /// Create a new task processor with the given name.
    ///
    /// Spawns a background tokio task that processes submitted work items
    /// sequentially.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let (sender, mut receiver) = mpsc::unbounded_channel::<BoxedTask>();
        let task_count = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));

        let tc = Arc::clone(&task_count);
        let r = Arc::clone(&running);
        let processor_name = name.clone();

        let handle = tokio::spawn(async move {
            tracing::debug!(name = %processor_name, "task processor started");
            while let Some(task) = receiver.recv().await {
                task.await;
                tc.fetch_sub(1, Ordering::Relaxed);
            }
            r.store(false, Ordering::Relaxed);
            tracing::debug!(name = %processor_name, "task processor stopped");
        });

        TaskProcessor {
            name,
            sender: Some(sender),
            task_count,
            running,
            handle: Some(handle),
        }
    }

    /// Get the task processor name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Push a task onto the processor's queue.
    ///
    /// The task will be executed after all previously queued tasks complete.
    /// Returns true if the task was successfully queued.
    pub fn push<F>(&self, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if let Some(sender) = &self.sender {
            self.task_count.fetch_add(1, Ordering::Relaxed);
            sender.send(Box::pin(task)).is_ok()
        } else {
            false
        }
    }

    /// Get the number of pending tasks.
    pub fn pending(&self) -> u64 {
        self.task_count.load(Ordering::Relaxed)
    }

    /// Whether the task processor is still running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Shut down the task processor.
    ///
    /// Drops the sender which causes the processor loop to end after
    /// all pending tasks complete.
    pub async fn shutdown(&mut self) {
        // Drop sender to signal the receiver to stop
        self.sender.take();

        // Wait for the background task to finish
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl fmt::Debug for TaskProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskProcessor")
            .field("name", &self.name)
            .field("pending", &self.task_count.load(Ordering::Relaxed))
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish()
    }
}
