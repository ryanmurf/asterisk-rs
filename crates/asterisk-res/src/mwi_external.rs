//! External MWI (Message Waiting Indicator).
//!
//! Port of `res/res_mwi_external.c`. Allows external systems (e.g., via
//! AMI or configuration) to set MWI state for mailboxes, which is then
//! published to internal subscribers (SIP NOTIFY, visual indicators, etc.).

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum MwiError {
    #[error("mailbox not found: {0}")]
    MailboxNotFound(String),
    #[error("MWI error: {0}")]
    Other(String),
}

pub type MwiResult<T> = Result<T, MwiError>;

// ---------------------------------------------------------------------------
// MWI state
// ---------------------------------------------------------------------------

/// Message Waiting Indicator state for a single mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MwiState {
    /// Mailbox identifier (e.g., "1001@default").
    pub mailbox: String,
    /// Number of new (unread) messages.
    pub new_messages: u32,
    /// Number of old (read) messages.
    pub old_messages: u32,
    /// Number of new urgent messages.
    pub new_urgent: u32,
    /// Number of old urgent messages.
    pub old_urgent: u32,
    /// Timestamp of last state change.
    pub last_updated: u64,
}

impl MwiState {
    /// Create a new MWI state for the given mailbox.
    pub fn new(mailbox: &str) -> Self {
        Self {
            mailbox: mailbox.to_string(),
            new_messages: 0,
            old_messages: 0,
            new_urgent: 0,
            old_urgent: 0,
            last_updated: current_timestamp(),
        }
    }

    /// Create a state with message counts.
    pub fn with_counts(mailbox: &str, new_msgs: u32, old_msgs: u32) -> Self {
        Self {
            mailbox: mailbox.to_string(),
            new_messages: new_msgs,
            old_messages: old_msgs,
            new_urgent: 0,
            old_urgent: 0,
            last_updated: current_timestamp(),
        }
    }

    /// Whether the mailbox has any waiting messages.
    pub fn has_messages(&self) -> bool {
        self.new_messages > 0 || self.new_urgent > 0
    }

    /// Total number of messages.
    pub fn total_messages(&self) -> u32 {
        self.new_messages + self.old_messages + self.new_urgent + self.old_urgent
    }

    /// Update message counts and refresh the timestamp.
    pub fn update(&mut self, new_msgs: u32, old_msgs: u32) {
        self.new_messages = new_msgs;
        self.old_messages = old_msgs;
        self.last_updated = current_timestamp();
    }

    /// Update with urgent counts.
    pub fn update_with_urgent(
        &mut self,
        new_msgs: u32,
        old_msgs: u32,
        new_urgent: u32,
        old_urgent: u32,
    ) {
        self.new_messages = new_msgs;
        self.old_messages = old_msgs;
        self.new_urgent = new_urgent;
        self.old_urgent = old_urgent;
        self.last_updated = current_timestamp();
    }

    /// Format as a SIP message-summary body (RFC 3842).
    pub fn to_message_summary(&self) -> String {
        let status = if self.has_messages() { "yes" } else { "no" };
        let mut body = format!("Messages-Waiting: {}\r\n", status);
        body.push_str(&format!(
            "Voice-Message: {}/{} ({}/{})\r\n",
            self.new_messages, self.old_messages, self.new_urgent, self.old_urgent,
        ));
        body
    }
}

impl fmt::Display for MwiState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: new={} old={} urgent_new={} urgent_old={}",
            self.mailbox,
            self.new_messages,
            self.old_messages,
            self.new_urgent,
            self.old_urgent,
        )
    }
}

// ---------------------------------------------------------------------------
// MWI change callback
// ---------------------------------------------------------------------------

/// Callback function type for MWI state changes.
pub type MwiChangeCallback = Box<dyn Fn(&MwiState) + Send + Sync>;

// ---------------------------------------------------------------------------
// External MWI manager
// ---------------------------------------------------------------------------

