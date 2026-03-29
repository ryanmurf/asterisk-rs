//! Timer abstraction.
//!
//! Port of `res/res_timing_pthread.c`. Provides a pluggable timing interface
//! and a condition-variable-based timer implementation for generating
//! periodic tick events used by the media subsystem.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum TimingError {
    #[error("timer not running")]
    NotRunning,
    #[error("timer already running")]
    AlreadyRunning,
    #[error("invalid rate: {0}")]
    InvalidRate(u32),
    #[error("timer error: {0}")]
    Other(String),
}

pub type TimingResult<T> = Result<T, TimingError>;

// ---------------------------------------------------------------------------
// Timer events
// ---------------------------------------------------------------------------

/// Events delivered by a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    /// A tick has occurred; continuous mode is active.
    Continuous,
    /// No event pending.
    None,
}

// ---------------------------------------------------------------------------
// Timing interface trait
// ---------------------------------------------------------------------------

/// Trait for timing backends.
///
/// Mirrors the `ast_timing_interface` structure from the C source. Each
/// timing module (pthread, timerfd, dahdi) implements this trait.
pub trait TimingInterface: Send + Sync + fmt::Debug {
    /// Backend name.
    fn name(&self) -> &str;

    /// Open/create a new timer instance and return an opaque handle.
    fn open(&self) -> TimingResult<Box<dyn TimerHandle>>;
}

/// Handle to an individual timer instance.
pub trait TimerHandle: Send + Sync {
    /// Close and release the timer.
    fn close(&mut self) -> TimingResult<()>;

    /// Set the tick rate in ticks per second. 0 disables ticking.
    fn set_rate(&self, rate: u32) -> TimingResult<()>;

    /// Acknowledge a pending event, resetting the event flag.
    fn ack(&self) -> TimingResult<()>;

    /// Check for a pending event (non-blocking).
    fn get_event(&self) -> TimerEvent;

    /// Block until the next tick or timeout. Returns the event.
    fn wait(&self, timeout: Duration) -> TimerEvent;
}

// ---------------------------------------------------------------------------
// Pthread timing implementation
// ---------------------------------------------------------------------------

/// Condition-variable-based timer.
///
/// Port of `res_timing_pthread.c`. Uses a background thread that signals a
/// condvar at the configured rate.
#[derive(Debug)]
pub struct PthreadTiming;

impl PthreadTiming {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PthreadTiming {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingInterface for PthreadTiming {
    fn name(&self) -> &str {
        "pthread"
    }

    fn open(&self) -> TimingResult<Box<dyn TimerHandle>> {
        Ok(Box::new(PthreadTimerHandle::new()))
    }
}

/// Internal state shared between the timer handle and its tick thread.
struct TimerState {
    /// Condvar used to signal ticks.
    condvar: Condvar,
    /// Mutex protecting the tick flag.
    mutex: Mutex<bool>,
    /// Current tick rate (ticks/sec). 0 = disabled.
    rate: AtomicU32,
    /// Total ticks fired (for diagnostics).
    tick_count: AtomicU64,
    /// Whether the timer is active.
    running: AtomicBool,
}

/// A single timer instance backed by a thread + condvar.
pub struct PthreadTimerHandle {
    state: Arc<TimerState>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PthreadTimerHandle {
    fn new() -> Self {
        let state = Arc::new(TimerState {
            condvar: Condvar::new(),
            mutex: Mutex::new(false),
            rate: AtomicU32::new(0),
            tick_count: AtomicU64::new(0),
            running: AtomicBool::new(true),
        });

        let tick_state = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("ast-timer".into())
            .spawn(move || {
                Self::tick_loop(tick_state);
            })
            .expect("failed to spawn timer thread");

        Self {
            state,
            thread: Some(thread),
        }
    }

    fn tick_loop(state: Arc<TimerState>) {
        let mut last_tick = Instant::now();

        while state.running.load(Ordering::Relaxed) {
            let rate = state.rate.load(Ordering::Relaxed);
            if rate == 0 {
                // Disabled: sleep briefly and re-check.
                std::thread::sleep(Duration::from_millis(10));
                last_tick = Instant::now();
                continue;
            }

            let interval = Duration::from_nanos(1_000_000_000 / rate as u64);
            let elapsed = last_tick.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
            last_tick = Instant::now();

            // Signal a tick.
            {
                let mut flag = state.mutex.lock();
                *flag = true;
            }
            state.condvar.notify_all();
            state.tick_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl TimerHandle for PthreadTimerHandle {
    fn close(&mut self) -> TimingResult<()> {
        self.state.running.store(false, Ordering::Relaxed);
        // Wake the tick loop so it sees the shutdown.
        self.state.condvar.notify_all();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        debug!("Pthread timer closed");
        Ok(())
    }

    fn set_rate(&self, rate: u32) -> TimingResult<()> {
        if rate > 1_000_000 {
            return Err(TimingError::InvalidRate(rate));
        }
        self.state.rate.store(rate, Ordering::Relaxed);
        debug!(rate, "Timer rate set");
        Ok(())
    }

    fn ack(&self) -> TimingResult<()> {
        let mut flag = self.state.mutex.lock();
        *flag = false;
        Ok(())
    }

    fn get_event(&self) -> TimerEvent {
        let flag = self.state.mutex.lock();
        if *flag {
            TimerEvent::Continuous
        } else {
            TimerEvent::None
        }
    }

    fn wait(&self, timeout: Duration) -> TimerEvent {
        let mut flag = self.state.mutex.lock();
        if *flag {
            return TimerEvent::Continuous;
        }
        self.state.condvar.wait_for(&mut flag, timeout);
        if *flag {
            TimerEvent::Continuous
        } else {
            TimerEvent::None
        }
    }
}

impl Drop for PthreadTimerHandle {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pthread_timing_open() {
        let timing = PthreadTiming::new();
        assert_eq!(timing.name(), "pthread");
        let mut handle = timing.open().unwrap();
        handle.close().unwrap();
    }

    #[test]
    fn test_timer_set_rate() {
        let timing = PthreadTiming::new();
        let handle = timing.open().unwrap();
        handle.set_rate(50).unwrap();
        assert!(handle.set_rate(2_000_000).is_err());
    }

    #[test]
    fn test_timer_tick() {
        let timing = PthreadTiming::new();
        let handle = timing.open().unwrap();
        handle.set_rate(100).unwrap();
        // Wait for at least one tick.
        let event = handle.wait(Duration::from_millis(100));
        assert_eq!(event, TimerEvent::Continuous);
        handle.ack().unwrap();
        assert_eq!(handle.get_event(), TimerEvent::None);
    }

    #[test]
    fn test_timer_no_event_when_disabled() {
        let timing = PthreadTiming::new();
        let handle = timing.open().unwrap();
        // Rate is 0 by default (disabled).
        let event = handle.wait(Duration::from_millis(30));
        assert_eq!(event, TimerEvent::None);
    }
}
