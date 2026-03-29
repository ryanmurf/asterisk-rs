//! AMI session management.
//!
//! Each connected AMI client gets an AmiSession that tracks authentication
//! state, event subscriptions, and provides methods for sending responses
//! and events over the TCP connection.

use crate::auth::AmiUser;
use crate::events::EventCategory;
use crate::protocol::{AmiEvent, AmiResponse};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Unique identifier for an AMI session.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An authenticated (or pending) AMI session.
pub struct AmiSession {
    /// Unique session identifier.
    pub id: SessionId,
    /// Remote address of the connected client.
    pub addr: SocketAddr,
    /// Whether the session has been authenticated.
    pub authenticated: bool,
    /// The authenticated username (set after successful login).
    pub username: Option<String>,
    /// Read permission (what events this session can receive).
    pub read_perm: EventCategory,
    /// Write permission (what actions this session can execute).
    pub write_perm: EventCategory,
    /// Event category filter (which events to send to this session).
    pub event_filter: EventCategory,
    /// When the session started.
    pub session_start: Instant,
    /// MD5 challenge string (for challenge/response auth).
    pub challenge: Option<String>,
    /// Channel for sending data to the client.
    event_tx: mpsc::Sender<String>,
    /// Whether events are enabled for this session.
    pub events_enabled: bool,
}

impl AmiSession {
    /// Create a new unauthenticated session.
    pub fn new(addr: SocketAddr, event_tx: mpsc::Sender<String>) -> Self {
        Self {
            id: SessionId::new(),
            addr,
            authenticated: false,
            username: None,
            read_perm: EventCategory::NONE,
            write_perm: EventCategory::NONE,
            event_filter: EventCategory::ALL,
            session_start: Instant::now(),
            challenge: None,
            event_tx,
            events_enabled: true,
        }
    }

    /// Mark this session as authenticated with the given user.
    pub fn authenticate(&mut self, user: &AmiUser) {
        self.authenticated = true;
        self.username = Some(user.username.clone());
        self.read_perm = user.read_perm;
        self.write_perm = user.write_perm;
        info!(
            "AMI session {}: authenticated as '{}'",
            self.id, user.username
        );
    }

    /// Check if this session should receive an event.
    pub fn should_receive_event(&self, event: &AmiEvent) -> bool {
        if !self.authenticated || !self.events_enabled {
            return false;
        }

        let category = EventCategory(event.category);
        self.read_perm.contains(category) && self.event_filter.contains(category)
    }

    /// Send a response to this session's client.
    pub async fn send_response(&self, response: AmiResponse) -> Result<(), SessionError> {
        let data = response.serialize();
        debug!(
            "AMI session {}: sending response ({})",
            self.id,
            if response.success { "success" } else { "error" }
        );
        self.event_tx
            .send(data)
            .await
            .map_err(|_| SessionError::SendFailed)
    }

    /// Send an event to this session's client.
    pub async fn send_event(&self, event: &AmiEvent) -> Result<(), SessionError> {
        if !self.should_receive_event(event) {
            return Ok(());
        }
        let data = event.serialize();
        self.event_tx
            .send(data)
            .await
            .map_err(|_| SessionError::SendFailed)
    }

    /// Send raw text to this session's client.
    pub async fn send_raw(&self, data: String) -> Result<(), SessionError> {
        self.event_tx
            .send(data)
            .await
            .map_err(|_| SessionError::SendFailed)
    }

    /// Set the event filter for this session.
    pub fn set_event_filter(&mut self, categories: EventCategory) {
        self.event_filter = categories;
        debug!(
            "AMI session {}: event filter set to 0x{:04x}",
            self.id, categories.0
        );
    }

    /// Enable or disable events for this session.
    pub fn set_events_enabled(&mut self, enabled: bool) {
        self.events_enabled = enabled;
        debug!(
            "AMI session {}: events {}",
            self.id,
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Get session uptime.
    pub fn uptime(&self) -> std::time::Duration {
        self.session_start.elapsed()
    }
}

impl std::fmt::Debug for AmiSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmiSession")
            .field("id", &self.id)
            .field("addr", &self.addr)
            .field("authenticated", &self.authenticated)
            .field("username", &self.username)
            .field("events_enabled", &self.events_enabled)
            .finish()
    }
}

/// Errors that can occur during session operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// Failed to send data to the client.
    SendFailed,
    /// Session is not authenticated.
    NotAuthenticated,
    /// Permission denied.
    PermissionDenied,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendFailed => write!(f, "failed to send data to client"),
            Self::NotAuthenticated => write!(f, "session not authenticated"),
            Self::PermissionDenied => write!(f, "permission denied"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Writer task that drains the event channel and writes to the TCP socket.
pub async fn session_writer(
    mut rx: mpsc::Receiver<String>,
    mut writer: OwnedWriteHalf,
) {
    while let Some(data) = rx.recv().await {
        if let Err(e) = writer.write_all(data.as_bytes()).await {
            debug!("AMI session writer: write error: {}", e);
            break;
        }
    }
    debug!("AMI session writer: shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_session() -> (AmiSession, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(32);
        let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        (AmiSession::new(addr, tx), rx)
    }

    #[test]
    fn test_session_id_unique() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_session_creation() {
        let (session, _rx) = make_test_session();
        assert!(!session.authenticated);
        assert!(session.username.is_none());
        assert!(session.events_enabled);
    }

    #[test]
    fn test_session_authentication() {
        let (mut session, _rx) = make_test_session();
        let user = AmiUser::new("admin", "secret");
        session.authenticate(&user);
        assert!(session.authenticated);
        assert_eq!(session.username.as_deref(), Some("admin"));
        assert!(session.read_perm.contains(EventCategory::CALL));
    }

    #[test]
    fn test_event_filter() {
        let (mut session, _rx) = make_test_session();
        let user = AmiUser::new("admin", "secret");
        session.authenticate(&user);

        let call_event = AmiEvent::new("Newchannel", EventCategory::CALL.0);
        let dtmf_event = AmiEvent::new("DTMFBegin", EventCategory::DTMF.0);

        // All events pass by default
        assert!(session.should_receive_event(&call_event));
        assert!(session.should_receive_event(&dtmf_event));

        // Filter to call events only
        session.set_event_filter(EventCategory::CALL);
        assert!(session.should_receive_event(&call_event));
        assert!(!session.should_receive_event(&dtmf_event));
    }

    #[test]
    fn test_unauthenticated_no_events() {
        let (session, _rx) = make_test_session();
        let event = AmiEvent::new("Newchannel", EventCategory::CALL.0);
        assert!(!session.should_receive_event(&event));
    }

    #[test]
    fn test_events_disabled() {
        let (mut session, _rx) = make_test_session();
        let user = AmiUser::new("admin", "secret");
        session.authenticate(&user);
        session.set_events_enabled(false);

        let event = AmiEvent::new("Newchannel", EventCategory::CALL.0);
        assert!(!session.should_receive_event(&event));
    }

    #[tokio::test]
    async fn test_send_response() {
        let (session, mut rx) = make_test_session();
        let resp = AmiResponse::success("Test message");
        session.send_response(resp).await.unwrap();

        let data = rx.recv().await.unwrap();
        assert!(data.contains("Response: Success"));
        assert!(data.contains("Test message"));
    }
}
