//! Directed call pickup applications.
//!
//! Port of app_directed_pickup.c from Asterisk C. Provides Pickup() for
//! directed extension-based call pickup and PickupChan() for channel
//! name-based call pickup. These allow a user to answer a call that is
//! ringing on another extension or channel.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// The special context name for pickup by PICKUPMARK variable.
pub const PICKUP_MARK_CONTEXT: &str = "PICKUPMARK";

/// Method used to find the pickup target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickupMethod {
    /// Pickup by extension and context.
    Extension { extension: String, context: String },
    /// Pickup by PICKUPMARK channel variable value.
    Mark(String),
    /// Pickup by pickup group (no arguments given).
    Group,
}

/// Options for PickupChan.
#[derive(Debug, Clone, Default)]
pub struct PickupChanOptions {
    /// Treat channel names as prefixes ('p' option).
    pub partial_match: bool,
}

impl PickupChanOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'p' => result.partial_match = true,
                _ => {
                    debug!("PickupChan: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parse the target list for Pickup().
///
/// Format: extension[@context][&extension2[@context2]...]
/// If context is PICKUPMARK, picks up by mark.
/// If no context, uses the channel's current context.
pub fn parse_pickup_targets(args: &str, default_context: &str) -> Vec<PickupMethod> {
    if args.trim().is_empty() {
        return vec![PickupMethod::Group];
    }

    let mut targets = Vec::new();

    for target in args.split('&') {
        let target = target.trim();
        if target.is_empty() {
            continue;
        }

        if let Some(at_pos) = target.find('@') {
            let extension = target[..at_pos].to_string();
            let context = target[at_pos + 1..].to_string();

            if context.eq_ignore_ascii_case(PICKUP_MARK_CONTEXT) {
                targets.push(PickupMethod::Mark(extension));
            } else {
                targets.push(PickupMethod::Extension {
                    extension,
                    context: if context.is_empty() {
                        default_context.to_string()
                    } else {
                        context
                    },
                });
            }
        } else {
            targets.push(PickupMethod::Extension {
                extension: target.to_string(),
                context: default_context.to_string(),
            });
        }
    }

    targets
}

/// The Pickup() dialplan application.
///
/// Usage: Pickup([extension[@context]][&extension2[@context2]...])
///
/// Directed extension call pickup. Picks up a ringing channel:
///
/// 1. No arguments: pickup by call group matching.
/// 2. extension@PICKUPMARK: pickup by channel variable PICKUPMARK value.
/// 3. extension[@context]: pickup by extension and context.
///
/// On success, returns -1 (channel is now a zombie connected to the
/// picked-up call). On failure, returns 0 and continues dialplan.
pub struct AppPickup;

impl DialplanApp for AppPickup {
    fn name(&self) -> &str {
        "Pickup"
    }

    fn description(&self) -> &str {
        "Directed extension call pickup"
    }
}

impl AppPickup {
    /// Execute the Pickup application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let targets = parse_pickup_targets(args, "default");

        info!(
            "Pickup: channel '{}' attempting pickup ({} targets)",
            channel.name,
            targets.len(),
        );

        // In a real implementation:
        //
        //   for target in &targets {
        //       let result = match target {
        //           PickupMethod::Group => {
        //               // Find by pickup group
        //               let target_chan = find_by_pickup_group(channel).await;
        //               if let Some(target) = target_chan {
        //                   do_pickup(channel, &target).await
        //               } else {
        //                   Err(PickupError::NotFound)
        //               }
        //           }
        //           PickupMethod::Mark(mark) => {
        //               // Find channel with PICKUPMARK variable == mark
        //               let target_chan = find_by_mark(channel, mark).await;
        //               if let Some(target) = target_chan {
        //                   do_pickup(channel, &target).await
        //               } else {
        //                   Err(PickupError::NotFound)
        //               }
        //           }
        //           PickupMethod::Extension { extension, context } => {
        //               // Find channel by extension@context
        //               let target_chan = find_by_exten(channel, extension, context).await;
        //               if let Some(target) = target_chan {
        //                   do_pickup(channel, &target).await
        //               } else {
        //                   Err(PickupError::NotFound)
        //               }
        //           }
        //       };
        //
        //       if result.is_ok() {
        //           // Pickup successful - stop dialplan, channel is now a zombie
        //           return PbxExecResult::Hangup;
        //       }
        //
        //       warn!("Pickup: no target found for {:?}", target);
        //   }
        //
        //   // All targets failed, continue dialplan
        //   PbxExecResult::Success

        info!(
            "Pickup: channel '{}' pickup attempted",
            channel.name,
        );
        PbxExecResult::Success
    }
}

/// The PickupChan() dialplan application.
///
/// Usage: PickupChan(channel[&channel2...][,options])
///
/// Picks up a ringing channel by channel name or unique ID.
///
/// Options:
///   p - Channel names are prefixes (e.g., "SIP/bob" matches "SIP/bob-00000000")
pub struct AppPickupChan;

impl DialplanApp for AppPickupChan {
    fn name(&self) -> &str {
        "PickupChan"
    }

    fn description(&self) -> &str {
        "Pickup a ringing channel"
    }
}

impl AppPickupChan {
    /// Execute the PickupChan application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let channel_list = match parts.first() {
            Some(c) if !c.trim().is_empty() => c.trim(),
            _ => {
                warn!("PickupChan: requires a channel argument");
                return PbxExecResult::Success; // Match Asterisk behavior: keep going
            }
        };

        let options = parts
            .get(1)
            .map(|o| PickupChanOptions::parse(o.trim()))
            .unwrap_or_default();

        let channel_names: Vec<&str> = channel_list
            .split('&')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        info!(
            "PickupChan: channel '{}' attempting pickup of {} channels (partial={})",
            channel.name,
            channel_names.len(),
            options.partial_match,
        );

        // In a real implementation:
        //
        //   for chan_name in &channel_names {
        //       let result = if options.partial_match {
        //           // Find by partial channel name (prefix match)
        //           pickup_by_part(channel, chan_name).await
        //       } else {
        //           // Find by exact channel name or unique ID
        //           pickup_by_channel(channel, chan_name).await
        //       };
        //
        //       if result.is_ok() {
        //           // Pickup successful
        //           return PbxExecResult::Hangup;
        //       }
        //
        //       warn!("PickupChan: no target found for '{}'", chan_name);
        //   }
        //
        //   // All targets failed
        //   PbxExecResult::Success

        info!(
            "PickupChan: channel '{}' pickup attempted",
            channel.name,
        );
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pickup_targets_empty() {
        let targets = parse_pickup_targets("", "default");
        assert_eq!(targets, vec![PickupMethod::Group]);
    }

    #[test]
    fn test_parse_pickup_targets_extension() {
        let targets = parse_pickup_targets("100", "default");
        assert_eq!(
            targets,
            vec![PickupMethod::Extension {
                extension: "100".to_string(),
                context: "default".to_string(),
            }]
        );
    }

    #[test]
    fn test_parse_pickup_targets_extension_context() {
        let targets = parse_pickup_targets("100@internal", "default");
        assert_eq!(
            targets,
            vec![PickupMethod::Extension {
                extension: "100".to_string(),
                context: "internal".to_string(),
            }]
        );
    }

    #[test]
    fn test_parse_pickup_targets_mark() {
        let targets = parse_pickup_targets("sales@PICKUPMARK", "default");
        assert_eq!(targets, vec![PickupMethod::Mark("sales".to_string())]);
    }

    #[test]
    fn test_parse_pickup_targets_multiple() {
        let targets = parse_pickup_targets("100@internal&200@sales", "default");
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn test_pickupchan_options() {
        let opts = PickupChanOptions::parse("p");
        assert!(opts.partial_match);
    }

    #[tokio::test]
    async fn test_pickup_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppPickup::exec(&mut channel, "100@default").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_pickupchan_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppPickupChan::exec(&mut channel, "SIP/bob").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
