//! Exec, TryExec, and ExecIf applications - dynamic application execution.
//!
//! Port of app_exec.c from Asterisk C. Allows invoking dialplan applications
//! dynamically by name at runtime, rather than hard-coding them in the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// The Exec() dialplan application.
///
/// Executes an arbitrary dialplan application by name. If the underlying
/// application terminates the dialplan or cannot be found, Exec will
/// also terminate the dialplan.
///
/// Usage: Exec(appname(args))
pub struct AppExec;

impl DialplanApp for AppExec {
    fn name(&self) -> &str {
        "Exec"
    }

    fn description(&self) -> &str {
        "Executes dialplan application"
    }
}

/// The TryExec() dialplan application.
///
/// Like Exec(), but always returns to the dialplan. Sets the TRYSTATUS
/// channel variable to indicate the result.
///
/// Usage: TryExec(appname(args))
///
/// Sets TRYSTATUS to SUCCESS, FAILED, or NOAPP.
pub struct AppTryExec;

impl DialplanApp for AppTryExec {
    fn name(&self) -> &str {
        "TryExec"
    }

    fn description(&self) -> &str {
        "Executes dialplan application, always returning"
    }
}

/// The ExecIf() dialplan application.
///
/// Conditionally executes an application based on an expression.
///
/// Usage: ExecIf(condition?appiftrue(args):appiffalse(args))
pub struct AppExecIf;

impl DialplanApp for AppExecIf {
    fn name(&self) -> &str {
        "ExecIf"
    }

    fn description(&self) -> &str {
        "Executes dialplan application, conditionally"
    }
}

/// Try-exec status set as TRYSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryExecStatus {
    /// Application returned successfully (0).
    Success,
    /// Application returned non-zero.
    Failed,
    /// Application not found or not specified.
    NoApp,
}

impl TryExecStatus {
    /// String representation for the TRYSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::NoApp => "NOAPP",
        }
    }
}

/// Parse an application invocation string into name and arguments.
///
/// Format: `appname(args)` or `appname,args`
///
/// Returns (app_name, app_args).
fn parse_app_invocation(data: &str) -> (String, String) {
    let data = data.trim();
    if data.is_empty() {
        return (String::new(), String::new());
    }

    // Try parenthesized form: appname(args)
    if let Some(paren_pos) = data.find('(') {
        let app_name = data[..paren_pos].trim().to_string();
        let args_str = &data[paren_pos + 1..];
        let args = if let Some(end_paren) = args_str.rfind(')') {
            args_str[..end_paren].to_string()
        } else {
            args_str.to_string()
        };
        return (app_name, args);
    }

    // Try comma-separated form: appname,args
    if let Some(comma_pos) = data.find(',') {
        let app_name = data[..comma_pos].trim().to_string();
        let args = data[comma_pos + 1..].to_string();
        return (app_name, args);
    }

    // Just an app name with no arguments
    (data.to_string(), String::new())
}

impl AppExec {
    /// Execute the Exec application.
    ///
    /// In a full implementation, this looks up the application in the
    /// registry and invokes it. For now, it parses the invocation and
    /// logs the action.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Application invocation: `appname(args)` or `appname,args`
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            return PbxExecResult::Success;
        }

        let (app_name, app_args) = parse_app_invocation(args);

        if app_name.is_empty() {
            return PbxExecResult::Success;
        }

        info!(
            "Exec: channel '{}' executing {}({})",
            channel.name, app_name, app_args
        );

        // In a full implementation:
        //
        //   let registry = AppRegistry::global();
        //   match registry.get(&app_name) {
        //       Some(app) => {
        //           let result = app.execute(channel, &app_args).await;
        //           match result {
        //               PbxResult::Success => PbxExecResult::Success,
        //               PbxResult::Failed => PbxExecResult::Failed,
        //           }
        //       }
        //       None => {
        //           warn!("Exec: could not find application '{}'", app_name);
        //           PbxExecResult::Failed
        //       }
        //   }

        // Stub: log and return success
        debug!(
            "Exec: would execute application '{}' with args '{}'",
            app_name, app_args
        );

        PbxExecResult::Success
    }
}

