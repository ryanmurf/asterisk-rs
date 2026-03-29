use serde::{Deserialize, Serialize};

/// Hangup causes corresponding to `ast_cause` defines in causes.h.
/// Based on Q.931 cause codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u32)]
pub enum HangupCause {
    #[default]
    NotDefined = 0,
    UnallocatedNumber = 1,
    NoRouteTransitNet = 2,
    NoRouteDestination = 3,
    ChannelUnacceptable = 6,
    NormalClearing = 16,
    UserBusy = 17,
    NoUserResponse = 18,
    NoAnswer = 19,
    SubscriberAbsent = 20,
    CallRejected = 21,
    NumberChanged = 22,
    DestinationOutOfOrder = 27,
    InvalidNumberFormat = 28,
    NormalUnspecified = 31,
    NormalCircuitCongestion = 34,
    NetworkOutOfOrder = 38,
    NormalTemporaryFailure = 41,
    SwitchCongestion = 42,
    BearerCapNotAvail = 58,
    BearerCapNotImplemented = 65,
    FacilityNotImplemented = 69,
    Interworking = 127,
    Failure = 200,
    NoSuchDriver = 201,
}

impl std::fmt::Display for HangupCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
