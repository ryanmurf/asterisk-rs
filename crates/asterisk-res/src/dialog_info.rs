//! Dialog-Info XML for BLF (Busy Lamp Field).
//!
//! Implements RFC 4235 dialog-info+xml document generation, used to
//! convey dialog state information for SIP presence subscriptions.
//! This is primarily used for BLF indicators on IP phones.

use std::fmt;

// ---------------------------------------------------------------------------
// Dialog state
// ---------------------------------------------------------------------------

/// State of an individual dialog.
///
/// Mirrors the dialog state values from RFC 4235 Section 4.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogState {
    /// INVITE sent or received, no provisional response.
    Trying,
    /// Provisional response received (1xx other than 100).
    Proceeding,
    /// Early dialog established (e.g., 180 Ringing).
    Early,
    /// Dialog confirmed (200 OK received, ACK sent).
    Confirmed,
    /// Dialog terminated (BYE sent/received or error).
    Terminated,
}

impl DialogState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trying => "trying",
            Self::Proceeding => "proceeding",
            Self::Early => "early",
            Self::Confirmed => "confirmed",
            Self::Terminated => "terminated",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "trying" => Some(Self::Trying),
            "proceeding" => Some(Self::Proceeding),
            "early" => Some(Self::Early),
            "confirmed" => Some(Self::Confirmed),
            "terminated" => Some(Self::Terminated),
            _ => None,
        }
    }
}

impl fmt::Display for DialogState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Dialog info state (document-level)
// ---------------------------------------------------------------------------

/// The overall state of the dialog-info document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogInfoState {
    /// Full state notification.
    Full,
    /// Partial state notification (delta).
    Partial,
}

impl DialogInfoState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }
}

// ---------------------------------------------------------------------------
// Direction
// ---------------------------------------------------------------------------

/// Direction of a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogDirection {
    /// Outbound (caller).
    Initiator,
    /// Inbound (callee).
    Recipient,
}

impl DialogDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initiator => "initiator",
            Self::Recipient => "recipient",
        }
    }
}

// ---------------------------------------------------------------------------
// Dialog participant
// ---------------------------------------------------------------------------

/// A participant in a dialog (local or remote).
#[derive(Debug, Clone)]
pub struct DialogParticipant {
    /// SIP URI of the participant.
    pub uri: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Target URI (contact).
    pub target: Option<String>,
}

impl DialogParticipant {
    pub fn new(uri: &str) -> Self {
        Self {
            uri: uri.to_string(),
            display_name: None,
            target: None,
        }
    }

    pub fn with_display_name(mut self, name: &str) -> Self {
        self.display_name = Some(name.to_string());
        self
    }

    pub fn with_target(mut self, target: &str) -> Self {
        self.target = Some(target.to_string());
        self
    }

    fn to_xml(&self, element: &str) -> String {
        let mut xml = format!("      <{element}>\n");
        xml.push_str(&format!(
            "        <identity{}>{}</identity>\n",
            match &self.display_name {
                Some(n) => format!(" display=\"{}\"", xml_escape(n)),
                None => String::new(),
            },
            xml_escape(&self.uri),
        ));
        if let Some(ref target) = self.target {
            xml.push_str(&format!(
                "        <target uri=\"{}\"/>\n",
                xml_escape(target)
            ));
        }
        xml.push_str(&format!("      </{element}>\n"));
        xml
    }
}

// ---------------------------------------------------------------------------
// Dialog entry
// ---------------------------------------------------------------------------

/// A single dialog element within the dialog-info document.
#[derive(Debug, Clone)]
pub struct DialogEntry {
    /// Dialog ID.
    pub id: String,
    /// Call-ID.
    pub call_id: Option<String>,
    /// Local tag.
    pub local_tag: Option<String>,
    /// Remote tag.
    pub remote_tag: Option<String>,
    /// Direction.
    pub direction: Option<DialogDirection>,
    /// Current state of this dialog.
    pub state: DialogState,
    /// Local participant.
    pub local: Option<DialogParticipant>,
    /// Remote participant.
    pub remote: Option<DialogParticipant>,
}

impl DialogEntry {
    pub fn new(id: &str, state: DialogState) -> Self {
        Self {
            id: id.to_string(),
            call_id: None,
            local_tag: None,
            remote_tag: None,
            direction: None,
            state,
            local: None,
            remote: None,
        }
    }

    pub fn with_call_id(mut self, call_id: &str) -> Self {
        self.call_id = Some(call_id.to_string());
        self
    }

    pub fn with_direction(mut self, direction: DialogDirection) -> Self {
        self.direction = Some(direction);
        self
    }

    pub fn with_local(mut self, participant: DialogParticipant) -> Self {
        self.local = Some(participant);
        self
    }

    pub fn with_remote(mut self, participant: DialogParticipant) -> Self {
        self.remote = Some(participant);
        self
    }

    pub fn with_tags(mut self, local_tag: &str, remote_tag: &str) -> Self {
        self.local_tag = Some(local_tag.to_string());
        self.remote_tag = Some(remote_tag.to_string());
        self
    }

