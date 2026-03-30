//! Dialplan execution loop.
//!
//! This implements `pbx_run`, the main PBX execution loop that processes
//! a channel through the dialplan. It mirrors the C `__ast_pbx_run` function
//! in `main/pbx.c`.
//!
//! The loop:
//! 1. Gets the current context/exten/priority from the channel
//! 2. Finds the matching extension and priority
//! 3. Substitutes variables in the app data
//! 4. Looks up and executes the application
//! 5. Handles the result (increment priority, error handling, hangup)
//! 6. After exit, runs the 'h' (hangup) extension if it exists

use crate::channel::Channel;
use crate::channel::softhangup;
use crate::pbx::app_registry::APP_REGISTRY;
use crate::pbx::substitute::substitute_variables;
use crate::pbx::{Dialplan, PbxResult};
use asterisk_types::HangupCause;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Softhangup flags, mirroring `AST_SOFTHANGUP_*` from the C code.
///
/// These flags indicate why a soft hangup was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SoftHangupFlag {
    /// Normal hangup initiated by the channel driver.
    Device = 1,
    /// AsyncGoto initiated.
    AsyncGoto = 1 << 1,
    /// Hangup after bridge.
    Shutdown = 1 << 2,
    /// Absolute timeout reached.
    Timeout = 1 << 3,
    /// Module is being unloaded.
    AppUnload = 1 << 4,
    /// Explicit hangup request.
    Explicit = 1 << 5,
}

/// The result of a PBX run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PbxRunResult {
    /// PBX completed successfully (channel processed through dialplan).
    Success,
    /// PBX failed to start (no matching extension, etc.).
    Failed,
    /// Call limit reached.
    CallLimit,
}

/// Maximum number of iterations allowed in the PBX execution loop.
/// This prevents infinite loops caused by GoTo cycles or other
/// redirect loops in the dialplan. Mirrors the `PBX_MAX_STACK`
/// limit in C Asterisk.
const PBX_MAX_ITERATIONS: u32 = 10_000;

