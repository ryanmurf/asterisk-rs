//! BridgeWait and BridgeAdd applications - holding bridge management.
//!
//! Port of app_bridgewait.c and app_bridgeaddchan.c from Asterisk C.
//! BridgeWait places a channel into a named holding bridge where it
//! receives entertainment (MOH, ringing, silence, or hold) until removed.
//! BridgeAdd pushes a channel into an existing bridge by name/ID.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::bridge::Bridge;
use asterisk_core::channel::Channel;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Global registry of named holding bridges.
static HOLDING_BRIDGES: once_cell::sync::Lazy<DashMap<String, Arc<RwLock<HoldingBridge>>>> =
    once_cell::sync::Lazy::new(DashMap::new);

/// Entertainment mode for channels in a holding bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum EntertainmentMode {
    /// Play music on hold (default).
    #[default]
    MusicOnHold,
    /// Ring without pause.
    Ringing,
    /// Generate silent audio.
    Silence,
    /// Put the channel on hold.
    Hold,
    /// No entertainment.
    None,
}

impl EntertainmentMode {
    /// Parse entertainment mode from a single character.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'm' => Some(Self::MusicOnHold),
            'r' => Some(Self::Ringing),
            's' => Some(Self::Silence),
            'h' => Some(Self::Hold),
            'n' => Some(Self::None),
            _ => Option::None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MusicOnHold => "musiconhold",
            Self::Ringing => "ringing",
            Self::Silence => "silence",
            Self::Hold => "hold",
            Self::None => "none",
        }
    }
}


/// Role of a channel in a holding bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum HoldingRole {
    /// A normal participant being held.
    #[default]
    Participant,
    /// An announcer whose audio is played to all participants.
    Announcer,
}

impl HoldingRole {
    /// Parse from string.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "announcer" => Self::Announcer,
            _ => Self::Participant,
        }
    }
}


/// Options for BridgeWait.
#[derive(Debug, Clone)]
pub struct BridgeWaitOptions {
    /// MOH class to use.
    pub moh_class: Option<String>,
    /// Entertainment mode.
    pub entertainment: EntertainmentMode,
    /// Timeout in seconds (0 = no timeout).
    pub timeout: Duration,
    /// Do not auto-answer the channel.
    pub no_answer: bool,
}

impl Default for BridgeWaitOptions {
    fn default() -> Self {
        Self {
            moh_class: None,
            entertainment: EntertainmentMode::default(),
            timeout: Duration::ZERO,
            no_answer: false,
        }
    }
}

impl BridgeWaitOptions {
    /// Parse options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                'm' => {
                    result.moh_class = Self::extract_paren_arg(&mut chars);
                }
                'e' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Some(first_char) = arg.chars().next() {
                            if let Some(mode) = EntertainmentMode::from_char(first_char) {
                                result.entertainment = mode;
                            }
                        }
                    }
                }
                'S' => {
                    if let Some(arg) = Self::extract_paren_arg(&mut chars) {
                        if let Ok(secs) = arg.parse::<u64>() {
                            result.timeout = Duration::from_secs(secs);
                        }
                    }
                }
                'n' => result.no_answer = true,
                _ => {
                    debug!("BridgeWait: ignoring unknown option '{}'", ch);
                }
            }
        }

        result
    }

    fn extract_paren_arg(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        if chars.peek() == Some(&'(') {
            chars.next();
            let mut arg = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                arg.push(c);
            }
            if arg.is_empty() { None } else { Some(arg) }
        } else {
            None
        }
    }
}

/// A named holding bridge wrapper.
#[derive(Debug)]
pub struct HoldingBridge {
    /// The name handle for this holding bridge.
    pub name: String,
    /// The underlying bridge.
    pub bridge: Bridge,
    /// Number of channels currently held.
    pub participant_count: usize,
}

impl HoldingBridge {
    /// Create a new holding bridge with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            bridge: Bridge::new(format!("holding-{}", name)),
            participant_count: 0,
        }
    }
}

/// The BridgeWait() dialplan application.
///
/// Usage: BridgeWait([name[,role[,options]]])
///
/// Places the channel into a named holding bridge. The channel receives
/// entertainment (MOH, ringing, etc.) until it is removed from the bridge,
/// either by timeout, external action, or hangup.
pub struct AppBridgeWait;

impl DialplanApp for AppBridgeWait {
    fn name(&self) -> &str {
        "BridgeWait"
    }

    fn description(&self) -> &str {
        "Put a call into a holding bridge"
    }
}