    fn to_xml(&self) -> String {
        let mut attrs = format!("id=\"{}\"", xml_escape(&self.id));
        if let Some(ref call_id) = self.call_id {
            attrs.push_str(&format!(" call-id=\"{}\"", xml_escape(call_id)));
        }
        if let Some(ref lt) = self.local_tag {
            attrs.push_str(&format!(" local-tag=\"{}\"", xml_escape(lt)));
        }
        if let Some(ref rt) = self.remote_tag {
            attrs.push_str(&format!(" remote-tag=\"{}\"", xml_escape(rt)));
        }
        if let Some(ref dir) = self.direction {
            attrs.push_str(&format!(" direction=\"{}\"", dir.as_str()));
        }

        let mut xml = format!("    <dialog {}>\n", attrs);
        xml.push_str(&format!(
            "      <state>{}</state>\n",
            self.state.as_str()
        ));
        if let Some(ref local) = self.local {
            xml.push_str(&local.to_xml("local"));
        }
        if let Some(ref remote) = self.remote {
            xml.push_str(&remote.to_xml("remote"));
        }
        xml.push_str("    </dialog>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// Dialog-info document
// ---------------------------------------------------------------------------

/// A dialog-info+xml document (RFC 4235).
#[derive(Debug, Clone)]
pub struct DialogInfo {
    /// Entity URI being monitored.
    pub entity: String,
    /// Notification version (incremented on each NOTIFY).
    pub version: u32,
    /// Whether this is a full or partial state update.
    pub state: DialogInfoState,
    /// Dialog entries.
    pub dialogs: Vec<DialogEntry>,
}

impl DialogInfo {
    pub fn new(entity: &str, version: u32, state: DialogInfoState) -> Self {
        Self {
            entity: entity.to_string(),
            version,
            state,
            dialogs: Vec::new(),
        }
    }

    /// Add a dialog entry.
    pub fn add_dialog(&mut self, dialog: DialogEntry) {
        self.dialogs.push(dialog);
    }

    /// Builder: add a dialog.
    pub fn with_dialog(mut self, dialog: DialogEntry) -> Self {
        self.dialogs.push(dialog);
        self
    }

    /// Generate the dialog-info XML document.
    pub fn generate_dialog_info_xml(&self) -> String {
        let mut xml = String::with_capacity(1024);
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<dialog-info xmlns=\"urn:ietf:params:xml:ns:dialog-info\"\n");
        xml.push_str(&format!(
            "  version=\"{}\" state=\"{}\" entity=\"{}\">\n",
            self.version,
            self.state.as_str(),
            xml_escape(&self.entity),
        ));
        for dialog in &self.dialogs {
            xml.push_str(&dialog.to_xml());
        }
        xml.push_str("</dialog-info>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialog_state_parse() {
        assert_eq!(
            DialogState::from_str_value("early"),
            Some(DialogState::Early)
        );
        assert_eq!(
            DialogState::from_str_value("confirmed"),
            Some(DialogState::Confirmed)
        );
        assert_eq!(DialogState::from_str_value("invalid"), None);
    }

    #[test]
    fn test_simple_dialog_info() {
        let doc = DialogInfo::new("sip:alice@example.com", 1, DialogInfoState::Full)
            .with_dialog(
                DialogEntry::new("d1", DialogState::Confirmed)
                    .with_call_id("abc123@example.com")
                    .with_direction(DialogDirection::Recipient)
                    .with_local(DialogParticipant::new("sip:alice@example.com"))
                    .with_remote(
                        DialogParticipant::new("sip:bob@example.com")
                            .with_display_name("Bob"),
                    ),
            );

        let xml = doc.generate_dialog_info_xml();
        assert!(xml.contains("entity=\"sip:alice@example.com\""));
        assert!(xml.contains("version=\"1\""));
        assert!(xml.contains("state=\"full\""));
        assert!(xml.contains("<state>confirmed</state>"));
        assert!(xml.contains("call-id=\"abc123@example.com\""));
        assert!(xml.contains("direction=\"recipient\""));
        assert!(xml.contains("display=\"Bob\""));
    }

    #[test]
    fn test_ringing_blf() {
        let doc = DialogInfo::new("sip:100@pbx.local", 5, DialogInfoState::Full)
            .with_dialog(
                DialogEntry::new("ring1", DialogState::Early)
                    .with_direction(DialogDirection::Recipient),
            );

        let xml = doc.generate_dialog_info_xml();
        assert!(xml.contains("<state>early</state>"));
    }

    #[test]
    fn test_idle_blf() {
        let doc = DialogInfo::new("sip:100@pbx.local", 6, DialogInfoState::Full)
            .with_dialog(DialogEntry::new("idle1", DialogState::Terminated));

        let xml = doc.generate_dialog_info_xml();
        assert!(xml.contains("<state>terminated</state>"));
    }

    #[test]
    fn test_empty_dialog_info() {
        let doc = DialogInfo::new("sip:100@pbx.local", 0, DialogInfoState::Full);
        let xml = doc.generate_dialog_info_xml();
        assert!(xml.contains("<dialog-info"));
        assert!(xml.contains("</dialog-info>"));
        assert!(!xml.contains("<dialog "));
    }
}
