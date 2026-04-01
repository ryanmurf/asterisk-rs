//! Adapter layer bridging the apps crate's `DialplanApp` implementations
//! into the core crate's `DialplanApp` trait (which has `async fn execute()`).
//!
//! The apps crate defines `DialplanApp` with only `name()` and `description()`,
//! while the core crate's `DialplanApp` additionally requires
//! `async fn execute(&self, channel: &mut Channel, args: &str) -> PbxResult`.
//!
//! Rather than rewriting all 78+ apps at once, this adapter wraps each app's
//! existing `exec()` function into the core trait, allowing registration
//! with the global `APP_REGISTRY`.

use std::sync::Arc;

use asterisk_core::channel::Channel;
use asterisk_core::pbx::app_registry::APP_REGISTRY;
use asterisk_core::pbx::{DialplanApp as CoreDialplanApp, PbxResult};

use crate::PbxExecResult;

// ---------------------------------------------------------------------------
// AppAdapter: wraps an app's exec function into the core DialplanApp trait
// ---------------------------------------------------------------------------

/// Type-erased exec function: takes a mutable channel reference and args,
/// returns a `PbxExecResult` that we map to the core `PbxResult`.
type ExecFn = Box<dyn Fn(&mut Channel, &str) -> PbxExecResult + Send + Sync>;

/// Async variant for apps that need `.await` in their exec function.
/// We store these as a boxed function returning a pinned future.
type AsyncExecFn = Box<
    dyn for<'a> Fn(&'a mut Channel, &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = PbxExecResult> + Send + 'a>>
        + Send
        + Sync,
>;

/// Wraps a synchronous app exec function into the core `DialplanApp` trait.
pub struct AppAdapter {
    app_name: String,
    synopsis: String,
    exec_fn: ExecFn,
}

impl std::fmt::Debug for AppAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppAdapter")
            .field("name", &self.app_name)
            .finish()
    }
}

impl AppAdapter {
    /// Create a new synchronous adapter.
    pub fn new(
        name: impl Into<String>,
        synopsis: impl Into<String>,
        exec_fn: impl Fn(&mut Channel, &str) -> PbxExecResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            app_name: name.into(),
            synopsis: synopsis.into(),
            exec_fn: Box::new(exec_fn),
        }
    }
}

#[async_trait::async_trait]
impl CoreDialplanApp for AppAdapter {
    fn name(&self) -> &str {
        &self.app_name
    }

    fn synopsis(&self) -> &str {
        &self.synopsis
    }

    async fn execute(&self, channel: &mut Channel, args: &str) -> PbxResult {
        map_result((self.exec_fn)(channel, args))
    }
}

/// Wraps an async app exec function into the core `DialplanApp` trait.
pub struct AsyncAppAdapter {
    app_name: String,
    synopsis: String,
    exec_fn: AsyncExecFn,
}

impl std::fmt::Debug for AsyncAppAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncAppAdapter")
            .field("name", &self.app_name)
            .finish()
    }
}

impl AsyncAppAdapter {
    /// Create a new async adapter.
    pub fn new(
        name: impl Into<String>,
        synopsis: impl Into<String>,
        exec_fn: impl for<'a> Fn(&'a mut Channel, &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = PbxExecResult> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            app_name: name.into(),
            synopsis: synopsis.into(),
            exec_fn: Box::new(exec_fn),
        }
    }
}

#[async_trait::async_trait]
impl CoreDialplanApp for AsyncAppAdapter {
    fn name(&self) -> &str {
        &self.app_name
    }

    fn synopsis(&self) -> &str {
        &self.synopsis
    }

    async fn execute(&self, channel: &mut Channel, args: &str) -> PbxResult {
        let result = (self.exec_fn)(channel, args).await;
        map_result(result)
    }
}

