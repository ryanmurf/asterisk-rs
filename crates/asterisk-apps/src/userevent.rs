//! UserEvent application.
//!
//! Port of app_userevent.c from Asterisk C. Sends a custom AMI
//! (Asterisk Manager Interface) user event from the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_ami::protocol::AmiEvent;
use asterisk_ami::events::EventCategory;
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The UserEvent() dialplan application.
///
/// Usage: UserEvent(eventname[,body])
///
/// Sends a custom "UserEvent" AMI event with the given name and body.
/// The body can contain key: value pairs separated by commas.
///
/// Example: UserEvent(MyEvent,Key1: Val1,Key2: Val2)
///
/// This generates an AMI event:
///   Event: UserEvent
///   UserEvent: MyEvent
///   Key1: Val1
///   Key2: Val2
pub struct AppUserEvent;

impl DialplanApp for AppUserEvent {
    fn name(&self) -> &str {
        "UserEvent"
    }

    fn description(&self) -> &str {
        "Send a custom AMI user event"
    }
}

impl AppUserEvent {
    /// Execute the UserEvent application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let event_name = parts.first().copied().unwrap_or("").trim();

        if event_name.is_empty() {
            warn!("UserEvent: requires event name argument");
            return PbxExecResult::Failed;
        }

        let body = parts.get(1).copied().unwrap_or("");

        info!(
            "UserEvent: channel '{}' event='{}' body='{}'",
            channel.name, event_name, body,
        );

        // Build the AMI event
        let mut event = AmiEvent::new("UserEvent", EventCategory::USER.0);
        event.add_header("UserEvent", event_name);
        event.add_header("Channel", &channel.name);
        event.add_header("Uniqueid", &channel.unique_id.0);

        // Parse body as comma-separated key: value pairs
        for item in body.split(',') {
            if let Some((k, v)) = item.split_once(':') {
                event.add_header(k.trim(), v.trim());
            }
        }

        asterisk_ami::publish_event(event);

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_userevent_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppUserEvent::exec(&mut channel, "CallComplete,Duration: 120,Status: OK").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_userevent_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppUserEvent::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_userevent_no_body() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppUserEvent::exec(&mut channel, "SimpleEvent").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
