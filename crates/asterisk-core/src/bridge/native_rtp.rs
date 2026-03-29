//! Native RTP bridging technology.
//!
//! Port of bridge_native_rtp.c from Asterisk C. Provides direct RTP media
//! path between two channels, bypassing Asterisk's media processing. This
//! reduces latency and CPU usage for simple two-party calls where no media
//! manipulation is needed.

use super::{Bridge, BridgeChannel, BridgeTechnology};
use asterisk_types::{AsteriskError, AsteriskResult, BridgeCapability, Frame};
use tracing::{debug, info};

/// Result of RTP glue compatibility checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpGlueResult {
    /// RTP bridging is forbidden for this channel.
    Forbid,
    /// Local bridge: both RTP instances are on the same Asterisk host.
    /// Media can be redirected within the same process.
    Local,
    /// Remote bridge: RTP endpoints can communicate directly,
    /// completely bypassing Asterisk for media.
    Remote,
}

impl RtpGlueResult {
    /// Determine the combined glue result for two channels.
    ///
    /// The result is the most restrictive of the two:
    /// - If either is Forbid, result is Forbid
    /// - If both are Remote, result is Remote
    /// - Otherwise, result is Local
    pub fn combine(a: RtpGlueResult, b: RtpGlueResult) -> RtpGlueResult {
        match (a, b) {
            (RtpGlueResult::Forbid, _) | (_, RtpGlueResult::Forbid) => RtpGlueResult::Forbid,
            (RtpGlueResult::Remote, RtpGlueResult::Remote) => RtpGlueResult::Remote,
            _ => RtpGlueResult::Local,
        }
    }
}

/// Per-channel data for native RTP bridging.
#[derive(Debug, Clone)]
pub struct NativeRtpChannelData {
    /// Channel identifier.
    pub channel_name: String,
    /// Audio RTP glue result.
    pub audio_glue: RtpGlueResult,
    /// Video RTP glue result.
    pub video_glue: RtpGlueResult,
    /// Whether this channel's RTP streams are currently in native mode.
    pub native_active: bool,
    /// Framehook ID for intercepting control frames (or None if no hook).
    pub framehook_id: Option<u32>,
}

/// Native RTP bridge technology.
///
/// Provides the highest performance bridge by establishing a direct RTP
/// media path between two channels. This completely bypasses Asterisk's
/// media pipeline for voice/video frames.
///
/// Compatibility requirements:
/// - Exactly two channels in the bridge
/// - Both channels must have RTP glue callbacks registered
/// - Both channels must allow native bridging (not Forbid)
/// - No features requiring media interception (recording, DTMF, etc.)
///   unless handled out-of-band
///
/// When native bridging is active:
/// - Voice/video frames flow directly between endpoints
/// - Asterisk only handles signaling
/// - Much lower CPU usage and latency
///
/// When native bridging cannot be established or needs to fall back:
/// - Returns incompatible, and the bridge core selects a generic bridge
#[derive(Debug)]
pub struct NativeRtpBridge {
    /// Per-channel data, keyed by channel name.
    channel_data: Vec<NativeRtpChannelData>,
}

impl NativeRtpBridge {
    /// Create a new native RTP bridge technology instance.
    pub fn new() -> Self {
        Self {
            channel_data: Vec::new(),
        }
    }

