//! Bridge event loop -- the core frame routing loop for bridged channels.
//!
//! Port of bridge_channel_internal_join / bridge_handle_trip from Asterisk C
//! (bridge_channel.c). Each channel in a bridge runs its own async task that
//! reads frames from the channel, processes DTMF features, and writes frames
//! to the bridge technology for routing to other participants.

use super::builtin_features::{BuiltinFeatures, DtmfFeatureResult};
use super::implementations::get_simple_routing;
use super::{Bridge, BridgeChannel, BridgeChannelState, BridgeTechnology};
use crate::channel::Channel;
use asterisk_types::{ControlFrame, Frame};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Result from processing a single frame in the bridge event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameAction {
    /// Frame was handled, continue the loop.
    Continue,
    /// Channel should leave the bridge (hangup or kicked).
    Leave,
    /// Frame should be written to the bridge technology for routing.
    WriteToTechnology(Frame),
}

/// Determine if a frame type should be passed through the bridge technology.
///
/// Mirrors the C bridge's frame filtering: only voice, DTMF, text, video,
/// and certain control frames are passed between channels.
pub fn should_pass_frame(frame: &Frame) -> bool {
    match frame {
        Frame::Voice { .. } => true,
        Frame::Video { .. } => true,
        Frame::DtmfBegin { .. } | Frame::DtmfEnd { .. } => true,
        Frame::Text { .. } => true,
        Frame::Control { control, .. } => {
            // Block hold/unhold from passing through -- handled locally.
            // Block hangup -- handled by the event loop directly.
            !matches!(
                control,
                ControlFrame::Hold | ControlFrame::Unhold | ControlFrame::Hangup
            )
        }
        Frame::Null => false,
        Frame::Cng { .. } => true,
        _ => false,
    }
}

/// Process a single frame read from a bridge channel.
///
/// This is the Rust port of `bridge_handle_trip()` from bridge_channel.c.
/// It inspects the frame type and decides what action should be taken.
pub fn process_frame(
    frame: &Frame,
    channel_id: &str,
    features: &mut Option<BuiltinFeatures>,
    bridge: &Bridge,
    bridge_channel: &BridgeChannel,
) -> FrameAction {
    match frame {
        // Hangup control frame: channel should leave immediately.
        Frame::Control {
            control: ControlFrame::Hangup,
            ..
        } => {
            info!(
                channel = channel_id,
                "Bridge event loop: received hangup control frame"
            );
            FrameAction::Leave
        }

        // Hold/Unhold: handle locally, do not pass through bridge.
        Frame::Control {
            control: ControlFrame::Hold,
            ..
        }
        | Frame::Control {
            control: ControlFrame::Unhold,
            ..
        } => {
            debug!(
                channel = channel_id,
                control = %match frame {
                    Frame::Control { control, .. } => control.to_string(),
                    _ => String::new(),
                },
                "Bridge event loop: hold/unhold handled locally"
            );
            FrameAction::Continue
        }

        // DTMF end frames: check against feature hooks.
        Frame::DtmfEnd { digit, .. } => {
            if let Some(ref mut feat_engine) = features {
                let result =
                    feat_engine.process_dtmf(channel_id, *digit, bridge, bridge_channel);
                match result {
                    DtmfFeatureResult::Triggered(action) => {
                        info!(
                            channel = channel_id,
                            action = action.as_str(),
                            "Bridge event loop: DTMF feature triggered"
                        );
                        // Feature was consumed -- do not pass the frame through.
                        FrameAction::Continue
                    }
                    DtmfFeatureResult::Collecting => {
                        debug!(
                            channel = channel_id,
                            digit = %digit,
                            "Bridge event loop: collecting DTMF for feature detection"
                        );
                        // Still collecting digits -- do not pass through yet.
                        FrameAction::Continue
                    }
                    DtmfFeatureResult::Timeout => {
                        debug!(
                            channel = channel_id,
                            "Bridge event loop: DTMF feature timeout, passing digits through"
                        );
                        // Timeout: pass the original frame through.
                        FrameAction::WriteToTechnology(frame.clone())
                    }
                    DtmfFeatureResult::PassThrough => {
                        // No match: pass through normally.
                        FrameAction::WriteToTechnology(frame.clone())
                    }
                }
            } else {
                // No feature engine: pass DTMF through.
                FrameAction::WriteToTechnology(frame.clone())
            }
        }

        // DTMF begin frames: pass through unless features are collecting.
        Frame::DtmfBegin { digit, .. } => {
            if let Some(ref mut feat_engine) = features {
                let result =
                    feat_engine.process_dtmf(channel_id, *digit, bridge, bridge_channel);
                match result {
                    DtmfFeatureResult::Triggered(action) => {
                        info!(
                            channel = channel_id,
                            action = action.as_str(),
                            "Bridge event loop: DTMF feature triggered (begin)"
                        );
                        FrameAction::Continue
                    }
                    DtmfFeatureResult::Collecting => {
                        FrameAction::Continue
                    }
                    _ => FrameAction::WriteToTechnology(frame.clone()),
                }
            } else {
                FrameAction::WriteToTechnology(frame.clone())
            }
        }

        // Null frames: timing/keepalive, do not route.
        Frame::Null => FrameAction::Continue,

        // All other frames: check if they should pass, then route.
        _ => {
            if should_pass_frame(frame) {
                FrameAction::WriteToTechnology(frame.clone())
            } else {
                FrameAction::Continue
            }
        }
    }
}

