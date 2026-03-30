//! Local proxy channel driver.
//!
//! Port of `main/core_local.c`. Creates paired channels (`;1` and `;2`) where
//! frames written to one side are readable on the other.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use asterisk_core::channel::{Channel, ChannelDriver, publish_channel_event};
use asterisk_core::{channel_store, get_global_dialplan, pbx_run};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame, HangupCause};

const LOCAL_FRAME_BUFFER: usize = 150;
static PAIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalSide {
    One,
    Two,
}

impl fmt::Display for LocalSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One => write!(f, "1"),
            Self::Two => write!(f, "2"),
        }
    }
}

#[derive(Debug)]
pub struct LocalPairState {
    pub pair_id: u64,
    pub context: String,
    pub extension: String,
    pub hungup: AtomicBool,
    pub optimize_away: AtomicBool,
    pub bridged: AtomicBool,
}

struct LocalPrivate {
    side: LocalSide,
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<Frame>>,
    pair: Arc<LocalPairState>,
}

impl fmt::Debug for LocalPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPrivate")
            .field("side", &self.side)
            .field("pair_id", &self.pair.pair_id)
            .finish()
    }
}

pub struct LocalChannelDriver {
    /// Private data for each channel side, keyed by channel name.
    /// We key by name (not unique_id) because the global channel store may
    /// reassign unique_ids when registering channels, but the name is stable.
    channels: RwLock<HashMap<String, Arc<LocalPrivate>>>,
    /// Pending ;2 channels, keyed by ;1's channel name.
    /// Stored after request(), consumed by call().
    pending_chan2: RwLock<HashMap<String, Channel>>,
}

impl fmt::Debug for LocalChannelDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalChannelDriver")
            .field("active_channels", &self.channels.read().len())
            .field("pending_chan2", &self.pending_chan2.read().len())
            .finish()
    }
}

