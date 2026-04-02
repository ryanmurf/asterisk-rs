//! AMI event categories and event generation.
//!
//! Events are asynchronous notifications sent to connected AMI clients.
//! Each event belongs to one or more categories, and clients can filter
//! which categories they receive.

use crate::protocol::AmiEvent;
use std::collections::HashMap;

/// Event category flags matching Asterisk's EVENT_FLAG_* defines.
///
/// Each category is a bit flag, allowing sessions to subscribe to
/// multiple categories using a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCategory(pub u32);

impl EventCategory {
    /// System events (module loads, shutdowns, etc.).
    pub const SYSTEM: Self = Self(1 << 0);
    /// Call/channel events (new channel, hangup, dial, etc.).
    pub const CALL: Self = Self(1 << 1);
    /// Log messages.
    pub const LOG: Self = Self(1 << 2);
    /// Verbose messages.
    pub const VERBOSE: Self = Self(1 << 3);
    /// Command responses.
    pub const COMMAND: Self = Self(1 << 4);
    /// Agent events (agent login, logoff, etc.).
    pub const AGENT: Self = Self(1 << 5);
    /// User-generated events.
    pub const USER: Self = Self(1 << 6);
    /// Configuration events.
    pub const CONFIG: Self = Self(1 << 7);
    /// DTMF events.
    pub const DTMF: Self = Self(1 << 8);
    /// Reporting events (CEL, billing, etc.).
    pub const REPORTING: Self = Self(1 << 9);
    /// CDR events.
    pub const CDR: Self = Self(1 << 10);
    /// Dialplan events (variable set, execution, etc.).
    pub const DIALPLAN: Self = Self(1 << 11);
    /// Security events (failed auth, etc.).
    pub const SECURITY: Self = Self(1 << 12);

    /// All event categories.
    pub const ALL: Self = Self(0x1FFF);

    /// No event categories.
    pub const NONE: Self = Self(0);

    /// Check if this category set includes a given category.
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    /// Combine two category sets.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Remove a category from this set.
    pub fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Parse a category name to its flag value.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "system" => Some(Self::SYSTEM),
            "call" => Some(Self::CALL),
            "log" => Some(Self::LOG),
            "verbose" => Some(Self::VERBOSE),
            "command" => Some(Self::COMMAND),
            "agent" => Some(Self::AGENT),
            "user" => Some(Self::USER),
            "config" => Some(Self::CONFIG),
            "dtmf" => Some(Self::DTMF),
            "reporting" => Some(Self::REPORTING),
            "cdr" => Some(Self::CDR),
            "dialplan" => Some(Self::DIALPLAN),
            "security" => Some(Self::SECURITY),
            "all" => Some(Self::ALL),
            _ => None,
        }
    }

    /// Get the string name for this category (if it's a single category).
    pub fn as_str(&self) -> &'static str {
        match self.0 {
            x if x == Self::SYSTEM.0 => "system",
            x if x == Self::CALL.0 => "call",
            x if x == Self::LOG.0 => "log",
            x if x == Self::VERBOSE.0 => "verbose",
            x if x == Self::COMMAND.0 => "command",
            x if x == Self::AGENT.0 => "agent",
            x if x == Self::USER.0 => "user",
            x if x == Self::CONFIG.0 => "config",
            x if x == Self::DTMF.0 => "dtmf",
            x if x == Self::REPORTING.0 => "reporting",
            x if x == Self::CDR.0 => "cdr",
            x if x == Self::DIALPLAN.0 => "dialplan",
            x if x == Self::SECURITY.0 => "security",
            _ => "unknown",
        }
    }

    /// Parse a comma-separated list of category names into a combined flag.
    pub fn parse_list(list: &str) -> Self {
        let mut result = Self::NONE;
        for name in list.split(',') {
            let name = name.trim();
            if let Some(cat) = Self::from_name(name) {
                result = result.union(cat);
            }
        }
        result
    }
}

/// Build common AMI events from Asterisk state changes.
pub struct EventBuilder;

