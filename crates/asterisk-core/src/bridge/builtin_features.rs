//! Built-in bridge features - in-bridge DTMF feature handling.
//!
//! Port of bridge_builtin_features.c from Asterisk C. Provides DTMF-triggered
//! features during bridged calls, including blind transfer, attended transfer,
//! disconnect, call parking, and recording control.

use super::{Bridge, BridgeChannel};
use std::collections::HashMap;
use tracing::{debug, info};

/// A DTMF feature code and its associated action.
#[derive(Debug, Clone)]
pub struct DtmfFeature {
    /// The DTMF sequence that triggers this feature.
    pub code: String,
    /// Human-readable name of the feature.
    pub name: String,
    /// Which side of the bridge can activate this feature.
    pub activate_on: FeatureActivation,
    /// The action to perform.
    pub action: FeatureAction,
}

/// Which side of the bridge can activate a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureActivation {
    /// Only the caller (first channel) can activate.
    Caller,
    /// Only the callee (second channel) can activate.
    Callee,
    /// Either side can activate.
    Both,
}

/// Actions that can be triggered by DTMF features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAction {
    /// Blind (unattended) transfer.
    BlindTransfer,
    /// Attended (consultative) transfer.
    AttendedTransfer,
    /// Disconnect the bridge (hangup).
    Disconnect,
    /// Park the call.
    ParkCall,
    /// Toggle call recording (automixmonitor).
    AutoMixMonitor,
    /// Toggle call recording (automonitor).
    AutoMonitor,
}

impl FeatureAction {
    /// String name for the action.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BlindTransfer => "blindxfer",
            Self::AttendedTransfer => "atxfer",
            Self::Disconnect => "disconnect",
            Self::ParkCall => "parkcall",
            Self::AutoMixMonitor => "automixmon",
            Self::AutoMonitor => "automon",
        }
    }
}

/// Configuration for built-in bridge features.
///
/// Maps DTMF codes to feature actions for caller and callee sides.
#[derive(Debug, Clone)]
pub struct BuiltinFeaturesConfig {
    /// DTMF code for blind transfer (default: "#").
    pub blind_transfer_code: String,
    /// DTMF code for attended transfer (default: "*").
    pub attended_transfer_code: String,
    /// DTMF code for disconnect (default: empty/disabled).
    pub disconnect_code: String,
    /// DTMF code for parking (default: empty/disabled).
    pub park_code: String,
    /// DTMF code for auto-mixmonitor toggle (default: empty/disabled).
    pub automixmon_code: String,
    /// DTMF code for auto-monitor toggle (default: empty/disabled).
    pub automon_code: String,
    /// Maximum number of DTMF digits to buffer before timeout.
    pub feature_digit_timeout_ms: u64,
}

impl Default for BuiltinFeaturesConfig {
    fn default() -> Self {
        Self {
            blind_transfer_code: "#".to_string(),
            attended_transfer_code: "*".to_string(),
            disconnect_code: String::new(),
            park_code: String::new(),
            automixmon_code: String::new(),
            automon_code: String::new(),
            feature_digit_timeout_ms: 1000,
        }
    }
}

/// Result of processing a DTMF digit through the feature engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DtmfFeatureResult {
    /// No feature matched; the digit should be passed through.
    PassThrough,
    /// Feature code is being collected (partial match); buffer the digit.
    Collecting,
    /// A feature was matched and triggered.
    Triggered(FeatureAction),
    /// Feature collection timed out; pass buffered digits through.
    Timeout,
}

/// Built-in DTMF features engine for bridges.
///
/// Handles registration and detection of DTMF feature codes during
/// bridged calls. When a channel presses a configured DTMF sequence,
/// the corresponding feature action is triggered.
#[derive(Debug)]
pub struct BuiltinFeatures {
    /// Feature configuration.
    config: BuiltinFeaturesConfig,
    /// Active features mapped by their DTMF codes.
    features: Vec<DtmfFeature>,
    /// DTMF digit buffer per channel for partial code matching.
    digit_buffers: HashMap<String, String>,
}

impl BuiltinFeatures {
    /// Create a new BuiltinFeatures engine with default configuration.
    pub fn new() -> Self {
        let config = BuiltinFeaturesConfig::default();
        let features = Self::build_feature_list(&config);
        Self {
            config,
            features,
            digit_buffers: HashMap::new(),
        }
    }