impl LocalChannelDriver {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            pending_chan2: RwLock::new(HashMap::new()),
        }
    }

    /// Create a Local channel pair. Returns (;1, ;2).
    pub fn request_pair(&self, dest: &str) -> AsteriskResult<(Channel, Channel)> {
        let (ext_context, no_optimize) = if let Some(stripped) = dest.strip_suffix("/n") {
            (stripped, true)
        } else {
            (dest, false)
        };

        let (extension, context) = match ext_context.split_once('@') {
            Some((ext, ctx)) => (ext.to_string(), ctx.to_string()),
            None => {
                // No @context specified -- default to "default" context
                // This matches C Asterisk behavior
                (ext_context.to_string(), "default".to_string())
            }
        };

        let pair_id = PAIR_COUNTER.fetch_add(1, Ordering::Relaxed);

        let pair_state = Arc::new(LocalPairState {
            pair_id,
            context: context.clone(),
            extension: extension.clone(),
            hungup: AtomicBool::new(false),
            optimize_away: AtomicBool::new(!no_optimize),
            bridged: AtomicBool::new(false),
        });

        let (tx_1_to_2, rx_1_to_2) = mpsc::channel(LOCAL_FRAME_BUFFER);
        let (tx_2_to_1, rx_2_to_1) = mpsc::channel(LOCAL_FRAME_BUFFER);

        let chan_name_1 = format!("Local/{}@{}-{:08x};1", extension, context, pair_id);
        let chan_name_2 = format!("Local/{}@{}-{:08x};2", extension, context, pair_id);

        let chan1 = Channel::new(chan_name_1.clone());
        let priv1 = Arc::new(LocalPrivate {
            side: LocalSide::One,
            tx: tx_1_to_2,
            rx: Mutex::new(rx_2_to_1),
            pair: Arc::clone(&pair_state),
        });

        let mut chan2 = Channel::new(chan_name_2.clone());
        chan2.context = context;
        chan2.exten = extension;
        chan2.priority = 1;

        let priv2 = Arc::new(LocalPrivate {
            side: LocalSide::Two,
            tx: tx_2_to_1,
            rx: Mutex::new(rx_1_to_2),
            pair: Arc::clone(&pair_state),
        });

        {
            let mut channels = self.channels.write();
            channels.insert(chan_name_1.clone(), priv1);
            channels.insert(chan_name_2.clone(), priv2);
        }

        info!(pair_id, chan1 = %chan_name_1, chan2 = %chan_name_2, "Created Local channel pair");

        Ok((chan1, chan2))
    }

    /// Store a ;2 channel in the pending map, keyed by the ;1 channel's name.
    ///
    /// We key by name (not unique_id) because register_existing_channel() may
    /// reassign the unique_id, but the channel name is stable.
    ///
    /// Called by `ChannelDriver::request()` after `request_pair()` returns.
    /// The stored ;2 is consumed by `call()` when it starts PBX on the ;2 side.
    fn store_pending_chan2(&self, chan1_name: &str, chan2: Channel) {
        self.pending_chan2.write().insert(chan1_name.to_string(), chan2);
    }

    /// Take the pending ;2 channel for a given ;1 channel name.
    ///
    /// Returns `None` if no pending ;2 exists (already consumed or never created).
    pub fn take_pending_chan2(&self, chan1_name: &str) -> Option<Channel> {
        self.pending_chan2.write().remove(chan1_name)
    }

    fn get_private(&self, channel_id: &str) -> Option<Arc<LocalPrivate>> {
        self.channels.read().get(channel_id).cloned()
    }

    fn remove_private(&self, channel_id: &str) -> Option<Arc<LocalPrivate>> {
        self.channels.write().remove(channel_id)
    }

    pub fn should_optimize(&self, channel_id: &str) -> bool {
        self.get_private(channel_id)
            .map(|p| p.pair.optimize_away.load(Ordering::Relaxed) && p.pair.bridged.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn disable_optimization(&self, channel_id: &str) {
        if let Some(p) = self.get_private(channel_id) {
            p.pair.optimize_away.store(false, Ordering::Relaxed);
        }
    }

    pub fn enable_optimization(&self, channel_id: &str) {
        if let Some(p) = self.get_private(channel_id) {
            p.pair.optimize_away.store(true, Ordering::Relaxed);
        }
    }
}

impl Default for LocalChannelDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for LocalChannelDriver {
    fn name(&self) -> &str {
        "Local"
    }

    fn description(&self) -> &str {
        "Local Proxy Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let (chan1, chan2) = self.request_pair(dest)?;
        // Store ;2 in the pending map keyed by ;1's name so call() on ;1
        // can retrieve it and start PBX on it. We use name (not unique_id)
        // because register_existing_channel() may reassign the unique_id.
        self.store_pending_chan2(&chan1.name, chan2);
        Ok(chan1)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if priv_data.pair.hungup.load(Ordering::Relaxed) {
            return Err(AsteriskError::Hangup("Local pair already hungup".into()));
        }

        channel.set_state(ChannelState::Ring);
        let _ = priv_data.tx.send(Frame::control(ControlFrame::Ringing)).await;

        let local_context = priv_data.pair.context.clone();
        let local_extension = priv_data.pair.extension.clone();

        // Retrieve the pending ;2 channel and start PBX on it
        // This mirrors C Asterisk's local_call() -> ast_pbx_start(p->chan)
        if let Some(chan2) = self.take_pending_chan2(&channel.name) {
            let chan2_name = chan2.name.clone();

            // Register ;2 in the global channel store and emit Newchannel
            let store_chan2 = channel_store::register_existing_channel(chan2);

            // Get the NEW unique_id assigned by register_existing_channel
            let chan2_uid = store_chan2.lock().unique_id.0.clone();

            // Emit LocalBridge event
            publish_channel_event("LocalBridge", &[
                ("Channel1", &channel.name),
                ("Channel2", &chan2_name),
                ("Context", &local_context),
                ("Exten", &local_extension),
            ]);

            // Get the dialplan for PBX execution
            if let Some(dialplan) = get_global_dialplan() {
                let chan1_name = channel.name.clone();
                let _chan1_uid = channel.unique_id.0.clone();
                let pair_state = Arc::clone(&priv_data.pair);

                // Create a tokio::sync::Mutex-wrapped Channel for pbx_run
                // by cloning the relevant fields from the store channel
                let pbx_chan2 = {
                    let guard = store_chan2.lock();
                    let mut ch = Channel::new(&guard.name);
                    ch.unique_id = guard.unique_id.clone();
                    ch.context = guard.context.clone();
                    ch.exten = guard.exten.clone();
                    ch.priority = guard.priority;
                    ch.linkedid = guard.linkedid.clone();
                    ch.caller = guard.caller.clone();
                    Arc::new(tokio::sync::Mutex::new(ch))
                };

                // Spawn PBX execution on ;2
                tokio::spawn(async move {
                    info!(
                        chan2 = %chan2_name,
                        context = %local_context,
                        exten = %local_extension,
                        "Starting PBX on Local ;2 channel"
                    );

                    let _result = pbx_run(pbx_chan2, dialplan).await;

                    info!(chan2 = %chan2_name, "Local ;2 PBX completed");

                    // When ;2's PBX completes, also soft-hangup ;1
                    // This mirrors C Asterisk's behavior where hanging up one
                    // side of a Local pair tears down the other side.
                    pair_state.hungup.store(true, Ordering::Relaxed);

                    // Hangup ;1 if it's still in the channel store
                    if let Some(chan1_arc) = channel_store::find_by_name(&chan1_name) {
                        let mut ch1 = chan1_arc.lock();
                        if ch1.state != ChannelState::Down {
                            ch1.hangup(HangupCause::NormalClearing);
                        }
                        let uid = ch1.unique_id.0.clone();
                        drop(ch1);
                        channel_store::deregister(&uid);
                    }

                    // Clean up ;2 from the channel store
                    channel_store::deregister(&chan2_uid);
                });
            }
        }

        info!(
            pair_id = priv_data.pair.pair_id,
            context = %priv_data.pair.context,
            extension = %priv_data.pair.extension,
            "Local channel call initiated"
        );
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        channel.answer();
        let _ = priv_data.tx.send(Frame::control(ControlFrame::Answer)).await;
        debug!(pair_id = priv_data.pair.pair_id, side = %priv_data.side, "Local channel answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = match self.remove_private(&channel.name) {
            Some(p) => p,
            None => return Ok(()),
        };

        priv_data.pair.hungup.store(true, Ordering::Relaxed);
        channel.set_state(ChannelState::Down);
        let _ = priv_data.tx.send(Frame::control(ControlFrame::Hangup)).await;
        info!(pair_id = priv_data.pair.pair_id, side = %priv_data.side, "Local channel hungup");
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if priv_data.pair.hungup.load(Ordering::Relaxed) {
            return Ok(Frame::control(ControlFrame::Hangup));
        }

        let mut rx = priv_data.rx.lock().await;
        match rx.recv().await {
            Some(frame) => Ok(frame),
            None => {
                priv_data.pair.hungup.store(true, Ordering::Relaxed);
                Ok(Frame::control(ControlFrame::Hangup))
            }
        }
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if priv_data.pair.hungup.load(Ordering::Relaxed) {
            return Err(AsteriskError::Hangup("Local pair hungup".into()));
        }

        priv_data
            .tx
            .send(frame.clone())
            .await
            .map_err(|_| AsteriskError::Hangup("Other side of Local channel gone".into()))?;
        Ok(())
    }

    async fn send_digit_begin(&self, channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        let frame = Frame::dtmf_begin(digit);
        self.write_frame(channel, &frame).await
    }

    async fn send_digit_end(&self, channel: &mut Channel, digit: char, duration: u32) -> AsteriskResult<()> {
        let frame = Frame::dtmf_end(digit, duration);
        self.write_frame(channel, &frame).await
    }

    async fn indicate(&self, channel: &mut Channel, condition: i32, data: &[u8]) -> AsteriskResult<()> {
        let control = match condition as u32 {
            x if x == ControlFrame::Hangup as u32 => ControlFrame::Hangup,
            x if x == ControlFrame::Ringing as u32 => ControlFrame::Ringing,
            x if x == ControlFrame::Answer as u32 => ControlFrame::Answer,
            x if x == ControlFrame::Busy as u32 => ControlFrame::Busy,
            x if x == ControlFrame::Congestion as u32 => ControlFrame::Congestion,
            x if x == ControlFrame::Progress as u32 => ControlFrame::Progress,
            x if x == ControlFrame::Proceeding as u32 => ControlFrame::Proceeding,
            x if x == ControlFrame::Hold as u32 => ControlFrame::Hold,
            x if x == ControlFrame::Unhold as u32 => ControlFrame::Unhold,
            _ => return Ok(()),
        };

        let frame = if data.is_empty() {
            Frame::control(control)
        } else {
            Frame::control_with_data(control, Bytes::copy_from_slice(data))
        };
        self.write_frame(channel, &frame).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_types::FrameType;

    #[test]
    fn test_local_pair_creation() {
        let driver = LocalChannelDriver::new();
        let (c1, c2) = driver.request_pair("100@default").unwrap();
        assert!(c1.name.contains(";1"));
        assert!(c2.name.contains(";2"));
        assert_eq!(c2.context, "default");
        assert_eq!(c2.exten, "100");
    }

    #[tokio::test]
    async fn test_local_frame_passing() {
        let driver = Arc::new(LocalChannelDriver::new());
        let (mut c1, mut c2) = driver.request_pair("100@default").unwrap();

        let frame = Frame::voice(0, 160, Bytes::from_static(&[0u8; 320]));
        driver.write_frame(&mut c1, &frame).await.unwrap();

        let read = driver.read_frame(&mut c2).await.unwrap();
        assert_eq!(read.frame_type(), FrameType::Voice);
    }

    #[tokio::test]
    async fn test_local_hangup_propagation() {
        let driver = Arc::new(LocalChannelDriver::new());
        let (mut c1, mut c2) = driver.request_pair("100@default").unwrap();

        driver.hangup(&mut c1).await.unwrap();
        let frame = driver.read_frame(&mut c2).await.unwrap();
        assert_eq!(frame.frame_type(), FrameType::Control);
    }
}
