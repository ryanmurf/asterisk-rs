//! Skinny/SCCP channel driver - Cisco Skinny Client Control Protocol.
//!
//! Port of chan_skinny.c from Asterisk C.
//!
//! SCCP is Cisco's proprietary protocol for controlling IP phones.
//! It uses TCP on port 2000 with fixed-size binary message headers.
//! This module provides frame parsing, station messages, and device registration.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use parking_lot::RwLock;
use tracing::info;

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, Frame};

// ---------------------------------------------------------------------------
// Skinny protocol constants
// ---------------------------------------------------------------------------

/// Default Skinny/SCCP TCP port.
pub const SKINNY_PORT: u16 = 2000;

/// Skinny message header size: 4 (length) + 4 (reserved) + 4 (message_id) = 12 bytes.
pub const SKINNY_HEADER_SIZE: usize = 12;

// ---------------------------------------------------------------------------
// Skinny message IDs (station -> server)
// ---------------------------------------------------------------------------

/// Skinny station-to-server message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SkinnyStationMessage {
    KeepAlive = 0x0000,
    Register = 0x0001,
    IpPort = 0x0002,
    KeypadButton = 0x0003,
    StimulusMessage = 0x0005,
    OffHook = 0x0006,
    OnHook = 0x0007,
    SpeedDialStat = 0x000A,
    LineStat = 0x000B,
    ConfigStat = 0x000C,
    TimeDateReq = 0x000D,
    ButtonTemplate = 0x000E,
    VersionReq = 0x000F,
    CapabilitiesRes = 0x0010,
    AlarmMessage = 0x0020,
    OpenReceiveChannelAck = 0x0022,
    SoftKeySet = 0x0025,
    SoftKeyEvent = 0x0026,
    UnregisterMessage = 0x0027,
    SoftKeyTemplateReq = 0x0028,
    HeadsetStatus = 0x002B,
    RegisterAvailableLines = 0x002D,
}

/// Skinny server-to-station message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SkinnyServerMessage {
    KeepAliveAck = 0x0100,
    RegisterAck = 0x0081,
    RegisterReject = 0x009D,
    StartTone = 0x0082,
    StopTone = 0x0083,
    SetRinger = 0x0085,
    SetLamp = 0x0086,
    SetSpeakerMode = 0x0088,
    StartMediaTransmission = 0x008A,
    StopMediaTransmission = 0x008B,
    CallInfo = 0x008F,
    DefineTimeDate = 0x0094,
    DisplayText = 0x0099,
    ClearDisplay = 0x009A,
    RegisterAvailableLinesAck = 0x009B,
    CapabilitiesReq = 0x009C,
    SelectSoftKeys = 0x0110,
    CallState = 0x0111,
    DisplayPromptStatus = 0x0112,
    ClearPromptStatus = 0x0113,
    ActivateCallPlane = 0x0116,
    OpenReceiveChannel = 0x0105,
    CloseReceiveChannel = 0x0106,
    Reset = 0x009F,
}

/// A parsed Skinny protocol message.
#[derive(Debug, Clone)]
pub struct SkinnyMessage {
    /// Message length (excluding the length field itself)
    pub length: u32,
    /// Reserved field (protocol version, typically 0)
    pub reserved: u32,
    /// Message ID
    pub message_id: u32,
    /// Message payload
    pub payload: Bytes,
}

impl SkinnyMessage {
    /// Parse a Skinny message from a byte buffer.
    /// Returns the message and bytes consumed, or None if incomplete.
    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < SKINNY_HEADER_SIZE {
            return None;
        }

        let length = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let reserved = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let message_id = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);

        // length includes reserved + message_id (8 bytes) + payload
        let total_size = 4 + length as usize; // 4 for the length field itself
        if data.len() < total_size {
            return None;
        }

        let payload = if total_size > SKINNY_HEADER_SIZE {
            Bytes::copy_from_slice(&data[SKINNY_HEADER_SIZE..total_size])
        } else {
            Bytes::new()
        };

        Some((
            Self {
                length,
                reserved,
                message_id,
                payload,
            },
            total_size,
        ))
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> BytesMut {
        let payload_len = self.payload.len();
        let length = 8 + payload_len as u32; // reserved(4) + message_id(4) + payload
        let mut buf = BytesMut::with_capacity(SKINNY_HEADER_SIZE + payload_len);
        buf.put_u32_le(length);
        buf.put_u32_le(self.reserved);
        buf.put_u32_le(self.message_id);
        buf.put_slice(&self.payload);
        buf
    }

    /// Create a KeepAlive Ack.
    pub fn keepalive_ack() -> Self {
        Self {
            length: 8,
            reserved: 0,
            message_id: SkinnyServerMessage::KeepAliveAck as u32,
            payload: Bytes::new(),
        }
    }

    /// Create a Register Ack.
    pub fn register_ack(keepalive_interval: u32) -> Self {
        let mut payload = BytesMut::new();
        payload.put_u32_le(keepalive_interval);
        // Date/time placeholder
        payload.put_u32_le(0); // secondary keepalive
        // Protocol version
        payload.put_slice(b"00000000"); // firmware date template
        Self {
            length: 8 + payload.len() as u32,
            reserved: 0,
            message_id: SkinnyServerMessage::RegisterAck as u32,
            payload: payload.freeze(),
        }
    }

    /// Create a Start Tone message.
    pub fn start_tone(tone: u32) -> Self {
        let mut payload = BytesMut::new();
        payload.put_u32_le(tone);
        payload.put_u32_le(0); // line instance
        payload.put_u32_le(0); // call reference
        Self {
            length: 8 + payload.len() as u32,
            reserved: 0,
            message_id: SkinnyServerMessage::StartTone as u32,
            payload: payload.freeze(),
        }
    }

    /// Create a Set Ringer message.
    pub fn set_ringer(mode: u32) -> Self {
        let mut payload = BytesMut::new();
        payload.put_u32_le(mode); // 0=off, 1=inside, 2=outside, 3=feature
        payload.put_u32_le(0); // always 0
        payload.put_u32_le(0); // line instance
        payload.put_u32_le(0); // call reference
        Self {
            length: 8 + payload.len() as u32,
            reserved: 0,
            message_id: SkinnyServerMessage::SetRinger as u32,
            payload: payload.freeze(),
        }
    }
}