/// Manages MWI state from external sources.
///
/// External systems (AMI, REST, config file) push mailbox state here.
/// Internal subscribers are notified of changes to trigger SIP NOTIFY
/// messages and phone indicator updates.
pub struct ExternalMwi {
    /// Mailbox states keyed by mailbox identifier.
    mailboxes: RwLock<HashMap<String, MwiState>>,
    /// Callbacks notified on state changes.
    callbacks: RwLock<Vec<(String, MwiChangeCallback)>>,
}

impl ExternalMwi {
    pub fn new() -> Self {
        Self {
            mailboxes: RwLock::new(HashMap::new()),
            callbacks: RwLock::new(Vec::new()),
        }
    }

    /// Set the MWI state for a mailbox.
    ///
    /// Creates the mailbox if it doesn't exist, or updates if it does.
    /// Notifies registered callbacks of the change.
    pub fn set_state(&self, mailbox: &str, new_msgs: u32, old_msgs: u32) {
        let state = {
            let mut mailboxes = self.mailboxes.write();
            let entry = mailboxes
                .entry(mailbox.to_string())
                .or_insert_with(|| MwiState::new(mailbox));
            entry.update(new_msgs, old_msgs);
            entry.clone()
        };

        info!(
            mailbox = %mailbox,
            new = new_msgs,
            old = old_msgs,
            "External MWI state updated"
        );
        self.notify_callbacks(&state);
    }

    /// Set the full MWI state including urgent counts.
    pub fn set_state_full(
        &self,
        mailbox: &str,
        new_msgs: u32,
        old_msgs: u32,
        new_urgent: u32,
        old_urgent: u32,
    ) {
        let state = {
            let mut mailboxes = self.mailboxes.write();
            let entry = mailboxes
                .entry(mailbox.to_string())
                .or_insert_with(|| MwiState::new(mailbox));
            entry.update_with_urgent(new_msgs, old_msgs, new_urgent, old_urgent);
            entry.clone()
        };

        debug!(mailbox = %mailbox, "External MWI state updated (full)");
        self.notify_callbacks(&state);
    }

    /// Get the current MWI state for a mailbox.
    pub fn get_state(&self, mailbox: &str) -> MwiResult<MwiState> {
        self.mailboxes
            .read()
            .get(mailbox)
            .cloned()
            .ok_or_else(|| MwiError::MailboxNotFound(mailbox.to_string()))
    }

    /// Remove a mailbox from external MWI tracking.
    pub fn remove(&self, mailbox: &str) -> MwiResult<()> {
        self.mailboxes
            .write()
            .remove(mailbox)
            .ok_or_else(|| MwiError::MailboxNotFound(mailbox.to_string()))?;
        debug!(mailbox, "External MWI mailbox removed");
        Ok(())
    }

    /// Subscribe to MWI state changes.
    pub fn subscribe(&self, name: &str, callback: MwiChangeCallback) {
        self.callbacks
            .write()
            .push((name.to_string(), callback));
        debug!(subscriber = name, "MWI change subscriber added");
    }

    /// Unsubscribe from MWI state changes.
    pub fn unsubscribe(&self, name: &str) {
        self.callbacks.write().retain(|(n, _)| n != name);
    }

    /// List all tracked mailboxes.
    pub fn mailboxes(&self) -> Vec<String> {
        let mut list: Vec<String> = self.mailboxes.read().keys().cloned().collect();
        list.sort();
        list
    }

    /// Get all mailbox states.
    pub fn all_states(&self) -> Vec<MwiState> {
        self.mailboxes.read().values().cloned().collect()
    }

    /// Number of tracked mailboxes.
    pub fn mailbox_count(&self) -> usize {
        self.mailboxes.read().len()
    }