/// Map the apps-crate `PbxExecResult` to the core-crate `PbxResult`.
fn map_result(r: PbxExecResult) -> PbxResult {
    match r {
        PbxExecResult::Success => PbxResult::Success,
        PbxExecResult::Failed => PbxResult::Failed,
        PbxExecResult::Hangup => PbxResult::Failed,
    }
}

// ---------------------------------------------------------------------------
// register_all_apps: register every app with the global APP_REGISTRY
// ---------------------------------------------------------------------------

/// Register all built-in dialplan applications with the core APP_REGISTRY.
///
/// This creates an adapter for each app, wrapping its `exec()` logic into
/// the core `DialplanApp` trait, then registers it in `APP_REGISTRY`.
pub fn register_all_apps() {
    use crate::answer::AppAnswer;
    use crate::confbridge::AppConfBridge;
    use crate::dial::AppDial;
    use crate::echo::AppEcho;
    use crate::exec::{AppExec, AppExecIf, AppTryExec};
    use crate::goto::AppGoto;
    use crate::hangup::AppHangup;
    use crate::if_::{AppGotoIf, AppGotoIfTime, AppIf, AppElseIf, AppElse, AppEndIf};
    use crate::playback::AppPlayback;
    use crate::set::{AppMSet, AppSet};
    use crate::stack::{AppGoSub, AppGoSubIf, AppReturn, AppStackPop};
    use crate::verbose::{AppLog, AppNoOp, AppVerbose};
    use crate::wait::{AppWait, AppWaitDigit, AppWaitExten, AppWaitUntil};
    use crate::while_::{AppWhile, AppEndWhile, AppExitWhile, AppContinueWhile};

    // -----------------------------------------------------------------------
    // Synchronous apps (exec takes &mut Channel / &Channel and &str)
    // -----------------------------------------------------------------------

    // Hangup - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "Hangup",
        "Hangup the calling channel",
        |channel, args| AppHangup::exec(channel, args),
    )));

    // Verbose - sync exec (takes &Channel, not &mut)
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "Verbose",
        "Send arbitrary text to verbose output",
        |channel, args| AppVerbose::exec(channel, args),
    )));

    // Log - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "Log",
        "Send arbitrary text to a selected log level",
        |channel, args| AppLog::exec(channel, args),
    )));

    // NoOp - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "NoOp",
        "Do nothing (but log arguments)",
        |channel, args| AppNoOp::exec(channel, args),
    )));

    // GoSub - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "GoSub",
        "Execute a dialplan subroutine",
        |channel, args| AppGoSub::exec(channel, args),
    )));

    // GoSubIf - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "GoSubIf",
        "Conditionally execute a dialplan subroutine",
        |channel, args| AppGoSubIf::exec(channel, args),
    )));

    // Return - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "Return",
        "Return from a GoSub",
        |channel, args| AppReturn::exec(channel, args),
    )));

    // StackPop - sync exec
    APP_REGISTRY.register(Arc::new(AppAdapter::new(
        "StackPop",
        "Pop the GoSub stack",
        |channel, args| AppStackPop::exec(channel, args),
    )));

    // -----------------------------------------------------------------------
    // Async apps (exec is async, need the AsyncAppAdapter)
    // -----------------------------------------------------------------------

    // Answer - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Answer",
        "Answer a channel if ringing",
        |channel, args| Box::pin(AppAnswer::exec(channel, args)),
    )));

    // Dial - async exec, returns (PbxExecResult, DialStatus)
    // We call the real exec and set DIALSTATUS on the channel.
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Dial",
        "Place a call and bridge the calling and called channels",
        |channel, args| {
            Box::pin(async move {
                let (result, _dial_status) = AppDial::exec(channel, args).await;
                // DIALSTATUS is already set inside AppDial::exec
                result
            })
        },
    )));

    // Playback - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Playback",
        "Play a file to the channel",
        |channel, args| Box::pin(AppPlayback::exec(channel, args)),
    )));

    // Echo - async exec (takes only channel, no args)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Echo",
        "Echo audio, video, DTMF back to the calling party",
        |channel, _args| Box::pin(AppEcho::exec(channel)),
    )));

    // ConfBridge - async exec, blocks while in conference
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ConfBridge",
        "N-way conference bridge",
        |channel, args| {
            Box::pin(async move {
                let (result, _conf_result) = AppConfBridge::exec(channel, args).await;
                // ConfBridge returns Hangup when the participant leaves (BYE/hangup).
                // This is normal completion — map to Success so dialplan continues.
                match result {
                    PbxExecResult::Hangup => PbxExecResult::Success,
                    other => other,
                }
            })
        },
    )));

    // Wait - async exec (takes &Channel not &mut Channel)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Wait",
        "Waits for some time",
        |channel, args| Box::pin(async move { AppWait::exec(channel, args).await }),
    )));

    // WaitExten - async exec (takes &Channel not &mut Channel)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "WaitExten",
        "Waits for an extension to be entered",
        |channel, args| Box::pin(async move { AppWaitExten::exec(channel, args).await }),
    )));

    // WaitDigit - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "WaitDigit",
        "Waits for a digit to be pressed",
        |channel, args| Box::pin(AppWaitDigit::exec(channel, args)),
    )));

    // WaitUntil - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "WaitUntil",
        "Wait until a specified time",
        |channel, args| Box::pin(AppWaitUntil::exec(channel, args)),
    )));

    // Set - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Set",
        "Set a channel variable",
        |channel, args| Box::pin(AppSet::exec(channel, args)),
    )));

    // MSet - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "MSet",
        "Set multiple channel variables at once",
        |channel, args| Box::pin(AppMSet::exec(channel, args)),
    )));

    // Goto - async exec
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Goto",
        "Unconditional goto in the dialplan",
        |channel, args| Box::pin(AppGoto::exec(channel, args)),
    )));

    // UserEvent - async exec (publishes to AMI event bus)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "UserEvent",
        "Send a custom AMI user event",
        |channel, args| Box::pin(crate::userevent::AppUserEvent::exec(channel, args)),
    )));

    // GotoIf - async exec (conditional dialplan jump)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "GotoIf",
        "Conditional goto",
        |channel, args| Box::pin(AppGotoIf::exec(channel, args)),
    )));

    // GotoIfTime - async exec (time-based conditional jump)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "GotoIfTime",
        "Time-based conditional goto",
        |channel, args| Box::pin(AppGotoIfTime::exec(channel, args)),
    )));

    // If/ElseIf/Else/EndIf - async exec (conditional blocks)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "If",
        "Start a conditional block",
        |channel, args| Box::pin(AppIf::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ElseIf",
        "Else-if block",
        |channel, args| Box::pin(AppElseIf::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Else",
        "Else block",
        |channel, args| Box::pin(AppElse::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "EndIf",
        "End conditional block",
        |channel, args| Box::pin(AppEndIf::exec(channel, args)),
    )));

    // While/EndWhile/ExitWhile/ContinueWhile - async exec (loop constructs)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "While",
        "Start a while loop",
        |channel, args| Box::pin(AppWhile::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "EndWhile",
        "End a while loop",
        |channel, args| Box::pin(AppEndWhile::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ExitWhile",
        "Exit a while loop",
        |channel, args| Box::pin(AppExitWhile::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ContinueWhile",
        "Continue a while loop",
        |channel, args| Box::pin(AppContinueWhile::exec(channel, args)),
    )));

    // Exec/TryExec/ExecIf - async exec (dynamic app execution)
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "Exec",
        "Executes dialplan application",
        |channel, args| Box::pin(AppExec::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "TryExec",
        "Executes dialplan application (non-fatal)",
        |channel, args| Box::pin(AppTryExec::exec(channel, args)),
    )));
    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ExecIf",
        "Conditionally execute a dialplan application",
        |channel, args| Box::pin(AppExecIf::exec(channel, args)),
    )));

    // -----------------------------------------------------------------------
    // ChanSpy / ExtenSpy - real async adapters
    // -----------------------------------------------------------------------

    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ChanSpy",
        "Listen to a channel, and optionally whisper into it",
        |channel, args| {
            Box::pin(async move {
                let (result, _spy_result) = crate::chanspy::AppChanSpy::exec(channel, args).await;
                result
            })
        },
    )));

    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "ExtenSpy",
        "Listen to a channel by extension",
        |channel, args| {
            Box::pin(async move {
                let (result, _spy_result) = crate::chanspy::AppExtenSpy::exec(channel, args).await;
                result
            })
        },
    )));

    // -----------------------------------------------------------------------
    // AgentLogin / AgentRequest - real async adapters
    // -----------------------------------------------------------------------

    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "AgentLogin",
        "Log an agent into the agent pool",
        |channel, args| {
            Box::pin(crate::agent_pool::AppAgentLogin::exec(channel, args))
        },
    )));

    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "AgentRequest",
        "Request an agent from the agent pool",
        |channel, args| {
            Box::pin(crate::agent_pool::AppAgentRequest::exec(channel, args))
        },
    )));

    // -----------------------------------------------------------------------
    // AGI - real async adapter (dispatches to script/FastAGI/async based on args)
    // -----------------------------------------------------------------------

    APP_REGISTRY.register(Arc::new(AsyncAppAdapter::new(
        "AGI",
        "Asterisk Gateway Interface - run external AGI script",
        |channel, args| {
            Box::pin(async move {
                crate::agi_app::AppAgi::exec(channel, args).await
            })
        },
    )));

    // Register the remaining apps as stubs (these will be wired incrementally)
    register_stub_apps();

    tracing::info!(
        "Registered {} apps with core APP_REGISTRY",
        APP_REGISTRY.count()
    );
}

