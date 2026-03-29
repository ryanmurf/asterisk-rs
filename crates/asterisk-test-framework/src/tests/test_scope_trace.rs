//! Port of asterisk/tests/test_scope_trace.c
//!
//! Tests scope entry/exit tracing:
//! - Scope enter and exit tracking
//! - Nested scope tracking
//! - Scope trace with format strings
//! - Guard-based automatic scope exit
//! - Performance: scope tracing overhead is minimal
//!
//! In Rust we use RAII (Drop trait) for scope tracing, which is more
//! natural than the C SCOPE_ENTER/SCOPE_EXIT macros.

use std::cell::RefCell;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Scope tracing implementation
// ---------------------------------------------------------------------------

thread_local! {
    static TRACE_LOG: RefCell<Vec<String>> = RefCell::new(Vec::new());
    static SCOPE_DEPTH: RefCell<usize> = RefCell::new(0);
}

/// Log a trace message with current depth.
fn trace_log(msg: &str) {
    TRACE_LOG.with(|log| {
        let depth = SCOPE_DEPTH.with(|d| *d.borrow());
        let indent = "  ".repeat(depth);
        log.borrow_mut().push(format!("{}{}", indent, msg));
    });
}

/// Get the current trace log.
fn get_trace_log() -> Vec<String> {
    TRACE_LOG.with(|log| log.borrow().clone())
}

/// Clear the trace log.
fn clear_trace_log() {
    TRACE_LOG.with(|log| log.borrow_mut().clear());
    SCOPE_DEPTH.with(|d| *d.borrow_mut() = 0);
}

/// RAII scope guard that logs entry/exit.
struct ScopeTrace {
    name: String,
}

impl ScopeTrace {
    fn enter(name: &str) -> Self {
        trace_log(&format!("--> {}", name));
        SCOPE_DEPTH.with(|d| *d.borrow_mut() += 1);
        Self {
            name: name.to_string(),
        }
    }
}

impl Drop for ScopeTrace {
    fn drop(&mut self) {
        SCOPE_DEPTH.with(|d| *d.borrow_mut() -= 1);
        trace_log(&format!("<-- {}", self.name));
    }
}

// ---------------------------------------------------------------------------
// Tests: Basic scope tracing
// ---------------------------------------------------------------------------

/// Port of scope_test basic behavior from test_scope_trace.c.
#[test]
fn test_scope_enter_exit() {
    clear_trace_log();
    {
        let _scope = ScopeTrace::enter("test_function");
        trace_log("inside function");
    }
    let log = get_trace_log();
    assert_eq!(log.len(), 3);
    assert!(log[0].contains("--> test_function"));
    assert!(log[1].contains("inside function"));
    assert!(log[2].contains("<-- test_function"));
}

/// Test nested scope tracking.
#[test]
fn test_scope_nested() {
    clear_trace_log();
    {
        let _outer = ScopeTrace::enter("outer");
        trace_log("in outer");
        {
            let _inner = ScopeTrace::enter("inner");
            trace_log("in inner");
        }
        trace_log("back in outer");
    }
    let log = get_trace_log();
    assert_eq!(log.len(), 7);
    assert!(log[0].contains("--> outer"));
    assert!(log[1].contains("in outer"));
    assert!(log[2].contains("--> inner"));
    assert!(log[3].contains("in inner"));
    assert!(log[4].contains("<-- inner"));
    assert!(log[5].contains("back in outer"));
    assert!(log[6].contains("<-- outer"));
}

/// Test deeply nested scopes.
#[test]
fn test_scope_deep_nesting() {
    clear_trace_log();
    {
        let _s1 = ScopeTrace::enter("level1");
        {
            let _s2 = ScopeTrace::enter("level2");
            {
                let _s3 = ScopeTrace::enter("level3");
                trace_log("deepest");
            }
        }
    }
    let log = get_trace_log();
    // 3 enters + 1 trace + 3 exits = 7
    assert_eq!(log.len(), 7);

    // Verify indentation increases.
    assert!(!log[0].starts_with(' ')); // level 0
    assert!(log[2].starts_with("  ")); // level 1 (after first enter increments depth)
}

// ---------------------------------------------------------------------------
// Tests: Scope with format strings
// ---------------------------------------------------------------------------

/// Port of SCOPE_ENTER with format parameters from test_scope_trace.c.
#[test]
fn test_scope_with_message() {
    clear_trace_log();
    {
        let msg = format!("top {} function", "scope_test");
        let _scope = ScopeTrace::enter(&msg);
        trace_log("test outer");
    }
    let log = get_trace_log();
    assert!(log[0].contains("top scope_test function"));
}

// ---------------------------------------------------------------------------
// Tests: Performance impact minimal
// ---------------------------------------------------------------------------

/// Verify scope tracing overhead is negligible.
#[test]
fn test_scope_trace_performance() {
    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        let _scope = ScopeTrace::enter("perf_test");
        // Minimal work inside scope.
        let _ = i * 2;
    }

    let elapsed = start.elapsed();
    // 10,000 scope enter/exit pairs should complete in well under 1 second.
    assert!(
        elapsed.as_millis() < 1000,
        "Scope tracing took {}ms for {} iterations, which is too slow",
        elapsed.as_millis(),
        iterations
    );
}

// ---------------------------------------------------------------------------
// Tests: Guard behavior
// ---------------------------------------------------------------------------

/// Verify scope exit happens even on early return (simulated by block exit).
#[test]
fn test_scope_exit_on_early_return() {
    clear_trace_log();

    fn simulate_early_return() {
        let _scope = ScopeTrace::enter("early_return_fn");
        trace_log("before early return");
        if true {
            return; // Early return -- Drop should still fire.
        }
        #[allow(unreachable_code)]
        {
            trace_log("unreachable");
        }
    }

    simulate_early_return();
    let log = get_trace_log();
    assert_eq!(log.len(), 3);
    assert!(log[0].contains("--> early_return_fn"));
    assert!(log[1].contains("before early return"));
    assert!(log[2].contains("<-- early_return_fn"));
}

/// Verify scope exit happens on panic recovery.
#[test]
fn test_scope_exit_on_panic() {
    clear_trace_log();

    let result = std::panic::catch_unwind(|| {
        let _scope = ScopeTrace::enter("panicking_fn");
        trace_log("before panic");
        panic!("test panic");
    });

    assert!(result.is_err());
    let log = get_trace_log();
    assert!(log.len() >= 3);
    assert!(log[0].contains("--> panicking_fn"));
    assert!(log[1].contains("before panic"));
    assert!(log[2].contains("<-- panicking_fn"));
}
