//! SHELL() function - execute shell commands.
//!
//! Port of func_shell.c from Asterisk C.
//!
//! Usage: SHELL(command) - execute shell command and return stdout output.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::process::Command;

/// SHELL() function.
///
/// Executes a shell command and returns its stdout output.
/// Trailing newlines are stripped (matching Asterisk behavior).
///
/// Usage: SHELL(command)
///
/// The command is executed via /bin/sh -c "command".
pub struct FuncShell;

impl DialplanFunc for FuncShell {
    fn name(&self) -> &str {
        "SHELL"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let command = args.trim();
        if command.is_empty() {
            return Err(FuncError::InvalidArgument(
                "SHELL: command argument is required".to_string(),
            ));
        }

        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| {
                FuncError::Internal(format!("SHELL: failed to execute '{}': {}", command, e))
            })?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();

        // Strip trailing newline (matches Asterisk C behavior)
        if stdout.ends_with('\n') {
            stdout.pop();
            if stdout.ends_with('\r') {
                stdout.pop();
            }
        }

        Ok(stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_echo() {
        let ctx = FuncContext::new();
        let func = FuncShell;
        let result = func.read(&ctx, "echo hello").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_shell_empty_command() {
        let ctx = FuncContext::new();
        let func = FuncShell;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_shell_pipeline() {
        let ctx = FuncContext::new();
        let func = FuncShell;
        let result = func.read(&ctx, "echo 'abc def' | wc -w").unwrap();
        let count: i32 = result.trim().parse().unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_shell_strips_trailing_newline() {
        let ctx = FuncContext::new();
        let func = FuncShell;
        let result = func.read(&ctx, "printf 'no newline'").unwrap();
        assert_eq!(result, "no newline");

        let result = func.read(&ctx, "printf 'with newline\\n'").unwrap();
        assert_eq!(result, "with newline");
    }
}
