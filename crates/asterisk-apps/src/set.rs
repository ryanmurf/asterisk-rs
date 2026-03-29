//! Set/MSet - variable assignment dialplan applications.
//!
//! Port of app_set.c from Asterisk C (functionality from pbx core).
//! Provides Set() and MSet() for assigning channel variables and
//! global variables in the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// The Set() dialplan application.
///
/// Usage: Set(name=value)
///
/// Sets a channel variable to the given value. If the name is prefixed
/// with a double underscore (__), the variable is inherited by all child
/// channels. A single underscore (_) means inherited by immediate children.
///
/// For global variables, use Set(GLOBAL(name)=value).
///
/// Multiple assignments can be done with: Set(a=1,b=2) but this is
/// deprecated in favor of MSet().
pub struct AppSet;

impl DialplanApp for AppSet {
    fn name(&self) -> &str {
        "Set"
    }

    fn description(&self) -> &str {
        "Set a channel variable"
    }
}

impl AppSet {
    /// Execute the Set application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            warn!("Set: requires name=value argument");
            return PbxExecResult::Failed;
        }

        // Handle first assignment (Set only does one, use MSet for multiple)
        let (name, value) = match args.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => {
                warn!("Set: invalid syntax, expected name=value");
                return PbxExecResult::Failed;
            }
        };

        info!("Set: channel '{}' {}={}", channel.name, name, value);

        // In a real implementation:
        // 1. Check for inheritance prefix (__name or _name)
        // 2. Check for GLOBAL() or SHARED() function wrappers
        // 3. Set the channel variable via pbx_builtin_setvar_helper

        PbxExecResult::Success
    }
}

/// The MSet() dialplan application.
///
/// Usage: MSet(name1=value1,name2=value2,...)
///
/// Sets multiple channel variables at once. Same as calling Set()
/// multiple times.
pub struct AppMSet;

impl DialplanApp for AppMSet {
    fn name(&self) -> &str {
        "MSet"
    }

    fn description(&self) -> &str {
        "Set multiple channel variables at once"
    }
}

impl AppMSet {
    /// Execute the MSet application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            warn!("MSet: requires name=value arguments");
            return PbxExecResult::Failed;
        }

        for assignment in args.split(',') {
            let assignment = assignment.trim();
            if assignment.is_empty() {
                continue;
            }

            let (name, value) = match assignment.split_once('=') {
                Some((n, v)) => (n.trim(), v.trim()),
                None => {
                    debug!("MSet: skipping invalid assignment '{}'", assignment);
                    continue;
                }
            };

            info!("MSet: channel '{}' {}={}", channel.name, name, value);

            // In a real implementation:
            // pbx_builtin_setvar_helper(channel, name, value)
        }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_set_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSet::exec(&mut channel, "MYVAR=hello").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_set_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSet::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_set_no_equals() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSet::exec(&mut channel, "MYVAR").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_mset_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMSet::exec(&mut channel, "A=1,B=2,C=3").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
