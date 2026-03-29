//! AudioSocket channel driver.
//!
//! Port of `channels/chan_audiosocket.c`. TCP-based external audio via
//! length-prefixed binary protocol.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use uuid::Uuid;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AudioSocketMsgType {
    Hangup = 0x00,
    Uuid = 0x01,
    Audio = 0x10,
    Dtmf = 0x11,
    Error = 0xFF,
}

impl AudioSocketMsgType {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Self::Hangup),
            0x01 => Some(Self::Uuid),
            0x10 => Some(Self::Audio),
            0x11 => Some(Self::Dtmf),
            0xFF => Some(Self::Error),
            _ => None,
        }
    }
}

struct AudioSocketPrivate {
    stream: Mutex<TcpStream>,
    uuid: Uuid,
    server: String,
}

impl fmt::Debug for AudioSocketPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSocketPrivate")
            .field("uuid", &self.uuid)
            .field("server", &self.server)
            .finish()
    }
}

pub struct AudioSocketDriver {
    channels: RwLock<HashMap<String, Arc<AudioSocketPrivate>>>,
}

impl fmt::Debug for AudioSocketDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSocketDriver")
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl AudioSocketDriver {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
        }
    }

    fn get_private(&self, id: &str) -> Option<Arc<AudioSocketPrivate>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<AudioSocketPrivate>> {
        self.channels.write().remove(id)
    }

    async fn send_message(
        stream: &mut TcpStream,
        msg_type: AudioSocketMsgType,
        payload: &[u8],
    ) -> AsteriskResult<()> {
        let len = payload.len();
        if len > 0x00FF_FFFF {
            return Err(AsteriskError::InvalidArgument("AudioSocket payload too large".into()));
        }
        let mut header = [0u8; 4];
        header[0] = msg_type as u8;
        header[1] = ((len >> 16) & 0xFF) as u8;
        header[2] = ((len >> 8) & 0xFF) as u8;
        header[3] = (len & 0xFF) as u8;
        stream.write_all(&header).await?;
        if !payload.is_empty() {
            stream.write_all(payload).await?;
        }
        Ok(())
    }

    async fn recv_message(stream: &mut TcpStream) -> AsteriskResult<(AudioSocketMsgType, Bytes)> {
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await?;
        let msg_type = AudioSocketMsgType::from_byte(header[0]).ok_or_else(|| {
            AsteriskError::Parse(format!("Unknown AudioSocket message type: 0x{:02x}", header[0]))
        })?;
        let len = ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | (header[3] as usize);
        if len == 0 {
            return Ok((msg_type, Bytes::new()));
        }
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;
        Ok((msg_type, Bytes::from(payload)))
    }

    async fn send_uuid(stream: &mut TcpStream, uuid: &Uuid) -> AsteriskResult<()> {
        Self::send_message(stream, AudioSocketMsgType::Uuid, uuid.to_string().as_bytes()).await
    }
}

impl Default for AudioSocketDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for AudioSocketDriver {
    fn name(&self) -> &str {
        "AudioSocket"
    }

    fn description(&self) -> &str {
        "AudioSocket Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let parts: Vec<&str> = dest.splitn(3, '/').collect();
        if parts.len() < 2 {
            return Err(AsteriskError::InvalidArgument(
                "AudioSocket destination must be addr:port/uuid".into(),
            ));
        }
        let server_addr = parts[0];
        let uuid_str = parts[1];
        let uuid = Uuid::parse_str(uuid_str).map_err(|e| {
            AsteriskError::InvalidArgument(format!("Invalid UUID '{}': {}", uuid_str, e))
        })?;

        let stream = TcpStream::connect(server_addr).await.map_err(|e| {
            AsteriskError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("Failed to connect to AudioSocket server '{}': {}", server_addr, e),
            ))
        })?;

        let chan_name = format!("AudioSocket/{}-{}", server_addr, uuid_str);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(AudioSocketPrivate {
            stream: Mutex::new(stream),
            uuid,
            server: server_addr.to_string(),
        });
        self.channels.write().insert(channel_id, priv_data);
        info!(server = server_addr, uuid = %uuid, "AudioSocket channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;
        let mut stream = priv_data.stream.lock().await;
        Self::send_uuid(&mut stream, &priv_data.uuid).await?;
        channel.answer();
        info!(uuid = %priv_data.uuid, "AudioSocket initialized");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        if let Some(priv_data) = self.remove_private(channel.unique_id.as_str()) {
            let mut stream = priv_data.stream.lock().await;
            let _ = Self::send_message(&mut stream, AudioSocketMsgType::Hangup, &[]).await;
            let _ = stream.shutdown().await;
            info!(uuid = %priv_data.uuid, "AudioSocket hungup");
        }
        channel.set_state(ChannelState::Down);
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;
        let mut stream = priv_data.stream.lock().await;
        let (msg_type, payload) = Self::recv_message(&mut stream).await?;
        match msg_type {
            AudioSocketMsgType::Audio => {
                let samples = (payload.len() / 2) as u32;
                Ok(Frame::voice(0, samples, payload))
            }
            AudioSocketMsgType::Dtmf => {
                if let Some(&b) = payload.first() {
                    Ok(Frame::dtmf_end(b as char, 100))
                } else {
                    Ok(Frame::null())
                }
            }
            AudioSocketMsgType::Hangup => Ok(Frame::control(ControlFrame::Hangup)),
            AudioSocketMsgType::Error => {
                error!(server = %priv_data.server, "AudioSocket error");
                Ok(Frame::control(ControlFrame::Hangup))
            }
            AudioSocketMsgType::Uuid => Ok(Frame::null()),
        }
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;
        let mut stream = priv_data.stream.lock().await;
        match frame {
            Frame::Voice { data, .. } => {
                Self::send_message(&mut stream, AudioSocketMsgType::Audio, data).await?;
            }
            Frame::DtmfEnd { digit, .. } => {
                Self::send_message(&mut stream, AudioSocketMsgType::Dtmf, &[*digit as u8]).await?;
            }
            _ => {
                debug!(frame_type = ?frame.frame_type(), "Ignoring unsupported frame for AudioSocket");
            }
        }
        Ok(())
    }

    async fn send_digit_end(&self, channel: &mut Channel, digit: char, duration: u32) -> AsteriskResult<()> {
        let frame = Frame::dtmf_end(digit, duration);
        self.write_frame(channel, &frame).await
    }
}
