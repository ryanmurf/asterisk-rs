//! AudioSocket resource module.
//!
//! Port of `res/res_audiosocket.c`. Provides helper functions for the
//! AudioSocket protocol, which allows streaming audio between Asterisk and
//! an external TCP-based audio processing service.
//!
//! Protocol framing: Each message consists of a 1-byte type indicator,
//! followed by a 2-byte big-endian payload length, followed by the payload.

use std::fmt;

use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants (from include/asterisk/res_audiosocket.h)
// ---------------------------------------------------------------------------

/// Maximum connection timeout in milliseconds.
pub const MAX_CONNECT_TIMEOUT_MS: u64 = 2000;

/// AudioSocket message header size (1 byte type + 2 bytes length).
pub const HEADER_SIZE: usize = 3;

/// UUID payload size in bytes.
pub const UUID_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Message types (from ast_audiosocket_kind enum)
// ---------------------------------------------------------------------------

/// AudioSocket protocol message types.
///
/// Mirrors the `ast_audiosocket_kind` enum from the C header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AudioSocketKind {
    /// Hangup signal (close connection).
    Hangup = 0x00,
    /// UUID message (sent at connection start).
    Uuid = 0x01,
    /// DTMF digit.
    Dtmf = 0x03,
    /// Audio data, format: signed linear 8kHz (slin).
    Audio = 0x10,
    /// Audio data, format: slin12.
    AudioSlin12 = 0x11,
    /// Audio data, format: slin16.
    AudioSlin16 = 0x12,
    /// Audio data, format: slin24.
    AudioSlin24 = 0x13,
    /// Audio data, format: slin32.
    AudioSlin32 = 0x14,
    /// Audio data, format: slin44.
    AudioSlin44 = 0x15,
    /// Audio data, format: slin48.
    AudioSlin48 = 0x16,
    /// Audio data, format: slin96.
    AudioSlin96 = 0x17,
    /// Audio data, format: slin192.
    AudioSlin192 = 0x18,
    /// An Asterisk-side error occurred.
    Error = 0xFF,
}

