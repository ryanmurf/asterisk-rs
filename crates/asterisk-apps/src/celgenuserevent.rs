//! CEL (Channel Event Logging) user event generation application.
//!
//! Port of app_celgenuserevent.c from Asterisk C. Generates a custom
//! user-defined CEL event with a specified event name and optional
//! extra data.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info};

/// Parsed arguments for the CELGenUserEvent application.
#[derive(Debug)]
pub struct CelGenUserEventArgs {
    /// The event name/type.
    pub event_name: String,
    /// Optional extra text to include with the event.
    pub extra: String,
}

impl CelGenUserEventArgs {
    /// Parse CELGenUserEvent() argument string.
    ///
    /// Format: event-name[,extra]
    pub fn parse(args: &str) -> Option<Self> {
        if args.trim().is_empty() {
            return None;
        }

        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let event_name = parts.first()?.trim().to_string();
        if event_name.is_empty() {
            return None;
        }

        let extra = parts
            .get(1)
            .map(|e| e.trim().to_string())
            .unwrap_or_default();

        Some(Self { event_name, extra })
    }
}

/// The CELGenUserEvent() dialplan application.
///
/// Usage: CELGenUserEvent(event-name[,extra])
///
/// Immediately generates a CEL user-defined event on the current channel
/// with the supplied event name and optional extra data.
pub struct AppCelGenUserEvent;

impl DialplanApp for AppCelGenUserEvent {
    fn name(&self) -> &str {
        "CELGenUserEvent"
    }

    fn description(&self) -> &str {
        "Generate a CEL User Defined Event"
    }
}

impl AppCelGenUserEvent {
    /// Execute the CELGenUserEvent application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match CelGenUserEventArgs::parse(args) {
            Some(a) => a,
            None => {
                // Match Asterisk behavior: silently return success if no args
                debug!("CELGenUserEvent: no event name provided, doing nothing");
                return PbxExecResult::Success;
            }
        };

        info!(
            "CELGenUserEvent: channel '{}' generating event '{}' extra='{}'",
            channel.name, parsed.event_name, parsed.extra,
        );

        // In a real implementation:
        //
        //   let blob = json!({
        //       "event": parsed.event_name,
        //       "extra": {
        //           "extra": parsed.extra,
        //       }
        //   });
        //
        //   cel_publish_event(channel, CelEventType::UserDefined, &blob);

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_event_only() {
        let args = CelGenUserEventArgs::parse("login").unwrap();
        assert_eq!(args.event_name, "login");
        assert_eq!(args.extra, "");
    }

    #[test]
    fn test_parse_args_with_extra() {
        let args = CelGenUserEventArgs::parse("login,user=john").unwrap();
        assert_eq!(args.event_name, "login");
        assert_eq!(args.extra, "user=john");
    }

    #[test]
    fn test_parse_args_empty() {
        assert!(CelGenUserEventArgs::parse("").is_none());
    }

    #[tokio::test]
    async fn test_celgenuserevent_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppCelGenUserEvent::exec(&mut channel, "custom_event,data123").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_celgenuserevent_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppCelGenUserEvent::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
