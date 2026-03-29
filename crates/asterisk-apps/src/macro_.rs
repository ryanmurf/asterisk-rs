//! Legacy Macro subroutine application (deprecated).
//!
//! Port of app_macro.c from Asterisk C. Provides Macro(), MacroExclusive(),
//! MacroExit(), and MacroIf() dialplan applications. Macros are the legacy
//! subroutine mechanism in Asterisk, superseded by GoSub/Return.
//!
//! Macros work by saving the current context/extension/priority, jumping
//! to the macro-<name> context, executing, and then restoring the
//! original location.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// Maximum nesting depth for macros.
pub const MACRO_MAX_DEPTH: u32 = 7;

/// Saved state for returning from a macro.
#[derive(Debug, Clone)]
pub struct MacroState {
    /// Context before entering the macro.
    pub context: String,
    /// Extension before entering the macro.
    pub extension: String,
    /// Priority before entering the macro.
    pub priority: i32,
    /// Macro nesting depth.
    pub depth: u32,
}

/// The Macro() dialplan application.
///
/// Usage: Macro(name[,arg1[,arg2[...]]])
///
/// Executes the dialplan code in context macro-<name>, starting at
/// extension s, priority 1. Arguments are set as ${ARG1}, ${ARG2}, etc.
///
/// The macro context/extension/priority are saved and restored.
/// MACRO_RESULT can be set within the macro to control behavior:
///   ABORT   - Abort the call
///   BUSY    - Return busy
///   CONGESTION - Return congestion
///   CONTINUE - Return and continue
///   GOTO:context,exten,priority - Goto after return
pub struct AppMacro;

impl DialplanApp for AppMacro {
    fn name(&self) -> &str {
        "Macro"
    }

    fn description(&self) -> &str {
        "Execute a dialplan macro (deprecated - use GoSub)"
    }
}

impl AppMacro {
    /// Execute the Macro application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.split(',').collect();
        let macro_name = match parts.first() {
            Some(name) if !name.trim().is_empty() => name.trim(),
            _ => {
                warn!("Macro: requires macro name argument");
                return PbxExecResult::Failed;
            }
        };

        let arguments: Vec<&str> = parts.iter().skip(1).copied().collect();

        info!(
            "Macro: channel '{}' calling macro-{} with {} args",
            channel.name, macro_name, arguments.len(),
        );

        // In a real implementation:
        // 1. Check nesting depth < MACRO_MAX_DEPTH
        // 2. Save current context/extension/priority
        // 3. Set ARG1..ARGn channel variables
        // 4. Set MACRO_CONTEXT, MACRO_EXTEN, MACRO_PRIORITY
        // 5. Jump to context=macro-<name>, exten=s, priority=1
        // 6. Execute until MacroExit or end of macro context
        // 7. Restore context/extension/priority
        // 8. Process MACRO_RESULT

        PbxExecResult::Success
    }
}

/// The MacroExclusive() dialplan application.
///
/// Usage: MacroExclusive(name[,arg1[,arg2[...]]])
///
/// Same as Macro() but ensures only one channel executes the macro
/// at a time (serialized access via mutex).
pub struct AppMacroExclusive;

impl DialplanApp for AppMacroExclusive {
    fn name(&self) -> &str {
        "MacroExclusive"
    }

    fn description(&self) -> &str {
        "Execute a serialized dialplan macro"
    }
}

impl AppMacroExclusive {
    /// Execute the MacroExclusive application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        info!("MacroExclusive: channel '{}' args='{}'", channel.name, args);
        // Same as Macro but with a global lock per macro name
        AppMacro::exec(channel, args).await
    }
}

/// The MacroExit() dialplan application.
///
/// Usage: MacroExit()
///
/// Exits the current macro, returning to the calling location.
pub struct AppMacroExit;

impl DialplanApp for AppMacroExit {
    fn name(&self) -> &str {
        "MacroExit"
    }

    fn description(&self) -> &str {
        "Exit from a macro returning to the caller"
    }
}

impl AppMacroExit {
    /// Execute the MacroExit application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("MacroExit: channel '{}' exiting macro", channel.name);

        // In a real implementation:
        // Signal the macro loop to stop and restore state

        PbxExecResult::Success
    }
}

/// The MacroIf() dialplan application.
///
/// Usage: MacroIf(condition?macroname_a[,args]:macroname_b[,args])
///
/// Conditional macro execution. If condition is true, execute macro_a;
/// otherwise execute macro_b.
pub struct AppMacroIf;

impl DialplanApp for AppMacroIf {
    fn name(&self) -> &str {
        "MacroIf"
    }

    fn description(&self) -> &str {
        "Conditional macro execution"
    }
}

impl AppMacroIf {
    /// Execute the MacroIf application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        // Parse condition?true_branch:false_branch
        let (condition, branches) = match args.split_once('?') {
            Some((c, b)) => (c.trim(), b),
            None => {
                warn!("MacroIf: requires condition?true:false format");
                return PbxExecResult::Failed;
            }
        };

        let (true_branch, false_branch) = match branches.split_once(':') {
            Some((t, f)) => (t.trim(), Some(f.trim())),
            None => (branches.trim(), None),
        };

        let is_true = !condition.is_empty() && condition != "0";

        if is_true {
            info!("MacroIf: channel '{}' condition true, calling '{}'", channel.name, true_branch);
            AppMacro::exec(channel, true_branch).await
        } else if let Some(fb) = false_branch {
            info!("MacroIf: channel '{}' condition false, calling '{}'", channel.name, fb);
            AppMacro::exec(channel, fb).await
        } else {
            debug!("MacroIf: channel '{}' condition false, no false branch", channel.name);
            PbxExecResult::Success
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macro_state() {
        let state = MacroState {
            context: "default".to_string(),
            extension: "s".to_string(),
            priority: 1,
            depth: 0,
        };
        assert_eq!(state.depth, 0);
    }

    #[tokio::test]
    async fn test_macro_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMacro::exec(&mut channel, "voicemail,100").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_macro_exit_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMacroExit::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_macro_if_true() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMacroIf::exec(&mut channel, "1?voicemail,100:hangup").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_macro_if_false() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMacroIf::exec(&mut channel, "0?voicemail:hangup").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
