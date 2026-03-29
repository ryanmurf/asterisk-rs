//! Frame utilities -- re-exports from asterisk-types with helpers.

pub use asterisk_types::control::ControlFrame;
pub use asterisk_types::frame::Frame;
pub use asterisk_types::frame_type::FrameType;

use bytes::Bytes;

/// Create a silence frame for a codec.
pub fn silence_frame(codec_id: u32, samples: u32, is_slin: bool) -> Frame {
    let byte_count = if is_slin {
        (samples * 2) as usize
    } else {
        samples as usize
    };
    let fill_byte = if is_slin { 0x00u8 } else { 0xFFu8 };
    let data = Bytes::from(vec![fill_byte; byte_count]);
    Frame::voice(codec_id, samples, data)
}

/// Create a hangup control frame.
pub fn hangup_frame() -> Frame {
    Frame::control(ControlFrame::Hangup)
}

/// Create a ringing control frame.
pub fn ringing_frame() -> Frame {
    Frame::control(ControlFrame::Ringing)
}

/// Create an answer control frame.
pub fn answer_frame() -> Frame {
    Frame::control(ControlFrame::Answer)
}

/// Create a busy control frame.
pub fn busy_frame() -> Frame {
    Frame::control(ControlFrame::Busy)
}

/// Create a congestion control frame.
pub fn congestion_frame() -> Frame {
    Frame::control(ControlFrame::Congestion)
}

/// Create a hold control frame.
pub fn hold_frame() -> Frame {
    Frame::control(ControlFrame::Hold)
}

/// Create an unhold control frame.
pub fn unhold_frame() -> Frame {
    Frame::control(ControlFrame::Unhold)
}

/// Create a progress control frame.
pub fn progress_frame() -> Frame {
    Frame::control(ControlFrame::Progress)
}

/// Check if a character is a valid DTMF digit.
pub fn is_valid_dtmf(digit: char) -> bool {
    matches!(digit, '0'..='9' | '*' | '#' | 'A'..='D' | 'a'..='d')
}
