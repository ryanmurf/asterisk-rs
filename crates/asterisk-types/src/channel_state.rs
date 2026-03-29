use serde::{Deserialize, Serialize};

/// Channel states corresponding to `ast_channel_state` in channel.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ChannelState {
    /// Channel is down and available
    Down = 0,
    /// Channel is down, but reserved
    Reserved = 1,
    /// Channel is off hook
    OffHook = 2,
    /// Digits (or equivalent) have been dialed
    Dialing = 3,
    /// Line is ringing
    Ring = 4,
    /// Remote end is ringing
    Ringing = 5,
    /// Line is up
    Up = 6,
    /// Line is busy
    Busy = 7,
    /// Digits (or equivalent) have been dialed while offhook
    DialingOffHook = 8,
    /// Channel has detected an incoming call and is waiting for ring
    PreRing = 9,
    /// Mute (suppress outgoing voice)
    Mute = 10,
}

impl std::fmt::Display for ChannelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Down => write!(f, "Down"),
            Self::Reserved => write!(f, "Reserved"),
            Self::OffHook => write!(f, "OffHook"),
            Self::Dialing => write!(f, "Dialing"),
            Self::Ring => write!(f, "Ring"),
            Self::Ringing => write!(f, "Ringing"),
            Self::Up => write!(f, "Up"),
            Self::Busy => write!(f, "Busy"),
            Self::DialingOffHook => write!(f, "DialingOffHook"),
            Self::PreRing => write!(f, "PreRing"),
            Self::Mute => write!(f, "Mute"),
        }
    }
}
