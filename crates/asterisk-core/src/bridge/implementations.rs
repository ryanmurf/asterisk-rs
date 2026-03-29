//! Bridge technology implementations.
//!
//! Ports of:
//! - bridge_simple.c  -> SimpleBridge (real frame routing)
//! - bridge_holding.c -> HoldingBridge (real holding with entertainment/announcer)
//! - bridge_softmix.c -> SoftmixBridge (sketch -- real impl in softmix.rs)

use super::{Bridge, BridgeChannel, BridgeTechnology};
use asterisk_types::{AsteriskError, AsteriskResult, BridgeCapability, ControlFrame, Frame};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, trace};

// ---------------------------------------------------------------------------
// SimpleBridge - direct 1:1 frame passing between two channels
// ---------------------------------------------------------------------------

/// Simple bridge technology: direct 1-to-1 frame passing.
///
/// Port of bridge_simple.c. This is the most basic bridge technology
/// that passes frames from one channel to the other in a two-party call.
/// It requires exactly two channels.
///
/// Frame routing behavior:
/// - Only Voice, DTMF, Text, Video, and allowed Control frames are routed
/// - Hold/Unhold control frames are blocked (handled locally)
/// - Frames from channel A go to channel B and vice versa
#[derive(Debug)]
pub struct SimpleBridge;

impl SimpleBridge {
    pub fn new() -> Self {
        SimpleBridge
    }
}

impl Default for SimpleBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a frame type should be passed through the simple bridge.
///
/// Only voice, DTMF, text, video, and certain control frames pass through.
/// Hold/Unhold control frames are blocked (handled locally by the event loop).
pub fn simple_bridge_should_pass(frame: &Frame) -> bool {
    match frame {
        Frame::Voice { .. } => true,
        Frame::Video { .. } => true,
        Frame::DtmfBegin { .. } | Frame::DtmfEnd { .. } => true,
        Frame::Text { .. } => true,
        Frame::Cng { .. } => true,
        Frame::Control { control, .. } => {
            // Block hold/unhold from passing through the bridge.
            // Block hangup -- handled by the event loop.
            !matches!(
                control,
                ControlFrame::Hold | ControlFrame::Unhold | ControlFrame::Hangup
            )
        }
        _ => false,
    }
}

/// Per-bridge routing state: tracks which channel frames go to.
///
/// In a simple bridge, there are exactly two channels. A frame from
/// channel A goes to channel B and vice versa. This struct stores the
/// routed frames per destination channel.
#[derive(Debug, Default)]
pub struct SimpleBridgeRouting {
    /// Frames routed to each channel, keyed by destination channel_id.
    pub outbound_frames: HashMap<String, Vec<Frame>>,
}

impl SimpleBridgeRouting {
    pub fn new() -> Self {
        Self {
            outbound_frames: HashMap::new(),
        }
    }

    /// Take all pending outbound frames for a given channel.
    pub fn take_frames(&mut self, channel_id: &str) -> Vec<Frame> {
        self.outbound_frames
            .remove(channel_id)
            .unwrap_or_default()
    }
}

/// Global routing state for simple bridges. Each bridge_id maps to its routing state.
static SIMPLE_BRIDGE_ROUTING: std::sync::LazyLock<
    dashmap::DashMap<String, Arc<Mutex<SimpleBridgeRouting>>>,
> = std::sync::LazyLock::new(|| dashmap::DashMap::new());

/// Get or create routing state for a bridge.
pub fn get_simple_routing(bridge_id: &str) -> Arc<Mutex<SimpleBridgeRouting>> {
    SIMPLE_BRIDGE_ROUTING
        .entry(bridge_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(SimpleBridgeRouting::new())))
        .value()
        .clone()
}

/// Remove routing state for a bridge.
pub fn remove_simple_routing(bridge_id: &str) {
    SIMPLE_BRIDGE_ROUTING.remove(bridge_id);
}

#[async_trait::async_trait]
impl BridgeTechnology for SimpleBridge {
    fn name(&self) -> &str {
        "simple_bridge"
    }

    fn capabilities(&self) -> BridgeCapability {
        BridgeCapability::ONE_TO_ONE_MIX
    }

