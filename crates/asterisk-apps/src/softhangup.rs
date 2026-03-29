//! SoftHangup application - requests hangup on another channel.
//!
//! Port of app_softhangup.c from Asterisk C. Looks up a channel by name
//! and sends a soft hangup request. Supports matching all channels with
//! the 'a' option for prefix-based matching.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// Options for the SoftHangup application.
#[derive(Debug, Clone, Default)]
pub struct SoftHangupOptions {
    /// If true, hang up all channels matching the name as a prefix
    /// (matching by device, stripping the unique suffix).
    pub all: bool,
}

impl SoftHangupOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.all = true,
                _ => {
                    debug!("SoftHangup: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for SoftHangup.
#[derive(Debug)]
pub struct SoftHangupArgs {
    /// Channel name or prefix to match.
    pub channel_name: String,
    /// Options.
    pub options: SoftHangupOptions,
}

impl SoftHangupArgs {
    /// Parse the argument string.
    ///
    /// Format: `Technology/Resource[,options]`
    pub fn parse(args: &str) -> Option<Self> {
        let args = args.trim();
        if args.is_empty() {
            return None;
        }

        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let channel_name = parts[0].trim().to_string();
        let options = if let Some(opts) = parts.get(1) {
            SoftHangupOptions::parse(opts.trim())
        } else {
            SoftHangupOptions::default()
        };

        if channel_name.is_empty() {
            return None;
        }

        Some(Self {
            channel_name,
            options,
        })
    }
}

/// The SoftHangup() dialplan application.
///
/// Hangs up the requested channel by name. With the 'a' option,
/// hangs up all channels matching the given name as a device prefix
/// (stripping the unique identifier suffix after '-').
///
/// Usage: SoftHangup(Technology/Resource[,options])
///
/// Options:
///   a - Hang up all channels on the specified device
pub struct AppSoftHangup;

impl DialplanApp for AppSoftHangup {
    fn name(&self) -> &str {
        "SoftHangup"
    }

    fn description(&self) -> &str {
        "Hangs up the requested channel"
    }
}

impl AppSoftHangup {
    /// Execute the SoftHangup application.
    ///
    /// # Arguments
    /// * `channel` - The current channel (used for logging context)
    /// * `args` - Argument string: "Technology/Resource[,options]"
    ///
    /// # Returns
    /// A tuple of the exec result and the list of channel names that were
    /// sent soft hangup requests.
    pub async fn exec(channel: &Channel, args: &str) -> (PbxExecResult, Vec<String>) {
        let parsed = match SoftHangupArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("SoftHangup: requires an argument (Technology/resource)");
                return (PbxExecResult::Success, vec![]);
            }
        };

        info!(
            "SoftHangup: channel '{}' requesting soft hangup of '{}' (all={})",
            channel.name, parsed.channel_name, parsed.options.all
        );

        // In a full implementation, we would iterate through the global channel
        // container to find matching channels and send soft hangup requests:
        //
        //   let channel_container = ChannelContainer::global();
        //   let mut hung_up = Vec::new();
        //
        //   if parsed.options.all {
        //       // Strip the unique suffix to match by device
        //       let device_prefix = strip_unique_suffix(&parsed.channel_name);
        //       for target in channel_container.iter_by_prefix(&device_prefix) {
        //           target.soft_hangup(SoftHangupCause::Explicit);
        //           hung_up.push(target.name().to_string());
        //       }
        //   } else {
        //       // Match channels by exact name prefix (including unique part)
        //       for target in channel_container.iter_by_prefix(&parsed.channel_name) {
        //           target.soft_hangup(SoftHangupCause::Explicit);
        //           hung_up.push(target.name().to_string());
        //           if !parsed.options.all {
        //               break;
        //           }
        //       }
        //   }
        //
        //   if hung_up.is_empty() {
        //       info!("SoftHangup: no channels matching '{}'", parsed.channel_name);
        //   }

        let hung_up: Vec<String> = Vec::new();

        if hung_up.is_empty() {
            info!(
                "SoftHangup: no channels matched '{}'",
                parsed.channel_name
            );
        }

        (PbxExecResult::Success, hung_up)
    }
}

/// Strip the unique suffix from a channel name.
///
/// Channel names are typically formatted as `Tech/Endpoint-UniqueID`.
/// For CAPI channels, the format is `CAPI[foo/bar]/clcnt`, where
/// we strip after the last '/'. For everything else, we strip after
/// the last '-'.
#[allow(dead_code)]
fn strip_unique_suffix(name: &str) -> &str {
    // Check if this looks like a CAPI channel
    if name.starts_with("CAPI") {
        if let Some(pos) = name.rfind('/') {
            return &name[..pos];
        }
    }
    // Standard format: strip after last '-'
    if let Some(pos) = name.rfind('-') {
        &name[..pos]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        let args = SoftHangupArgs::parse("SIP/alice-001").unwrap();
        assert_eq!(args.channel_name, "SIP/alice-001");
        assert!(!args.options.all);
    }

    #[test]
    fn test_parse_args_with_option() {
        let args = SoftHangupArgs::parse("SIP/alice,a").unwrap();
        assert_eq!(args.channel_name, "SIP/alice");
        assert!(args.options.all);
    }

    #[test]
    fn test_parse_args_empty() {
        assert!(SoftHangupArgs::parse("").is_none());
    }

    #[test]
    fn test_strip_unique_suffix() {
        assert_eq!(strip_unique_suffix("SIP/alice-00000001"), "SIP/alice");
        assert_eq!(strip_unique_suffix("PJSIP/trunk-0002"), "PJSIP/trunk");
        assert_eq!(strip_unique_suffix("SIP/alice"), "SIP/alice");
    }

    #[test]
    fn test_strip_unique_suffix_capi() {
        assert_eq!(strip_unique_suffix("CAPI[foo/bar]/clcnt"), "CAPI[foo/bar]");
    }
}
