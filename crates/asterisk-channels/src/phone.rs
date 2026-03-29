//! Linux phone device channel driver (stub).
//!
//! Port of chan_phone.c from Asterisk C.
//!
//! Provides a channel driver for Linux telephony devices using the
//! Linux Telephony Interface (/dev/phone*). These are hardware devices
//! like Quicknet LineJack/PhoneJack cards.
//!
//! This is a stub - actual device I/O requires Linux-specific ioctls.

use std::fmt;

use async_trait::async_trait;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

/// Phone device configuration.
#[derive(Debug, Clone)]
pub struct PhoneConfig {
    /// Device path (e.g., "/dev/phone0")
    pub device: String,
    /// Caller ID to present
    pub caller_id: String,
    /// Extension context
    pub context: String,
    /// Whether to use DTMF detection
    pub dtmf_detect: bool,
}

impl Default for PhoneConfig {
    fn default() -> Self {
        Self {
            device: "/dev/phone0".to_string(),
            caller_id: String::new(),
            context: "default".to_string(),
            dtmf_detect: true,
        }
    }
}

/// Linux phone device channel driver.
///
/// Stub implementation for Linux telephony interface devices.
pub struct PhoneDriver {
    config: PhoneConfig,
}

impl fmt::Debug for PhoneDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhoneDriver")
            .field("device", &self.config.device)
            .finish()
    }
}

impl PhoneDriver {
    pub fn new() -> Self {
        Self {
            config: PhoneConfig::default(),
        }
    }

    pub fn with_config(config: PhoneConfig) -> Self {
        Self { config }
    }
}

impl Default for PhoneDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for PhoneDriver {
    fn name(&self) -> &str {
        "Phone"
    }

    fn description(&self) -> &str {
        "Linux Telephony Device Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let chan_name = format!("Phone/{}", dest);
        let channel = Channel::new(chan_name);
        info!(dest, device = %self.config.device, "Phone channel created (stub)");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        info!(channel = %channel.name, "Phone channel call (stub)");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        info!(channel = %channel.name, "Phone channel answered (stub)");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "Phone channel hungup (stub)");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        Err(AsteriskError::NotSupported(
            "Phone device read not available (stub)".into(),
        ))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phone_driver() {
        let driver = PhoneDriver::new();
        assert_eq!(driver.name(), "Phone");
    }

    #[test]
    fn test_phone_config() {
        let config = PhoneConfig::default();
        assert_eq!(config.device, "/dev/phone0");
        assert_eq!(config.context, "default");
    }
}