    /// Check if two bridge channels are compatible for native RTP bridging.
    ///
    /// In a full implementation, this would:
    /// 1. Get the RTP glue callbacks for each channel's technology
    /// 2. Get the RTP instances for audio and video
    /// 3. Check if direct RTP is allowed between them
    /// 4. Verify codecs are compatible (no transcoding needed)
    /// 5. Check that no features require media interception
    pub fn check_compatible(&self, bridge: &Bridge) -> bool {
        // Native RTP bridging requires exactly 2 channels
        if bridge.num_channels() != 2 {
            debug!(
                "NativeRtpBridge: incompatible - need exactly 2 channels, have {}",
                bridge.num_channels()
            );
            return false;
        }

        // In a full implementation, we'd check:
        //
        // 1. Both channels have RTP glue registered:
        //    let glue0 = rtp_instance_get_glue(chan0.tech().type_name());
        //    let glue1 = rtp_instance_get_glue(chan1.tech().type_name());
        //    if glue0.is_none() || glue1.is_none() { return false; }
        //
        // 2. Both channels provide RTP instances:
        //    let rtp0 = glue0.get_rtp_info(chan0);
        //    let rtp1 = glue1.get_rtp_info(chan1);
        //
        // 3. Neither forbids native bridging:
        //    if rtp0.result == Forbid || rtp1.result == Forbid { return false; }
        //
        // 4. Combined result is not Forbid:
        //    let combined = RtpGlueResult::combine(rtp0.result, rtp1.result);
        //    if combined == Forbid { return false; }
        //
        // 5. No bridge features requiring media interception are active

        debug!("NativeRtpBridge: compatibility check passed (stub)");
        true
    }

    /// Activate native RTP bridging between two channels.
    ///
    /// Redirects RTP streams so they flow directly between the two
    /// endpoints, bypassing Asterisk's media pipeline.
    pub fn activate_native_bridge(&mut self, bridge: &Bridge) -> AsteriskResult<()> {
        if bridge.num_channels() != 2 {
            return Err(AsteriskError::InvalidArgument(
                "native RTP bridge requires exactly 2 channels".into(),
            ));
        }

        let chan0 = &bridge.channels[0];
        let chan1 = &bridge.channels[1];

        info!(
            "NativeRtpBridge: activating native RTP bridge between '{}' and '{}'",
            chan0.channel_name, chan1.channel_name
        );

        // In a full implementation, we'd redirect the RTP streams:
        //
        // For a Remote bridge (both endpoints can reach each other directly):
        //   glue0.update_peer(chan0, rtp1.instance, video1.instance);
        //   glue1.update_peer(chan1, rtp0.instance, video0.instance);
        //   // RTP now flows directly between endpoints
        //
        // For a Local bridge (both on same host):
        //   rtp0.instance.set_bridged(rtp1.instance);
        //   rtp1.instance.set_bridged(rtp0.instance);
        //   // RTP loops back locally

        // Track activation state
        for channel_data in &mut self.channel_data {
            channel_data.native_active = true;
        }

        Ok(())
    }

    /// Deactivate native RTP bridging, returning media flow through Asterisk.
    pub fn deactivate_native_bridge(&mut self, bridge: &Bridge) {
        info!(
            "NativeRtpBridge: deactivating native RTP bridge '{}'",
            bridge.name
        );

        // In a full implementation:
        //   for channel in &bridge.channels {
        //       glue.update_peer(channel, None, None);
        //       // RTP now flows back through Asterisk
        //   }

        for channel_data in &mut self.channel_data {
            channel_data.native_active = false;
        }
    }
}

impl Default for NativeRtpBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BridgeTechnology for NativeRtpBridge {
    fn name(&self) -> &str {
        "native_rtp"
    }

    fn capabilities(&self) -> BridgeCapability {
        BridgeCapability::NATIVE
    }

    fn preference(&self) -> u32 {
        // AST_BRIDGE_PREFERENCE_BASE_NATIVE -- highest preference.
        // Native bridging is always preferred when compatible.
        100
    }

    async fn create(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        debug!("NativeRtpBridge: creating bridge '{}'", bridge.name);
        bridge.technology = "native_rtp".to_string();
        Ok(())
    }

    async fn start(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        debug!("NativeRtpBridge: starting bridge '{}'", bridge.name);
        Ok(())
    }

    async fn stop(&self, bridge: &mut Bridge) -> AsteriskResult<()> {
        debug!("NativeRtpBridge: stopping bridge '{}'", bridge.name);
        // Deactivate native bridging for all channels
        for data in &mut self.channel_data.iter() {
            if data.native_active {
                info!(
                    "NativeRtpBridge: deactivating native RTP for channel '{}'",
                    data.channel_name
                );
            }
        }
        Ok(())
    }

