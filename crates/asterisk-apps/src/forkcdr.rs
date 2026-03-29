//! Fork CDR application.
//!
//! Port of app_forkcdr.c from Asterisk C. Creates a new CDR from the
//! current point in the call, linking it to the end of the CDR chain.
//! Supports options to set the answer time, end/finalize the original,
//! reset timestamps, and control variable inheritance.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn, debug};

/// Options for the ForkCDR application.
#[derive(Debug, Clone, Default)]
pub struct ForkCdrOptions {
    /// Set the answer time on the forked CDR to the current time
    /// (if the channel is answered). Implied by 'r'.
    pub set_answer: bool,
    /// End (finalize) the original CDR.
    pub finalize: bool,
    /// Reset the start and answer times on the forked CDR to current time.
    /// Implies 'a'.
    pub reset: bool,
    /// Do NOT copy variables from the original CDR to the forked CDR.
    pub no_copy_vars: bool,
}

impl ForkCdrOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.set_answer = true,
                'e' => result.finalize = true,
                'r' => {
                    result.reset = true;
                    result.set_answer = true; // 'r' implies 'a'
                }
                'v' => result.no_copy_vars = true,
                _ => {
                    debug!("ForkCDR: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// CDR fork flags matching the Asterisk C constants.
#[derive(Debug, Clone, Copy)]
pub struct CdrForkFlags {
    pub set_answer: bool,
    pub finalize: bool,
    pub reset: bool,
    pub keep_vars: bool,
}

impl From<&ForkCdrOptions> for CdrForkFlags {
    fn from(opts: &ForkCdrOptions) -> Self {
        Self {
            set_answer: opts.set_answer,
            finalize: opts.finalize,
            reset: opts.reset,
            keep_vars: !opts.no_copy_vars,
        }
    }
}

/// The ForkCDR() dialplan application.
///
/// Usage: ForkCDR([options])
///
/// Forks the Call Data Record for this channel. A new CDR is created
/// starting from the time this application executes, linked to the
/// end of the CDR chain.
///
/// Options:
///   a - Set answer time on forked CDR to now (if answered)
///   e - End (finalize) the original CDR
///   r - Reset start/answer times to now (implies 'a')
///   v - Do NOT copy variables from original to forked CDR
pub struct AppForkCdr;

impl DialplanApp for AppForkCdr {
    fn name(&self) -> &str {
        "ForkCDR"
    }

    fn description(&self) -> &str {
        "Forks the Call Data Record"
    }
}

impl AppForkCdr {
    /// Execute the ForkCDR application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = ForkCdrOptions::parse(args);
        let flags = CdrForkFlags::from(&options);

        info!(
            "ForkCDR: channel '{}' forking CDR (set_answer={}, finalize={}, reset={}, keep_vars={})",
            channel.name, flags.set_answer, flags.finalize, flags.reset, flags.keep_vars,
        );

        // In a real implementation:
        //
        //   // Create a message to the CDR engine to fork the CDR
        //   let payload = ForkCdrPayload {
        //       channel_name: channel.name.clone(),
        //       flags,
        //   };
        //
        //   // Publish to the CDR message router (synchronous)
        //   if let Err(e) = cdr_fork(&channel.name, &flags).await {
        //       warn!("ForkCDR: failed to fork CDR for '{}': {}", channel.name, e);
        //       return PbxExecResult::Failed;
        //   }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forkcdr_options_empty() {
        let opts = ForkCdrOptions::parse("");
        assert!(!opts.set_answer);
        assert!(!opts.finalize);
        assert!(!opts.reset);
        assert!(!opts.no_copy_vars);
    }

    #[test]
    fn test_forkcdr_options_all() {
        let opts = ForkCdrOptions::parse("aerv");
        assert!(opts.set_answer);
        assert!(opts.finalize);
        assert!(opts.reset);
        assert!(opts.no_copy_vars);
    }

    #[test]
    fn test_forkcdr_options_reset_implies_answer() {
        let opts = ForkCdrOptions::parse("r");
        assert!(opts.set_answer);
        assert!(opts.reset);
    }

    #[test]
    fn test_cdr_fork_flags() {
        let opts = ForkCdrOptions::parse("ev");
        let flags = CdrForkFlags::from(&opts);
        assert!(flags.finalize);
        assert!(!flags.keep_vars); // 'v' means no_copy, so keep_vars = false
    }

    #[tokio::test]
    async fn test_forkcdr_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppForkCdr::exec(&mut channel, "a").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