/// Run the PBX execution loop on a channel.
///
/// This is the main dialplan execution engine. It takes a channel and a
/// reference to the dialplan, then executes priorities in sequence until
/// the channel is hung up or no more matching extensions are found.
///
/// After the main loop exits, the 'h' (hangup) extension is executed
/// if it exists in the current context.
pub async fn pbx_run(
    channel: Arc<Mutex<Channel>>,
    dialplan: Arc<Dialplan>,
) -> PbxRunResult {
    let mut found = false;
    let mut error = false;
    let mut iterations: u32 = 0;

    // Initialize: if exten is empty, fall back to 's'
    {
        let mut chan = channel.lock().await;
        if chan.exten.is_empty() {
            tracing::info!(
                channel = %chan.name,
                context = %chan.context,
                "Empty extension, falling back to 's'"
            );
            chan.exten = "s".to_string();
            chan.priority = 1;
        }
    }

    // Main execution loop
    'outer: loop {
        // Inner loop: execute priorities in sequence
        loop {
            // Guard against infinite loops (GoTo cycles, etc.)
            iterations += 1;
            if iterations > PBX_MAX_ITERATIONS {
                tracing::error!(
                    "PBX execution loop exceeded maximum iterations ({}), \
                     possible infinite loop in dialplan",
                    PBX_MAX_ITERATIONS
                );
                error = true;
                break 'outer;
            }
            let (context, exten, priority, channel_name) = {
                let chan = channel.lock().await;
                (
                    chan.context.clone(),
                    chan.exten.clone(),
                    chan.priority,
                    chan.name.clone(),
                )
            };

            // Find the extension in the dialplan
            let (app_name, app_data) = match dialplan.find_extension(&context, &exten) {
                Some((_ctx, ext)) => {
                    match ext.get_priority(priority) {
                        Some(prio) => {
                            found = true;
                            (
                                prio.app.clone(),
                                prio.app_data.clone(),
                            )
                        }
                        None => {
                            // No priority at this number -- extension exhausted
                            break;
                        }
                    }
                }
                None => {
                    // No matching extension
                    break;
                }
            };

            tracing::debug!(
                channel = %channel_name,
                context = %context,
                exten = %exten,
                priority = priority,
                app = %app_name,
                "Executing dialplan priority"
            );

            // Emit Newexten AMI event
            {
                let priority_str = priority.to_string();
                crate::channel::publish_channel_event("Newexten", &[
                    ("Channel", &channel_name),
                    ("Context", &context),
                    ("Extension", &exten),
                    ("Priority", &priority_str),
                    ("Application", &app_name),
                    ("AppData", &app_data),
                ]);
            }

            // Substitute variables in app data
            let substituted_data = {
                let chan = channel.lock().await;
                substitute_variables(&chan, &app_data)
            };

            // Look up the application
            let app = match APP_REGISTRY.find(&app_name) {
                Some(app) => app,
                None => {
                    tracing::warn!(
                        channel = %channel_name,
                        app = %app_name,
                        "No such application"
                    );
                    // Treat as error
                    error = true;
                    break;
                }
            };

            // Execute the application
            let result = {
                let mut chan = channel.lock().await;
                app.execute(&mut chan, &substituted_data).await
            };

            // Check if the channel's location changed (e.g., GoTo was called)
            let (new_context, new_exten, new_priority) = {
                let chan = channel.lock().await;
                (chan.context.clone(), chan.exten.clone(), chan.priority)
            };

            let location_changed = new_context != context
                || new_exten != exten
                || new_priority != priority;

            match result {
                PbxResult::Success => {
                    // Check for hangup (softhangup flags)
                    let should_hangup = {
                        let chan = channel.lock().await;
                        chan.check_hangup()
                    };

                    if should_hangup {
                        // Check for AsyncGoto (redirect, not real hangup)
                        let is_async_goto = {
                            let chan = channel.lock().await;
                            chan.softhangup_flags & softhangup::AST_SOFTHANGUP_ASYNCGOTO != 0
                        };
                        if is_async_goto {
                            let mut chan = channel.lock().await;
                            chan.clear_softhangup(softhangup::AST_SOFTHANGUP_ASYNCGOTO);
                            continue;
                        }
                        break 'outer;
                    }

                    if location_changed {
                        // Application changed the dialplan location (GoTo, etc.)
                        // Continue from the new location
                        continue;
                    }

                    // Increment priority and continue
                    {
                        let mut chan = channel.lock().await;
                        chan.priority += 1;
                    }
                }
                PbxResult::Failed => {
                    tracing::debug!(
                        channel = %channel_name,
                        context = %context,
                        exten = %exten,
                        priority = priority,
                        app = %app_name,
                        "Application returned failure"
                    );

                    // Try error extension 'e'
                    if exten != "e" {
                        if dialplan.find_extension(&context, "e").is_some() {
                            let mut chan = channel.lock().await;
                            chan.exten = "e".to_string();
                            chan.priority = 1;
                            continue;
                        }
                    }

                    error = true;
                    break;
                }
                PbxResult::Incomplete => {
                    tracing::debug!(
                        channel = %channel_name,
                        "Extension match incomplete, waiting for more digits"
                    );
                    // In a real implementation, we would wait for more DTMF digits
                    // For now, treat as if we need to break
                    break;
                }
            }
        }

        // We've exhausted the current extension's priorities, or hit an error.
        // Check for special extensions.
        {
            let chan = channel.lock().await;
            let context = chan.context.clone();
            let exten = chan.exten.clone();
            drop(chan);

            if !found && !error {
                // No matching extension found at all
                // Try 'i' (invalid) extension
                if dialplan.find_extension(&context, "i").is_some() {
                    let mut chan = channel.lock().await;
                    tracing::info!(
                        channel = %chan.name,
                        context = %context,
                        exten = %exten,
                        "Sent to invalid extension handler"
                    );
                    chan.set_variable("INVALID_EXTEN", &exten);
                    chan.exten = "i".to_string();
                    chan.priority = 1;
                    found = false;
                    continue;
                }

                // Try 't' (timeout) extension
                if dialplan.find_extension(&context, "t").is_some() {
                    let mut chan = channel.lock().await;
                    tracing::info!(
                        channel = %chan.name,
                        context = %context,
                        "Sent to timeout extension handler"
                    );
                    chan.exten = "t".to_string();
                    chan.priority = 1;
                    found = false;
                    continue;
                }

                tracing::warn!(
                    context = %context,
                    exten = %exten,
                    "No matching extension and no 'i' or 't' handler"
                );
            }
        }

        break;
    }

    // Run hangup extension 'h' if it exists
    run_hangup_extension(&channel, &dialplan).await;

    // Hangup the channel if not already done
    {
        let mut chan = channel.lock().await;
        chan.hangup(HangupCause::NormalClearing);
    }

    if found || error {
        PbxRunResult::Success
    } else {
        PbxRunResult::Failed
    }
}

