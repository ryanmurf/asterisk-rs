//! Bridge media channel driver.
//!
//! Port of `channels/chan_bridge_media.c`. Creates internal channel pairs used
//! during attended transfers to maintain media while setting up a new call leg.
//!
//! Two channel technologies are provided:
//! - **Announcer**: plays announcements into a bridge.
//! - **Recorder**: records audio from a bridge.
//!
//! Both use the same underlying paired-channel mechanism (similar to Local
//! channels but simpler -- no optimization, always `Up`).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{mpsc, Mutex};
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

const MEDIA_FRAME_BUFFER: usize = 150;
static PAIR_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Which role this half of the pair serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRole {
    /// The owner channel exposed to the requester (;1).
    Owner,
    /// The channel inserted into the bridge (;2).
    Bridge,
}

impl fmt::Display for MediaRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(f, "owner"),
            Self::Bridge => write!(f, "bridge"),
        }
    }
}

/// Which bridge media technology this driver represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMediaType {
    Announcer,
    Recorder,
}

impl BridgeMediaType {
    fn tech_name(&self) -> &'static str {
        match self {
            Self::Announcer => "Announcer",
            Self::Recorder => "Recorder",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Announcer => "Bridge Media Announcing Channel Driver",
            Self::Recorder => "Bridge Media Recording Channel Driver",
        }
    }
}

/// Shared state between the two halves of a bridge media pair.
#[derive(Debug)]
#[allow(dead_code)]
struct BridgeMediaPair {
    pair_id: u64,
    media_type: BridgeMediaType,
    name: String,
}

/// Private data for one side of a bridge media pair.
struct BridgeMediaPrivate {
    role: MediaRole,
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<Frame>>,
    pair: Arc<BridgeMediaPair>,
}

impl fmt::Debug for BridgeMediaPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeMediaPrivate")
            .field("role", &self.role)
            .field("pair_id", &self.pair.pair_id)
            .finish()
    }
}

/// Bridge media channel driver.
///
/// Port of `chan_bridge_media.c`. Creates paired channels for bridge
/// announce/record operations during attended transfers.
pub struct BridgeMediaDriver {
    media_type: BridgeMediaType,
    channels: RwLock<HashMap<String, Arc<BridgeMediaPrivate>>>,
}

