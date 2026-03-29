//! Message Waiting Indicator (port of res_pjsip_mwi.c).
//!
//! Manages MWI subscriptions and sends NOTIFY messages with
//! `application/simple-message-summary` bodies per RFC 3842.

use std::collections::HashMap;

use parking_lot::RwLock;
use uuid::Uuid;

use crate::parser::{
    header_names, RequestLine, SipHeader, SipMessage, SipMethod, SipUri, StartLine,
};

/// MIME type for MWI notifications.
pub const MWI_CONTENT_TYPE: &str = "application/simple-message-summary";

// ---------------------------------------------------------------------------
// MWI state
// ---------------------------------------------------------------------------

/// Voicemail state for a single mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MwiState {
    /// Mailbox identifier (e.g. "1000@default").
    pub mailbox: String,
    /// Number of new (unread) messages.
    pub new_messages: u32,
    /// Number of old (read) messages.
    pub old_messages: u32,
    /// Number of new urgent messages.
    pub new_urgent: u32,
    /// Number of old urgent messages.
    pub old_urgent: u32,
}

impl MwiState {
    pub fn new(mailbox: &str) -> Self {
        Self {
            mailbox: mailbox.to_string(),
            ..Default::default()
        }
    }

    /// Whether there are any waiting messages (new > 0).
    pub fn has_waiting(&self) -> bool {
        self.new_messages > 0 || self.new_urgent > 0
    }

    /// Build the RFC 3842 `simple-message-summary` body.
    pub fn to_message_summary(&self) -> String {
        let waiting = if self.has_waiting() { "yes" } else { "no" };
        let mut body = format!(
            "Messages-Waiting: {}\r\nMessage-Account: {}\r\n",
            waiting, self.mailbox
        );

        // Voice-Message line: new/old (urgent/not-urgent)
        body.push_str(&format!(
            "Voice-Message: {}/{} ({}/{})\r\n",
            self.new_messages, self.old_messages, self.new_urgent, self.old_urgent
        ));

        body
    }
}

// ---------------------------------------------------------------------------
// Aggregated MWI state
// ---------------------------------------------------------------------------

/// Aggregated MWI state across multiple mailboxes for a single subscriber.
#[derive(Debug, Clone)]
pub struct AggregatedMwi {
    /// Individual mailbox states.
    pub mailboxes: Vec<MwiState>,
}

impl AggregatedMwi {
    pub fn new() -> Self {
        Self {
            mailboxes: Vec::new(),
        }
    }

    /// Add or update a mailbox state.
    pub fn update(&mut self, state: MwiState) {
        if let Some(existing) = self.mailboxes.iter_mut().find(|m| m.mailbox == state.mailbox) {
            *existing = state;
        } else {
            self.mailboxes.push(state);
        }
    }

    /// Remove a mailbox.
    pub fn remove(&mut self, mailbox: &str) {
        self.mailboxes.retain(|m| m.mailbox != mailbox);
    }

    /// Total new messages across all mailboxes.
    pub fn total_new(&self) -> u32 {
        self.mailboxes.iter().map(|m| m.new_messages).sum()
    }

    /// Total old messages.
    pub fn total_old(&self) -> u32 {
        self.mailboxes.iter().map(|m| m.old_messages).sum()
    }

    /// Total new urgent.
    pub fn total_new_urgent(&self) -> u32 {
        self.mailboxes.iter().map(|m| m.new_urgent).sum()
    }

    /// Total old urgent.
    pub fn total_old_urgent(&self) -> u32 {
        self.mailboxes.iter().map(|m| m.old_urgent).sum()
    }

    /// Whether any mailbox has waiting messages.
    pub fn has_waiting(&self) -> bool {
        self.mailboxes.iter().any(|m| m.has_waiting())
    }

    /// Build an aggregated message-summary body.
    pub fn to_message_summary(&self, account_uri: &str) -> String {
        let waiting = if self.has_waiting() { "yes" } else { "no" };
        let mut body = format!(
            "Messages-Waiting: {}\r\nMessage-Account: {}\r\n",
            waiting, account_uri,
        );

        body.push_str(&format!(
            "Voice-Message: {}/{} ({}/{})\r\n",
            self.total_new(),
            self.total_old(),
            self.total_new_urgent(),
            self.total_old_urgent(),
        ));

        body
    }
}

