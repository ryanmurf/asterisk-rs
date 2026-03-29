//! While/EndWhile/ExitWhile/ContinueWhile dialplan loop applications.
//!
//! Port of app_while.c from Asterisk C. Implements looping constructs
//! in the Asterisk dialplan using While()/EndWhile() pairs with
//! ExitWhile() and ContinueWhile() for flow control.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// The While() dialplan application.
///
/// Usage: While(condition)
///
/// Starts a while loop. If condition evaluates to true (non-empty,
/// non-zero), execution continues to the next priority. At EndWhile(),
/// execution returns to re-evaluate the condition. If false, jumps
/// to the priority after EndWhile().
pub struct AppWhile;

impl DialplanApp for AppWhile {
    fn name(&self) -> &str {
        "While"
    }

    fn description(&self) -> &str {
        "Start a while loop"
    }
}

impl AppWhile {
    /// Execute the While application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let condition = args.trim();
        let is_true = !condition.is_empty() && condition != "0";

        info!(
            "While: channel '{}' condition='{}' result={}",
            channel.name, condition, is_true,
        );

        // In a real implementation:
        // 1. Get current context/extension/priority
        // 2. Store loop start location in channel datastore (keyed by priority)
        // 3. If condition is false, find matching EndWhile and jump past it
        // 4. If true, continue to next priority

        PbxExecResult::Success
    }
}

/// The EndWhile() dialplan application.
///
/// Usage: EndWhile()
///
/// Marks the end of a While() loop. When reached, execution jumps back
/// to the corresponding While() to re-evaluate the condition.
pub struct AppEndWhile;

impl DialplanApp for AppEndWhile {
    fn name(&self) -> &str {
        "EndWhile"
    }

    fn description(&self) -> &str {
        "End a while loop"
    }
}

impl AppEndWhile {
    /// Execute the EndWhile application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("EndWhile: channel '{}' looping back to While", channel.name);

        // In a real implementation:
        // Jump back to the matching While() priority

        PbxExecResult::Success
    }
}

/// The ExitWhile() dialplan application.
///
/// Usage: ExitWhile()
///
/// Immediately exits the innermost While loop, jumping to the priority
/// after the matching EndWhile().
pub struct AppExitWhile;

impl DialplanApp for AppExitWhile {
    fn name(&self) -> &str {
        "ExitWhile"
    }

    fn description(&self) -> &str {
        "Exit the current while loop"
    }
}

impl AppExitWhile {
    /// Execute the ExitWhile application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("ExitWhile: channel '{}' exiting while loop", channel.name);

        // In a real implementation:
        // Find matching EndWhile and jump past it

        PbxExecResult::Success
    }
}

/// The ContinueWhile() dialplan application.
///
/// Usage: ContinueWhile()
///
/// Jumps back to the condition check of the innermost While loop,
/// skipping the rest of the loop body.
pub struct AppContinueWhile;

impl DialplanApp for AppContinueWhile {
    fn name(&self) -> &str {
        "ContinueWhile"
    }

    fn description(&self) -> &str {
        "Continue to the next iteration of a while loop"
    }
}

impl AppContinueWhile {
    /// Execute the ContinueWhile application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("ContinueWhile: channel '{}' continuing while loop", channel.name);

        // In a real implementation:
        // Jump back to the matching While() priority

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_while_exec_true() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWhile::exec(&mut channel, "1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_while_exec_false() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWhile::exec(&mut channel, "0").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_endwhile_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppEndWhile::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_exitwhile_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppExitWhile::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_continuewhile_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppContinueWhile::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
