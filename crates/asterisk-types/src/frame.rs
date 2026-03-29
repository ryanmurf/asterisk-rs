use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::control::ControlFrame;
use crate::frame_type::FrameType;

/// A media or signalling frame -- the fundamental unit of data transport in Asterisk.
///
/// This is a Rust enum representation that captures the different frame variants
/// with their associated data, rather than the C union-based `struct ast_frame`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frame {
    /// Voice data with codec ID, sample count, and audio payload
    Voice {
        codec_id: u32,
        samples: u32,
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
        timestamp_ms: u64,
        seqno: i32,
        stream_num: i32,
    },
    /// Video frame with codec ID and video payload
    Video {
        codec_id: u32,
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
        timestamp_ms: u64,
        seqno: i32,
        frame_ending: bool,
        stream_num: i32,
    },
    /// DTMF digit begin event
    DtmfBegin {
        digit: char,
    },
    /// DTMF digit end event
    DtmfEnd {
        digit: char,
        duration_ms: u32,
    },
    /// Control/signalling frame
    Control {
        control: ControlFrame,
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
    /// Null (empty) frame -- used for timing/keepalive
    Null,
    /// Text message
    Text {
        text: String,
    },
    /// Image data
    Image {
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
    /// HTML content
    Html {
        subclass: u32,
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
    /// Comfort noise generation
    Cng {
        level: i32,
    },
    /// Modem-over-IP (T.38, V.150)
    Modem {
        subclass: u32,
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
    /// RTCP feedback
    Rtcp {
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
    /// Bridge action (internal)
    BridgeAction {
        #[serde(with = "serde_bytes_compat")]
        data: Bytes,
    },
}

impl Frame {
    /// Return the FrameType discriminant for this frame.
    pub fn frame_type(&self) -> FrameType {
        match self {
            Frame::Voice { .. } => FrameType::Voice,
            Frame::Video { .. } => FrameType::Video,
            Frame::DtmfBegin { .. } => FrameType::DtmfBegin,
            Frame::DtmfEnd { .. } => FrameType::DtmfEnd,
            Frame::Control { .. } => FrameType::Control,
            Frame::Null => FrameType::Null,
            Frame::Text { .. } => FrameType::Text,
            Frame::Image { .. } => FrameType::Image,
            Frame::Html { .. } => FrameType::Html,
            Frame::Cng { .. } => FrameType::Cng,
            Frame::Modem { .. } => FrameType::Modem,
            Frame::Rtcp { .. } => FrameType::Rtcp,
            Frame::BridgeAction { .. } => FrameType::BridgeAction,
        }
    }

    /// Create a voice frame with audio data.
    pub fn voice(codec_id: u32, samples: u32, data: Bytes) -> Self {
        Frame::Voice {
            codec_id,
            samples,
            data,
            timestamp_ms: 0,
            seqno: -1,
            stream_num: 0,
        }
    }

    /// Create a DTMF begin frame.
    pub fn dtmf_begin(digit: char) -> Self {
        Frame::DtmfBegin { digit }
    }

    /// Create a DTMF end frame.
    pub fn dtmf_end(digit: char, duration_ms: u32) -> Self {
        Frame::DtmfEnd { digit, duration_ms }
    }

    /// Create a control frame.
    pub fn control(control: ControlFrame) -> Self {
        Frame::Control {
            control,
            data: Bytes::new(),
        }
    }

    /// Create a control frame with associated data.
    pub fn control_with_data(control: ControlFrame, data: Bytes) -> Self {
        Frame::Control { control, data }
    }

    /// Create a null frame.
    pub fn null() -> Self {
        Frame::Null
    }

    /// Create a text frame.
    pub fn text(text: String) -> Self {
        Frame::Text { text }
    }

    /// Returns true if this is a voice frame.
    pub fn is_voice(&self) -> bool {
        matches!(self, Frame::Voice { .. })
    }

    /// Returns true if this is a video frame.
    pub fn is_video(&self) -> bool {
        matches!(self, Frame::Video { .. })
    }

    /// Returns true if this is a control frame.
    pub fn is_control(&self) -> bool {
        matches!(self, Frame::Control { .. })
    }

    /// Returns true if this is a DTMF frame (begin or end).
    pub fn is_dtmf(&self) -> bool {
        matches!(self, Frame::DtmfBegin { .. } | Frame::DtmfEnd { .. })
    }
}

/// Serde compatibility for Bytes
mod serde_bytes_compat {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error> {
        let vec: Vec<u8> = bytes.to_vec();
        vec.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Bytes, D::Error> {
        let vec = Vec::<u8>::deserialize(deserializer)?;
        Ok(Bytes::from(vec))
    }
}
