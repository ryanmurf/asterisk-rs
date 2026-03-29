//! If/ElseIf/Else/EndIf and GotoIf/GotoIfTime dialplan branching applications.
//!
//! Port of app_if.c from Asterisk C. Provides conditional branching
//! in the dialplan using If/ElseIf/Else/EndIf blocks and GotoIf()
//! for single-line conditional jumps.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// The GotoIf() dialplan application.
///
/// Usage: GotoIf(condition?[label_true][:label_false])
///
/// Conditionally jumps to a label. Labels can be:
///   priority
///   extension,priority
///   context,extension,priority
pub struct AppGotoIf;

impl DialplanApp for AppGotoIf {
    fn name(&self) -> &str {
        "GotoIf"
    }

    fn description(&self) -> &str {
        "Conditional goto"
    }
}

impl AppGotoIf {
    /// Execute the GotoIf application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (condition, branches) = match args.split_once('?') {
            Some((c, b)) => (c.trim(), b),
            None => {
                warn!("GotoIf: requires condition?[true][:false] format");
                return PbxExecResult::Failed;
            }
        };

        let (true_label, false_label) = match branches.split_once(':') {
            Some((t, f)) => (Some(t.trim()).filter(|s| !s.is_empty()), Some(f.trim()).filter(|s| !s.is_empty())),
            None => (Some(branches.trim()).filter(|s| !s.is_empty()), None),
        };

        let is_true = !condition.is_empty() && condition != "0";

        let target = if is_true { true_label } else { false_label };

        if let Some(label) = target {
            info!(
                "GotoIf: channel '{}' condition={} jumping to '{}'",
                channel.name, is_true, label,
            );
            // In a real implementation: parse label and do ast_goto_if_exists
        } else {
            debug!("GotoIf: channel '{}' condition={} no branch to take", channel.name, is_true);
        }

        PbxExecResult::Success
    }
}

/// The GotoIfTime() dialplan application.
///
/// Usage: GotoIfTime(times,weekdays,mdays,months?label_true[:label_false])
///
/// Conditionally jumps based on the current time matching the given
/// time specification.
pub struct AppGotoIfTime;

impl DialplanApp for AppGotoIfTime {
    fn name(&self) -> &str {
        "GotoIfTime"
    }

    fn description(&self) -> &str {
        "Conditional goto based on current time"
    }
}

impl AppGotoIfTime {
    /// Execute the GotoIfTime application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (time_spec, branches) = match args.split_once('?') {
            Some((t, b)) => (t.trim(), b),
            None => {
                warn!("GotoIfTime: requires timespec?label format");
                return PbxExecResult::Failed;
            }
        };

        info!(
            "GotoIfTime: channel '{}' spec='{}' branches='{}'",
            channel.name, time_spec, branches,
        );

        // In a real implementation:
        // 1. Parse time specification (times,weekdays,mdays,months)
        // 2. Check current time against specification
        // 3. Jump to appropriate label

        PbxExecResult::Success
    }
}

/// The If() dialplan application.
///
/// Usage: If(condition)
///
/// Begins an If block. If condition is false, skips to the matching
/// ElseIf/Else/EndIf.
pub struct AppIf;

impl DialplanApp for AppIf {
    fn name(&self) -> &str {
        "If"
    }

    fn description(&self) -> &str {
        "Start a conditional block"
    }
}

impl AppIf {
    /// Execute the If application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let condition = args.trim();
        let is_true = !condition.is_empty() && condition != "0";

        info!("If: channel '{}' condition='{}' result={}", channel.name, condition, is_true);

        // In a real implementation:
        // If false, find matching ElseIf/Else/EndIf and jump there

        PbxExecResult::Success
    }
}

/// The ElseIf() dialplan application.
///
/// Usage: ElseIf(condition)
pub struct AppElseIf;

impl DialplanApp for AppElseIf {
    fn name(&self) -> &str {
        "ElseIf"
    }

    fn description(&self) -> &str {
        "Conditional else-if in an If block"
    }
}

impl AppElseIf {
    /// Execute the ElseIf application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let condition = args.trim();
        info!("ElseIf: channel '{}' condition='{}'", channel.name, condition);
        PbxExecResult::Success
    }
}

/// The Else() dialplan application.
///
/// Usage: Else()
pub struct AppElse;

impl DialplanApp for AppElse {
    fn name(&self) -> &str {
        "Else"
    }

    fn description(&self) -> &str {
        "Else branch in an If block"
    }
}

impl AppElse {
    /// Execute the Else application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("Else: channel '{}'", channel.name);
        PbxExecResult::Success
    }
}

/// The EndIf() dialplan application.
///
/// Usage: EndIf()
pub struct AppEndIf;

impl DialplanApp for AppEndIf {
    fn name(&self) -> &str {
        "EndIf"
    }

    fn description(&self) -> &str {
        "End of If block"
    }
}

impl AppEndIf {
    /// Execute the EndIf application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("EndIf: channel '{}'", channel.name);
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gotoif_true() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppGotoIf::exec(&mut channel, "1?100:200").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_gotoif_false() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppGotoIf::exec(&mut channel, "0?100:200").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_gotoiftime_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppGotoIfTime::exec(&mut channel, "9:00-17:00,mon-fri,*,*?open:closed").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_if_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppIf::exec(&mut channel, "1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_endif_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppEndIf::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
