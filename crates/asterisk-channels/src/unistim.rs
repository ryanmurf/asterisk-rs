//! UNISTIM channel driver - Nortel UNISTIM protocol.
//!
//! Port of chan_unistim.c from Asterisk C.
//!
//! UNISTIM (Unified Networks IP Stimulus) is Nortel's proprietary VoIP
//! protocol used in i20xx series IP phones. This driver implements the
//! protocol frame parsing and basic call control stubs.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use parking_lot::RwLock;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

// ---------------------------------------------------------------------------
// UNISTIM protocol constants
// ---------------------------------------------------------------------------

/// Default UNISTIM port.
pub const UNISTIM_PORT: u16 = 5000;

/// Protocol packet types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UnistimPacketType {
    /// Initial discovery/registration
    Discovery = 0x00,
    /// Keepalive / heartbeat
    Keepalive = 0xFF,
    /// Display command
    Display = 0x09,
    /// Key/hook event from phone
    KeyHook = 0x08,
    /// Audio control
    AudioControl = 0x16,
    /// Ringer control
    Ringer = 0x24,
    /// LED control
    Led = 0x04,
    /// Configuration / firmware
    Config = 0x02,
}

impl UnistimPacketType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x00 => Some(Self::Discovery),
            0xFF => Some(Self::Keepalive),
            0x09 => Some(Self::Display),
            0x08 => Some(Self::KeyHook),
            0x16 => Some(Self::AudioControl),
            0x24 => Some(Self::Ringer),
            0x04 => Some(Self::Led),
            0x02 => Some(Self::Config),
            _ => None,
        }
    }
}

/// UNISTIM protocol frame.
#[derive(Debug, Clone)]
pub struct UnistimFrame {
    /// Sequence number
    pub seq: u16,
    /// Packet type
    pub packet_type: u8,
    /// Payload data
    pub payload: Bytes,
}

impl UnistimFrame {
    /// Parse a UNISTIM frame from bytes.
    /// Format: [seq_hi][seq_lo][type][len][payload...]
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        let seq = u16::from_be_bytes([data[0], data[1]]);
        let packet_type = data[2];
        let len = data[3] as usize;
        if data.len() < 4 + len {
            return None;
        }
        Some(Self {
            seq,
            packet_type,
            payload: Bytes::copy_from_slice(&data[4..4 + len]),
        })
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(4 + self.payload.len());
        buf.put_u16(self.seq);
        buf.put_u8(self.packet_type);
        buf.put_u8(self.payload.len() as u8);
        buf.put_slice(&self.payload);
        buf
    }

    /// Create a display command to show text on the phone LCD.
    pub fn display_text(seq: u16, line: u8, text: &str) -> Self {
        let mut payload = BytesMut::new();
        payload.put_u8(line); // display line number
        payload.put_u8(0x00); // column offset
        let text_bytes = text.as_bytes();
        let len = text_bytes.len().min(24); // max 24 chars per line
        payload.put_slice(&text_bytes[..len]);
        Self {
            seq,
            packet_type: UnistimPacketType::Display as u8,
            payload: payload.freeze(),
        }
    }

    /// Create a ringer control command.
    pub fn ring(seq: u16, ring_type: u8) -> Self {
        let mut payload = BytesMut::new();
        payload.put_u8(ring_type); // 0=off, 1=inside, 2=outside, 3=feature
        Self {
            seq,
            packet_type: UnistimPacketType::Ringer as u8,
            payload: payload.freeze(),
        }
    }

    /// Create a keepalive/ack frame.
    pub fn keepalive(seq: u16) -> Self {
        Self {
            seq,
            packet_type: UnistimPacketType::Keepalive as u8,
            payload: Bytes::new(),
        }
    }
}

/// Phone device registration state.
#[derive(Debug, Clone)]
pub struct UnistimDevice {
    pub mac_address: String,
    pub firmware_version: String,
    pub registered: bool,
    pub line_count: u8,
}

/// UNISTIM channel driver.
///
/// Basic connect/ring/answer stubs with protocol frame definitions
/// for Nortel i20xx IP phones.
pub struct UnistimDriver {
    devices: RwLock<HashMap<String, UnistimDevice>>,
}

impl fmt::Debug for UnistimDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnistimDriver")
            .field("devices", &self.devices.read().len())
            .finish()
    }
}

impl UnistimDriver {
    pub fn new() -> Self {
        Self {
            devices: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device by MAC address.
    pub fn register_device(&self, mac: &str, firmware: &str, lines: u8) {
        self.devices.write().insert(
            mac.to_string(),
            UnistimDevice {
                mac_address: mac.to_string(),
                firmware_version: firmware.to_string(),
                registered: true,
                line_count: lines,
            },
        );
    }
}

impl Default for UnistimDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for UnistimDriver {
    fn name(&self) -> &str {
        "USTM"
    }

    fn description(&self) -> &str {
        "UNISTIM Channel Driver (Nortel IP Phones)"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let chan_name = format!("USTM/{}", dest);
        let channel = Channel::new(chan_name);
        info!(dest, "UNISTIM channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        // In production: send ringer command to phone
        info!(channel = %channel.name, "UNISTIM channel ringing");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        info!(channel = %channel.name, "UNISTIM channel answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "UNISTIM channel hungup");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        Err(AsteriskError::NotSupported("UNISTIM read_frame stub".into()))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_parse() {
        let data = [0x00, 0x01, 0x09, 0x03, 0x01, 0x00, b'H'];
        let frame = UnistimFrame::parse(&data).unwrap();
        assert_eq!(frame.seq, 1);
        assert_eq!(frame.packet_type, UnistimPacketType::Display as u8);
        assert_eq!(frame.payload.len(), 3);
    }

    #[test]
    fn test_frame_roundtrip() {
        let frame = UnistimFrame::display_text(42, 0, "Hello");
        let bytes = frame.to_bytes();
        let parsed = UnistimFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.seq, 42);
        assert_eq!(parsed.packet_type, UnistimPacketType::Display as u8);
    }

    #[test]
    fn test_ring_frame() {
        let frame = UnistimFrame::ring(1, 1);
        assert_eq!(frame.packet_type, UnistimPacketType::Ringer as u8);
        assert_eq!(frame.payload[0], 1); // inside ring
    }

    #[test]
    fn test_keepalive() {
        let frame = UnistimFrame::keepalive(99);
        assert_eq!(frame.packet_type, UnistimPacketType::Keepalive as u8);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn test_device_registration() {
        let driver = UnistimDriver::new();
        driver.register_device("00:0F:E2:12:34:56", "3.0.0", 4);
        let devices = driver.devices.read();
        assert!(devices.contains_key("00:0F:E2:12:34:56"));
    }

    #[test]
    fn test_driver_name() {
        let driver = UnistimDriver::new();
        assert_eq!(driver.name(), "USTM");
    }
}
