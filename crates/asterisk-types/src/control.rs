use serde::{Deserialize, Serialize};

/// Control frame subtypes corresponding to `ast_control_frame_type` in frame.h.
///
/// These are signalling indications sent between channels and devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum ControlFrame {
    /// Other end has hung up
    Hangup = 1,
    /// Local ring
    Ring = 2,
    /// Remote end is ringing
    Ringing = 3,
    /// Remote end has answered
    Answer = 4,
    /// Remote end is busy
    Busy = 5,
    /// Make it go off hook
    TakeOffHook = 6,
    /// Line is off hook
    OffHook = 7,
    /// Congestion (circuits busy)
    Congestion = 8,
    /// Flash hook
    Flash = 9,
    /// Wink
    Wink = 10,
    /// Set a low-level option
    Option = 11,
    /// Key radio
    RadioKey = 12,
    /// Un-key radio
    RadioUnkey = 13,
    /// Indicate call progress
    Progress = 14,
    /// Indicate call proceeding
    Proceeding = 15,
    /// Call is placed on hold
    Hold = 16,
    /// Call is back from hold
    Unhold = 17,
    /// Video update requested
    VidUpdate = 18,
    /// Source of media has changed (RTP marker bit must change)
    SrcUpdate = 20,
    /// Indicate status of a transfer request
    Transfer = 21,
    /// Connected line has changed
    ConnectedLine = 22,
    /// Redirecting information has changed
    Redirecting = 23,
    /// T.38 state change request/notification with parameters
    T38Parameters = 24,
    /// Call completion service is possible
    Cc = 25,
    /// Media source has changed (RTP marker bit and SSRC must change)
    SrcChange = 26,
    /// Tell ast_read to take a specific action
    ReadAction = 27,
    /// Advice of charge
    Aoc = 28,
    /// End of channel queue for softhangup
    EndOfQ = 29,
    /// Extension dialed is incomplete
    Incomplete = 30,
    /// Caller is being malicious (MCID)
    Mcid = 31,
    /// Interrupt bridge to update peer
    UpdateRtpPeer = 32,
    /// Protocol-specific cause code update
    PvtCauseCode = 33,
    /// Masquerade is about to begin/end
    MasqueradeNotify = 34,
    /// Stream topology change requested
    StreamTopologyRequestChange = 35,
    /// Stream topology has changed
    StreamTopologyChanged = 36,
    /// Stream topology source changed
    StreamTopologySourceChanged = 37,

    // Stream playback control (values > 1000 to avoid DTMF conflicts)
    /// Stop stream playback
    StreamStop = 1000,
    /// Suspend stream playback
    StreamSuspend = 1001,
    /// Restart stream playback
    StreamRestart = 1002,
    /// Rewind stream playback
    StreamReverse = 1003,
    /// Fast forward stream playback
    StreamForward = 1004,
    /// Playback of audio file should begin
    PlaybackBegin = 1005,

    // Recording control
    /// Cancel recording and discard file
    RecordCancel = 1100,
    /// Stop recording
    RecordStop = 1101,
    /// Suspend/unsuspend recording
    RecordSuspend = 1102,
    /// Mute/unmute recording (write silence)
    RecordMute = 1103,
}

impl std::fmt::Display for ControlFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hangup => write!(f, "Hangup"),
            Self::Ring => write!(f, "Ring"),
            Self::Ringing => write!(f, "Ringing"),
            Self::Answer => write!(f, "Answer"),
            Self::Busy => write!(f, "Busy"),
            Self::TakeOffHook => write!(f, "TakeOffHook"),
            Self::OffHook => write!(f, "OffHook"),
            Self::Congestion => write!(f, "Congestion"),
            Self::Flash => write!(f, "Flash"),
            Self::Wink => write!(f, "Wink"),
            Self::Option => write!(f, "Option"),
            Self::RadioKey => write!(f, "RadioKey"),
            Self::RadioUnkey => write!(f, "RadioUnkey"),
            Self::Progress => write!(f, "Progress"),
            Self::Proceeding => write!(f, "Proceeding"),
            Self::Hold => write!(f, "Hold"),
            Self::Unhold => write!(f, "Unhold"),
            Self::VidUpdate => write!(f, "VidUpdate"),
            Self::SrcUpdate => write!(f, "SrcUpdate"),
            Self::Transfer => write!(f, "Transfer"),
            Self::ConnectedLine => write!(f, "ConnectedLine"),
            Self::Redirecting => write!(f, "Redirecting"),
            Self::T38Parameters => write!(f, "T38Parameters"),
            Self::Cc => write!(f, "CC"),
            Self::SrcChange => write!(f, "SrcChange"),
            Self::ReadAction => write!(f, "ReadAction"),
            Self::Aoc => write!(f, "AOC"),
            Self::EndOfQ => write!(f, "EndOfQ"),
            Self::Incomplete => write!(f, "Incomplete"),
            Self::Mcid => write!(f, "MCID"),
            Self::UpdateRtpPeer => write!(f, "UpdateRtpPeer"),
            Self::PvtCauseCode => write!(f, "PvtCauseCode"),
            Self::MasqueradeNotify => write!(f, "MasqueradeNotify"),
            Self::StreamTopologyRequestChange => write!(f, "StreamTopologyRequestChange"),
            Self::StreamTopologyChanged => write!(f, "StreamTopologyChanged"),
            Self::StreamTopologySourceChanged => write!(f, "StreamTopologySourceChanged"),
            Self::StreamStop => write!(f, "StreamStop"),
            Self::StreamSuspend => write!(f, "StreamSuspend"),
            Self::StreamRestart => write!(f, "StreamRestart"),
            Self::StreamReverse => write!(f, "StreamReverse"),
            Self::StreamForward => write!(f, "StreamForward"),
            Self::PlaybackBegin => write!(f, "PlaybackBegin"),
            Self::RecordCancel => write!(f, "RecordCancel"),
            Self::RecordStop => write!(f, "RecordStop"),
            Self::RecordSuspend => write!(f, "RecordSuspend"),
            Self::RecordMute => write!(f, "RecordMute"),
        }
    }
}