impl fmt::Debug for BridgeMediaDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeMediaDriver")
            .field("media_type", &self.media_type)
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl BridgeMediaDriver {
    /// Create a new Announcer driver.
    pub fn announcer() -> Self {
        Self {
            media_type: BridgeMediaType::Announcer,
            channels: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new Recorder driver.
    pub fn recorder() -> Self {
        Self {
            media_type: BridgeMediaType::Recorder,
            channels: RwLock::new(HashMap::new()),
        }
    }

    /// Create a pair of channels. Returns `(owner_channel, bridge_channel)`.
    ///
    /// Both channels start in `Up` state. The owner channel is returned to
    /// the caller of `request()`, and the bridge channel is inserted into
    /// the target bridge.
    pub fn create_pair(&self, data: &str) -> AsteriskResult<(Channel, Channel)> {
        let pair_id = PAIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tech = self.media_type.tech_name();

        let pair = Arc::new(BridgeMediaPair {
            pair_id,
            media_type: self.media_type,
            name: data.to_string(),
        });

        let (tx_owner_to_bridge, rx_owner_to_bridge) = mpsc::channel(MEDIA_FRAME_BUFFER);
        let (tx_bridge_to_owner, rx_bridge_to_owner) = mpsc::channel(MEDIA_FRAME_BUFFER);

        let owner_name = format!("{}/{};1", tech, data);
        let bridge_name = format!("{}/{};2", tech, data);

        let mut owner_chan = Channel::new(owner_name.clone());
        owner_chan.set_state(ChannelState::Up);

        let mut bridge_chan = Channel::new(bridge_name.clone());
        bridge_chan.set_state(ChannelState::Up);

        let owner_priv = Arc::new(BridgeMediaPrivate {
            role: MediaRole::Owner,
            tx: tx_owner_to_bridge,
            rx: Mutex::new(rx_bridge_to_owner),
            pair: Arc::clone(&pair),
        });

        let bridge_priv = Arc::new(BridgeMediaPrivate {
            role: MediaRole::Bridge,
            tx: tx_bridge_to_owner,
            rx: Mutex::new(rx_owner_to_bridge),
            pair: Arc::clone(&pair),
        });

        {
            let mut channels = self.channels.write();
            channels.insert(owner_chan.unique_id.as_str().to_string(), owner_priv);
            channels.insert(bridge_chan.unique_id.as_str().to_string(), bridge_priv);
        }

        info!(
            pair_id,
            owner = %owner_name,
            bridge = %bridge_name,
            tech,
            "Bridge media pair created"
        );

        Ok((owner_chan, bridge_chan))
    }

    fn get_private(&self, id: &str) -> Option<Arc<BridgeMediaPrivate>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<BridgeMediaPrivate>> {
        self.channels.write().remove(id)
    }
}

#[async_trait]
impl ChannelDriver for BridgeMediaDriver {
    fn name(&self) -> &str {
        self.media_type.tech_name()
    }

    fn description(&self) -> &str {
        self.media_type.description()
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let (owner, _bridge) = self.create_pair(dest)?;
        Ok(owner)
    }

    /// Call always fails for bridge media channels (by design, matching C).
    async fn call(
        &self,
        _channel: &mut Channel,
        _dest: &str,
        _timeout: i32,
    ) -> AsteriskResult<()> {
        Err(AsteriskError::NotSupported(
            "Bridge media channels cannot initiate calls".into(),
        ))
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        if let Some(priv_data) = self.remove_private(channel.unique_id.as_str()) {
            let _ = priv_data
                .tx
                .send(Frame::control(ControlFrame::Hangup))
                .await;
            info!(
                pair_id = priv_data.pair.pair_id,
                role = %priv_data.role,
                "Bridge media channel hungup"
            );
        }
        channel.set_state(ChannelState::Down);
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let mut rx = priv_data.rx.lock().await;
        match rx.recv().await {
            Some(frame) => Ok(frame),
            None => Ok(Frame::control(ControlFrame::Hangup)),
        }
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        priv_data
            .tx
            .send(frame.clone())
            .await
            .map_err(|_| AsteriskError::Hangup("Other side of bridge media gone".into()))?;
        Ok(())
    }

    async fn indicate(
        &self,
        channel: &mut Channel,
        condition: i32,
        data: &[u8],
    ) -> AsteriskResult<()> {
        let frame = if data.is_empty() {
            Frame::Control {
                control: match condition as u32 {
                    x if x == ControlFrame::Hangup as u32 => ControlFrame::Hangup,
                    x if x == ControlFrame::Hold as u32 => ControlFrame::Hold,
                    x if x == ControlFrame::Unhold as u32 => ControlFrame::Unhold,
                    _ => return Ok(()),
                },
                data: Bytes::new(),
            }
        } else {
            return Ok(());
        };
        self.write_frame(channel, &frame).await
    }

    async fn send_digit_begin(&self, channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        self.write_frame(channel, &Frame::dtmf_begin(digit)).await
    }

    async fn send_digit_end(
        &self,
        channel: &mut Channel,
        digit: char,
        duration: u32,
    ) -> AsteriskResult<()> {
        self.write_frame(channel, &Frame::dtmf_end(digit, duration))
            .await
    }

    async fn send_text(&self, channel: &mut Channel, text: &str) -> AsteriskResult<()> {
        self.write_frame(channel, &Frame::text(text.to_string()))
            .await
    }

    async fn fixup(
        &self,
        _old_channel: &Channel,
        _new_channel: &mut Channel,
    ) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_types::FrameType;

    #[test]
    fn test_announcer_pair_creation() {
        let driver = BridgeMediaDriver::announcer();
        let (owner, bridge) = driver.create_pair("test").unwrap();
        assert!(owner.name.contains("Announcer"));
        assert!(owner.name.contains(";1"));
        assert!(bridge.name.contains(";2"));
        assert_eq!(owner.state, ChannelState::Up);
        assert_eq!(bridge.state, ChannelState::Up);
    }

    #[tokio::test]
    async fn test_bridge_media_frame_passing() {
        let driver = Arc::new(BridgeMediaDriver::announcer());
        let (mut owner, mut bridge) = driver.create_pair("test").unwrap();

        let frame = Frame::voice(0, 160, Bytes::from_static(&[0u8; 320]));
        driver.write_frame(&mut owner, &frame).await.unwrap();

        let read = driver.read_frame(&mut bridge).await.unwrap();
        assert_eq!(read.frame_type(), FrameType::Voice);
    }

    #[tokio::test]
    async fn test_bridge_media_call_fails() {
        let driver = BridgeMediaDriver::recorder();
        let mut chan = driver.request("test", None).await.unwrap();
        let result = driver.call(&mut chan, "test", 30).await;
        assert!(result.is_err());
    }
}
