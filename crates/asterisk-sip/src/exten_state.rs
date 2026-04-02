//! Extension state NOTIFY for BLF (Busy Lamp Field).
//!
//! Port of `res/res_pjsip_exten_state.c`. Generates SIP NOTIFY messages
//! for extension state changes, enabling BLF on SIP phones. Uses the
//! `dialog` event package with `application/dialog-info+xml` bodies.

use std::fmt;


// ---------------------------------------------------------------------------
// Extension state
// ---------------------------------------------------------------------------

/// Extension state values matching the internal hint state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtenState {
    /// Extension is not in use (idle).
    NotInUse,
    /// Extension is in use (active call).
    InUse,
    /// Extension is busy.
    Busy,
    /// Extension is unavailable / unregistered.
    Unavailable,
    /// Extension is ringing.
    Ringing,
    /// Extension is in use and ringing.
    InUseRinging,
    /// Extension is on hold.
    OnHold,
}

impl ExtenState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotInUse => "NOT_INUSE",
            Self::InUse => "INUSE",
            Self::Busy => "BUSY",
            Self::Unavailable => "UNAVAILABLE",
            Self::Ringing => "RINGING",
            Self::InUseRinging => "RINGINUSE",
            Self::OnHold => "ONHOLD",
        }
    }

    /// Map to PIDF basic status for dialog-info XML.
    pub fn to_pidf_status(&self) -> &'static str {
        match self {
            Self::NotInUse | Self::Unavailable => "closed",
            _ => "open",
        }
    }

    /// Map to dialog state string for dialog-info+xml body.
    pub fn to_dialog_state(&self) -> &'static str {
        match self {
            Self::NotInUse => "terminated",
            Self::Ringing | Self::InUseRinging => "early",
            Self::InUse | Self::Busy | Self::OnHold => "confirmed",
            Self::Unavailable => "terminated",
        }
    }
}

impl fmt::Display for ExtenState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Subscription info
// ---------------------------------------------------------------------------

/// An extension state subscription (BLF hint subscription).
#[derive(Debug, Clone)]
pub struct ExtenStateSubscription {
    /// The extension being monitored.
    pub exten: String,
    /// The context.
    pub context: String,
    /// Subscriber endpoint name.
    pub subscriber: String,
    /// Current known state.
    pub last_state: ExtenState,
}

impl ExtenStateSubscription {
    pub fn new(exten: &str, context: &str, subscriber: &str) -> Self {
        Self {
            exten: exten.to_string(),
            context: context.to_string(),
            subscriber: subscriber.to_string(),
            last_state: ExtenState::Unavailable,
        }
    }

    /// Full extension@context identifier.
    pub fn full_exten(&self) -> String {
        format!("{}@{}", self.exten, self.context)
    }
}

// ---------------------------------------------------------------------------
// NOTIFY body generation
// ---------------------------------------------------------------------------

/// Generate a `dialog-info+xml` NOTIFY body for an extension state change.
///
/// This produces the XML body conforming to RFC 4235 that BLF-capable
/// SIP phones expect.
pub fn generate_dialog_info_body(
    entity: &str,
    exten: &str,
    state: ExtenState,
    version: u32,
) -> String {
    let dialog_state = state.to_dialog_state();
    let is_terminated = dialog_state == "terminated";

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<dialog-info xmlns=\"urn:ietf:params:xml:ns:dialog-info\" version=\"{}\" state=\"full\" entity=\"{}\">\n",
        version, entity,
    ));

    if !is_terminated {
        xml.push_str(&format!("  <dialog id=\"{}\">\n", exten));
        xml.push_str(&format!("    <state>{}</state>\n", dialog_state));

        if state == ExtenState::Ringing || state == ExtenState::InUseRinging {
            xml.push_str("    <local>\n");
            xml.push_str(&format!(
                "      <identity>{}</identity>\n",
                entity
            ));
            xml.push_str("    </local>\n");
        }

        xml.push_str("  </dialog>\n");
    }

    xml.push_str("</dialog-info>\n");
    xml
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exten_state_mapping() {
        assert_eq!(ExtenState::NotInUse.to_dialog_state(), "terminated");
        assert_eq!(ExtenState::Ringing.to_dialog_state(), "early");
        assert_eq!(ExtenState::InUse.to_dialog_state(), "confirmed");
    }

    #[test]
    fn test_pidf_status() {
        assert_eq!(ExtenState::NotInUse.to_pidf_status(), "closed");
        assert_eq!(ExtenState::InUse.to_pidf_status(), "open");
    }

    #[test]
    fn test_subscription() {
        let sub = ExtenStateSubscription::new("1001", "default", "phone-a");
        assert_eq!(sub.full_exten(), "1001@default");
    }

    #[test]
    fn test_dialog_info_body_idle() {
        let body = generate_dialog_info_body(
            "sip:1001@example.com",
            "1001",
            ExtenState::NotInUse,
            1,
        );
        assert!(body.contains("dialog-info"));
        assert!(body.contains("version=\"1\""));
        // No <dialog> element for terminated state
        assert!(!body.contains("<state>"));
    }

    #[test]
    fn test_dialog_info_body_ringing() {
        let body = generate_dialog_info_body(
            "sip:1001@example.com",
            "1001",
            ExtenState::Ringing,
            2,
        );
        assert!(body.contains("<state>early</state>"));
        assert!(body.contains("<identity>"));
    }

    #[test]
    fn test_dialog_info_body_inuse() {
        let body = generate_dialog_info_body(
            "sip:1001@example.com",
            "1001",
            ExtenState::InUse,
            3,
        );
        assert!(body.contains("<state>confirmed</state>"));
    }
}