impl AudioSocketKind {
    /// Parse a byte into an `AudioSocketKind`.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Self::Hangup),
            0x01 => Some(Self::Uuid),
            0x03 => Some(Self::Dtmf),
            0x10 => Some(Self::Audio),
            0x11 => Some(Self::AudioSlin12),
            0x12 => Some(Self::AudioSlin16),
            0x13 => Some(Self::AudioSlin24),
            0x14 => Some(Self::AudioSlin32),
            0x15 => Some(Self::AudioSlin44),
            0x16 => Some(Self::AudioSlin48),
            0x17 => Some(Self::AudioSlin96),
            0x18 => Some(Self::AudioSlin192),
            0xFF => Some(Self::Error),
            _ => None,
        }
    }

    /// Whether this kind represents an audio payload.
    pub fn is_audio(&self) -> bool {
        matches!(
            self,
            Self::Audio
                | Self::AudioSlin12
                | Self::AudioSlin16
                | Self::AudioSlin24
                | Self::AudioSlin32
                | Self::AudioSlin44
                | Self::AudioSlin48
                | Self::AudioSlin96
                | Self::AudioSlin192
        )
    }

    /// Get the sample rate for audio kinds, or None for non-audio types.
    pub fn sample_rate(&self) -> Option<u32> {
        match self {
            Self::Audio => Some(8000),
            Self::AudioSlin12 => Some(12000),
            Self::AudioSlin16 => Some(16000),
            Self::AudioSlin24 => Some(24000),
            Self::AudioSlin32 => Some(32000),
            Self::AudioSlin44 => Some(44100),
            Self::AudioSlin48 => Some(48000),
            Self::AudioSlin96 => Some(96000),
            Self::AudioSlin192 => Some(192000),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AudioSocketError {
    #[error("AudioSocket I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("AudioSocket connection timeout to '{0}'")]
    ConnectTimeout(String),
    #[error("AudioSocket protocol error: {0}")]
    Protocol(String),
    #[error("AudioSocket invalid UUID: {0}")]
    InvalidUuid(String),
    #[error("AudioSocket hangup received")]
    Hangup,
    #[error("AudioSocket connection closed")]
    Closed,
}

pub type AudioSocketResult<T> = Result<T, AudioSocketError>;

// ---------------------------------------------------------------------------
// Received message
// ---------------------------------------------------------------------------

/// A message received from an AudioSocket server.
#[derive(Debug, Clone)]
pub struct AudioSocketMessage {
    /// Message type.
    pub kind: AudioSocketKind,
    /// Payload data.
    pub payload: Bytes,
}

impl AudioSocketMessage {
    /// If this is a DTMF message, return the digit.
    pub fn dtmf_digit(&self) -> Option<char> {
        if self.kind == AudioSocketKind::Dtmf && !self.payload.is_empty() {
            Some(self.payload[0] as char)
        } else {
            None
        }
    }

    /// Compute the number of audio samples (assuming 16-bit signed linear).
    pub fn audio_samples(&self) -> usize {
        if self.kind.is_audio() {
            self.payload.len() / 2
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------------------
// AudioSocket connection
// ---------------------------------------------------------------------------

/// An AudioSocket connection to an external audio processing server.
///
/// Corresponds to the `ast_audiosocket_connect` / `ast_audiosocket_init` /
/// `ast_audiosocket_send_frame` / `ast_audiosocket_receive_frame` C functions.
pub struct AudioSocketConnection {
    /// TCP stream to the server.
    stream: TcpStream,
    /// Whether the connection is still alive.
    alive: bool,
    /// Connection identifier (UUID as string).
    pub uuid: String,
}

impl AudioSocketConnection {
    /// Connect to an AudioSocket server and send the UUID.
    ///
    /// This combines `ast_audiosocket_connect()` and `ast_audiosocket_init()`.
    pub async fn connect(server: &str, uuid_str: &str) -> AudioSocketResult<Self> {
        // Resolve and connect with timeout.
        let stream = timeout(
            Duration::from_millis(MAX_CONNECT_TIMEOUT_MS),
            TcpStream::connect(server),
        )
        .await
        .map_err(|_| AudioSocketError::ConnectTimeout(server.to_string()))?
        .map_err(AudioSocketError::Io)?;

        // Disable Nagle's algorithm for low latency.
        stream.set_nodelay(true)?;

        info!(server, uuid = uuid_str, "AudioSocket connected");

        let mut conn = Self {
            stream,
            alive: true,
            uuid: uuid_str.to_string(),
        };

        // Send UUID.
        conn.send_uuid(uuid_str).await?;

        Ok(conn)
    }

    /// Send the UUID to the server (the init handshake).
    ///
    /// Mirrors `ast_audiosocket_init()`: sends a UUID message with
    /// type=0x01, length=0x0010, payload=16 bytes of UUID.
    async fn send_uuid(&mut self, uuid_str: &str) -> AudioSocketResult<()> {
        let uuid = uuid::Uuid::parse_str(uuid_str)
            .map_err(|e| AudioSocketError::InvalidUuid(format!("{}: {}", uuid_str, e)))?;

        let uuid_bytes = uuid.as_bytes();

        let mut buf = BytesMut::with_capacity(HEADER_SIZE + UUID_SIZE);
        buf.put_u8(AudioSocketKind::Uuid as u8);
        buf.put_u16(UUID_SIZE as u16); // 2-byte big-endian length
        buf.put_slice(uuid_bytes);

        self.stream.write_all(&buf).await?;
        self.stream.flush().await?;

        debug!(uuid = uuid_str, "Sent AudioSocket UUID");
        Ok(())
    }

    /// Send an audio frame to the server.
    ///
    /// Mirrors `ast_audiosocket_send_frame()` for voice frames.
    pub async fn send_audio(
        &mut self,
        kind: AudioSocketKind,
        data: &[u8],
    ) -> AudioSocketResult<()> {
        if !self.alive {
            return Err(AudioSocketError::Closed);
        }

        let data_len = data.len();
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + data_len);
        buf.put_u8(kind as u8);
        buf.put_u16(data_len as u16);
        buf.put_slice(data);

        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Send a DTMF digit to the server.
    pub async fn send_dtmf(&mut self, digit: char) -> AudioSocketResult<()> {
        if !self.alive {
            return Err(AudioSocketError::Closed);
        }

        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 1);
        buf.put_u8(AudioSocketKind::Dtmf as u8);
        buf.put_u16(1u16);
        buf.put_u8(digit as u8);

        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Send a hangup signal to the server.
    pub async fn send_hangup(&mut self) -> AudioSocketResult<()> {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE);
        buf.put_u8(AudioSocketKind::Hangup as u8);
        buf.put_u16(0u16);

        let _ = self.stream.write_all(&buf).await;
        self.alive = false;
        Ok(())
    }

    /// Receive a message from the server.
    ///
    /// Mirrors `ast_audiosocket_receive_frame()` / `ast_audiosocket_receive_frame_with_hangup()`.
    ///
    /// Returns `Ok(None)` on clean disconnect or hangup signal.
    pub async fn receive(&mut self) -> AudioSocketResult<Option<AudioSocketMessage>> {
        if !self.alive {
            return Err(AudioSocketError::Closed);
        }

        // Read 3-byte header.
        let mut header = [0u8; HEADER_SIZE];
        match self.stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.alive = false;
                return Ok(None);
            }
            Err(e) => return Err(AudioSocketError::Io(e)),
        }

        let kind_byte = header[0];
        let length = u16::from_be_bytes([header[1], header[2]]) as usize;

        // Check for hangup.
        if kind_byte == AudioSocketKind::Hangup as u8 {
            self.alive = false;
            return Ok(None);
        }

        let kind = AudioSocketKind::from_byte(kind_byte).ok_or_else(|| {
            AudioSocketError::Protocol(format!("Unknown message type: 0x{:02x}", kind_byte))
        })?;

        if length == 0 {
            return Err(AudioSocketError::Protocol(
                "Zero-length payload for non-hangup message".into(),
            ));
        }

        // Read payload.
        let mut payload = vec![0u8; length];
        self.stream.read_exact(&mut payload).await?;

        Ok(Some(AudioSocketMessage {
            kind,
            payload: Bytes::from(payload),
        }))
    }

    /// Check if the connection is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive
    }

    /// Close the connection.
    pub async fn close(&mut self) {
        if self.alive {
            let _ = self.send_hangup().await;
        }
        let _ = self.stream.shutdown().await;
        self.alive = false;
    }
}

impl fmt::Debug for AudioSocketConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSocketConnection")
            .field("uuid", &self.uuid)
            .field("alive", &self.alive)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Utility: build raw messages
// ---------------------------------------------------------------------------

/// Build a raw AudioSocket message as bytes.
pub fn build_message(kind: AudioSocketKind, payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    buf.put_u8(kind as u8);
    buf.put_u16(payload.len() as u16);
    buf.put_slice(payload);
    buf.freeze()
}

/// Parse a raw AudioSocket header from bytes.
///
/// Returns `(kind_byte, payload_length)`.
pub fn parse_header(header: &[u8; HEADER_SIZE]) -> (u8, u16) {
    let kind = header[0];
    let length = u16::from_be_bytes([header[1], header[2]]);
    (kind, length)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_socket_kind_from_byte() {
        assert_eq!(AudioSocketKind::from_byte(0x00), Some(AudioSocketKind::Hangup));
        assert_eq!(AudioSocketKind::from_byte(0x01), Some(AudioSocketKind::Uuid));
        assert_eq!(AudioSocketKind::from_byte(0x03), Some(AudioSocketKind::Dtmf));
        assert_eq!(AudioSocketKind::from_byte(0x10), Some(AudioSocketKind::Audio));
        assert_eq!(AudioSocketKind::from_byte(0x11), Some(AudioSocketKind::AudioSlin12));
        assert_eq!(AudioSocketKind::from_byte(0x18), Some(AudioSocketKind::AudioSlin192));
        assert_eq!(AudioSocketKind::from_byte(0xFF), Some(AudioSocketKind::Error));
        assert_eq!(AudioSocketKind::from_byte(0x02), None);
        assert_eq!(AudioSocketKind::from_byte(0x50), None);
    }

    #[test]
    fn test_audio_socket_kind_is_audio() {
        assert!(AudioSocketKind::Audio.is_audio());
        assert!(AudioSocketKind::AudioSlin16.is_audio());
        assert!(AudioSocketKind::AudioSlin48.is_audio());
        assert!(!AudioSocketKind::Hangup.is_audio());
        assert!(!AudioSocketKind::Uuid.is_audio());
        assert!(!AudioSocketKind::Dtmf.is_audio());
    }

    #[test]
    fn test_audio_socket_kind_sample_rate() {
        assert_eq!(AudioSocketKind::Audio.sample_rate(), Some(8000));
        assert_eq!(AudioSocketKind::AudioSlin16.sample_rate(), Some(16000));
        assert_eq!(AudioSocketKind::AudioSlin48.sample_rate(), Some(48000));
        assert_eq!(AudioSocketKind::AudioSlin192.sample_rate(), Some(192000));
        assert_eq!(AudioSocketKind::Hangup.sample_rate(), None);
    }

    #[test]
    fn test_build_message() {
        let msg = build_message(AudioSocketKind::Audio, &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(msg.len(), 7); // 3 header + 4 payload
        assert_eq!(msg[0], 0x10); // Audio kind
        assert_eq!(msg[1], 0x00); // Length high byte
        assert_eq!(msg[2], 0x04); // Length low byte
        assert_eq!(&msg[3..], &[0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn test_build_uuid_message() {
        let uuid_bytes = [0u8; 16];
        let msg = build_message(AudioSocketKind::Uuid, &uuid_bytes);
        assert_eq!(msg.len(), 19); // 3 + 16
        assert_eq!(msg[0], 0x01);
        assert_eq!(msg[1], 0x00);
        assert_eq!(msg[2], 0x10); // 16
    }

    #[test]
    fn test_build_hangup_message() {
        let msg = build_message(AudioSocketKind::Hangup, &[]);
        assert_eq!(msg.len(), 3);
        assert_eq!(msg[0], 0x00);
        assert_eq!(msg[1], 0x00);
        assert_eq!(msg[2], 0x00);
    }

    #[test]
    fn test_parse_header() {
        let header = [0x10, 0x00, 0xA0]; // Audio, length 160
        let (kind, length) = parse_header(&header);
        assert_eq!(kind, 0x10);
        assert_eq!(length, 160);
    }

    #[test]
    fn test_audio_socket_message_dtmf_digit() {
        let msg = AudioSocketMessage {
            kind: AudioSocketKind::Dtmf,
            payload: Bytes::from_static(&[b'5']),
        };
        assert_eq!(msg.dtmf_digit(), Some('5'));

        let audio_msg = AudioSocketMessage {
            kind: AudioSocketKind::Audio,
            payload: Bytes::from_static(&[0; 320]),
        };
        assert_eq!(audio_msg.dtmf_digit(), None);
    }

    #[test]
    fn test_audio_socket_message_samples() {
        let msg = AudioSocketMessage {
            kind: AudioSocketKind::Audio,
            payload: Bytes::from(vec![0u8; 320]),
        };
        assert_eq!(msg.audio_samples(), 160); // 320 bytes / 2 bytes per sample
    }

    #[test]
    fn test_build_dtmf_message() {
        let msg = build_message(AudioSocketKind::Dtmf, &[b'#']);
        assert_eq!(msg.len(), 4); // 3 header + 1 payload
        assert_eq!(msg[0], 0x03); // DTMF kind
        assert_eq!(msg[3], b'#');
    }
}