impl AppTryExec {
    /// Execute the TryExec application.
    ///
    /// Like Exec(), but always returns to the dialplan with TRYSTATUS set.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Application invocation
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            channel.set_variable("TRYSTATUS", TryExecStatus::NoApp.as_str());
            return PbxExecResult::Success;
        }

        let (app_name, app_args) = parse_app_invocation(args);

        if app_name.is_empty() {
            channel.set_variable("TRYSTATUS", TryExecStatus::NoApp.as_str());
            return PbxExecResult::Success;
        }

        info!(
            "TryExec: channel '{}' executing {}({})",
            channel.name, app_name, app_args
        );

        // In a full implementation:
        //
        //   let registry = AppRegistry::global();
        //   match registry.get(&app_name) {
        //       Some(app) => {
        //           let result = app.execute(channel, &app_args).await;
        //           let status = if result == PbxResult::Success {
        //               TryExecStatus::Success
        //           } else {
        //               TryExecStatus::Failed
        //           };
        //           channel.set_variable("TRYSTATUS", status.as_str());
        //       }
        //       None => {
        //           channel.set_variable("TRYSTATUS", TryExecStatus::NoApp.as_str());
        //       }
        //   }

        // Stub: report success
        channel.set_variable("TRYSTATUS", TryExecStatus::Success.as_str());

        debug!(
            "TryExec: TRYSTATUS={}",
            TryExecStatus::Success.as_str()
        );

        PbxExecResult::Success
    }
}

impl AppExecIf {
    /// Execute the ExecIf application.
    ///
    /// Evaluates a condition and executes one of two applications.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - `condition?appiftrue(args)[:appiffalse(args)]`
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (condition, remainder) = match args.split_once('?') {
            Some((c, r)) => (c.trim(), r),
            None => {
                warn!("ExecIf: requires condition?appiftrue[:appiffalse]");
                return PbxExecResult::Failed;
            }
        };

        // Evaluate condition: non-empty, non-zero is true
        let is_true = !condition.is_empty() && condition != "0";

        let app_invocation = if is_true {
            // Use app before ':'
            if let Some(colon_pos) = find_unparenthesized_colon(remainder) {
                &remainder[..colon_pos]
            } else {
                remainder
            }
        } else {
            // Use app after ':'
            if let Some(colon_pos) = find_unparenthesized_colon(remainder) {
                &remainder[colon_pos + 1..]
            } else {
                // No false branch -- just continue
                return PbxExecResult::Success;
            }
        };

        let app_invocation = app_invocation.trim();
        if app_invocation.is_empty() {
            return PbxExecResult::Success;
        }

        debug!(
            "ExecIf: condition='{}' is {}, executing '{}'",
            condition,
            if is_true { "true" } else { "false" },
            app_invocation
        );

        // Execute the selected application
        AppExec::exec(channel, app_invocation).await
    }
}

/// Find the position of a colon ':' that is not inside parentheses.
///
/// This is needed because application arguments may contain colons
/// within parentheses, e.g., `Playback(hello):Hangup()`.
fn find_unparenthesized_colon(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_app_invocation_parens() {
        let (name, args) = parse_app_invocation("Playback(hello-world)");
        assert_eq!(name, "Playback");
        assert_eq!(args, "hello-world");
    }

    #[test]
    fn test_parse_app_invocation_comma() {
        let (name, args) = parse_app_invocation("Playback,hello-world");
        assert_eq!(name, "Playback");
        assert_eq!(args, "hello-world");
    }

    #[test]
    fn test_parse_app_invocation_no_args() {
        let (name, args) = parse_app_invocation("Answer");
        assert_eq!(name, "Answer");
        assert_eq!(args, "");
    }

    #[test]
    fn test_parse_app_invocation_empty() {
        let (name, args) = parse_app_invocation("");
        assert_eq!(name, "");
        assert_eq!(args, "");
    }

    #[test]
    fn test_find_unparenthesized_colon() {
        assert_eq!(
            find_unparenthesized_colon("Playback(hello):Hangup()"),
            Some(15)
        );
        assert_eq!(
            find_unparenthesized_colon("Playback(a:b):Hangup()"),
            Some(13)
        );
        assert_eq!(find_unparenthesized_colon("Playback(hello)"), None);
    }

    #[test]
    fn test_try_exec_status_strings() {
        assert_eq!(TryExecStatus::Success.as_str(), "SUCCESS");
        assert_eq!(TryExecStatus::Failed.as_str(), "FAILED");
        assert_eq!(TryExecStatus::NoApp.as_str(), "NOAPP");
    }

    #[tokio::test]
    async fn test_tryexec_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTryExec::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("TRYSTATUS"), Some("NOAPP"));
    }

    #[tokio::test]
    async fn test_tryexec_with_app() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppTryExec::exec(&mut channel, "Answer()").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("TRYSTATUS"), Some("SUCCESS"));
    }

    #[tokio::test]
    async fn test_execif_true() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppExecIf::exec(&mut channel, "1?Answer():Hangup()").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_execif_false_no_branch() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppExecIf::exec(&mut channel, "0?Answer()").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
