//! Basic bridge -- the standard two-party bridge used by Dial().
//!
//! Port of bridge_basic.c from Asterisk C. This is the most common bridge
//! type, used for standard two-party calls. It uses SimpleBridge technology
//! underneath and adds DTMF feature detection (transfer, disconnect, park)
//! and bridge personality (caller/callee feature sets).
//!
//! Key concepts from C:
//! - NORMAL_FLAGS = DISSOLVE_HANGUP | DISSOLVE_EMPTY | SMART
//! - BasicBridgePersonality holds per-side feature configurations
//! - Connected line exchange happens on bridge join
//! - After-bridge callbacks (GoTo, run app) execute when bridge ends

use super::builtin_features::{BuiltinFeatures, BuiltinFeaturesConfig};
use super::{Bridge, BridgeSnapshot};
use asterisk_types::BridgeFlags;
use tracing::{debug, info};

/// Normal bridge flags: dissolve on hangup, dissolve when empty, smart technology selection.
pub const NORMAL_FLAGS: BridgeFlags = BridgeFlags::from_bits_truncate(
    BridgeFlags::DISSOLVE_HANGUP.bits()
        | BridgeFlags::DISSOLVE_EMPTY.bits()
        | BridgeFlags::SMART.bits(),
);

/// Transfer bridge flags: smart technology selection only.
pub const TRANSFER_FLAGS: BridgeFlags = BridgeFlags::SMART;

/// Personality type for the basic bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicBridgePersonalityType {
    /// Normal two-party call personality.
    Normal,
    /// Attended transfer personality.
    AttendedTransfer,
}

/// Per-side feature configuration for the basic bridge.
///
/// Each side (caller/callee) can have different DTMF feature codes enabled.
#[derive(Debug, Clone)]
pub struct SideFeatures {
    /// Whether blind transfer is enabled for this side.
    pub blind_transfer: bool,
    /// Whether attended transfer is enabled for this side.
    pub attended_transfer: bool,
    /// Whether disconnect is enabled for this side.
    pub disconnect: bool,
    /// Whether parking is enabled for this side.
    pub park_call: bool,
    /// Whether auto mixmonitor toggle is enabled.
    pub automixmon: bool,
}

impl Default for SideFeatures {
    fn default() -> Self {
        Self {
            blind_transfer: true,
            attended_transfer: true,
            disconnect: false,
            park_call: false,
            automixmon: false,
        }
    }
}

/// Personality for the basic bridge, controlling feature availability.
///
/// Port of the `bridge_basic_personality` concept from C. The personality
/// determines which features are available to which side of the bridge.
#[derive(Debug, Clone)]
pub struct BasicBridgePersonality {
    /// Current personality type.
    pub personality_type: BasicBridgePersonalityType,
    /// Features available to the caller (first channel).
    pub caller_features: SideFeatures,
    /// Features available to the callee (second channel).
    pub callee_features: SideFeatures,
}

impl Default for BasicBridgePersonality {
    fn default() -> Self {
        Self {
            personality_type: BasicBridgePersonalityType::Normal,
            caller_features: SideFeatures::default(),
            callee_features: SideFeatures::default(),
        }
    }
}

impl BasicBridgePersonality {
    /// Create a normal personality with default features.
    pub fn normal() -> Self {
        Self::default()
    }

    /// Create an attended-transfer personality.
    pub fn attended_transfer() -> Self {
        Self {
            personality_type: BasicBridgePersonalityType::AttendedTransfer,
            // During attended transfer, limited features.
            caller_features: SideFeatures {
                blind_transfer: false,
                attended_transfer: false,
                disconnect: true,
                park_call: false,
                automixmon: false,
            },
            callee_features: SideFeatures {
                blind_transfer: false,
                attended_transfer: false,
                disconnect: true,
                park_call: false,
                automixmon: false,
            },
        }
    }

