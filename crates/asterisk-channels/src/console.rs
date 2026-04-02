//! Console audio channel driver (stub).
//!
//! Port of chan_console.c from Asterisk C.
//!
//! Provides a channel driver that uses the system audio input/output devices
//! (microphone and speaker) for development testing. Allows developers to
//! interact with the PBX directly from the console.
//!
//! This is a stub implementation - actual audio I/O would require platform
//! audio APIs (ALSA, PortAudio, CoreAudio, etc.).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::{debug, info};

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskResult, ChannelState, Frame};

/// Console channel configuration.
#[derive(Debug, Clone)]
pub struct ConsoleConfig {
    /// Audio device name (e.g., "default", "hw:0,0")
    pub device: String,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Input gain (1.0 = unity)
    pub input_gain: f32,
    /// Output gain (1.0 = unity)
    pub output_gain: f32,
    /// Whether to auto-answer incoming calls
    pub auto_answer: bool,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            device: "default".to_string(),
            sample_rate: 8000,
            input_gain: 1.0,
            output_gain: 1.0,
            auto_answer: true,
        }
    }
}

/// State of a console channel session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleState {
    Idle,
    Ringing,
    Active,
    OnHold,
}

/// Per-channel private data for console channels.
#[derive(Debug)]
struct ConsolePrivate {
    config: ConsoleConfig,
    state: ConsoleState,
}

/// Console channel driver.
///
/// Stub implementation of chan_console.c for development/testing.
/// In production, this would integrate with system audio APIs.
pub struct ConsoleDriver {
    config: ConsoleConfig,
    channels: RwLock<HashMap<String, Arc<RwLock<ConsolePrivate>>>>,
}

impl fmt::Debug for ConsoleDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsoleDriver")
            .field("device", &self.config.device)
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl ConsoleDriver {
    pub fn new() -> Self {
        Self {
            config: ConsoleConfig::default(),
            channels: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_config(config: ConsoleConfig) -> Self {
        Self {
            config,
            channels: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for ConsoleDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for ConsoleDriver {
    fn name(&self) -> &str {
        "Console"
    }

    fn description(&self) -> &str {
        "Console Channel Driver (System Audio)"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let chan_name = format!("Console/{}", dest);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(RwLock::new(ConsolePrivate {
            config: self.config.clone(),
            state: ConsoleState::Idle,
        }));

        self.channels.write().insert(channel_id, priv_data);
        info!(dest, "Console channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        if let Some(priv_data) = self.channels.read().get(channel.unique_id.as_str()) {
            priv_data.write().state = ConsoleState::Ringing;
        }
        info!(channel = %channel.name, "Console channel ringing");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        if let Some(priv_data) = self.channels.read().get(channel.unique_id.as_str()) {
            priv_data.write().state = ConsoleState::Active;
        }
        channel.answer();
        info!(channel = %channel.name, "Console channel answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        self.channels.write().remove(channel.unique_id.as_str());
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "Console channel hungup");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        // Stub: In production, this would read from the audio device.
        // Return silence frame.
        let silence = vec![0u8; 320]; // 20ms of 8kHz 16-bit silence
        Ok(Frame::voice(0, 160, bytes::Bytes::from(silence)))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        // Stub: In production, this would write to the audio device.
        debug!("Console: write_frame (stub)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_console_config_default() {
        let config = ConsoleConfig::default();
        assert_eq!(config.device, "default");
        assert_eq!(config.sample_rate, 8000);
    }

    #[test]
    fn test_console_driver_creation() {
        let driver = ConsoleDriver::new();
        assert_eq!(driver.name(), "Console");
    }

    #[tokio::test]
    async fn test_console_request() {
        let driver = ConsoleDriver::new();
        let channel = driver.request("default", None).await.unwrap();
        assert!(channel.name.starts_with("Console/"));
    }
}