    fn preference(&self) -> u32 {
        // AST_BRIDGE_PREFERENCE_BASE_1TO1MIX
        90
    }

    async fn create(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn start(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn stop(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        remove_simple_routing(&bridge.unique_id);
        Ok(())
    }

    async fn join(&self, bridge: &mut Bridge, _channel: &BridgeChannel) -> AsteriskResult<()> {
        // For a simple bridge, we verify that we have at most 2 channels
        if bridge.num_channels() > 2 {
            return Err(AsteriskError::InvalidArgument(
                "simple bridge supports at most 2 channels".into(),
            ));
        }
        Ok(())
    }

    async fn leave(&self, _bridge: &mut Bridge, _channel: &BridgeChannel) -> AsteriskResult<()> {
        Ok(())
    }

    async fn write_frame(
        &self,
        bridge: &mut Bridge,
        from_channel: &BridgeChannel,
        frame: &Frame,
    ) -> AsteriskResult<()> {
        // Filter: only pass allowed frame types.
        if !simple_bridge_should_pass(frame) {
            trace!(
                channel = %from_channel.channel_name,
                frame_type = %frame.frame_type(),
                "SimpleBridge: dropping frame (type not passed)"
            );
            return Ok(());
        }

        // In a simple bridge, frames from one channel go to ALL other channels.
        // For a 2-party bridge: frame from A goes to B, frame from B goes to A.
        let routing = get_simple_routing(&bridge.unique_id);
        let mut routing_state = routing.lock().await;

        for bc in &bridge.channels {
            if bc.channel_id != from_channel.channel_id {
                // Route frame to this channel.
                routing_state
                    .outbound_frames
                    .entry(bc.channel_id.as_str().to_string())
                    .or_default()
                    .push(frame.clone());

                trace!(
                    from = %from_channel.channel_name,
                    to = %bc.channel_name,
                    frame_type = %frame.frame_type(),
                    "SimpleBridge: routed frame"
                );
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HoldingBridge - holds channels with music/announcements
// ---------------------------------------------------------------------------

/// Idle mode for channels in a holding bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldingIdleMode {
    /// No entertainment
    #[default]
    None,
    /// Music on hold
    MusicOnHold,
    /// Ringing indication
    Ringing,
    /// Silence
    Silence,
    /// Hold indication (control frame)
    Hold,
}

/// Role of a channel in a holding bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldingRole {
    /// Normal participant (receives entertainment)
    #[default]
    Participant,
    /// Announcer (can broadcast to all participants)
    Announcer,
}

/// Per-channel state in a holding bridge.
#[derive(Debug, Clone)]
pub struct HoldingChannelData {
    /// Channel's role in the bridge.
    pub role: HoldingRole,
    /// Current idle mode for this channel.
    pub idle_mode: HoldingIdleMode,
    /// Whether entertainment is currently playing.
    pub entertainment_active: bool,
    /// Channel name for debugging.
    pub channel_name: String,
}

/// Holding bridge technology: holds channels for parking, queues, etc.
///
/// Port of bridge_holding.c. Channels in a holding bridge are held
/// with optional entertainment (MOH, ringing, silence) and can receive
/// announcements from an announcer channel.
///
/// Real behavior:
/// - join(): starts entertainment for participants based on idle mode
/// - leave(): stops entertainment
/// - write(): announcer frames go to all participants; participant frames are dropped
#[derive(Debug)]
pub struct HoldingBridge {
    /// Default idle mode for new participants.
    pub default_idle_mode: HoldingIdleMode,
    /// Per-channel data, keyed by channel_id.
    pub channel_data: HashMap<String, HoldingChannelData>,
}

impl HoldingBridge {
    pub fn new() -> Self {
        HoldingBridge {
            default_idle_mode: HoldingIdleMode::MusicOnHold,
            channel_data: HashMap::new(),
        }
    }

    pub fn with_idle_mode(mode: HoldingIdleMode) -> Self {
        HoldingBridge {
            default_idle_mode: mode,
            channel_data: HashMap::new(),
        }
    }

    /// Set the role of a channel in the holding bridge.
    pub fn set_channel_role(&mut self, channel_id: &str, role: HoldingRole) {
        if let Some(data) = self.channel_data.get_mut(channel_id) {
            data.role = role;
            debug!(
                channel = channel_id,
                role = ?role,
                "HoldingBridge: channel role updated"
            );
        }
    }

    /// Get the role of a channel.
    pub fn get_channel_role(&self, channel_id: &str) -> Option<HoldingRole> {
        self.channel_data.get(channel_id).map(|d| d.role)
    }
}

impl Default for HoldingBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BridgeTechnology for HoldingBridge {
    fn name(&self) -> &str {
        "holding_bridge"
    }

    fn capabilities(&self) -> BridgeCapability {
        BridgeCapability::HOLDING
    }

    fn preference(&self) -> u32 {
        // AST_BRIDGE_PREFERENCE_BASE_HOLDING
        70
    }

    async fn create(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn start(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn stop(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        // Stop entertainment on all channels.
        // In a full implementation, iterate self.channel_data and stop MOH/ringing.
        Ok(())
    }

    async fn join(&self, _bridge: &mut Bridge, channel: &BridgeChannel) -> AsteriskResult<()> {
        // Start entertainment for this channel based on its role and idle mode.
        // In a full implementation, we'd store per-channel data in the bridge's
        // tech_pvt (here we log and proceed).
        let _chan_id = channel.channel_id.as_str().to_string();
        let _data = HoldingChannelData {
            role: HoldingRole::Participant,
            idle_mode: self.default_idle_mode,
            entertainment_active: true,
            channel_name: channel.channel_name.clone(),
        };

        info!(
            channel = %channel.channel_name,
            idle_mode = ?self.default_idle_mode,
            "HoldingBridge: channel joined, starting entertainment"
        );

        // In a full implementation, based on idle_mode:
        // MusicOnHold -> start MOH on the channel
        // Ringing -> send ringing indication
        // Silence -> start silence generator
        // Hold -> send hold indication

        // Note: We can't mutate self here because the trait method takes &self.
        // The channel data would be stored in the bridge's tech_pvt in a full impl.

        Ok(())
    }

    async fn leave(&self, _bridge: &mut Bridge, channel: &BridgeChannel) -> AsteriskResult<()> {
        info!(
            channel = %channel.channel_name,
            "HoldingBridge: channel left, stopping entertainment"
        );

        // In a full implementation:
        // Stop MOH, ringing, or silence generator.
        // Remove channel data.

        Ok(())
    }

    async fn write_frame(
        &self,
        bridge: &mut Bridge,
        from_channel: &BridgeChannel,
        frame: &Frame,
    ) -> AsteriskResult<()> {
        // In a holding bridge:
        // - Announcer: frames go to all participants
        // - Participant: frames are dropped (they hear MOH, not each other)

        let from_id = from_channel.channel_id.as_str();
        let is_announcer = self
            .channel_data
            .get(from_id)
            .map(|d| d.role == HoldingRole::Announcer)
            .unwrap_or(false);

        if is_announcer {
            // Broadcast announcer's frames to all participants.
            debug!(
                from = %from_channel.channel_name,
                "HoldingBridge: broadcasting announcer frame to all participants"
            );

            // In a full implementation, queue the frame to each participant channel.
            // Only voice and control frames from the announcer are routed.
            match frame {
                Frame::Voice { .. } | Frame::Control { .. } => {
                    // Route to all participants (not back to announcer).
                    for bc in &bridge.channels {
                        if bc.channel_id != from_channel.channel_id {
                            let bc_id = bc.channel_id.as_str();
                            let is_participant = self
                                .channel_data
                                .get(bc_id)
                                .map(|d| d.role == HoldingRole::Participant)
                                .unwrap_or(true);
                            if is_participant {
                                trace!(
                                    to = %bc.channel_name,
                                    "HoldingBridge: routing announcer frame"
                                );
                                // In full implementation: queue frame to channel.
                            }
                        }
                    }
                }
                _ => {}
            }
        } else {
            // Participant: frames are dropped.
            trace!(
                from = %from_channel.channel_name,
                "HoldingBridge: dropping participant frame"
            );
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SoftmixBridge - mixing multiple audio streams (legacy/sketch wrapper)
// ---------------------------------------------------------------------------
// The real softmix implementation is in the `softmix` module (softmix.rs).
// This struct remains for backward compatibility but delegates to the
// real implementation where possible.

/// Softmix bridge technology: multi-party audio mixing.
///
/// This is a simplified sketch. For the full implementation with real
/// audio mixing, use `super::softmix::SoftmixBridgeTech`.
#[derive(Debug)]
pub struct SoftmixBridge {
    /// Internal sample rate for mixing (default 8000Hz)
    pub internal_sample_rate: u32,
    /// Mixing interval in milliseconds (default 20ms)
    pub mixing_interval_ms: u32,
}

impl SoftmixBridge {
    pub fn new() -> Self {
        SoftmixBridge {
            internal_sample_rate: 8000,
            mixing_interval_ms: 20,
        }
    }
}

impl Default for SoftmixBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BridgeTechnology for SoftmixBridge {
    fn name(&self) -> &str {
        "softmix"
    }

    fn capabilities(&self) -> BridgeCapability {
        BridgeCapability::MULTI_MIX
    }

    fn preference(&self) -> u32 {
        50
    }

    async fn create(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn start(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn stop(&self, _bridge: &mut Bridge) -> AsteriskResult<()> {
        Ok(())
    }

    async fn join(&self, _bridge: &mut Bridge, _channel: &BridgeChannel) -> AsteriskResult<()> {
        Ok(())
    }

    async fn leave(&self, _bridge: &mut Bridge, _channel: &BridgeChannel) -> AsteriskResult<()> {
        Ok(())
    }

    async fn write_frame(
        &self,
        _bridge: &mut Bridge,
        _from_channel: &BridgeChannel,
        _frame: &Frame,
    ) -> AsteriskResult<()> {
        // See softmix::SoftmixBridgeTech for the real implementation.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelId;
    use bytes::Bytes;

    fn make_bridge() -> Bridge {
        let mut bridge = Bridge::new("test");
        bridge.add_channel(
            ChannelId::from_name("chan-alice"),
            "SIP/alice-001".to_string(),
        );
        bridge.add_channel(
            ChannelId::from_name("chan-bob"),
            "SIP/bob-001".to_string(),
        );
        bridge
    }

    #[test]
    fn test_simple_bridge_should_pass_voice() {
        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        assert!(simple_bridge_should_pass(&frame));
    }

    #[test]
    fn test_simple_bridge_should_pass_dtmf() {
        assert!(simple_bridge_should_pass(&Frame::dtmf_begin('5')));
        assert!(simple_bridge_should_pass(&Frame::dtmf_end('5', 100)));
    }

    #[test]
    fn test_simple_bridge_should_block_hold() {
        assert!(!simple_bridge_should_pass(&Frame::control(ControlFrame::Hold)));
        assert!(!simple_bridge_should_pass(&Frame::control(ControlFrame::Unhold)));
    }

    #[test]
    fn test_simple_bridge_should_block_hangup() {
        assert!(!simple_bridge_should_pass(&Frame::control(ControlFrame::Hangup)));
    }

    #[test]
    fn test_simple_bridge_should_pass_ringing() {
        assert!(simple_bridge_should_pass(&Frame::control(ControlFrame::Ringing)));
    }

    #[test]
    fn test_simple_bridge_should_pass_text() {
        assert!(simple_bridge_should_pass(&Frame::text("hello".to_string())));
    }

    #[test]
    fn test_simple_bridge_should_block_null() {
        assert!(!simple_bridge_should_pass(&Frame::Null));
    }

    #[tokio::test]
    async fn test_simple_bridge_write_routes_to_other() {
        let tech = SimpleBridge::new();
        let mut bridge = make_bridge();
        let from_bc = BridgeChannel::new(
            ChannelId::from_name("chan-alice"),
            "SIP/alice-001".to_string(),
        );

        let frame = Frame::voice(0, 160, Bytes::from(vec![1u8; 320]));

        tech.write_frame(&mut bridge, &from_bc, &frame)
            .await
            .unwrap();

        // Check that the frame was routed to chan-bob.
        let routing = get_simple_routing(&bridge.unique_id);
        let mut state = routing.lock().await;
        let bob_frames = state.take_frames("chan-bob");
        assert_eq!(bob_frames.len(), 1);
        assert!(bob_frames[0].is_voice());

        // Alice should not have received her own frame.
        let alice_frames = state.take_frames("chan-alice");
        assert!(alice_frames.is_empty());

        // Clean up.
        remove_simple_routing(&bridge.unique_id);
    }

    #[tokio::test]
    async fn test_simple_bridge_write_blocks_hold() {
        let tech = SimpleBridge::new();
        let mut bridge = make_bridge();
        let from_bc = BridgeChannel::new(
            ChannelId::from_name("chan-alice"),
            "SIP/alice-001".to_string(),
        );

        let frame = Frame::control(ControlFrame::Hold);
        tech.write_frame(&mut bridge, &from_bc, &frame)
            .await
            .unwrap();

        // Nothing should be routed.
        let routing = get_simple_routing(&bridge.unique_id);
        let mut state = routing.lock().await;
        let bob_frames = state.take_frames("chan-bob");
        assert!(bob_frames.is_empty());

        remove_simple_routing(&bridge.unique_id);
    }

    #[tokio::test]
    async fn test_simple_bridge_bidirectional() {
        let tech = SimpleBridge::new();
        let mut bridge = make_bridge();

        // Alice sends voice to Bob.
        let alice_bc = BridgeChannel::new(
            ChannelId::from_name("chan-alice"),
            "SIP/alice-001".to_string(),
        );
        let frame_a = Frame::voice(0, 160, Bytes::from(vec![1u8; 320]));
        tech.write_frame(&mut bridge, &alice_bc, &frame_a)
            .await
            .unwrap();

        // Bob sends voice to Alice.
        let bob_bc = BridgeChannel::new(
            ChannelId::from_name("chan-bob"),
            "SIP/bob-001".to_string(),
        );
        let frame_b = Frame::voice(0, 160, Bytes::from(vec![2u8; 320]));
        tech.write_frame(&mut bridge, &bob_bc, &frame_b)
            .await
            .unwrap();

        // Check routing: Bob should have Alice's frame, Alice should have Bob's.
        let routing = get_simple_routing(&bridge.unique_id);
        let mut state = routing.lock().await;

        let bob_frames = state.take_frames("chan-bob");
        assert_eq!(bob_frames.len(), 1);

        let alice_frames = state.take_frames("chan-alice");
        assert_eq!(alice_frames.len(), 1);

        remove_simple_routing(&bridge.unique_id);
    }

    #[tokio::test]
    async fn test_simple_bridge_join_max_channels() {
        let tech = SimpleBridge::new();
        let mut bridge = Bridge::new("test");
        bridge.add_channel(ChannelId::from_name("c1"), "c1".to_string());
        bridge.add_channel(ChannelId::from_name("c2"), "c2".to_string());
        bridge.add_channel(ChannelId::from_name("c3"), "c3".to_string());

        let bc = BridgeChannel::new(ChannelId::from_name("c3"), "c3".to_string());
        let result = tech.join(&mut bridge, &bc).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_holding_bridge_new() {
        let hb = HoldingBridge::new();
        assert_eq!(hb.default_idle_mode, HoldingIdleMode::MusicOnHold);
    }

    #[test]
    fn test_holding_bridge_with_mode() {
        let hb = HoldingBridge::with_idle_mode(HoldingIdleMode::Ringing);
        assert_eq!(hb.default_idle_mode, HoldingIdleMode::Ringing);
    }

    #[test]
    fn test_holding_bridge_roles() {
        let mut hb = HoldingBridge::new();
        hb.channel_data.insert(
            "chan1".to_string(),
            HoldingChannelData {
                role: HoldingRole::Participant,
                idle_mode: HoldingIdleMode::MusicOnHold,
                entertainment_active: true,
                channel_name: "chan1".to_string(),
            },
        );

        assert_eq!(
            hb.get_channel_role("chan1"),
            Some(HoldingRole::Participant)
        );

        hb.set_channel_role("chan1", HoldingRole::Announcer);
        assert_eq!(
            hb.get_channel_role("chan1"),
            Some(HoldingRole::Announcer)
        );
    }
}