    /// Create with a specific configuration.
    pub fn with_config(config: BuiltinFeaturesConfig) -> Self {
        let features = Self::build_feature_list(&config);
        Self {
            config,
            features,
            digit_buffers: HashMap::new(),
        }
    }

    /// Build the list of active features from configuration.
    fn build_feature_list(config: &BuiltinFeaturesConfig) -> Vec<DtmfFeature> {
        let mut features = Vec::new();

        if !config.blind_transfer_code.is_empty() {
            features.push(DtmfFeature {
                code: config.blind_transfer_code.clone(),
                name: "Blind Transfer".to_string(),
                activate_on: FeatureActivation::Both,
                action: FeatureAction::BlindTransfer,
            });
        }

        if !config.attended_transfer_code.is_empty() {
            features.push(DtmfFeature {
                code: config.attended_transfer_code.clone(),
                name: "Attended Transfer".to_string(),
                activate_on: FeatureActivation::Both,
                action: FeatureAction::AttendedTransfer,
            });
        }

        if !config.disconnect_code.is_empty() {
            features.push(DtmfFeature {
                code: config.disconnect_code.clone(),
                name: "Disconnect".to_string(),
                activate_on: FeatureActivation::Both,
                action: FeatureAction::Disconnect,
            });
        }

        if !config.park_code.is_empty() {
            features.push(DtmfFeature {
                code: config.park_code.clone(),
                name: "Park Call".to_string(),
                activate_on: FeatureActivation::Caller,
                action: FeatureAction::ParkCall,
            });
        }

        if !config.automixmon_code.is_empty() {
            features.push(DtmfFeature {
                code: config.automixmon_code.clone(),
                name: "Auto MixMonitor".to_string(),
                activate_on: FeatureActivation::Both,
                action: FeatureAction::AutoMixMonitor,
            });
        }

        if !config.automon_code.is_empty() {
            features.push(DtmfFeature {
                code: config.automon_code.clone(),
                name: "Auto Monitor".to_string(),
                activate_on: FeatureActivation::Both,
                action: FeatureAction::AutoMonitor,
            });
        }

        features
    }

    /// Process a DTMF digit from a bridge channel.
    ///
    /// Returns what should happen with the digit. The caller should:
    /// - PassThrough: Forward the digit normally
    /// - Collecting: Buffer the digit, wait for more
    /// - Triggered: Execute the feature action
    /// - Timeout: Pass all buffered digits through
    pub fn process_dtmf(
        &mut self,
        channel_id: &str,
        digit: char,
        _bridge: &Bridge,
        _bridge_channel: &BridgeChannel,
    ) -> DtmfFeatureResult {
        // Get or create digit buffer for this channel
        let buffer = self
            .digit_buffers
            .entry(channel_id.to_string())
            .or_default();

        buffer.push(digit);

        debug!(
            "BuiltinFeatures: channel {} digit buffer: '{}'",
            channel_id, buffer
        );

        // Check for exact matches
        for feature in &self.features {
            if buffer.as_str() == feature.code {
                info!(
                    "BuiltinFeatures: feature '{}' ({}) triggered by channel {}",
                    feature.name,
                    feature.action.as_str(),
                    channel_id
                );
                self.digit_buffers.remove(channel_id);
                return DtmfFeatureResult::Triggered(feature.action);
            }
        }

        // Check for partial matches (prefix of any feature code)
        let has_partial_match = self
            .features
            .iter()
            .any(|f| f.code.starts_with(buffer.as_str()));

        if has_partial_match {
            debug!("BuiltinFeatures: partial match, collecting more digits");
            DtmfFeatureResult::Collecting
        } else {
            // No match possible; flush the buffer
            debug!("BuiltinFeatures: no match, passing through");
            self.digit_buffers.remove(channel_id);
            DtmfFeatureResult::PassThrough
        }
    }

    /// Check if a channel has a partial DTMF match that should time out.
    ///
    /// This is called by the event loop when a DTMF interdigit timeout expires
    /// (default 500ms). If there are buffered digits that don't form a complete
    /// feature code, they should be flushed and passed through as regular DTMF.
    ///
    /// Returns the buffered digits if timeout should apply, None otherwise.
    pub fn check_timeout(&mut self, channel_id: &str) -> Option<String> {
        let buffer = self.digit_buffers.get(channel_id)?;
        if buffer.is_empty() {
            return None;
        }
        // If there's a partial match, the timeout fires and we flush.
        let has_partial = self
            .features
            .iter()
            .any(|f| f.code.starts_with(buffer.as_str()));
        if has_partial {
            let digits = buffer.clone();
            self.digit_buffers.remove(channel_id);
            debug!(
                "BuiltinFeatures: timeout for channel {}, flushing '{}' as regular DTMF",
                channel_id, digits
            );
            Some(digits)
        } else {
            // No partial match, buffer should have already been cleared.
            self.digit_buffers.remove(channel_id);
            None
        }
    }

