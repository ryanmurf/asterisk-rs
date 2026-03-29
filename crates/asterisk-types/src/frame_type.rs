use serde::{Deserialize, Serialize};

/// Frame types corresponding to `ast_frame_type` in frame.h.
///
/// These values are wire-compatible with IAX2 and must not be reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum FrameType {
    /// DTMF end event, subclass is the digit
    DtmfEnd = 1,
    /// Voice data, subclass is codec format
    Voice = 2,
    /// Video frame
    Video = 3,
    /// Control frame, subclass is ControlFrame variant
    Control = 4,
    /// Empty, useless frame
    Null = 5,
    /// Inter Asterisk Exchange private frame type
    Iax = 6,
    /// Text messages
    Text = 7,
    /// Image frames
    Image = 8,
    /// HTML frame
    Html = 9,
    /// Comfort Noise Generation
    Cng = 10,
    /// Modem-over-IP data streams (T.38, V.150)
    Modem = 11,
    /// DTMF begin event, subclass is the digit
    DtmfBegin = 12,
    /// Internal bridge module action
    BridgeAction = 13,
    /// Internal synchronous bridge module action
    BridgeActionSync = 14,
    /// RTCP feedback
    Rtcp = 15,
    /// Text message in structured data
    TextData = 16,
}

impl std::fmt::Display for FrameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DtmfEnd => write!(f, "DTMF End"),
            Self::Voice => write!(f, "Voice"),
            Self::Video => write!(f, "Video"),
            Self::Control => write!(f, "Control"),
            Self::Null => write!(f, "Null"),
            Self::Iax => write!(f, "IAX"),
            Self::Text => write!(f, "Text"),
            Self::Image => write!(f, "Image"),
            Self::Html => write!(f, "HTML"),
            Self::Cng => write!(f, "CNG"),
            Self::Modem => write!(f, "Modem"),
            Self::DtmfBegin => write!(f, "DTMF Begin"),
            Self::BridgeAction => write!(f, "Bridge Action"),
            Self::BridgeActionSync => write!(f, "Bridge Action Sync"),
            Self::Rtcp => write!(f, "RTCP"),
            Self::TextData => write!(f, "Text Data"),
        }
    }
}