/// Run the 'h' (hangup) extension if it exists in the current context.
///
/// This is called after the main PBX loop exits, regardless of how it exited.
/// The 'h' extension allows the dialplan to perform cleanup (CDR finalization,
/// variable logging, etc.).
async fn run_hangup_extension(
    channel: &Arc<Mutex<Channel>>,
    dialplan: &Arc<Dialplan>,
) {
    let context = {
        let chan = channel.lock().await;
        chan.context.clone()
    };

    // Check if 'h' extension exists
    if dialplan.find_extension(&context, "h").is_none() {
        return;
    }

    tracing::debug!(context = %context, "Running hangup extension 'h'");

    // Set to h,1
    {
        let mut chan = channel.lock().await;
        chan.exten = "h".to_string();
        chan.priority = 1;
    }

    // Execute priorities in 'h' extension
    loop {
        let (ctx, priority, channel_name) = {
            let chan = channel.lock().await;
            (chan.context.clone(), chan.priority, chan.name.clone())
        };

        let (app_name, app_data) = match dialplan.find_extension(&ctx, "h") {
            Some((_c, ext)) => match ext.get_priority(priority) {
                Some(prio) => (prio.app.clone(), prio.app_data.clone()),
                None => break,
            },
            None => break,
        };

        let substituted_data = {
            let chan = channel.lock().await;
            substitute_variables(&chan, &app_data)
        };

        if let Some(app) = APP_REGISTRY.find(&app_name) {
            let mut chan = channel.lock().await;
            let _result = app.execute(&mut chan, &substituted_data).await;
        } else {
            tracing::warn!(
                channel = %channel_name,
                app = %app_name,
                "No such application in hangup handler"
            );
        }

        // Increment priority
        {
            let mut chan = channel.lock().await;
            chan.priority += 1;
        }
    }
}