    /// Get the current buffered digits for a channel (for testing/debugging).
    pub fn get_buffered_digits(&self, channel_id: &str) -> Option<&str> {
        self.digit_buffers.get(channel_id).map(|s| s.as_str())
    }

    /// Check if a channel has any buffered digits (partial match in progress).
    pub fn has_buffered_digits(&self, channel_id: &str) -> bool {
        self.digit_buffers
            .get(channel_id)
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    /// Clear the digit buffer for a channel (e.g., when it leaves the bridge).
    pub fn clear_buffer(&mut self, channel_id: &str) {
        self.digit_buffers.remove(channel_id);
    }

    /// Handle a triggered feature action.
    ///
    /// In a full implementation, this dispatches to the appropriate handler
    /// (transfer, disconnect, park, etc.).
    pub async fn handle_feature(
        &self,
        action: FeatureAction,
        bridge: &mut Bridge,
        bridge_channel: &BridgeChannel,
    ) {
        match action {
            FeatureAction::BlindTransfer => {
                self.handle_blind_transfer(bridge, bridge_channel).await;
            }
            FeatureAction::AttendedTransfer => {
                self.handle_attended_transfer(bridge, bridge_channel).await;
            }
            FeatureAction::Disconnect => {
                self.handle_disconnect(bridge, bridge_channel).await;
            }
            FeatureAction::ParkCall => {
                self.handle_park_call(bridge, bridge_channel).await;
            }
            FeatureAction::AutoMixMonitor => {
                self.handle_automixmonitor(bridge, bridge_channel).await;
            }
            FeatureAction::AutoMonitor => {
                self.handle_automonitor(bridge, bridge_channel).await;
            }
        }
    }

    /// Handle blind transfer feature.
    ///
    /// In a full implementation:
    /// 1. Break out of the bridge
    /// 2. Collect transfer destination digits from the transferring channel
    /// 3. Initiate a blind transfer to the collected destination
    /// 4. Report result via TRANSFERSTATUS
    async fn handle_blind_transfer(&self, _bridge: &Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: blind transfer initiated by channel '{}'",
            bridge_channel.channel_name
        );
        // Implementation would:
        // 1. Play transfer prompt
        // 2. Collect destination digits
        // 3. Execute blind transfer
        // 4. If successful, remove channel from bridge
        // 5. If failed, return channel to bridge
    }

    /// Handle attended transfer feature.
    ///
    /// In a full implementation:
    /// 1. Put the bridge on hold
    /// 2. Collect transfer destination
    /// 3. Originate call to destination
    /// 4. Bridge the transferring channel with the destination
    /// 5. On confirmation, complete the transfer
    async fn handle_attended_transfer(&self, _bridge: &Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: attended transfer initiated by channel '{}'",
            bridge_channel.channel_name
        );
    }

    /// Handle disconnect feature.
    async fn handle_disconnect(&self, bridge: &mut Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: disconnect requested by channel '{}' in bridge '{}'",
            bridge_channel.channel_name, bridge.name
        );
        // In a full implementation, this would set the bridge to dissolve
        // and cause both channels to leave.
    }

    /// Handle park call feature.
    async fn handle_park_call(&self, _bridge: &Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: park call requested by channel '{}'",
            bridge_channel.channel_name
        );
        // In a full implementation, this would:
        // 1. Find an available parking lot/slot
        // 2. Move the other channel to the parking bridge
        // 3. Announce the parking slot number
    }

    /// Handle auto-mixmonitor toggle.
    async fn handle_automixmonitor(&self, _bridge: &Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: auto mixmonitor toggle by channel '{}'",
            bridge_channel.channel_name
        );
        // In a full implementation, this would start or stop MixMonitor
        // recording on the bridge channels.
    }

    /// Handle auto-monitor toggle.
    async fn handle_automonitor(&self, _bridge: &Bridge, bridge_channel: &BridgeChannel) {
        info!(
            "BuiltinFeatures: auto monitor toggle by channel '{}'",
            bridge_channel.channel_name
        );
    }

    /// Get the current configuration.
    pub fn config(&self) -> &BuiltinFeaturesConfig {
        &self.config
    }

    /// Update the configuration and rebuild feature list.
    pub fn set_config(&mut self, config: BuiltinFeaturesConfig) {
        self.features = Self::build_feature_list(&config);
        self.config = config;
        // Clear all buffers on config change
        self.digit_buffers.clear();
    }

    /// List all currently active features.
    pub fn active_features(&self) -> &[DtmfFeature] {
        &self.features
    }
}