    /// Change the personality type and reset features accordingly.
    pub fn change_personality(&mut self, new_type: BasicBridgePersonalityType) {
        match new_type {
            BasicBridgePersonalityType::Normal => {
                *self = Self::normal();
            }
            BasicBridgePersonalityType::AttendedTransfer => {
                *self = Self::attended_transfer();
            }
        }
        debug!(personality = ?new_type, "Basic bridge personality changed");
    }
}

/// After-bridge action: what to do when a channel leaves the bridge.
#[derive(Debug, Clone)]
#[derive(Default)]
pub enum AfterBridgeAction {
    /// No action -- channel returns to dialplan.
    #[default]
    None,
    /// GoTo a specific dialplan location after bridge.
    GoTo {
        context: String,
        exten: String,
        priority: i32,
    },
    /// Run an application after bridge.
    RunApp {
        app_name: String,
        app_args: String,
    },
}


/// A basic bridge -- the standard two-party bridge used by Dial().
///
/// This wraps a Bridge with additional state for DTMF features,
/// personality, and after-bridge actions.
#[derive(Debug)]
pub struct BasicBridge {
    /// The underlying bridge.
    pub bridge: Bridge,
    /// Bridge personality controlling features.
    pub personality: BasicBridgePersonality,
    /// DTMF features engine.
    pub features: BuiltinFeatures,
    /// After-bridge action for the caller.
    pub caller_after_action: AfterBridgeAction,
    /// After-bridge action for the callee.
    pub callee_after_action: AfterBridgeAction,
}

impl BasicBridge {
    /// Create a new basic bridge for a standard two-party call.
    ///
    /// This is the Rust equivalent of `ast_bridge_basic_new()`.
    pub fn new() -> Self {
        let bridge = Bridge::with_flags("basic", NORMAL_FLAGS);
        Self {
            bridge,
            personality: BasicBridgePersonality::normal(),
            features: BuiltinFeatures::new(),
            caller_after_action: AfterBridgeAction::None,
            callee_after_action: AfterBridgeAction::None,
        }
    }

    /// Create with a specific name.
    pub fn with_name(name: impl Into<String>) -> Self {
        let bridge = Bridge::with_flags(name, NORMAL_FLAGS);
        Self {
            bridge,
            personality: BasicBridgePersonality::normal(),
            features: BuiltinFeatures::new(),
            caller_after_action: AfterBridgeAction::None,
            callee_after_action: AfterBridgeAction::None,
        }
    }

    /// Create with specific features configuration.
    pub fn with_features(name: impl Into<String>, config: BuiltinFeaturesConfig) -> Self {
        let bridge = Bridge::with_flags(name, NORMAL_FLAGS);
        Self {
            bridge,
            personality: BasicBridgePersonality::normal(),
            features: BuiltinFeatures::with_config(config),
            caller_after_action: AfterBridgeAction::None,
            callee_after_action: AfterBridgeAction::None,
        }
    }

    /// Get the bridge's unique ID.
    pub fn unique_id(&self) -> &str {
        &self.bridge.unique_id
    }

    /// Set the after-bridge action for the caller.
    pub fn set_caller_after_action(&mut self, action: AfterBridgeAction) {
        self.caller_after_action = action;
    }

    /// Set the after-bridge action for the callee.
    pub fn set_callee_after_action(&mut self, action: AfterBridgeAction) {
        self.callee_after_action = action;
    }

    /// Change the bridge personality.
    pub fn change_personality(&mut self, personality_type: BasicBridgePersonalityType) {
        self.personality.change_personality(personality_type);

        // Update bridge flags based on personality.
        match personality_type {
            BasicBridgePersonalityType::Normal => {
                self.bridge.flags = NORMAL_FLAGS;
            }
            BasicBridgePersonalityType::AttendedTransfer => {
                self.bridge.flags = TRANSFER_FLAGS;
            }
        }
    }