/// Register remaining apps as stubs that log and return Success.
///
/// These will be replaced with real adapters as each app is individually
/// wired up. Having them registered means the PBX can find them by name
/// and report them in `core show applications`.
fn register_stub_apps() {
    // Macro to reduce boilerplate for stub registrations
    macro_rules! register_stub {
        ($name:expr, $synopsis:expr) => {
            // Only register if not already registered (the real adapter above takes precedence)
            if APP_REGISTRY.find($name).is_none() {
                APP_REGISTRY.register(Arc::new(AppAdapter::new(
                    $name,
                    $synopsis,
                    |_channel, _args| {
                        tracing::debug!(app = $name, "Stub app executed (not yet wired)");
                        PbxExecResult::Success
                    },
                )));
            }
        };
    }

    // Dial, Playback, Echo, ConfBridge are registered as real adapters above
    register_stub!("Record", "Record to a file");
    register_stub!("Queue", "Queue a call for a call queue");
    register_stub!("VoiceMail", "Leave a voicemail");
    register_stub!("Transfer", "Transfer caller to a new extension");
    register_stub!("SoftHangup", "Request a hangup on a given channel");
    register_stub!("Originate", "Originate a new outbound call");
    register_stub!("Read", "Read DTMF digits from the caller");
    register_stub!("System", "Execute a system command");
    register_stub!("TrySystem", "Execute a system command (non-fatal)");
    register_stub!("SendText", "Send text to a channel");
    // Wait, WaitExten, WaitDigit, WaitUntil are registered as real adapters above
    // GoSub, GoSubIf, Return, StackPop are registered as real adapters above
    register_stub!("Exec", "Executes dialplan application");
    register_stub!("TryExec", "Executes dialplan application (non-fatal)");
    register_stub!("ExecIf", "Conditionally execute a dialplan application");
    register_stub!("MixMonitor", "Record a call and mix the audio");
    register_stub!("StopMixMonitor", "Stop recording a call");
    // ChanSpy and ExtenSpy are registered as real adapters above
    register_stub!("Page", "Page/intercom system");
    register_stub!("Directory", "Provide directory of voicemail extensions");
    register_stub!("BlindTransfer", "Blind transfer a channel");
    register_stub!("AttnTransfer", "Attended transfer a channel");
    register_stub!("BridgeWait", "Wait in a holding bridge");
    register_stub!("BridgeAdd", "Add a channel to a bridge");
    register_stub!("PrivacyManager", "Require phone number for privacy");
    register_stub!("Authenticate", "Authenticate a caller");
    register_stub!("ResetCDR", "Reset the CDR for a channel");
    register_stub!("CELGenUserEvent", "Generate a CEL user-defined event");
    register_stub!("Dictate", "Virtual dictation machine");
    register_stub!("DISA", "Direct Inward System Access");
    register_stub!("ExternalIVR", "External IVR interface");
    register_stub!("FollowMe", "Find-Me/Follow-Me application");
    register_stub!("ForkCDR", "Fork the CDR");
    register_stub!("Milliwatt", "Generate a 1004Hz milliwatt tone");
    register_stub!("Morsecode", "Play Morse code");
    register_stub!("Pickup", "Directed call pickup");
    register_stub!("PickupChan", "Pickup a specific channel");
    register_stub!("PlayTones", "Play a tone list");
    register_stub!("StopPlayTones", "Stop playing tones");
    register_stub!("SayCountedNoun", "Say a noun with a count");
    register_stub!("SayCountedAdj", "Say an adjective with a count");
    register_stub!("SLAStation", "Shared Line Appearance station");
    register_stub!("SLATrunk", "Shared Line Appearance trunk");
    register_stub!("SpeechCreate", "Create a speech recognition instance");
    register_stub!("SpeechActivateGrammar", "Activate a speech grammar");
    register_stub!("SpeechStart", "Start speech recognition");
    register_stub!("SpeechBackground", "Background speech recognition");
    register_stub!("SpeechDeactivateGrammar", "Deactivate a speech grammar");
    register_stub!("SpeechProcessingSound", "Set speech processing sound");
    register_stub!("SpeechDestroy", "Destroy a speech instance");
    register_stub!("SpeechLoadGrammar", "Load a speech grammar");
    register_stub!("SpeechUnloadGrammar", "Unload a speech grammar");
    register_stub!("SendURL", "Send a URL to the channel");
    register_stub!("Zapateller", "Block telemarketers with SIT tone");
    register_stub!("MinivmRecord", "Mini voicemail record");
    register_stub!("MinivmGreet", "Mini voicemail greet");
    register_stub!("MinivmNotify", "Mini voicemail notify");
    register_stub!("MinivmDelete", "Mini voicemail delete");
    register_stub!("MinivmAccMess", "Mini voicemail account message");
    register_stub!("MP3Player", "Play MP3 files");
    register_stub!("DAHDIRas", "DAHDI remote access server");
    register_stub!("SMS", "SMS application");
    register_stub!("AlarmReceiver", "Alarm receiver");
    // AgentLogin and AgentRequest are registered as real adapters above
    register_stub!("Festival", "Festival TTS");
    register_stub!("JACK", "JACK audio");
    register_stub!("ICES", "Icecast streaming");
    register_stub!("NBScat", "NBS audio streaming");
    register_stub!("TestServer", "Test server");
    register_stub!("TestClient", "Test client");
    register_stub!("ChannelRedirect", "Redirect a channel");
    register_stub!("ControlPlayback", "Playback with controls");
    register_stub!("DBput", "Write to AstDB");
    register_stub!("DBget", "Read from AstDB");
    register_stub!("DBdel", "Delete from AstDB");
    register_stub!("DBdeltree", "Delete tree from AstDB");
    register_stub!("DumpChan", "Dump channel info");
    register_stub!("SendDTMF", "Send DTMF digits");
    register_stub!("ReceiveDTMF", "Receive DTMF digits");
    register_stub!("ReadExten", "Read extension with matching");
    register_stub!("Macro", "Legacy macro subroutine");
    register_stub!("MacroExclusive", "Exclusive macro subroutine");
    register_stub!("MacroExit", "Exit a macro");
    register_stub!("MacroIf", "Conditional macro");
    register_stub!("While", "Start a while loop");
    register_stub!("EndWhile", "End a while loop");
    register_stub!("ExitWhile", "Exit a while loop");
    register_stub!("ContinueWhile", "Continue a while loop");
    register_stub!("GotoIf", "Conditional goto");
    register_stub!("GotoIfTime", "Time-based conditional goto");
    register_stub!("If", "Conditional block");
    register_stub!("ElseIf", "Else-if block");
    register_stub!("Else", "Else block");
    register_stub!("EndIf", "End conditional block");
    // Set, MSet, Goto are registered as real adapters above
    register_stub!("SayUnixTime", "Say date/time from timestamp");
    register_stub!("DateTime", "Say date/time");
    register_stub!("TDD", "TDD/TTY for hearing impaired");
    register_stub!("AMD", "Answering machine detection");
    register_stub!("StatsD", "Send metrics to StatsD");
    register_stub!("ChanIsAvail", "Check channel availability");
    register_stub!("IVRDemo", "IVR demo");
    register_stub!("SendImage", "Send image to channel");
    register_stub!("ADSIProg", "ADSI programming");
    // UserEvent is registered as a real adapter above -- do NOT re-register as stub.
    register_stub!("WaitForRing", "Wait for ring");
    register_stub!("WaitForSilence", "Wait for silence");
    register_stub!("WaitForNoise", "Wait for noise");
    register_stub!("StreamEcho", "Multistream echo test");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_result() {
        assert_eq!(map_result(PbxExecResult::Success), PbxResult::Success);
        assert_eq!(map_result(PbxExecResult::Failed), PbxResult::Failed);
        assert_eq!(map_result(PbxExecResult::Hangup), PbxResult::Failed);
    }

    #[test]
    fn test_sync_adapter() {
        let adapter = AppAdapter::new(
            "TestSync",
            "A test sync app",
            |_channel, _args| PbxExecResult::Success,
        );
        assert_eq!(adapter.name(), "TestSync");
        assert_eq!(adapter.synopsis(), "A test sync app");
    }

    #[test]
    fn test_async_adapter() {
        let adapter = AsyncAppAdapter::new(
            "TestAsync",
            "A test async app",
            |_channel, _args| Box::pin(async { PbxExecResult::Success }),
        );
        assert_eq!(adapter.name(), "TestAsync");
        assert_eq!(adapter.synopsis(), "A test async app");
    }

    #[tokio::test]
    async fn test_sync_adapter_execute() {
        let adapter = AppAdapter::new(
            "TestExec",
            "Test",
            |channel, args| {
                channel.set_variable("TEST", args);
                PbxExecResult::Success
            },
        );

        let mut channel = Channel::new("Test/test");
        let result = adapter.execute(&mut channel, "hello").await;
        assert_eq!(result, PbxResult::Success);
        assert_eq!(channel.get_variable("TEST"), Some("hello"));
    }

    #[tokio::test]
    async fn test_async_adapter_execute() {
        let adapter = AsyncAppAdapter::new(
            "TestAsyncExec",
            "Test",
            |channel, args| {
                Box::pin(async move {
                    channel.set_variable("ASYNC_TEST", args);
                    PbxExecResult::Success
                })
            },
        );

        let mut channel = Channel::new("Test/async");
        let result = adapter.execute(&mut channel, "world").await;
        assert_eq!(result, PbxResult::Success);
        assert_eq!(channel.get_variable("ASYNC_TEST"), Some("world"));
    }
}
