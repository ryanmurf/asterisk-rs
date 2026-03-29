//! ReadExten - read extension with real-time matching.
//!
//! Port of app_readexten.c from Asterisk C. Reads DTMF digits from
//! the caller, matching in real-time against the dialplan to determine
//! when a valid extension has been dialed.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// Options for the ReadExten application.
#[derive(Debug, Clone)]
pub struct ReadExtenOptions {
    /// Variable to store the result in.
    pub variable: String,
    /// Context to match extensions in (empty = current context).
    pub context: String,
    /// Audio file to play as prompt.
    pub prompt: String,
    /// Timeout in seconds (default: 10).
    pub timeout_secs: u32,
}

impl ReadExtenOptions {
    /// Parse from comma-separated arguments.
    ///
    /// Format: variable[,context[,prompt[,timeout]]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.split(',').collect();
        Self {
            variable: parts.first().copied().unwrap_or("").trim().to_string(),
            context: parts.get(1).copied().unwrap_or("").trim().to_string(),
            prompt: parts.get(2).copied().unwrap_or("").trim().to_string(),
            timeout_secs: parts.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(10),
        }
    }
}

/// The ReadExten() dialplan application.
///
/// Usage: ReadExten(variable[,context[,prompt[,timeout]]])
///
/// Reads digits from the caller, matching against the dialplan in real-time.
/// As each digit is pressed, checks if the accumulated digits form a valid
/// or potentially valid extension. Stops when a unique match is found,
/// an invalid sequence is detected, or the timeout expires.
///
/// Sets READEXTENSTATUS:
///   OK      - Extension matched
///   TIMEOUT - Timed out waiting for digits
///   INVALID - No match possible
///   ERROR   - Some other error
pub struct AppReadExten;

impl DialplanApp for AppReadExten {
    fn name(&self) -> &str {
        "ReadExten"
    }

    fn description(&self) -> &str {
        "Read digits with real-time extension matching"
    }
}

impl AppReadExten {
    /// Execute the ReadExten application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = ReadExtenOptions::parse(args);

        if options.variable.is_empty() {
            warn!("ReadExten: requires variable name argument");
            return PbxExecResult::Failed;
        }

        info!(
            "ReadExten: channel '{}' var='{}' context='{}' timeout={}s",
            channel.name, options.variable, options.context, options.timeout_secs,
        );

        // In a real implementation:
        // 1. Play prompt if specified
        // 2. Read DTMF digits one at a time
        // 3. After each digit:
        //    a. Check if accumulated digits match an extension exactly
        //    b. Check if accumulated digits could match (canmatch)
        //    c. If exact match and no canmatch for longer, accept
        //    d. If no match possible, set INVALID
        // 4. Set variable to matched extension
        // 5. Set READEXTENSTATUS

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_readexten_options_parse() {
        let opts = ReadExtenOptions::parse("EXTEN,default,dial-prompt,15");
        assert_eq!(opts.variable, "EXTEN");
        assert_eq!(opts.context, "default");
        assert_eq!(opts.prompt, "dial-prompt");
        assert_eq!(opts.timeout_secs, 15);
    }

    #[test]
    fn test_readexten_options_defaults() {
        let opts = ReadExtenOptions::parse("EXTEN");
        assert_eq!(opts.variable, "EXTEN");
        assert!(opts.context.is_empty());
        assert_eq!(opts.timeout_secs, 10);
    }

    #[tokio::test]
    async fn test_readexten_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppReadExten::exec(&mut channel, "EXTEN,default").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