    /// Perform connected line exchange when both channels are in the bridge.
    ///
    /// In Asterisk C, this sends each channel's caller ID to the other
    /// channel's connected line. This is important for display on phones.
    pub fn exchange_connected_line(&self) {
        if self.bridge.num_channels() == 2 {
            let chan0 = &self.bridge.channels[0];
            let chan1 = &self.bridge.channels[1];
            info!(
                "BasicBridge: connected line exchange between '{}' and '{}'",
                chan0.channel_name, chan1.channel_name
            );
            // In a full implementation, we would:
            // 1. Get caller ID from chan0, send as connected line to chan1
            // 2. Get caller ID from chan1, send as connected line to chan0
            // This updates the phone displays for both parties.
        }
    }

    /// Get a snapshot of the underlying bridge.
    pub fn snapshot(&self) -> BridgeSnapshot {
        self.bridge.snapshot()
    }
}

impl Default for BasicBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a new standard two-party bridge, as called by Dial() and similar.
///
/// Convenience function matching `ast_bridge_basic_new()` from C.
pub fn ast_bridge_basic_new() -> BasicBridge {
    BasicBridge::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelId;

    #[test]
    fn test_basic_bridge_new() {
        let bb = BasicBridge::new();
        assert_eq!(bb.bridge.name, "basic");
        assert_eq!(bb.bridge.flags, NORMAL_FLAGS);
        assert_eq!(
            bb.personality.personality_type,
            BasicBridgePersonalityType::Normal
        );
    }

    #[test]
    fn test_basic_bridge_with_name() {
        let bb = BasicBridge::with_name("my-call");
        assert_eq!(bb.bridge.name, "my-call");
    }

    #[test]
    fn test_normal_flags() {
        assert!(NORMAL_FLAGS.contains(BridgeFlags::DISSOLVE_HANGUP));
        assert!(NORMAL_FLAGS.contains(BridgeFlags::DISSOLVE_EMPTY));
        assert!(NORMAL_FLAGS.contains(BridgeFlags::SMART));
    }

    #[test]
    fn test_change_personality() {
        let mut bb = BasicBridge::new();
        bb.change_personality(BasicBridgePersonalityType::AttendedTransfer);
        assert_eq!(
            bb.personality.personality_type,
            BasicBridgePersonalityType::AttendedTransfer
        );
        assert_eq!(bb.bridge.flags, TRANSFER_FLAGS);

        bb.change_personality(BasicBridgePersonalityType::Normal);
        assert_eq!(
            bb.personality.personality_type,
            BasicBridgePersonalityType::Normal
        );
        assert_eq!(bb.bridge.flags, NORMAL_FLAGS);
    }

    #[test]
    fn test_after_bridge_action() {
        let mut bb = BasicBridge::new();
        bb.set_caller_after_action(AfterBridgeAction::GoTo {
            context: "default".to_string(),
            exten: "s".to_string(),
            priority: 1,
        });
        assert!(matches!(
            bb.caller_after_action,
            AfterBridgeAction::GoTo { .. }
        ));
    }

    #[test]
    fn test_connected_line_exchange() {
        let mut bb = BasicBridge::new();
        bb.bridge.add_channel(
            ChannelId::from_name("chan1"),
            "SIP/alice-001".to_string(),
        );
        bb.bridge.add_channel(
            ChannelId::from_name("chan2"),
            "SIP/bob-001".to_string(),
        );
        // Should not panic; connected line exchange is a no-op for now.
        bb.exchange_connected_line();
    }

    #[test]
    fn test_ast_bridge_basic_new() {
        let bb = ast_bridge_basic_new();
        assert_eq!(bb.bridge.name, "basic");
    }

    #[test]
    fn test_side_features_default() {
        let sf = SideFeatures::default();
        assert!(sf.blind_transfer);
        assert!(sf.attended_transfer);
        assert!(!sf.disconnect);
        assert!(!sf.park_call);
    }

    #[test]
    fn test_attended_transfer_personality() {
        let p = BasicBridgePersonality::attended_transfer();
        assert_eq!(
            p.personality_type,
            BasicBridgePersonalityType::AttendedTransfer
        );
        assert!(!p.caller_features.blind_transfer);
        assert!(p.caller_features.disconnect);
    }
}
