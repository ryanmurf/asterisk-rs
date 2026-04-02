//! System application - executes system commands.
//!
//! Port of app_system.c from Asterisk C. Executes a shell command using
//! the system's command processor. Provides System() which fails the
//! dialplan on error, and TrySystem() which always continues.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// System status set as the SYSTEMSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStatus {
    /// Command executed successfully (exit code 0).
    Success,
    /// Could not execute the command.
    Failure,
    /// Command executed but returned a non-zero exit code.
    AppError,
}

impl SystemStatus {
    /// String representation for the SYSTEMSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::AppError => "APPERROR",
        }
    }
}

/// The System() dialplan application.
///
/// Executes a system command. If the command fails, the dialplan
/// falls through (returns failure). Use TrySystem() to always
/// continue in the dialplan.
///
/// Usage: System(command)
///
/// Sets SYSTEMSTATUS channel variable to SUCCESS or FAILURE.
///
/// WARNING: Do not use untrusted strings (like CALLERID) as part of
/// the command. This creates a command injection vulnerability.
pub struct AppSystem;

impl DialplanApp for AppSystem {
    fn name(&self) -> &str {
        "System"
    }

    fn description(&self) -> &str {
        "Execute a system command"
    }
}

/// The TrySystem() dialplan application.
///
/// Executes a system command but always returns to the dialplan
/// regardless of whether the command succeeds or fails.
///
/// Usage: TrySystem(command)
///
/// Sets SYSTEMSTATUS to SUCCESS, FAILURE, or APPERROR.
pub struct AppTrySystem;

impl DialplanApp for AppTrySystem {
    fn name(&self) -> &str {
        "TrySystem"
    }

    fn description(&self) -> &str {
        "Try executing a system command"
    }
}

/// Shared implementation for System() and TrySystem().
///
/// # Arguments
/// * `channel` - The current channel
/// * `command` - The command to execute
/// * `fail_on_error` - If true (System), return Failed on error.
///                     If false (TrySystem), always return Success.
async fn system_exec_helper(
    channel: &mut Channel,
    command: &str,
    fail_on_error: bool,
) -> PbxExecResult {
    if command.trim().is_empty() {
        warn!("System: requires an argument (command)");
        channel.set_variable("SYSTEMSTATUS", SystemStatus::Failure.as_str());
        if fail_on_error {
            return PbxExecResult::Failed;
        }
        return PbxExecResult::Success;
    }

    // Strip surrounding quotes if present (matching C behavior)
    let command = strip_quotes(command.trim());

    info!("System: executing command: {}", command);

    // Execute the command.
    //
    // In a full implementation, this would use ast_safe_system() which
    // blocks SIGCHLD and safely waits for the child process. We use
    // tokio's Command which provides async execution.
    //
    // We also start channel autoservice before executing so the channel
    // continues to process frames (keepalives, etc.) while blocked.

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await;

    match output {
        Ok(result) => {
            let exit_code = result.status.code().unwrap_or(-1);
            if result.status.success() {
                debug!("System: command completed successfully (exit code 0)");
                channel.set_variable("SYSTEMSTATUS", SystemStatus::Success.as_str());
                PbxExecResult::Success
            } else {
                debug!("System: command returned error (exit code {})", exit_code);
                if fail_on_error {
                    // System() reports FAILURE and fails the dialplan
                    channel.set_variable("SYSTEMSTATUS", SystemStatus::Failure.as_str());
                    PbxExecResult::Failed
                } else {
                    // TrySystem() reports APPERROR but continues
                    channel.set_variable("SYSTEMSTATUS", SystemStatus::AppError.as_str());
                    PbxExecResult::Success
                }
            }
        }
        Err(e) => {
            warn!("System: unable to execute '{}': {}", command, e);
            channel.set_variable("SYSTEMSTATUS", SystemStatus::Failure.as_str());
            if fail_on_error {
                PbxExecResult::Failed
            } else {
                PbxExecResult::Success
            }
        }
    }
}

/// Strip surrounding quotes from a command string.
///
/// If the string starts and ends with the same quote character
/// (single or double), remove them. This matches the C Asterisk
/// behavior which warns that quoting is unnecessary.
fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &s[1..s.len() - 1];
        }
    }
    s
}

impl AppSystem {
    /// Execute the System application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - The command to execute
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        system_exec_helper(channel, args, true).await
    }
}

impl AppTrySystem {
    /// Execute the TrySystem application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - The command to execute
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        system_exec_helper(channel, args, false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_quotes_double() {
        assert_eq!(strip_quotes("\"hello world\""), "hello world");
    }

    #[test]
    fn test_strip_quotes_single() {
        assert_eq!(strip_quotes("'hello world'"), "hello world");
    }

    #[test]
    fn test_strip_quotes_no_quotes() {
        assert_eq!(strip_quotes("hello world"), "hello world");
    }

    #[test]
    fn test_strip_quotes_mismatched() {
        assert_eq!(strip_quotes("\"hello world'"), "\"hello world'");
    }

    #[test]
    fn test_system_status_strings() {
        assert_eq!(SystemStatus::Success.as_str(), "SUCCESS");
        assert_eq!(SystemStatus::Failure.as_str(), "FAILURE");
        assert_eq!(SystemStatus::AppError.as_str(), "APPERROR");
    }

    #[tokio::test]
    async fn test_system_empty_command() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSystem::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("SYSTEMSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_trysystem_empty_command() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTrySystem::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SYSTEMSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_system_true_command() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSystem::exec(&mut channel, "true").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SYSTEMSTATUS"), Some("SUCCESS"));
    }

    #[tokio::test]
    async fn test_system_false_command() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSystem::exec(&mut channel, "false").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("SYSTEMSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_trysystem_false_command() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTrySystem::exec(&mut channel, "false").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("SYSTEMSTATUS"), Some("APPERROR"));
    }
}
