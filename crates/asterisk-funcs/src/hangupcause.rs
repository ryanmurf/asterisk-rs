//! HANGUPCAUSE() and HANGUPCAUSE_KEYS() functions.
//!
//! Port of func_hangupcause.c from Asterisk C.
//!
//! Provides per-technology hangup cause information:
//! - HANGUPCAUSE_KEYS() - Returns comma-separated list of channels with tech causes
//! - HANGUPCAUSE(channel,field) - Returns specific hangup cause info for a channel
//!
//! Fields: tech (technology name), ast (Asterisk cause code), desc (description)

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Standard Asterisk hangup cause codes (from causes.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HangupCause {
    NotDefined = 0,
    UnallocatedNumber = 1,
    NoRouteTransit = 2,
    NoRouteDestination = 3,
    ChannelUnacceptable = 6,
    CallAwardedDelivered = 7,
    NormalClearing = 16,
    UserBusy = 17,
    NoUserResponse = 18,
    NoAnswer = 19,
    SubscriberAbsent = 20,
    CallRejected = 21,
    NumberChanged = 22,
    DestOutOfOrder = 27,
    InvalidNumberFormat = 28,
    FacilityRejected = 29,
    NormalUnspecified = 31,
    NormalCircuitCongestion = 34,
    NetworkOutOfOrder = 38,
    NormalTemporaryFailure = 41,
    SwitchCongestion = 42,
    RequestedChanUnavail = 44,
    PreEmpted = 45,
    FacilityNotSubscribed = 50,
    OutgoingCallBarred = 52,
    IncomingCallBarred = 54,
    BearerCapNotAvail = 58,
    BearerCapNotImpl = 65,
    ChanNotImplemented = 66,
    FacilityNotImplemented = 69,
    InvalidCallReference = 81,
    IncompatibleDestination = 88,
    InvalidMsgUnspecified = 95,
    MandatoryIeMissing = 96,
    MessageTypeNonexist = 97,
    WrongMessage = 98,
    IeNonexist = 99,
    InvalidIeContents = 100,
    WrongCallState = 101,
    RecoveryOnTimerExpire = 102,
    MandatoryIeLengthError = 103,
    ProtocolError = 111,
    Interworking = 127,
}

impl HangupCause {
    pub fn description(&self) -> &'static str {
        match self {
            Self::NotDefined => "Not Defined",
            Self::UnallocatedNumber => "Unallocated Number",
            Self::NoRouteTransit => "No Route to Transit Network",
            Self::NoRouteDestination => "No Route to Destination",
            Self::NormalClearing => "Normal Clearing",
            Self::UserBusy => "User Busy",
            Self::NoUserResponse => "No User Response",
            Self::NoAnswer => "No Answer",
            Self::CallRejected => "Call Rejected",
            Self::NumberChanged => "Number Changed",
            Self::DestOutOfOrder => "Destination Out of Order",
            Self::NormalUnspecified => "Normal, Unspecified",
            Self::NormalCircuitCongestion => "Circuit/Channel Congestion",
            Self::NetworkOutOfOrder => "Network Out of Order",
            Self::NormalTemporaryFailure => "Normal Temporary Failure",
            Self::SwitchCongestion => "Switch Congestion",
            Self::Interworking => "Interworking",
            _ => "Other Cause",
        }
    }

    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::NotDefined,
            1 => Self::UnallocatedNumber,
            16 => Self::NormalClearing,
            17 => Self::UserBusy,
            18 => Self::NoUserResponse,
            19 => Self::NoAnswer,
            21 => Self::CallRejected,
            27 => Self::DestOutOfOrder,
            31 => Self::NormalUnspecified,
            34 => Self::NormalCircuitCongestion,
            38 => Self::NetworkOutOfOrder,
            _ => Self::NotDefined,
        }
    }
}

/// HANGUPCAUSE_KEYS() function.
///
/// Returns comma-separated list of channel names with hangup cause data.
pub struct FuncHangupCauseKeys;

impl DialplanFunc for FuncHangupCauseKeys {
    fn name(&self) -> &str {
        "HANGUPCAUSE_KEYS"
    }

    fn read(&self, ctx: &FuncContext, _args: &str) -> FuncResult {
        Ok(ctx
            .get_variable("__HANGUPCAUSE_KEYS")
            .cloned()
            .unwrap_or_default())
    }
}

/// HANGUPCAUSE() function.
///
/// Usage: HANGUPCAUSE(channel,field)
/// Fields: tech, ast, desc
pub struct FuncHangupCause;

impl DialplanFunc for FuncHangupCause {
    fn name(&self) -> &str {
        "HANGUPCAUSE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "HANGUPCAUSE: requires channel,field arguments".to_string(),
            ));
        }

        let channel = parts[0].trim();
        let field = parts[1].trim().to_lowercase();

        let var_key = format!("__HANGUPCAUSE_{}_{}", channel, field.to_uppercase());
        Ok(ctx.get_variable(&var_key).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hangupcause_keys_empty() {
        let ctx = FuncContext::new();
        assert_eq!(FuncHangupCauseKeys.read(&ctx, "").unwrap(), "");
    }

    #[test]
    fn test_hangupcause_keys_set() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__HANGUPCAUSE_KEYS", "SIP/alice,SIP/bob");
        assert_eq!(
            FuncHangupCauseKeys.read(&ctx, "").unwrap(),
            "SIP/alice,SIP/bob"
        );
    }

    #[test]
    fn test_hangupcause_read() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__HANGUPCAUSE_SIP/alice_TECH", "SIP");
        ctx.set_variable("__HANGUPCAUSE_SIP/alice_AST", "16");
        let func = FuncHangupCause;
        assert_eq!(func.read(&ctx, "SIP/alice,tech").unwrap(), "SIP");
        assert_eq!(func.read(&ctx, "SIP/alice,ast").unwrap(), "16");
    }

    #[test]
    fn test_hangupcause_missing_args() {
        let ctx = FuncContext::new();
        assert!(FuncHangupCause.read(&ctx, "just_channel").is_err());
    }

    #[test]
    fn test_hangup_cause_description() {
        assert_eq!(HangupCause::NormalClearing.description(), "Normal Clearing");
        assert_eq!(HangupCause::UserBusy.description(), "User Busy");
    }
}