/// Skinny device registration info.
#[derive(Debug, Clone)]
pub struct SkinnyDevice {
    pub name: String,
    pub device_type: u32,
    pub max_streams: u32,
    pub lines: Vec<SkinnyLine>,
    pub registered: bool,
}

/// A line on a Skinny device.
#[derive(Debug, Clone)]
pub struct SkinnyLine {
    pub instance: u32,
    pub name: String,
    pub label: String,
}

/// Skinny/SCCP channel driver.
pub struct SkinnyDriver {
    devices: RwLock<HashMap<String, SkinnyDevice>>,
}

impl fmt::Debug for SkinnyDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkinnyDriver")
            .field("devices", &self.devices.read().len())
            .finish()
    }
}

impl SkinnyDriver {
    pub fn new() -> Self {
        Self {
            devices: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device.
    pub fn register_device(&self, name: &str, device_type: u32) {
        self.devices.write().insert(
            name.to_string(),
            SkinnyDevice {
                name: name.to_string(),
                device_type,
                max_streams: 5,
                lines: Vec::new(),
                registered: true,
            },
        );
    }
}

impl Default for SkinnyDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for SkinnyDriver {
    fn name(&self) -> &str {
        "Skinny"
    }

    fn description(&self) -> &str {
        "Skinny/SCCP Channel Driver (Cisco IP Phones)"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let chan_name = format!("Skinny/{}", dest);
        let channel = Channel::new(chan_name);
        info!(dest, "Skinny channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, _dest: &str, _timeout: i32) -> AsteriskResult<()> {
        // Would send SetRinger + CallState
        info!(channel = %channel.name, "Skinny channel ringing");
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.answer();
        info!(channel = %channel.name, "Skinny channel answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        channel.set_state(ChannelState::Down);
        info!(channel = %channel.name, "Skinny channel hungup");
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        Err(AsteriskError::NotSupported("Skinny read_frame stub".into()))
    }

    async fn write_frame(&self, _channel: &mut Channel, _frame: &Frame) -> AsteriskResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skinny_message_parse_roundtrip() {
        let msg = SkinnyMessage::keepalive_ack();
        let bytes = msg.to_bytes();
        let (parsed, consumed) = SkinnyMessage::parse(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.message_id, SkinnyServerMessage::KeepAliveAck as u32);
    }

    #[test]
    fn test_skinny_start_tone() {
        let msg = SkinnyMessage::start_tone(0x21); // inside dial tone
        let bytes = msg.to_bytes();
        let (parsed, _) = SkinnyMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.message_id, SkinnyServerMessage::StartTone as u32);
        assert!(!parsed.payload.is_empty());
    }

    #[test]
    fn test_skinny_set_ringer() {
        let msg = SkinnyMessage::set_ringer(1); // inside ring
        let bytes = msg.to_bytes();
        let (parsed, _) = SkinnyMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.message_id, SkinnyServerMessage::SetRinger as u32);
    }

    #[test]
    fn test_skinny_incomplete_data() {
        let data = [0u8; 6]; // less than header size
        assert!(SkinnyMessage::parse(&data).is_none());
    }

    #[test]
    fn test_device_registration() {
        let driver = SkinnyDriver::new();
        driver.register_device("SEP001122334455", 30006);
        let devices = driver.devices.read();
        let dev = devices.get("SEP001122334455").unwrap();
        assert!(dev.registered);
        assert_eq!(dev.device_type, 30006);
    }

    #[test]
    fn test_driver_name() {
        let driver = SkinnyDriver::new();
        assert_eq!(driver.name(), "Skinny");
    }
}