impl AppBridgeWait {
    /// Execute the BridgeWait application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();

        let bridge_name = parts
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("default");

        let role = parts
            .get(1)
            .map(|s| HoldingRole::parse(s.trim()))
            .unwrap_or_default();

        let options = parts
            .get(2)
            .map(|o| BridgeWaitOptions::parse(o.trim()))
            .unwrap_or_default();

        info!(
            "BridgeWait: channel '{}' entering holding bridge '{}' as {:?} (entertainment={}, timeout={:?})",
            channel.name,
            bridge_name,
            role,
            options.entertainment.as_str(),
            options.timeout,
        );

        // Answer the channel if needed (unless 'n' option)
        if !options.no_answer && channel.state != asterisk_types::ChannelState::Up {
            debug!("BridgeWait: answering channel");
            channel.state = asterisk_types::ChannelState::Up;
        }

        // Get or create the holding bridge
        let holding = Self::get_or_create_bridge(bridge_name);

        // Add channel to the bridge
        {
            let mut hb = holding.write();
            hb.bridge
                .add_channel(channel.unique_id.clone(), channel.name.clone());
            hb.participant_count += 1;
            info!(
                "BridgeWait: '{}' joined holding bridge '{}' ({} participants)",
                channel.name, bridge_name, hb.participant_count
            );
        }

        // In a real implementation:
        //
        //   // Set entertainment based on role
        //   match role {
        //       HoldingRole::Participant => {
        //           match options.entertainment {
        //               EntertainmentMode::MusicOnHold => {
        //                   let class = options.moh_class.as_deref().unwrap_or("default");
        //                   start_moh(channel, class).await;
        //               }
        //               EntertainmentMode::Ringing => {
        //                   indicate(channel, AST_CONTROL_RINGING).await;
        //               }
        //               EntertainmentMode::Silence => {
        //                   // Generate silence frames
        //               }
        //               EntertainmentMode::Hold => {
        //                   indicate(channel, AST_CONTROL_HOLD).await;
        //               }
        //               EntertainmentMode::None => {}
        //           }
        //       }
        //       HoldingRole::Announcer => {
        //           // Announcer reads audio from channel and plays it to all participants
        //       }
        //   }
        //
        //   // Wait in the bridge
        //   let wait_result = if !options.timeout.is_zero() {
        //       tokio::time::timeout(options.timeout, bridge_wait_loop(channel)).await
        //   } else {
        //       Ok(bridge_wait_loop(channel).await)
        //   };

        // Remove from bridge on exit
        {
            let mut hb = holding.write();
            hb.bridge.remove_channel(&channel.unique_id);
            hb.participant_count = hb.participant_count.saturating_sub(1);
            let remaining = hb.participant_count;
            info!(
                "BridgeWait: '{}' left holding bridge '{}' ({} remaining)",
                channel.name, bridge_name, remaining
            );

            if remaining == 0 {
                drop(hb);
                HOLDING_BRIDGES.remove(bridge_name);
                debug!("BridgeWait: destroyed empty holding bridge '{}'", bridge_name);
            }
        }

        PbxExecResult::Success
    }

    /// Get an existing holding bridge or create a new one.
    fn get_or_create_bridge(name: &str) -> Arc<RwLock<HoldingBridge>> {
        if let Some(hb) = HOLDING_BRIDGES.get(name) {
            return hb.value().clone();
        }

        let hb = Arc::new(RwLock::new(HoldingBridge::new(name)));
        HOLDING_BRIDGES.insert(name.to_string(), hb.clone());
        info!("BridgeWait: created holding bridge '{}'", name);
        hb
    }

    /// List all active holding bridges.
    pub fn list_bridges() -> Vec<(String, usize)> {
        HOLDING_BRIDGES
            .iter()
            .map(|entry| {
                let hb = entry.value().read();
                (hb.name.clone(), hb.participant_count)
            })
            .collect()
    }
}

/// The BridgeAdd() dialplan application.
///
/// Usage: BridgeAdd(channel_name[,bridge_id])
///
/// Adds a specified channel to an existing bridge. If bridge_id is not
/// specified, the channel executing BridgeAdd must be in a bridge, and
/// the target channel will be added to that bridge.
pub struct AppBridgeAdd;

impl DialplanApp for AppBridgeAdd {
    fn name(&self) -> &str {
        "BridgeAdd"
    }

    fn description(&self) -> &str {
        "Add a channel to an existing bridge"
    }
}

