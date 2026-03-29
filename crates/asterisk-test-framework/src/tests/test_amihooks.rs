//! Port of asterisk/tests/test_amihooks.c
//!
//! Tests AMI (Asterisk Manager Interface) hook registration:
//!
//! - Registering a hook for AMI events
//! - Sending an action and verifying the hook fires
//! - Unregistering the hook
//!
//! Since we do not have a live AMI server, we model the hook registration
//! and event dispatch locally using callbacks.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// AMI hook model
// ---------------------------------------------------------------------------

type HookCallback = Box<dyn Fn(i32, &str, &str) -> i32 + Send + Sync>;

struct ManagerCustomHook {
    file: String,
    helper: HookCallback,
}

struct AmiHookRegistry {
    hooks: Vec<Arc<ManagerCustomHook>>,
}

impl AmiHookRegistry {
    fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    fn register_hook(&mut self, hook: Arc<ManagerCustomHook>) {
        // Avoid double registration
        self.hooks.retain(|h| h.file != hook.file);
        self.hooks.push(hook);
    }

    fn unregister_hook(&mut self, file: &str) {
        self.hooks.retain(|h| h.file != file);
    }

    /// Simulate sending an AMI action and dispatching to all hooks.
    fn send_action(&self, action: &str) -> i32 {
        let event = "Command";
        let content = action;
        let mut result = 0;
        for hook in &self.hooks {
            result = (hook.helper)(1, event, content);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(amihook_cli_send).
///
/// Register an AMI hook, send an action, verify the hook fires.
#[test]
fn test_amihook_cli_send() {
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done_clone = done.clone();

    let mut registry = AmiHookRegistry::new();

    let hook = Arc::new(ManagerCustomHook {
        file: "test_amihooks.rs".to_string(),
        helper: Box::new(move |_category, _event, _content| {
            let (lock, cvar) = &*done_clone;
            let mut d = lock.lock().unwrap();
            *d = true;
            cvar.notify_all();
            0
        }),
    });

    registry.register_hook(hook);

    // Send test action
    registry.send_action("Action: Command\nCommand: core show version\nActionID: 987654321\n");

    // Wait for hook to fire (with timeout)
    let (lock, cvar) = &*done;
    let d = lock.lock().unwrap();
    let result = if *d {
        true
    } else {
        let (d, _) = cvar.wait_timeout(d, Duration::from_secs(2)).unwrap();
        *d
    };

    assert!(result, "Hook should have been invoked");

    // Unregister and verify
    registry.unregister_hook("test_amihooks.rs");
    assert!(registry.hooks.is_empty());
}
