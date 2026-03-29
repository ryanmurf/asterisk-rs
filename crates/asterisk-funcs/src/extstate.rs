//! Extension state, device state, and presence state functions.
//!
//! Port of func_extstate.c, func_devstate.c, func_presencestate.c from Asterisk C.
//!
//! Provides:
//! - EXTENSION_STATE(exten@context) - return device/extension state
//! - DEVICE_STATE(device) - return specific device state
//! - PRESENCE_STATE(provider) - return presence state

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Extension / device state values.
///
/// Mirrors `enum ast_extension_states` from pbx.h and
/// `enum ast_device_state` from devicestate.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Device is not in use
    NotInuse,
    /// Device is in use
    Inuse,
    /// Device is busy
    Busy,
    /// Device is unavailable
    Unavailable,
    /// Device is ringing
    Ringing,
    /// Device is ringing while in use
    RingInuse,
    /// Device is on hold
    OnHold,
    /// State is unknown
    Unknown,
    /// Device is invalid
    Invalid,
}

impl DeviceState {
    /// Convert state to its canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceState::NotInuse => "NOT_INUSE",
            DeviceState::Inuse => "INUSE",
            DeviceState::Busy => "BUSY",
            DeviceState::Unavailable => "UNAVAILABLE",
            DeviceState::Ringing => "RINGING",
            DeviceState::RingInuse => "RINGINUSE",
            DeviceState::OnHold => "ONHOLD",
            DeviceState::Unknown => "UNKNOWN",
            DeviceState::Invalid => "INVALID",
        }
    }

    /// Parse a device state from a string.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "NOT_INUSE" | "IDLE" => DeviceState::NotInuse,
            "INUSE" => DeviceState::Inuse,
            "BUSY" => DeviceState::Busy,
            "UNAVAILABLE" | "UNREACHABLE" => DeviceState::Unavailable,
            "RINGING" => DeviceState::Ringing,
            "RINGINUSE" => DeviceState::RingInuse,
            "ONHOLD" => DeviceState::OnHold,
            "INVALID" => DeviceState::Invalid,
            _ => DeviceState::Unknown,
        }
    }

    /// Convert state to a numeric value (matches Asterisk's enum values).
    pub fn as_numeric(&self) -> i32 {
        match self {
            DeviceState::Unknown => -1,
            DeviceState::NotInuse => 0,
            DeviceState::Inuse => 1,
            DeviceState::Busy => 2,
            DeviceState::Invalid => 4,
            DeviceState::Unavailable => 5,
            DeviceState::Ringing => 8,
            DeviceState::RingInuse => 9,
            DeviceState::OnHold => 16,
        }
    }
}

/// Presence state values.
///
/// Mirrors `enum ast_presence_state` from presencestate.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceState {
    /// Not set
    NotSet,
    /// Available
    Available,
    /// Unavailable
    Unavailable,
    /// Chat available
    Chat,
    /// Away
    Away,
    /// Extended away
    Xa,
    /// Do not disturb
    Dnd,
}

impl PresenceState {
    /// Convert state to its canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            PresenceState::NotSet => "not_set",
            PresenceState::Available => "available",
            PresenceState::Unavailable => "unavailable",
            PresenceState::Chat => "chat",
            PresenceState::Away => "away",
            PresenceState::Xa => "xa",
            PresenceState::Dnd => "dnd",
        }
    }

    /// Parse a presence state from a string.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "available" => PresenceState::Available,
            "unavailable" => PresenceState::Unavailable,
            "chat" => PresenceState::Chat,
            "away" => PresenceState::Away,
            "xa" => PresenceState::Xa,
            "dnd" => PresenceState::Dnd,
            _ => PresenceState::NotSet,
        }
    }
}

/// EXTENSION_STATE() function.
///
/// Returns the state of an extension/hint as a string.
///
/// Usage: EXTENSION_STATE(exten[@context])
///
/// Returns one of: UNKNOWN, NOT_INUSE, INUSE, BUSY, UNAVAILABLE,
/// RINGING, RINGINUSE, ONHOLD.
pub struct FuncExtensionState;

impl DialplanFunc for FuncExtensionState {
    fn name(&self) -> &str {
        "EXTENSION_STATE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let args = args.trim();
        if args.is_empty() {
            return Err(FuncError::InvalidArgument(
                "EXTENSION_STATE: extension argument is required".to_string(),
            ));
        }

        // Parse exten@context
        let (exten, context) = if let Some(at_pos) = args.find('@') {
            let e = args[..at_pos].trim();
            let c = args[at_pos + 1..].trim();
            (
                e.to_string(),
                if c.is_empty() {
                    ctx.context.clone()
                } else {
                    c.to_string()
                },
            )
        } else {
            (args.to_string(), ctx.context.clone())
        };

        // Look up state from channel variable store
        // In a full implementation, this queries the hint/device state subsystem
        let key = format!("__EXTSTATE_{}_{}", context, exten);
        let state = ctx
            .get_variable(&key)
            .map(|v| DeviceState::from_str(v))
            .unwrap_or(DeviceState::Unknown);

        Ok(state.as_str().to_string())
    }
}

/// DEVICE_STATE() function.
///
/// Returns or sets the state of a device.
///
/// Read usage:  DEVICE_STATE(device) - returns device state string
/// Write usage: Set(DEVICE_STATE(Custom:mydev)=INUSE) - set custom device state
pub struct FuncDeviceState;

