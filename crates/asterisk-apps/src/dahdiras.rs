//! DAHDI RAS (Remote Access Server) application.
//!
//! Port of app_dahdiras.c from Asterisk C. Launches pppd over a DAHDI
//! channel to provide PPP-based remote access. This is primarily used
//! for legacy modem/ISDN dial-in scenarios where PPP is needed over
//! a telephony channel.
//!
//! In practice, this application hands the DAHDI file descriptor to pppd
//! and waits for the PPP session to complete.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{info, warn};

/// Parsed arguments for the DAHDIRas application.
#[derive(Debug, Clone)]
pub struct DahdiRasArgs {
    /// Arguments to pass to pppd.
    pub pppd_args: Vec<String>,
}

impl DahdiRasArgs {
    /// Parse from a dialplan argument string.
    ///
    /// Format: DAHDIRas(pppd_args)
    ///
    /// The arguments are pipe-separated and passed directly to pppd.
    /// Example: DAHDIRas(debug|192.168.1.1:192.168.1.2|ms-dns 1.1.1.1)
    pub fn parse(args: &str) -> Result<Self, String> {
        let args = args.trim();
        if args.is_empty() {
            return Err("missing pppd arguments".into());
        }

        let pppd_args: Vec<String> = args
            .split('|')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if pppd_args.is_empty() {
            return Err("no valid pppd arguments provided".into());
        }

        Ok(Self { pppd_args })
    }
}

/// The DAHDIRas() dialplan application.
///
/// Runs pppd on a DAHDI channel to provide PPP remote access. The channel
/// must be a DAHDI channel. The application answers the channel, then
/// hands it over to pppd.
///
/// Usage: DAHDIRas(pppd_args)
///
/// This is a stub implementation since DAHDI hardware support and direct
/// file descriptor passing are not yet implemented in the Rust port.
pub struct AppDahdiRas;

impl DialplanApp for AppDahdiRas {
    fn name(&self) -> &str {
        "DAHDIRas"
    }

    fn description(&self) -> &str {
        "DAHDI roles - roles roles for PPP on a DAHDI channel"
    }
}

impl AppDahdiRas {
    /// Execute the DAHDIRas application.
    ///
    /// # Arguments
    /// * `channel` - The DAHDI channel to run PPP on
    /// * `args` - Pipe-separated pppd arguments
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match DahdiRasArgs::parse(args) {
            Ok(a) => a,
            Err(e) => {
                warn!("DAHDIRas: invalid arguments: {}", e);
                return PbxExecResult::Failed;
            }
        };

        info!(
            "DAHDIRas: starting PPP session on channel '{}' with args: {:?}",
            channel.name, parsed.pppd_args
        );

        // Verify this is a DAHDI channel
        if !channel.name.starts_with("DAHDI/") {
            warn!(
                "DAHDIRas: channel '{}' is not a DAHDI channel",
                channel.name
            );
            return PbxExecResult::Failed;
        }

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // In a full implementation:
        // 1. Answer the channel if not already answered
        // 2. Get the DAHDI file descriptor from the channel driver
        // 3. Set the DAHDI channel to linear mode for data
        // 4. Fork pppd with the file descriptor:
        //    pppd nodetach noaccomp <pppd_args> sync notty <fd>
        // 5. Wait for pppd to exit
        // 6. Restore the DAHDI channel settings
        // 7. Continue dialplan execution (the PPP session is done)

        warn!(
            "DAHDIRas: DAHDI hardware support not yet implemented; \
             would launch pppd with args: {:?}",
            parsed.pppd_args
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dahdi_ras_args_parse() {
        let args = DahdiRasArgs::parse("debug|192.168.1.1:192.168.1.2").unwrap();
        assert_eq!(args.pppd_args.len(), 2);
        assert_eq!(args.pppd_args[0], "debug");
        assert_eq!(args.pppd_args[1], "192.168.1.1:192.168.1.2");
    }

    #[test]
    fn test_dahdi_ras_args_single() {
        let args = DahdiRasArgs::parse("debug").unwrap();
        assert_eq!(args.pppd_args.len(), 1);
    }

    #[test]
    fn test_dahdi_ras_args_empty() {
        assert!(DahdiRasArgs::parse("").is_err());
        assert!(DahdiRasArgs::parse("  ").is_err());
    }
}
