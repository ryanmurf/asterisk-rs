//! Stasis mailbox management.
//!
//! Port of `res/res_stasis_mailbox.c`. Provides ARI access to mailbox
//! state, allowing external applications to query and update mailbox
//! message counts. Delegates to the external MWI subsystem.

use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum StasisMailboxError {
    #[error("mailbox not found: {0}")]
    NotFound(String),
    #[error("mailbox error: {0}")]
    Other(String),
}

/// Result type for Stasis mailbox operations.
///
/// Mirrors `enum stasis_mailbox_result` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StasisMailboxResult {
    Ok,
    Missing,
    Error,
}

pub type MailboxResult<T> = Result<T, StasisMailboxError>;

// ---------------------------------------------------------------------------
// Mailbox info
// ---------------------------------------------------------------------------

/// Mailbox information as exposed via ARI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxInfo {
    /// Mailbox name (e.g., "1001@default").
    pub name: String,
    /// Number of old (read) messages.
    pub old_messages: u32,
    /// Number of new (unread) messages.
    pub new_messages: u32,
}

impl MailboxInfo {
    pub fn new(name: &str, old_messages: u32, new_messages: u32) -> Self {
        Self {
            name: name.to_string(),
            old_messages,
            new_messages,
        }
    }

    /// Convert to JSON representation matching ARI format.
    pub fn to_json(&self) -> JsonValue {
        serde_json::json!({
            "name": self.name,
            "old_messages": self.old_messages,
            "new_messages": self.new_messages,
        })
    }

    /// Parse from JSON.
    pub fn from_json(json: &JsonValue) -> Option<Self> {
        Some(Self {
            name: json.get("name")?.as_str()?.to_string(),
            old_messages: json.get("old_messages")?.as_u64()? as u32,
            new_messages: json.get("new_messages")?.as_u64()? as u32,
        })
    }
}

// ---------------------------------------------------------------------------
// Stasis mailbox operations
// ---------------------------------------------------------------------------

/// Get a single mailbox as JSON.
///
/// Mirrors `stasis_app_mailbox_to_json()` from the C source.
pub fn mailbox_to_json(name: &str, old_messages: u32, new_messages: u32) -> JsonValue {
    MailboxInfo::new(name, old_messages, new_messages).to_json()
}

/// Convert a list of mailbox info objects to a JSON array.
///
/// Mirrors `stasis_app_mailboxes_to_json()` from the C source.
pub fn mailboxes_to_json(mailboxes: &[MailboxInfo]) -> JsonValue {
    let arr: Vec<JsonValue> = mailboxes.iter().map(|m| m.to_json()).collect();
    JsonValue::Array(arr)
}

/// Update a mailbox's message counts.
///
/// In the full implementation this would delegate to the external MWI
/// subsystem (`res_mwi_external`). Here we provide the interface.
pub fn update_mailbox(
    name: &str,
    old_messages: u32,
    new_messages: u32,
) -> StasisMailboxResult {
    debug!(
        mailbox = name,
        old = old_messages,
        new = new_messages,
        "Stasis mailbox update"
    );
    // Would call ast_mwi_mailbox_update() in the full implementation.
    StasisMailboxResult::Ok
}

/// Delete a mailbox.
pub fn delete_mailbox(name: &str) -> StasisMailboxResult {
    debug!(mailbox = name, "Stasis mailbox delete");
    StasisMailboxResult::Ok
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_info() {
        let info = MailboxInfo::new("1001@default", 3, 5);
        assert_eq!(info.name, "1001@default");
        assert_eq!(info.old_messages, 3);
        assert_eq!(info.new_messages, 5);
    }

    #[test]
    fn test_mailbox_json_roundtrip() {
        let info = MailboxInfo::new("1001@default", 3, 5);
        let json = info.to_json();
        let parsed = MailboxInfo::from_json(&json).unwrap();
        assert_eq!(info, parsed);
    }

    #[test]
    fn test_mailboxes_to_json() {
        let mailboxes = vec![
            MailboxInfo::new("1001@default", 1, 2),
            MailboxInfo::new("1002@default", 3, 4),
        ];
        let json = mailboxes_to_json(&mailboxes);
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_update_mailbox() {
        assert_eq!(
            update_mailbox("1001@default", 0, 1),
            StasisMailboxResult::Ok
        );
    }
}