impl EventBuilder {
    /// Create a Newchannel event.
    pub fn new_channel(
        channel: &str,
        channel_state: &str,
        channel_state_desc: &str,
        caller_id_num: &str,
        caller_id_name: &str,
        account_code: &str,
        unique_id: &str,
        linked_id: &str,
    ) -> AmiEvent {
        AmiEvent::new("Newchannel", EventCategory::CALL.0)
            .with_header("Channel", channel)
            .with_header("ChannelState", channel_state)
            .with_header("ChannelStateDesc", channel_state_desc)
            .with_header("CallerIDNum", caller_id_num)
            .with_header("CallerIDName", caller_id_name)
            .with_header("AccountCode", account_code)
            .with_header("Uniqueid", unique_id)
            .with_header("Linkedid", linked_id)
    }

    /// Create a Hangup event.
    pub fn hangup(
        channel: &str,
        unique_id: &str,
        cause: u32,
        cause_txt: &str,
    ) -> AmiEvent {
        AmiEvent::new("Hangup", EventCategory::CALL.0)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
            .with_header("Cause", cause.to_string())
            .with_header("Cause-txt", cause_txt)
    }

    /// Create a Newstate event (channel state change).
    pub fn new_state(
        channel: &str,
        channel_state: &str,
        channel_state_desc: &str,
        unique_id: &str,
    ) -> AmiEvent {
        AmiEvent::new("Newstate", EventCategory::CALL.0)
            .with_header("Channel", channel)
            .with_header("ChannelState", channel_state)
            .with_header("ChannelStateDesc", channel_state_desc)
            .with_header("Uniqueid", unique_id)
    }

    /// Create a Dial event.
    pub fn dial(
        sub_event: &str,
        channel: &str,
        destination: &str,
        caller_id_num: &str,
        caller_id_name: &str,
        unique_id: &str,
        dest_unique_id: &str,
        dialstring: &str,
    ) -> AmiEvent {
        AmiEvent::new("Dial", EventCategory::CALL.0)
            .with_header("SubEvent", sub_event)
            .with_header("Channel", channel)
            .with_header("Destination", destination)
            .with_header("CallerIDNum", caller_id_num)
            .with_header("CallerIDName", caller_id_name)
            .with_header("Uniqueid", unique_id)
            .with_header("DestUniqueid", dest_unique_id)
            .with_header("Dialstring", dialstring)
    }

    /// Create a BridgeEnter event.
    pub fn bridge_enter(
        bridge_unique_id: &str,
        bridge_type: &str,
        channel: &str,
        unique_id: &str,
    ) -> AmiEvent {
        AmiEvent::new("BridgeEnter", EventCategory::CALL.0)
            .with_header("BridgeUniqueid", bridge_unique_id)
            .with_header("BridgeType", bridge_type)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
    }

    /// Create a BridgeLeave event.
    pub fn bridge_leave(
        bridge_unique_id: &str,
        channel: &str,
        unique_id: &str,
    ) -> AmiEvent {
        AmiEvent::new("BridgeLeave", EventCategory::CALL.0)
            .with_header("BridgeUniqueid", bridge_unique_id)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
    }

    /// Create a DTMFBegin event.
    pub fn dtmf_begin(channel: &str, unique_id: &str, digit: char, direction: &str) -> AmiEvent {
        AmiEvent::new("DTMFBegin", EventCategory::DTMF.0)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
            .with_header("Digit", digit.to_string())
            .with_header("Direction", direction)
    }

    /// Create a DTMFEnd event.
    pub fn dtmf_end(
        channel: &str,
        unique_id: &str,
        digit: char,
        duration_ms: u32,
        direction: &str,
    ) -> AmiEvent {
        AmiEvent::new("DTMFEnd", EventCategory::DTMF.0)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
            .with_header("Digit", digit.to_string())
            .with_header("DurationMs", duration_ms.to_string())
            .with_header("Direction", direction)
    }

    /// Create a VarSet event (channel variable changed).
    pub fn var_set(channel: &str, unique_id: &str, variable: &str, value: &str) -> AmiEvent {
        AmiEvent::new("VarSet", EventCategory::DIALPLAN.0)
            .with_header("Channel", channel)
            .with_header("Uniqueid", unique_id)
            .with_header("Variable", variable)
            .with_header("Value", value)
    }

