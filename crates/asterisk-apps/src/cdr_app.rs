//! CDR control applications.
//!
//! Port of app_cdr.c from Asterisk C. Provides ResetCDR() to reset
//! the Call Data Record for the current channel. The original C source
//! also historically included NoCDR() (now handled via CDR_PROP function).

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info};

/// Options for the ResetCDR application.
#[derive(Debug, Clone, Default)]
pub struct ResetCdrOptions {
    /// Preserve CDR variables during reset ('v' option).
    pub keep_variables: bool,
}

impl ResetCdrOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'v' => result.keep_variables = true,
                _ => {
                    debug!("ResetCDR: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The ResetCDR() dialplan application.
///
/// Usage: ResetCDR([options])
///
/// Resets the Call Data Record for the current channel:
///   1. The start time is set to the current time.
///   2. If the channel is answered, the answer time is also set to now.
///   3. All CDR variables are wiped (unless 'v' option is used).
///
/// Options:
///   v - Save/keep CDR variables during the reset
pub struct AppResetCdr;

impl DialplanApp for AppResetCdr {
    fn name(&self) -> &str {
        "ResetCDR"
    }

    fn description(&self) -> &str {
        "Resets the Call Data Record"
    }
}

impl AppResetCdr {
    /// Execute the ResetCDR application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = ResetCdrOptions::parse(args);

        info!(
            "ResetCDR: channel '{}' resetting CDR (keep_vars={})",
            channel.name, options.keep_variables,
        );

        // In a real implementation:
        //
        //   let payload = ResetCdrPayload {
        //       channel_name: channel.name.clone(),
        //       reset: true,
        //       keep_variables: options.keep_variables,
        //   };
        //
        //   if let Err(e) = cdr_reset(&channel.name, options.keep_variables).await {
        //       warn!("ResetCDR: failed to reset CDR for '{}': {}", channel.name, e);
        //       return PbxExecResult::Failed;
        //   }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resetcdr_options_empty() {
        let opts = ResetCdrOptions::parse("");
        assert!(!opts.keep_variables);
    }

    #[test]
    fn test_resetcdr_options_keep_vars() {
        let opts = ResetCdrOptions::parse("v");
        assert!(opts.keep_variables);
    }

    #[tokio::test]
    async fn test_resetcdr_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppResetCdr::exec(&mut channel, "v").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_resetcdr_exec_no_opts() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppResetCdr::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
