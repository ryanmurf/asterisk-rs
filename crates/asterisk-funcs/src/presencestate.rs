//! Extended presence state functions.
//!
//! Extensions to func_presencestate.c providing:
//! - PRESENCE_STATE(provider) - Read/write presence state
//! - PRESENCE_STATE_EXT(provider,field) - Extended presence fields

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Presence states (RFC 3863 / PIDF basic status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceState {
    NotSet,
    Available,
    Unavailable,
    Chat,
    Away,
    Xa,
    Dnd,
}

impl PresenceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotSet => "not_set",
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Chat => "chat",
            Self::Away => "away",
            Self::Xa => "xa",
            Self::Dnd => "dnd",
        }
    }

    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "available" | "open" => Self::Available,
            "unavailable" | "closed" => Self::Unavailable,
            "chat" => Self::Chat,
            "away" => Self::Away,
            "xa" | "extended_away" => Self::Xa,
            "dnd" | "do_not_disturb" => Self::Dnd,
            _ => Self::NotSet,
        }
    }
}

/// PRESENCE_STATE() function.
///
/// Usage:
///   ${PRESENCE_STATE(CustomPresence:alice)} - Read presence state
///   Set(PRESENCE_STATE(CustomPresence:alice)=available,message) - Set state
pub struct FuncPresenceState;

impl DialplanFunc for FuncPresenceState {
    fn name(&self) -> &str {
        "PRESENCE_STATE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let provider = args.trim();
        if provider.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PRESENCE_STATE: provider is required".to_string(),
            ));
        }
        let key = format!("__PRESENCE_{}", provider);
        Ok(ctx
            .get_variable(&key)
            .cloned()
            .unwrap_or_else(|| "not_set".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let provider = args.trim();
        if provider.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PRESENCE_STATE: provider is required".to_string(),
            ));
        }

        let parts: Vec<&str> = value.splitn(2, ',').collect();
        let state = PresenceState::from_str_name(parts[0].trim());
        let message = parts.get(1).map(|s| s.trim()).unwrap_or("");

        let key = format!("__PRESENCE_{}", provider);
        ctx.set_variable(&key, state.as_str());

        if !message.is_empty() {
            let msg_key = format!("__PRESENCE_{}_MESSAGE", provider);
            ctx.set_variable(&msg_key, message);
        }

        Ok(())
    }
}

/// PRESENCE_STATE_EXT() function - extended presence fields.
///
/// Usage:
///   ${PRESENCE_STATE_EXT(CustomPresence:alice,message)} - Get message
///   ${PRESENCE_STATE_EXT(CustomPresence:alice,subtype)} - Get subtype
pub struct FuncPresenceStateExt;

impl DialplanFunc for FuncPresenceStateExt {
    fn name(&self) -> &str {
        "PRESENCE_STATE_EXT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "PRESENCE_STATE_EXT: requires provider,field".to_string(),
            ));
        }
        let provider = parts[0].trim();
        let field = parts[1].trim().to_lowercase();

        match field.as_str() {
            "message" | "subtype" | "status" => {
                let key = format!("__PRESENCE_{}_{}", provider, field.to_uppercase());
                Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "PRESENCE_STATE_EXT: unknown field '{}'",
                field
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presence_default() {
        let ctx = FuncContext::new();
        assert_eq!(
            FuncPresenceState.read(&ctx, "CustomPresence:alice").unwrap(),
            "not_set"
        );
    }

    #[test]
    fn test_set_presence() {
        let mut ctx = FuncContext::new();
        FuncPresenceState
            .write(&mut ctx, "CustomPresence:alice", "available,In office")
            .unwrap();
        assert_eq!(
            FuncPresenceState.read(&ctx, "CustomPresence:alice").unwrap(),
            "available"
        );
    }

    #[test]
    fn test_presence_ext_message() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__PRESENCE_CustomPresence:alice_MESSAGE", "In office");
        assert_eq!(
            FuncPresenceStateExt
                .read(&ctx, "CustomPresence:alice,message")
                .unwrap(),
            "In office"
        );
    }

    #[test]
    fn test_presence_state_values() {
        assert_eq!(PresenceState::from_str_name("available"), PresenceState::Available);
        assert_eq!(PresenceState::from_str_name("dnd"), PresenceState::Dnd);
        assert_eq!(PresenceState::from_str_name("unknown"), PresenceState::NotSet);
    }
}