    /// Create a PeerStatus event.
    pub fn peer_status(
        channel_type: &str,
        peer: &str,
        peer_status: &str,
    ) -> AmiEvent {
        AmiEvent::new("PeerStatus", EventCategory::SYSTEM.0)
            .with_header("ChannelType", channel_type)
            .with_header("Peer", peer)
            .with_header("PeerStatus", peer_status)
    }

    /// Create a QueueMemberStatus event.
    pub fn queue_member_status(
        queue: &str,
        member_name: &str,
        interface: &str,
        status: u32,
        paused: bool,
    ) -> AmiEvent {
        AmiEvent::new("QueueMemberStatus", EventCategory::AGENT.0)
            .with_header("Queue", queue)
            .with_header("MemberName", member_name)
            .with_header("Interface", interface)
            .with_header("Status", status.to_string())
            .with_header("Paused", if paused { "1" } else { "0" })
    }

    /// Create a FullyBooted system event.
    pub fn fully_booted() -> AmiEvent {
        AmiEvent::new("FullyBooted", EventCategory::SYSTEM.0)
            .with_header("Status", "Fully Booted")
    }

    /// Create a Shutdown system event.
    pub fn shutdown(restart: bool) -> AmiEvent {
        AmiEvent::new("Shutdown", EventCategory::SYSTEM.0)
            .with_header("Shutdown", if restart { "Cleanly" } else { "Uncleanly" })
            .with_header("Restart", if restart { "True" } else { "False" })
    }

    /// Create a UserEvent.
    pub fn user_event(event_name: &str, headers: HashMap<String, String>) -> AmiEvent {
        let mut event = AmiEvent::new(format!("UserEvent{}", event_name), EventCategory::USER.0);
        for (k, v) in headers {
            event.headers.insert(k, v);
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_category_contains() {
        let cats = EventCategory::CALL.union(EventCategory::SYSTEM);
        assert!(cats.contains(EventCategory::CALL));
        assert!(cats.contains(EventCategory::SYSTEM));
        assert!(!cats.contains(EventCategory::DTMF));
    }

    #[test]
    fn test_event_category_from_name() {
        assert_eq!(EventCategory::from_name("system"), Some(EventCategory::SYSTEM));
        assert_eq!(EventCategory::from_name("call"), Some(EventCategory::CALL));
        assert_eq!(EventCategory::from_name("all"), Some(EventCategory::ALL));
        assert_eq!(EventCategory::from_name("bogus"), None);
    }

    #[test]
    fn test_event_category_parse_list() {
        let cats = EventCategory::parse_list("system,call,dtmf");
        assert!(cats.contains(EventCategory::SYSTEM));
        assert!(cats.contains(EventCategory::CALL));
        assert!(cats.contains(EventCategory::DTMF));
        assert!(!cats.contains(EventCategory::LOG));
    }

    #[test]
    fn test_event_builder_new_channel() {
        let event = EventBuilder::new_channel(
            "SIP/100-00000001",
            "6",
            "Up",
            "100",
            "Alice",
            "",
            "unique-1",
            "linked-1",
        );
        assert_eq!(event.name, "Newchannel");
        assert_eq!(event.category, EventCategory::CALL.0);
        assert_eq!(event.headers.get("Channel").unwrap(), "SIP/100-00000001");
    }

    #[test]
    fn test_event_builder_hangup() {
        let event = EventBuilder::hangup("SIP/100-00000001", "unique-1", 16, "Normal Clearing");
        assert_eq!(event.name, "Hangup");
        assert_eq!(event.headers.get("Cause").unwrap(), "16");
    }

    #[test]
    fn test_event_category_difference() {
        let all = EventCategory::ALL;
        let without_dtmf = all.difference(EventCategory::DTMF);
        assert!(!without_dtmf.contains(EventCategory::DTMF));
        assert!(without_dtmf.contains(EventCategory::CALL));
    }
}