/// The core bridge channel event loop.
///
/// This is the Rust port of `bridge_channel_internal_join()` and the
/// `bridge_channel_wait()` loop from Asterisk C. It is spawned as a
/// tokio task for each channel that joins a bridge.
///
/// The loop:
/// 1. Reads a frame from the channel
/// 2. Processes hangup, DTMF features, hold/unhold
/// 3. Writes eligible frames to the bridge technology for routing
/// 4. Checks the bridge_channel state -- if leaving, breaks
/// 5. Checks for queued actions (transfer, park, etc.)
pub async fn bridge_channel_run(
    bridge: Arc<Mutex<Bridge>>,
    bridge_chan: Arc<Mutex<BridgeChannel>>,
    channel: Arc<Mutex<Channel>>,
    technology: Arc<dyn BridgeTechnology>,
    features: Option<BuiltinFeatures>,
) {
    let channel_id = {
        let bc = bridge_chan.lock().await;
        bc.channel_id.as_str().to_string()
    };
    let channel_name = {
        let bc = bridge_chan.lock().await;
        bc.channel_name.clone()
    };

    info!(
        channel = %channel_name,
        "Bridge event loop: started for channel"
    );

    let mut features = features;

    loop {
        // 1. Check bridge_channel state first.
        {
            let bc = bridge_chan.lock().await;
            if bc.state == BridgeChannelState::Leaving {
                debug!(
                    channel = %channel_name,
                    "Bridge event loop: channel state is Leaving, exiting loop"
                );
                break;
            }
        }

        // 2. Deliver inbound frames: drain frames routed TO this channel
        //    from the bridge technology's routing state and place them on
        //    the channel's write queue. The write queue is separate from the
        //    read queue (frame_queue) to avoid re-routing loops -- frames
        //    in the write queue are destined for the channel driver, not
        //    for re-reading by this event loop.
        {
            let bridge_id = {
                let br = bridge.lock().await;
                br.unique_id.clone()
            };
            let routing = get_simple_routing(&bridge_id);
            let mut routing_state = routing.lock().await;
            let inbound = routing_state.take_frames(&channel_id);
            if !inbound.is_empty() {
                let mut chan = channel.lock().await;
                for f in inbound {
                    chan.queue_write_frame(f);
                }
            }
        }

        // 3. Read a frame from the channel's frame queue.
        //    This picks up both frames queued directly (control, DTMF)
        //    and frames routed from other channels via step 2.
        let frame = {
            let mut chan = channel.lock().await;
            chan.dequeue_frame()
        };

        let frame = match frame {
            Some(f) => f,
            None => {
                // No frame available. Sleep briefly to avoid busy-spinning.
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };

        // 3. Process the frame.
        let bc_snapshot = {
            let bc = bridge_chan.lock().await;
            bc.clone()
        };

        let action = {
            let br = bridge.lock().await;
            process_frame(&frame, &channel_id, &mut features, &br, &bc_snapshot)
        };

        match action {
            FrameAction::Leave => {
                info!(
                    channel = %channel_name,
                    "Bridge event loop: leaving bridge due to hangup"
                );
                let mut bc = bridge_chan.lock().await;
                bc.state = BridgeChannelState::Leaving;
                break;
            }
            FrameAction::WriteToTechnology(frame_to_write) => {
                // 4. Write the frame to the bridge technology for routing.
                let result = {
                    let mut br = bridge.lock().await;
                    technology
                        .write_frame(&mut br, &bc_snapshot, &frame_to_write)
                        .await
                };
                if let Err(e) = result {
                    warn!(
                        channel = %channel_name,
                        error = %e,
                        "Bridge event loop: technology write_frame failed"
                    );
                }
            }
            FrameAction::Continue => {
                // Frame was handled, nothing more to do.
            }
        }

        // 5. Check for state changes again.
        {
            let bc = bridge_chan.lock().await;
            if bc.state == BridgeChannelState::Leaving {
                debug!(
                    channel = %channel_name,
                    "Bridge event loop: channel state changed to Leaving"
                );
                break;
            }
        }
    }

    // Clean up: clear any DTMF buffers.
    if let Some(ref mut feat_engine) = features {
        feat_engine.clear_buffer(&channel_id);
    }

    info!(
        channel = %channel_name,
        "Bridge event loop: exited"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::builtin_features::{BuiltinFeatures, BuiltinFeaturesConfig};
    use crate::channel::ChannelId;
    use bytes::Bytes;

    fn make_bridge() -> Bridge {
        let mut b = Bridge::new("test-bridge");
        b.add_channel(
            ChannelId::from_name("chan-alice"),
            "SIP/alice-001".to_string(),
        );
        b.add_channel(
            ChannelId::from_name("chan-bob"),
            "SIP/bob-001".to_string(),
        );
        b
    }

    fn make_bc(name: &str) -> BridgeChannel {
        BridgeChannel::new(ChannelId::from_name(name), name.to_string())
    }

    #[test]
    fn test_should_pass_frame_voice() {
        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        assert!(should_pass_frame(&frame));
    }

    #[test]
    fn test_should_pass_frame_null() {
        assert!(!should_pass_frame(&Frame::Null));
    }

    #[test]
    fn test_should_pass_frame_hold_blocked() {
        let frame = Frame::control(ControlFrame::Hold);
        assert!(!should_pass_frame(&frame));
    }

    #[test]
    fn test_should_pass_frame_hangup_blocked() {
        let frame = Frame::control(ControlFrame::Hangup);
        assert!(!should_pass_frame(&frame));
    }

    #[test]
    fn test_should_pass_frame_ringing_passed() {
        let frame = Frame::control(ControlFrame::Ringing);
        assert!(should_pass_frame(&frame));
    }

    #[test]
    fn test_process_frame_hangup() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::control(ControlFrame::Hangup);
        let action = process_frame(&frame, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::Leave));
    }

    #[test]
    fn test_process_frame_voice_passthrough() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let action = process_frame(&frame, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::WriteToTechnology(_)));
    }

    #[test]
    fn test_process_frame_dtmf_no_features() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::dtmf_end('5', 100);
        let action = process_frame(&frame, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::WriteToTechnology(_)));
    }

    #[test]
    fn test_process_frame_dtmf_feature_triggered() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::dtmf_end('#', 100);
        let mut features = Some(BuiltinFeatures::new());
        // '#' is default blind transfer code.
        let action = process_frame(&frame, "SIP/alice-001", &mut features, &bridge, &bc);
        assert!(matches!(action, FrameAction::Continue));
    }

    #[test]
    fn test_process_frame_dtmf_partial_match() {
        let config = BuiltinFeaturesConfig {
            blind_transfer_code: "##".to_string(),
            ..Default::default()
        };
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::dtmf_end('#', 100);
        let mut features = Some(BuiltinFeatures::with_config(config));
        let action = process_frame(&frame, "SIP/alice-001", &mut features, &bridge, &bc);
        // First '#' should be collecting (partial match).
        assert!(matches!(action, FrameAction::Continue));
    }

    #[test]
    fn test_process_frame_hold_local() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::control(ControlFrame::Hold);
        let action = process_frame(&frame, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::Continue));
    }

    #[test]
    fn test_process_frame_null_ignored() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let action = process_frame(&Frame::Null, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::Continue));
    }

    #[test]
    fn test_process_frame_text_passthrough() {
        let bridge = make_bridge();
        let bc = make_bc("SIP/alice-001");
        let frame = Frame::text("hello".to_string());
        let action = process_frame(&frame, "SIP/alice-001", &mut None, &bridge, &bc);
        assert!(matches!(action, FrameAction::WriteToTechnology(_)));
    }

    // -----------------------------------------------------------------------
    // Integration tests: end-to-end bridge frame routing
    // -----------------------------------------------------------------------

    use crate::bridge::implementations::SimpleBridge;
    use crate::bridge::{bridge_create, bridge_join, bridge_dissolve};

    /// Create two mock channels, join both to a bridge, queue a voice frame
    /// on channel A, and verify channel B receives it through the bridge
    /// event loop within a bounded time.
    #[tokio::test]
    async fn test_bridge_frame_routing_a_to_b() {
        let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
        let bridge = bridge_create("routing-test", &tech).await.unwrap();

        let chan_a = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/alice-001")));
        let chan_b = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/bob-001")));

        let bc_a = bridge_join(&bridge, &chan_a, &tech).await.unwrap();
        let bc_b = bridge_join(&bridge, &chan_b, &tech).await.unwrap();

        // Spawn bridge event loops for both channels.
        let bridge_a = bridge.clone();
        let bc_a_clone = bc_a.clone();
        let chan_a_clone = chan_a.clone();
        let tech_a = tech.clone();
        let handle_a = tokio::spawn(async move {
            bridge_channel_run(bridge_a, bc_a_clone, chan_a_clone, tech_a, None).await;
        });

        let bridge_b = bridge.clone();
        let bc_b_clone = bc_b.clone();
        let chan_b_clone = chan_b.clone();
        let tech_b = tech.clone();
        let handle_b = tokio::spawn(async move {
            bridge_channel_run(bridge_b, bc_b_clone, chan_b_clone, tech_b, None).await;
        });

        // Queue a voice frame on channel A (simulating a driver read).
        let voice_frame = Frame::voice(0, 160, Bytes::from(vec![0xABu8; 320]));
        {
            let mut ch_a = chan_a.lock().await;
            ch_a.queue_frame(voice_frame.clone());
        }

        // Wait for the frame to be routed through the bridge to channel B.
        // Channel A's event loop reads it, writes to SimpleBridge technology,
        // which stores it in SimpleBridgeRouting for channel B.
        // Channel B's event loop drains SimpleBridgeRouting and queues it
        // on B's write_queue (the queue for frames going to the driver).
        //
        // We poll channel B's write queue with a timeout.
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                {
                    let mut ch_b = chan_b.lock().await;
                    if let Some(f) = ch_b.dequeue_write_frame() {
                        return f;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(received.is_ok(), "Timed out waiting for frame on channel B");
        let received_frame = received.unwrap();
        assert!(received_frame.is_voice(), "Expected voice frame on channel B, got {:?}", received_frame);

        // Verify the frame data matches.
        if let Frame::Voice { data, .. } = &received_frame {
            assert_eq!(data.as_ref(), &[0xABu8; 320]);
        } else {
            panic!("Expected voice frame");
        }

        // Clean up: signal both event loops to stop.
        {
            let mut bc = bc_a.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }
        {
            let mut bc = bc_b.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }

        let _ = tokio::time::timeout(Duration::from_secs(1), handle_a).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle_b).await;

        bridge_dissolve(&bridge, &tech).await.ok();
    }

    /// Verify bidirectional routing: frame from A reaches B and from B reaches A.
    #[tokio::test]
    async fn test_bridge_frame_routing_bidirectional() {
        let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
        let bridge = bridge_create("bidir-test", &tech).await.unwrap();

        let chan_a = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/alice-002")));
        let chan_b = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/bob-002")));

        let bc_a = bridge_join(&bridge, &chan_a, &tech).await.unwrap();
        let bc_b = bridge_join(&bridge, &chan_b, &tech).await.unwrap();

        // Spawn event loops.
        let handle_a = {
            let br = bridge.clone();
            let bc = bc_a.clone();
            let ch = chan_a.clone();
            let t = tech.clone();
            tokio::spawn(async move { bridge_channel_run(br, bc, ch, t, None).await })
        };
        let handle_b = {
            let br = bridge.clone();
            let bc = bc_b.clone();
            let ch = chan_b.clone();
            let t = tech.clone();
            tokio::spawn(async move { bridge_channel_run(br, bc, ch, t, None).await })
        };

        // Send voice from A.
        {
            let mut ch_a = chan_a.lock().await;
            ch_a.queue_frame(Frame::voice(0, 160, Bytes::from(vec![1u8; 160])));
        }

        // Send voice from B.
        {
            let mut ch_b = chan_b.lock().await;
            ch_b.queue_frame(Frame::voice(0, 160, Bytes::from(vec![2u8; 160])));
        }

        // Wait for B to receive A's frame (in write queue).
        let b_got = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                {
                    let mut ch = chan_b.lock().await;
                    if let Some(f) = ch.dequeue_write_frame() {
                        if f.is_voice() {
                            return f;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(b_got.is_ok(), "B did not receive A's frame");

        // Wait for A to receive B's frame (in write queue).
        let a_got = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                {
                    let mut ch = chan_a.lock().await;
                    if let Some(f) = ch.dequeue_write_frame() {
                        if f.is_voice() {
                            return f;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(a_got.is_ok(), "A did not receive B's frame");

        // Clean up.
        {
            let mut bc = bc_a.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }
        {
            let mut bc = bc_b.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }

        let _ = tokio::time::timeout(Duration::from_secs(1), handle_a).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle_b).await;

        bridge_dissolve(&bridge, &tech).await.ok();
    }

    /// Verify that a hangup frame causes the bridge event loop to exit
    /// and the bridge channel state to transition to Leaving.
    #[tokio::test]
    async fn test_bridge_hangup_causes_leave() {
        let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
        let bridge = bridge_create("hangup-test", &tech).await.unwrap();

        let chan_a = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/alice-003")));
        let chan_b = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/bob-003")));

        let bc_a = bridge_join(&bridge, &chan_a, &tech).await.unwrap();
        let bc_b = bridge_join(&bridge, &chan_b, &tech).await.unwrap();

        // Spawn event loop for channel A only.
        let handle_a = {
            let br = bridge.clone();
            let bc = bc_a.clone();
            let ch = chan_a.clone();
            let t = tech.clone();
            tokio::spawn(async move { bridge_channel_run(br, bc, ch, t, None).await })
        };

        // Queue a hangup control frame on channel A.
        {
            let mut ch_a = chan_a.lock().await;
            ch_a.queue_frame(Frame::control(ControlFrame::Hangup));
        }

        // The event loop for A should exit promptly.
        let result = tokio::time::timeout(Duration::from_secs(2), handle_a).await;
        assert!(
            result.is_ok(),
            "Bridge event loop for A did not exit after hangup"
        );

        // Verify BridgeChannel state is Leaving.
        {
            let bc = bc_a.lock().await;
            assert_eq!(
                bc.state,
                BridgeChannelState::Leaving,
                "BridgeChannel should be Leaving after hangup"
            );
        }

        // Clean up.
        {
            let mut bc = bc_b.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }
        bridge_dissolve(&bridge, &tech).await.ok();
    }

    /// Verify that control frames (like Ringing) are routed through the
    /// bridge to the other channel.
    #[tokio::test]
    async fn test_bridge_routes_control_frames() {
        let tech: Arc<dyn BridgeTechnology> = Arc::new(SimpleBridge::new());
        let bridge = bridge_create("ctrl-route-test", &tech).await.unwrap();

        let chan_a = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/alice-004")));
        let chan_b = Arc::new(Mutex::new(crate::channel::Channel::new("SIP/bob-004")));

        let bc_a = bridge_join(&bridge, &chan_a, &tech).await.unwrap();
        let bc_b = bridge_join(&bridge, &chan_b, &tech).await.unwrap();

        let handle_a = {
            let br = bridge.clone();
            let bc = bc_a.clone();
            let ch = chan_a.clone();
            let t = tech.clone();
            tokio::spawn(async move { bridge_channel_run(br, bc, ch, t, None).await })
        };
        let handle_b = {
            let br = bridge.clone();
            let bc = bc_b.clone();
            let ch = chan_b.clone();
            let t = tech.clone();
            tokio::spawn(async move { bridge_channel_run(br, bc, ch, t, None).await })
        };

        // Queue a Ringing control frame on A.
        {
            let mut ch_a = chan_a.lock().await;
            ch_a.queue_frame(Frame::control(ControlFrame::Ringing));
        }

        // B should receive the Ringing control frame (in write queue).
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                {
                    let mut ch_b = chan_b.lock().await;
                    if let Some(f) = ch_b.dequeue_write_frame() {
                        if f.is_control() {
                            return f;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(received.is_ok(), "B did not receive control frame from A");
        let f = received.unwrap();
        assert!(matches!(
            f,
            Frame::Control {
                control: ControlFrame::Ringing,
                ..
            }
        ));

        // Clean up.
        {
            let mut bc = bc_a.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }
        {
            let mut bc = bc_b.lock().await;
            bc.state = BridgeChannelState::Leaving;
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), handle_a).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle_b).await;
        bridge_dissolve(&bridge, &tech).await.ok();
    }
}