/// Execute a single application by name on a channel.
///
/// This is a convenience function that looks up the application in the
/// registry, substitutes variables in the arguments, and executes it.
/// Mirrors `ast_pbx_exec_application` from the C code.
pub async fn pbx_exec_application(
    channel: &mut Channel,
    app_name: &str,
    app_args: &str,
) -> PbxResult {
    let substituted_args = substitute_variables(channel, app_args);

    match APP_REGISTRY.find(app_name) {
        Some(app) => app.execute(channel, &substituted_args).await,
        None => {
            tracing::warn!(app = %app_name, "Application not found");
            PbxResult::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbx::{Context, DialplanApp, Extension, Priority};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Debug)]
    struct CounterApp {
        app_name: String,
        counter: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl DialplanApp for CounterApp {
        fn name(&self) -> &str {
            &self.app_name
        }

        async fn execute(&self, _channel: &mut Channel, _args: &str) -> PbxResult {
            self.counter.fetch_add(1, Ordering::SeqCst);
            PbxResult::Success
        }
    }

    #[derive(Debug)]
    struct TestHangupApp {
        app_name: String,
    }

    #[async_trait::async_trait]
    impl DialplanApp for TestHangupApp {
        fn name(&self) -> &str {
            &self.app_name
        }

        async fn execute(&self, channel: &mut Channel, _args: &str) -> PbxResult {
            // In real Asterisk, the Hangup() app sets softhangup flags,
            // which the PBX loop detects to trigger the hangup sequence.
            channel.softhangup(softhangup::AST_SOFTHANGUP_EXPLICIT);
            PbxResult::Success
        }
    }

    #[derive(Debug)]
    struct TestGotoApp {
        app_name: String,
    }

    #[async_trait::async_trait]
    impl DialplanApp for TestGotoApp {
        fn name(&self) -> &str {
            &self.app_name
        }

        async fn execute(&self, channel: &mut Channel, args: &str) -> PbxResult {
            // Simple Goto: context,exten,priority
            let parts: Vec<&str> = args.split(',').collect();
            match parts.len() {
                3 => {
                    channel.context = parts[0].to_string();
                    channel.exten = parts[1].to_string();
                    channel.priority = parts[2].parse().unwrap_or(1);
                }
                2 => {
                    channel.exten = parts[0].to_string();
                    channel.priority = parts[1].parse().unwrap_or(1);
                }
                1 => {
                    channel.priority = parts[0].parse().unwrap_or(1);
                }
                _ => return PbxResult::Failed,
            }
            PbxResult::Success
        }
    }

    #[tokio::test]
    async fn test_basic_pbx_run() {
        // Use unique app names to avoid conflicts with other tests via the global registry
        let counter = Arc::new(AtomicU32::new(0));

        APP_REGISTRY.register(Arc::new(CounterApp {
            app_name: "BasicCounter".to_string(),
            counter: counter.clone(),
        }));
        APP_REGISTRY.register(Arc::new(TestHangupApp {
            app_name: "BasicHangup".to_string(),
        }));

        let mut dp = Dialplan::new();
        let mut ctx = Context::new("default");
        let mut ext = Extension::new("s");
        ext.add_priority(Priority {
            priority: 1,
            app: "BasicCounter".to_string(),
            app_data: String::new(),
            label: None,
        });
        ext.add_priority(Priority {
            priority: 2,
            app: "BasicCounter".to_string(),
            app_data: String::new(),
            label: None,
        });
        ext.add_priority(Priority {
            priority: 3,
            app: "BasicHangup".to_string(),
            app_data: String::new(),
            label: None,
        });
        ctx.add_extension(ext);
        dp.add_context(ctx);

        let ch = Arc::new(Mutex::new(Channel::new("Test/basic")));
        let result = pbx_run(ch, Arc::new(dp)).await;
        assert_eq!(result, PbxRunResult::Success);

        // Counter should have been called exactly 2 times
        // (priorities 1 and 2, then Hangup at 3)
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_pbx_run_no_extension() {
        let dp = Arc::new(Dialplan::new()); // empty dialplan
        let ch = Arc::new(Mutex::new(Channel::new("Test/noext")));

        let result = pbx_run(ch, dp).await;
        assert_eq!(result, PbxRunResult::Failed);
    }

    #[tokio::test]
    async fn test_pbx_run_with_goto() {
        let goto_counter = Arc::new(AtomicU32::new(0));

        APP_REGISTRY.register(Arc::new(TestGotoApp {
            app_name: "TestGoto".to_string(),
        }));
        APP_REGISTRY.register(Arc::new(CounterApp {
            app_name: "GotoCounter".to_string(),
            counter: goto_counter.clone(),
        }));
        APP_REGISTRY.register(Arc::new(TestHangupApp {
            app_name: "GotoHangup".to_string(),
        }));

        let mut dp = Dialplan::new();
        let mut ctx = Context::new("default");

        let mut ext_s = Extension::new("s");
        ext_s.add_priority(Priority {
            priority: 1,
            app: "TestGoto".to_string(),
            app_data: "default,200,1".to_string(),
            label: None,
        });
        ctx.add_extension(ext_s);

        let mut ext_200 = Extension::new("200");
        ext_200.add_priority(Priority {
            priority: 1,
            app: "GotoCounter".to_string(),
            app_data: String::new(),
            label: None,
        });
        ext_200.add_priority(Priority {
            priority: 2,
            app: "GotoHangup".to_string(),
            app_data: String::new(),
            label: None,
        });
        ctx.add_extension(ext_200);

        dp.add_context(ctx);

        let ch = Arc::new(Mutex::new(Channel::new("Test/goto")));
        let result = pbx_run(ch, Arc::new(dp)).await;

        assert_eq!(result, PbxRunResult::Success);
        assert_eq!(goto_counter.load(Ordering::SeqCst), 1);
    }
}