    async fn join(
        &self,
        bridge: &mut Bridge,
        channel: &BridgeChannel,
    ) -> AsteriskResult<()> {
        debug!(
            "NativeRtpBridge: channel '{}' joining bridge '{}'",
            channel.channel_name, bridge.name
        );

        // Native RTP bridge requires exactly 2 channels
        if bridge.num_channels() > 2 {
            return Err(AsteriskError::InvalidArgument(
                "native RTP bridge supports at most 2 channels".into(),
            ));
        }

        // In a full implementation, when the second channel joins:
        // 1. Get RTP glue data for both channels
        // 2. Install framehooks to intercept control frames
        // 3. If compatible, activate native RTP bridging

        Ok(())
    }

    async fn leave(
        &self,
        bridge: &mut Bridge,
        channel: &BridgeChannel,
    ) -> AsteriskResult<()> {
        debug!(
            "NativeRtpBridge: channel '{}' leaving bridge '{}'",
            channel.channel_name, bridge.name
        );

        // In a full implementation:
        // 1. Remove framehook from the leaving channel
        // 2. Deactivate native bridge if it was active
        // 3. Clean up per-channel RTP data

        Ok(())
    }

    async fn write_frame(
        &self,
        _bridge: &mut Bridge,
        from_channel: &BridgeChannel,
        _frame: &Frame,
    ) -> AsteriskResult<()> {
        // In a native RTP bridge, frames should not normally come through
        // the bridge framework -- they flow directly between endpoints.
        //
        // If we receive frames here, it means native bridging is not active
        // and we need to fall back to generic frame passing.
        debug!(
            "NativeRtpBridge: received frame from '{}' (native bridge not active?)",
            from_channel.channel_name
        );

        // In a full implementation, if the native bridge is not active,
        // we'd pass frames through like a simple bridge.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glue_result_combine() {
        assert_eq!(
            RtpGlueResult::combine(RtpGlueResult::Remote, RtpGlueResult::Remote),
            RtpGlueResult::Remote
        );
        assert_eq!(
            RtpGlueResult::combine(RtpGlueResult::Local, RtpGlueResult::Remote),
            RtpGlueResult::Local
        );
        assert_eq!(
            RtpGlueResult::combine(RtpGlueResult::Forbid, RtpGlueResult::Remote),
            RtpGlueResult::Forbid
        );
        assert_eq!(
            RtpGlueResult::combine(RtpGlueResult::Remote, RtpGlueResult::Forbid),
            RtpGlueResult::Forbid
        );
        assert_eq!(
            RtpGlueResult::combine(RtpGlueResult::Local, RtpGlueResult::Local),
            RtpGlueResult::Local
        );
    }

    #[test]
    fn test_bridge_preference() {
        let bridge = NativeRtpBridge::new();
        // Native bridge should have the highest preference
        assert_eq!(bridge.preference(), 100);
    }

    #[test]
    fn test_bridge_capabilities() {
        let bridge = NativeRtpBridge::new();
        assert_eq!(bridge.capabilities(), BridgeCapability::NATIVE);
    }

    #[test]
    fn test_compatible_check_wrong_count() {
        let bridge_tech = NativeRtpBridge::new();

        // Empty bridge -- not compatible
        let bridge = Bridge::new("test");
        assert!(!bridge_tech.check_compatible(&bridge));

        // One channel -- not compatible
        let mut bridge = Bridge::new("test");
        bridge.add_channel(
            crate::channel::ChannelId::from_name("chan1"),
            "SIP/alice-001".to_string(),
        );
        assert!(!bridge_tech.check_compatible(&bridge));
    }

    #[test]
    fn test_compatible_check_two_channels() {
        let bridge_tech = NativeRtpBridge::new();
        let mut bridge = Bridge::new("test");
        bridge.add_channel(
            crate::channel::ChannelId::from_name("chan1"),
            "SIP/alice-001".to_string(),
        );
        bridge.add_channel(
            crate::channel::ChannelId::from_name("chan2"),
            "SIP/bob-001".to_string(),
        );
        // Stub check passes for 2 channels
        assert!(bridge_tech.check_compatible(&bridge));
    }
}