    /// Clear all mailbox states.
    pub fn clear(&self) {
        self.mailboxes.write().clear();
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn notify_callbacks(&self, state: &MwiState) {
        let callbacks = self.callbacks.read();
        for (name, cb) in callbacks.iter() {
            debug!(subscriber = name.as_str(), mailbox = %state.mailbox, "Notifying MWI subscriber");
            cb(state);
        }
    }
}

impl Default for ExternalMwi {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ExternalMwi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalMwi")
            .field("mailboxes", &self.mailboxes.read().len())
            .field("subscribers", &self.callbacks.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_mwi_state_basic() {
        let state = MwiState::with_counts("1001@default", 3, 5);
        assert_eq!(state.new_messages, 3);
        assert_eq!(state.old_messages, 5);
        assert!(state.has_messages());
        assert_eq!(state.total_messages(), 8);
    }

    #[test]
    fn test_mwi_state_no_messages() {
        let state = MwiState::new("1001@default");
        assert!(!state.has_messages());
        assert_eq!(state.total_messages(), 0);
    }

    #[test]
    fn test_message_summary() {
        let state = MwiState::with_counts("1001@default", 2, 3);
        let summary = state.to_message_summary();
        assert!(summary.contains("Messages-Waiting: yes"));
        assert!(summary.contains("Voice-Message: 2/3"));
    }

    #[test]
    fn test_message_summary_no_messages() {
        let state = MwiState::new("1001@default");
        let summary = state.to_message_summary();
        assert!(summary.contains("Messages-Waiting: no"));
    }

    #[test]
    fn test_set_and_get_state() {
        let mwi = ExternalMwi::new();
        mwi.set_state("1001@default", 5, 10);

        let state = mwi.get_state("1001@default").unwrap();
        assert_eq!(state.new_messages, 5);
        assert_eq!(state.old_messages, 10);
    }

    #[test]
    fn test_update_state() {
        let mwi = ExternalMwi::new();
        mwi.set_state("1001@default", 5, 10);
        mwi.set_state("1001@default", 3, 12);

        let state = mwi.get_state("1001@default").unwrap();
        assert_eq!(state.new_messages, 3);
        assert_eq!(state.old_messages, 12);
    }

    #[test]
    fn test_remove_mailbox() {
        let mwi = ExternalMwi::new();
        mwi.set_state("1001@default", 1, 0);
        mwi.remove("1001@default").unwrap();
        assert!(mwi.get_state("1001@default").is_err());
    }

    #[test]
    fn test_list_mailboxes() {
        let mwi = ExternalMwi::new();
        mwi.set_state("1002@default", 0, 0);
        mwi.set_state("1001@default", 0, 0);

        let list = mwi.mailboxes();
        assert_eq!(list, vec!["1001@default", "1002@default"]);
    }

    #[test]
    fn test_callback_notification() {
        let mwi = ExternalMwi::new();
        let counter = Arc::new(AtomicU32::new(0));
        let cb_counter = Arc::clone(&counter);

        mwi.subscribe("test", Box::new(move |_state| {
            cb_counter.fetch_add(1, Ordering::Relaxed);
        }));

        mwi.set_state("1001@default", 1, 0);
        mwi.set_state("1001@default", 2, 0);

        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_unsubscribe() {
        let mwi = ExternalMwi::new();
        let counter = Arc::new(AtomicU32::new(0));
        let cb_counter = Arc::clone(&counter);

        mwi.subscribe("test", Box::new(move |_state| {
            cb_counter.fetch_add(1, Ordering::Relaxed);
        }));

        mwi.set_state("1001@default", 1, 0);
        mwi.unsubscribe("test");
        mwi.set_state("1001@default", 2, 0);

        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_set_state_full() {
        let mwi = ExternalMwi::new();
        mwi.set_state_full("1001@default", 2, 3, 1, 0);

        let state = mwi.get_state("1001@default").unwrap();
        assert_eq!(state.new_urgent, 1);
        assert_eq!(state.old_urgent, 0);
        assert_eq!(state.total_messages(), 6);
    }
}