impl Default for BuiltinFeatures {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelId;

    fn make_bridge_channel(name: &str) -> BridgeChannel {
        BridgeChannel::new(ChannelId::from_name(name), name.to_string())
    }

    #[test]
    fn test_default_config() {
        let config = BuiltinFeaturesConfig::default();
        assert_eq!(config.blind_transfer_code, "#");
        assert_eq!(config.attended_transfer_code, "*");
        assert!(config.disconnect_code.is_empty());
    }

    #[test]
    fn test_feature_matching() {
        let mut features = BuiltinFeatures::new();
        let bridge = Bridge::new("test");
        let bc = make_bridge_channel("SIP/alice-001");

        // Single '#' should trigger blind transfer
        let result = features.process_dtmf("SIP/alice-001", '#', &bridge, &bc);
        assert_eq!(
            result,
            DtmfFeatureResult::Triggered(FeatureAction::BlindTransfer)
        );
    }

    #[test]
    fn test_feature_no_match() {
        let mut features = BuiltinFeatures::new();
        let bridge = Bridge::new("test");
        let bc = make_bridge_channel("SIP/alice-001");

        // '5' doesn't match any feature
        let result = features.process_dtmf("SIP/alice-001", '5', &bridge, &bc);
        assert_eq!(result, DtmfFeatureResult::PassThrough);
    }

    #[test]
    fn test_multi_digit_feature() {
        let config = BuiltinFeaturesConfig {
            blind_transfer_code: "##".to_string(),
            ..Default::default()
        };
        let mut features = BuiltinFeatures::with_config(config);
        let bridge = Bridge::new("test");
        let bc = make_bridge_channel("SIP/alice-001");

        // First '#' should be collecting
        let result = features.process_dtmf("SIP/alice-001", '#', &bridge, &bc);
        assert_eq!(result, DtmfFeatureResult::Collecting);

        // Second '#' should trigger
        let result = features.process_dtmf("SIP/alice-001", '#', &bridge, &bc);
        assert_eq!(
            result,
            DtmfFeatureResult::Triggered(FeatureAction::BlindTransfer)
        );
    }

    #[test]
    fn test_clear_buffer() {
        let mut features = BuiltinFeatures::new();
        let bridge = Bridge::new("test");
        let bc = make_bridge_channel("SIP/alice-001");

        // Set up multi-digit code
        let config = BuiltinFeaturesConfig {
            blind_transfer_code: "##".to_string(),
            ..Default::default()
        };
        features.set_config(config);

        // Start collecting
        features.process_dtmf("SIP/alice-001", '#', &bridge, &bc);

        // Clear the buffer
        features.clear_buffer("SIP/alice-001");

        // Next '#' should start fresh (collecting again, not triggered)
        let result = features.process_dtmf("SIP/alice-001", '#', &bridge, &bc);
        assert_eq!(result, DtmfFeatureResult::Collecting);
    }

    #[test]
    fn test_feature_action_names() {
        assert_eq!(FeatureAction::BlindTransfer.as_str(), "blindxfer");
        assert_eq!(FeatureAction::AttendedTransfer.as_str(), "atxfer");
        assert_eq!(FeatureAction::Disconnect.as_str(), "disconnect");
        assert_eq!(FeatureAction::ParkCall.as_str(), "parkcall");
    }

    #[test]
    fn test_disabled_features() {
        let config = BuiltinFeaturesConfig {
            blind_transfer_code: String::new(),
            attended_transfer_code: String::new(),
            disconnect_code: String::new(),
            park_code: String::new(),
            automixmon_code: String::new(),
            automon_code: String::new(),
            feature_digit_timeout_ms: 1000,
        };
        let features = BuiltinFeatures::with_config(config);
        assert!(features.active_features().is_empty());
    }
}