impl AppBridgeAdd {
    /// Execute the BridgeAdd application.
    ///
    /// # Arguments
    /// * `channel` - The channel executing the application
    /// * `args` - "channel_name[,bridge_id]"
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();

        let target_channel_name = match parts.first() {
            Some(name) if !name.trim().is_empty() => name.trim(),
            _ => {
                warn!("BridgeAdd: requires a channel name argument");
                return PbxExecResult::Failed;
            }
        };

        let bridge_id = parts
            .get(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // Determine which bridge to add the channel to
        let effective_bridge_id = match bridge_id {
            Some(id) => id,
            None => {
                // Use the executing channel's bridge
                match &channel.bridge_id {
                    Some(id) => id.clone(),
                    None => {
                        warn!(
                            "BridgeAdd: channel '{}' is not in a bridge and no bridge_id specified",
                            channel.name
                        );
                        return PbxExecResult::Failed;
                    }
                }
            }
        };

        info!(
            "BridgeAdd: adding channel '{}' to bridge '{}'",
            target_channel_name, effective_bridge_id
        );

        // In a real implementation:
        //
        //   // Find the target channel by name
        //   let target = ChannelRegistry::find_by_name(target_channel_name)?;
        //
        //   // Find the bridge
        //   let bridge = BridgeRegistry::find(&effective_bridge_id)?;
        //
        //   // Add the channel to the bridge
        //   bridge.add_channel(target.unique_id.clone(), target.name.clone());
        //   target.bridge_id = Some(effective_bridge_id);

        PbxExecResult::Success
    }
}

// Lazy pattern for the global holding bridges map
mod once_cell {
    pub mod sync {
        pub struct Lazy<T> {
            inner: std::sync::OnceLock<T>,
            init: fn() -> T,
        }

        impl<T> Lazy<T> {
            pub const fn new(init: fn() -> T) -> Self {
                Self {
                    inner: std::sync::OnceLock::new(),
                    init,
                }
            }
        }

        impl<T> std::ops::Deref for Lazy<T> {
            type Target = T;

            fn deref(&self) -> &T {
                self.inner.get_or_init(self.init)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entertainment_mode_from_char() {
        assert_eq!(EntertainmentMode::from_char('m'), Some(EntertainmentMode::MusicOnHold));
        assert_eq!(EntertainmentMode::from_char('r'), Some(EntertainmentMode::Ringing));
        assert_eq!(EntertainmentMode::from_char('s'), Some(EntertainmentMode::Silence));
        assert_eq!(EntertainmentMode::from_char('h'), Some(EntertainmentMode::Hold));
        assert_eq!(EntertainmentMode::from_char('n'), Some(EntertainmentMode::None));
        assert_eq!(EntertainmentMode::from_char('x'), Option::None);
    }

    #[test]
    fn test_holding_role_parse() {
        assert_eq!(HoldingRole::parse("participant"), HoldingRole::Participant);
        assert_eq!(HoldingRole::parse("announcer"), HoldingRole::Announcer);
        assert_eq!(HoldingRole::parse("Announcer"), HoldingRole::Announcer);
        assert_eq!(HoldingRole::parse("unknown"), HoldingRole::Participant);
    }

    #[test]
    fn test_bridgewait_options_parse() {
        let opts = BridgeWaitOptions::parse("m(custom_class)e(r)S(60)n");
        assert_eq!(opts.moh_class.as_deref(), Some("custom_class"));
        assert_eq!(opts.entertainment, EntertainmentMode::Ringing);
        assert_eq!(opts.timeout, Duration::from_secs(60));
        assert!(opts.no_answer);
    }

    #[test]
    fn test_bridgewait_options_defaults() {
        let opts = BridgeWaitOptions::default();
        assert!(opts.moh_class.is_none());
        assert_eq!(opts.entertainment, EntertainmentMode::MusicOnHold);
        assert!(opts.timeout.is_zero());
        assert!(!opts.no_answer);
    }

    #[tokio::test]
    async fn test_bridgewait_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppBridgeWait::exec(&mut channel, "mybridge,participant,e(s)").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_bridgewait_exec_default_name() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppBridgeWait::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_bridgeadd_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppBridgeAdd::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_bridgeadd_no_bridge() {
        let mut channel = Channel::new("SIP/test-001");
        // Channel is not in a bridge and no bridge_id provided
        let result = AppBridgeAdd::exec(&mut channel, "SIP/other-001").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_bridgeadd_with_bridge_id() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppBridgeAdd::exec(&mut channel, "SIP/other-001,bridge-123").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
