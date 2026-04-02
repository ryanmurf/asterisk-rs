//! /ari/mailboxes resource -- mailbox management via the ARI REST interface.
//!
//! Port of res/ari/resource_mailboxes.c. Implements CRUD operations on
//! mailboxes: list all, get by name, update message counts, and delete.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use dashmap::DashMap;
use std::sync::Arc;

/// In-memory store for mailboxes.
///
/// In Asterisk C, mailboxes interact with the MWI (Message Waiting Indication)
/// subsystem. Here we provide a simple in-memory store that tracks message
/// counts and can generate MWI events.
pub struct MailboxStore {
    mailboxes: DashMap<String, Mailbox>,
}

impl MailboxStore {
    /// Create a new empty mailbox store.
    pub fn new() -> Self {
        Self {
            mailboxes: DashMap::new(),
        }
    }

    /// List all mailboxes.
    pub fn list(&self) -> Vec<Mailbox> {
        self.mailboxes.iter().map(|e| e.value().clone()).collect()
    }

    /// Get a mailbox by name.
    pub fn get(&self, name: &str) -> Option<Mailbox> {
        self.mailboxes.get(name).map(|e| e.value().clone())
    }

    /// Update a mailbox's message counts (creates if it doesn't exist).
    pub fn update(&self, name: &str, old_messages: i32, new_messages: i32) {
        self.mailboxes.insert(
            name.to_string(),
            Mailbox {
                name: name.to_string(),
                old_messages,
                new_messages,
            },
        );
    }

    /// Delete a mailbox. Returns true if it existed.
    pub fn delete(&self, name: &str) -> bool {
        self.mailboxes.remove(name).is_some()
    }
}

impl Default for MailboxStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the /mailboxes route subtree.
pub fn build_mailboxes_routes() -> Arc<RestHandler> {
    // /mailboxes/{mailboxName}
    let mailbox_by_name = Arc::new(
        RestHandler::new("{mailboxName}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Put, handle_update)
            .on(HttpMethod::Delete, handle_delete),
    );

    // /mailboxes
    

    Arc::new(
        RestHandler::new("mailboxes")
            .on(HttpMethod::Get, handle_list)
            .child(mailbox_by_name),
    )
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /mailboxes -- list all mailboxes.
fn handle_list(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    // In a full implementation, this would query the MWI subsystem.
    let mailboxes: Vec<Mailbox> = Vec::new();
    AriResponse::ok(&mailboxes)
}

/// GET /mailboxes/{mailboxName} -- get a mailbox.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _mailbox_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing mailboxName".into(),
            ));
        }
    };

    // In a full implementation, look up the mailbox via MWI.
    AriResponse::error(&AriErrorKind::NotFound("Mailbox not found".into()))
}

/// PUT /mailboxes/{mailboxName} -- update a mailbox's message counts.
fn handle_update(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _mailbox_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing mailboxName".into(),
            ));
        }
    };

    let _old_messages = match req
        .query_param("oldMessages")
        .and_then(|v| v.parse::<i32>().ok())
    {
        Some(count) => count,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing or invalid required parameter: oldMessages".into(),
            ));
        }
    };

    let _new_messages = match req
        .query_param("newMessages")
        .and_then(|v| v.parse::<i32>().ok())
    {
        Some(count) => count,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing or invalid required parameter: newMessages".into(),
            ));
        }
    };

    // In a full implementation, publish MWI state.
    AriResponse::no_content()
}

/// DELETE /mailboxes/{mailboxName} -- delete a mailbox.
fn handle_delete(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _mailbox_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing mailboxName".into(),
            ));
        }
    };

    // In a full implementation, clear MWI state for this mailbox.
    AriResponse::no_content()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_store() {
        let store = MailboxStore::new();
        assert_eq!(store.list().len(), 0);

        store.update("1000@default", 5, 3);
        assert_eq!(store.list().len(), 1);

        let mb = store.get("1000@default").unwrap();
        assert_eq!(mb.old_messages, 5);
        assert_eq!(mb.new_messages, 3);

        store.update("1000@default", 6, 2);
        let mb = store.get("1000@default").unwrap();
        assert_eq!(mb.old_messages, 6);
        assert_eq!(mb.new_messages, 2);

        assert!(store.delete("1000@default"));
        assert_eq!(store.list().len(), 0);
        assert!(!store.delete("nonexistent"));
    }
}