impl DialplanFunc for FuncDeviceState {
    fn name(&self) -> &str {
        "DEVICE_STATE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let device = args.trim();
        if device.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DEVICE_STATE: device argument is required".to_string(),
            ));
        }

        let key = format!("__DEVSTATE_{}", device);
        let state = ctx
            .get_variable(&key)
            .map(|v| DeviceState::from_str(v))
            .unwrap_or(DeviceState::Unknown);

        Ok(state.as_str().to_string())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let device = args.trim();
        if device.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DEVICE_STATE: device argument is required".to_string(),
            ));
        }

        // Only Custom: devices can be set
        if !device.starts_with("Custom:") {
            return Err(FuncError::InvalidArgument(format!(
                "DEVICE_STATE: can only set state on Custom: devices, not '{}'",
                device
            )));
        }

        let state = DeviceState::from_str(value);
        let key = format!("__DEVSTATE_{}", device);
        ctx.set_variable(&key, state.as_str());

        Ok(())
    }
}

/// PRESENCE_STATE() function.
///
/// Returns the presence state for a provider.
///
/// Usage: PRESENCE_STATE(provider[,field])
///
/// Fields: status, subtype, message
/// Without a field, returns the status string.
pub struct FuncPresenceState;

impl DialplanFunc for FuncPresenceState {
    fn name(&self) -> &str {
        "PRESENCE_STATE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let provider = parts[0].trim();
        let field = parts.get(1).map(|s| s.trim()).unwrap_or("status");

        if provider.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PRESENCE_STATE: provider argument is required".to_string(),
            ));
        }

        match field {
            "status" => {
                let key = format!("__PRESENCE_{}", provider);
                let state = ctx
                    .get_variable(&key)
                    .map(|v| PresenceState::from_str(v))
                    .unwrap_or(PresenceState::NotSet);
                Ok(state.as_str().to_string())
            }
            "subtype" => {
                let key = format!("__PRESENCE_{}_subtype", provider);
                Ok(ctx
                    .get_variable(&key)
                    .cloned()
                    .unwrap_or_default())
            }
            "message" => {
                let key = format!("__PRESENCE_{}_message", provider);
                Ok(ctx
                    .get_variable(&key)
                    .cloned()
                    .unwrap_or_default())
            }
            other => Err(FuncError::InvalidArgument(format!(
                "PRESENCE_STATE: unknown field '{}', expected status|subtype|message",
                other
            ))),
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let provider = parts[0].trim();

        if provider.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PRESENCE_STATE: provider argument is required".to_string(),
            ));
        }

        // Value format: state[,subtype[,message]]
        let value_parts: Vec<&str> = value.splitn(3, ',').collect();
        let state_str = value_parts[0].trim();
        let subtype = value_parts.get(1).map(|s| s.trim()).unwrap_or("");
        let message = value_parts.get(2).map(|s| s.trim()).unwrap_or("");

        let state = PresenceState::from_str(state_str);
        let key = format!("__PRESENCE_{}", provider);
        ctx.set_variable(&key, state.as_str());

        if !subtype.is_empty() {
            let key = format!("__PRESENCE_{}_subtype", provider);
            ctx.set_variable(&key, subtype);
        }
        if !message.is_empty() {
            let key = format!("__PRESENCE_{}_message", provider);
            ctx.set_variable(&key, message);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_state_values() {
        assert_eq!(DeviceState::from_str("NOT_INUSE").as_str(), "NOT_INUSE");
        assert_eq!(DeviceState::from_str("INUSE").as_str(), "INUSE");
        assert_eq!(DeviceState::from_str("BUSY").as_str(), "BUSY");
        assert_eq!(DeviceState::from_str("garbage").as_str(), "UNKNOWN");
    }

    #[test]
    fn test_extension_state_read() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__EXTSTATE_default_100", "INUSE");
        let func = FuncExtensionState;
        assert_eq!(func.read(&ctx, "100@default").unwrap(), "INUSE");
    }

    #[test]
    fn test_extension_state_unknown() {
        let ctx = FuncContext::new();
        let func = FuncExtensionState;
        assert_eq!(func.read(&ctx, "999@default").unwrap(), "UNKNOWN");
    }

    #[test]
    fn test_device_state_custom() {
        let mut ctx = FuncContext::new();
        let func = FuncDeviceState;
        func.write(&mut ctx, "Custom:mydev", "INUSE").unwrap();
        assert_eq!(func.read(&ctx, "Custom:mydev").unwrap(), "INUSE");
    }

    #[test]
    fn test_device_state_non_custom_write() {
        let mut ctx = FuncContext::new();
        let func = FuncDeviceState;
        assert!(func.write(&mut ctx, "SIP/100", "INUSE").is_err());
    }

    #[test]
    fn test_presence_state() {
        let mut ctx = FuncContext::new();
        let func = FuncPresenceState;
        func.write(&mut ctx, "CustomPresence:bob", "away,mobile,On the road")
            .unwrap();
        assert_eq!(
            func.read(&ctx, "CustomPresence:bob,status").unwrap(),
            "away"
        );
        assert_eq!(
            func.read(&ctx, "CustomPresence:bob,subtype").unwrap(),
            "mobile"
        );
        assert_eq!(
            func.read(&ctx, "CustomPresence:bob,message").unwrap(),
            "On the road"
        );
    }
}