impl Default for AggregatedMwi {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MWI NOTIFY builder
// ---------------------------------------------------------------------------

/// Build an MWI NOTIFY request for a subscriber.
///
/// This creates the full SIP NOTIFY message with a
/// `application/simple-message-summary` body.
pub fn build_mwi_notify(
    to_uri: &str,
    from_uri: &str,
    call_id: &str,
    local_tag: &str,
    remote_tag: &str,
    cseq: u32,
    state: &MwiState,
    subscription_state: &str,
) -> SipMessage {
    let branch = format!(
        "z9hG4bK{}",
        &Uuid::new_v4().to_string().replace('-', "")[..16]
    );

    let target_uri = SipUri::parse(to_uri).unwrap_or_else(|_| SipUri {
        scheme: "sip".to_string(),
        user: None,
        password: None,
        host: "localhost".to_string(),
        port: Some(5060),
        parameters: Default::default(),
        headers: Default::default(),
    });

    let body = state.to_message_summary();

    let headers = vec![
        SipHeader {
            name: header_names::VIA.to_string(),
            value: format!("SIP/2.0/UDP placeholder;branch={}", branch),
        },
        SipHeader {
            name: header_names::MAX_FORWARDS.to_string(),
            value: "70".to_string(),
        },
        SipHeader {
            name: header_names::FROM.to_string(),
            value: format!("<{}>;tag={}", from_uri, local_tag),
        },
        SipHeader {
            name: header_names::TO.to_string(),
            value: format!("<{}>;tag={}", to_uri, remote_tag),
        },
        SipHeader {
            name: header_names::CALL_ID.to_string(),
            value: call_id.to_string(),
        },
        SipHeader {
            name: header_names::CSEQ.to_string(),
            value: format!("{} NOTIFY", cseq),
        },
        SipHeader {
            name: "Event".to_string(),
            value: "message-summary".to_string(),
        },
        SipHeader {
            name: "Subscription-State".to_string(),
            value: subscription_state.to_string(),
        },
        SipHeader {
            name: header_names::CONTENT_TYPE.to_string(),
            value: MWI_CONTENT_TYPE.to_string(),
        },
        SipHeader {
            name: header_names::CONTENT_LENGTH.to_string(),
            value: body.len().to_string(),
        },
    ];

    SipMessage {
        start_line: StartLine::Request(RequestLine {
            method: SipMethod::Notify,
            uri: target_uri,
            version: "SIP/2.0".to_string(),
        }),
        headers,
        body,
    }
}

/// Build an unsolicited MWI NOTIFY (no prior SUBSCRIBE dialog).
pub fn build_unsolicited_mwi_notify(
    to_uri: &str,
    from_uri: &str,
    state: &MwiState,
) -> SipMessage {
    let call_id = format!("mwi-{}", Uuid::new_v4());
    let local_tag = Uuid::new_v4().to_string()[..8].to_string();

    build_mwi_notify(
        to_uri,
        from_uri,
        &call_id,
        &local_tag,
        "",  // No remote tag for unsolicited.
        1,
        state,
        "terminated;reason=deactivated",
    )
}

// ---------------------------------------------------------------------------
// MWI state store
// ---------------------------------------------------------------------------

/// A simple in-memory MWI state store keyed by mailbox name.
#[derive(Debug, Default)]
pub struct MwiStore {
    states: RwLock<HashMap<String, MwiState>>,
}

impl MwiStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the state for a mailbox.
    pub fn update(&self, state: MwiState) {
        self.states
            .write()
            .insert(state.mailbox.clone(), state);
    }

    /// Get the state for a mailbox.
    pub fn get(&self, mailbox: &str) -> Option<MwiState> {
        self.states.read().get(mailbox).cloned()
    }

    /// Remove a mailbox state.
    pub fn remove(&self, mailbox: &str) {
        self.states.write().remove(mailbox);
    }

    /// Get all mailbox states.
    pub fn all(&self) -> Vec<MwiState> {
        self.states.read().values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mwi_state_body() {
        let state = MwiState {
            mailbox: "sip:1000@example.com".to_string(),
            new_messages: 3,
            old_messages: 5,
            new_urgent: 1,
            old_urgent: 0,
        };

        let body = state.to_message_summary();
        assert!(body.contains("Messages-Waiting: yes"));
        assert!(body.contains("Voice-Message: 3/5 (1/0)"));
    }

    #[test]
    fn test_mwi_state_no_waiting() {
        let state = MwiState::new("sip:1000@example.com");
        let body = state.to_message_summary();
        assert!(body.contains("Messages-Waiting: no"));
        assert!(body.contains("Voice-Message: 0/0 (0/0)"));
    }

    #[test]
    fn test_aggregated_mwi() {
        let mut agg = AggregatedMwi::new();
        agg.update(MwiState {
            mailbox: "mb1".to_string(),
            new_messages: 2,
            old_messages: 1,
            new_urgent: 0,
            old_urgent: 0,
        });
        agg.update(MwiState {
            mailbox: "mb2".to_string(),
            new_messages: 1,
            old_messages: 3,
            new_urgent: 1,
            old_urgent: 0,
        });

        assert_eq!(agg.total_new(), 3);
        assert_eq!(agg.total_old(), 4);
        assert_eq!(agg.total_new_urgent(), 1);
        assert!(agg.has_waiting());
    }

    #[test]
    fn test_build_mwi_notify() {
        let state = MwiState {
            mailbox: "sip:1000@example.com".to_string(),
            new_messages: 2,
            old_messages: 1,
            new_urgent: 0,
            old_urgent: 0,
        };

        let notify = build_mwi_notify(
            "sip:phone@10.0.0.1",
            "sip:1000@example.com",
            "mwi-call-123",
            "localtag",
            "remotetag",
            1,
            &state,
            "active;expires=3600",
        );

        assert_eq!(notify.method(), Some(SipMethod::Notify));
        assert!(notify.body.contains("Messages-Waiting: yes"));
        assert!(notify.body.contains("Voice-Message: 2/1 (0/0)"));
    }
}
