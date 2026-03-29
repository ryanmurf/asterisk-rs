//! Hangup application - hangs up the current channel.
//!
//! Port of Hangup() from Asterisk C. Sets the hangup cause and
//! terminates the channel.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::HangupCause;
use tracing::{debug, warn};

/// The Hangup() dialplan application.
///
/// Unconditionally hangs up the channel. An optional argument can specify
/// the hangup cause code (Q.850 value).
///
/// Usage in dialplan: Hangup([cause])
///   cause - optional Q.850/Q.931 hangup cause code
pub struct AppHangup;

impl DialplanApp for AppHangup {
    fn name(&self) -> &str {
        "Hangup"
    }

    fn description(&self) -> &str {
        "Hangup the calling channel"
    }
}

impl AppHangup {
    /// Execute the Hangup application on a channel.
    ///
    /// # Arguments
    /// * `channel` - The channel to hang up
    /// * `args` - Optional hangup cause code (Q.850 integer value)
    pub fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let cause = if args.is_empty() {
            HangupCause::NormalClearing
        } else {
            match args.trim().parse::<u32>() {
                Ok(code) => hangup_cause_from_code(code),
                Err(_) => {
                    warn!(
                        "Hangup: invalid cause code '{}', using NormalClearing",
                        args
                    );
                    HangupCause::NormalClearing
                }
            }
        };

        debug!(
            "Hangup: hanging up channel '{}' with cause {:?} ({})",
            channel.name,
            cause,
            cause as u32
        );

        // Set the hangup cause and request a soft hangup.
        // The actual hangup (state transition + AMI event) is performed by
        // pbx_run after it detects the softhangup flag, so we must NOT set
        // state = Down here -- that would cause pbx_run's final
        // channel.hangup() call to be a no-op and suppress the Hangup event.
        //
        // Return Success so that pbx_run's check_hangup() detects the
        // softhangup flag and breaks out cleanly (mirroring real Asterisk
        // behavior where Hangup returns AST_PBX_KEEPALIVE).
        channel.hangup_cause = cause;
        channel.softhangup(asterisk_core::channel::softhangup::AST_SOFTHANGUP_EXPLICIT);

        PbxExecResult::Success
    }
}

/// Convert a Q.850 cause code integer to a HangupCause enum value.
fn hangup_cause_from_code(code: u32) -> HangupCause {
    match code {
        0 => HangupCause::NotDefined,
        1 => HangupCause::UnallocatedNumber,
        2 => HangupCause::NoRouteTransitNet,
        3 => HangupCause::NoRouteDestination,
        6 => HangupCause::ChannelUnacceptable,
        16 => HangupCause::NormalClearing,
        17 => HangupCause::UserBusy,
        18 => HangupCause::NoUserResponse,
        19 => HangupCause::NoAnswer,
        20 => HangupCause::SubscriberAbsent,
        21 => HangupCause::CallRejected,
        22 => HangupCause::NumberChanged,
        27 => HangupCause::DestinationOutOfOrder,
        28 => HangupCause::InvalidNumberFormat,
        31 => HangupCause::NormalUnspecified,
        34 => HangupCause::NormalCircuitCongestion,
        38 => HangupCause::NetworkOutOfOrder,
        41 => HangupCause::NormalTemporaryFailure,
        42 => HangupCause::SwitchCongestion,
        127 => HangupCause::Interworking,
        _ => HangupCause::NormalClearing,
    }
}
