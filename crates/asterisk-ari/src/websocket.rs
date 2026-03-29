//! WebSocket event streaming for ARI.
//!
//! Manages WebSocket connections from ARI clients. Each client connects to
//! /ari/events?app=... and receives JSON-encoded ARI events for the specified
//! Stasis applications. This is the Rust equivalent of the WebSocket portion
//! of res_ari.c and res_ari_events.c.

use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// A handle to send JSON event strings to a connected WebSocket client.
pub type EventSender = mpsc::UnboundedSender<String>;

/// A receiver for JSON event strings on a WebSocket connection.
pub type EventReceiver = mpsc::UnboundedReceiver<String>;

/// A single WebSocket session connected to one or more Stasis applications.
#[derive(Debug)]
pub struct WebSocketSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Application names this session is subscribed to.
    pub app_names: RwLock<Vec<String>>,
    /// Channel for sending events to this session.
    pub sender: EventSender,
}

impl WebSocketSession {
    /// Create a new WebSocket session.
    pub fn new(session_id: String, app_names: Vec<String>, sender: EventSender) -> Self {
        Self {
            session_id,
            app_names: RwLock::new(app_names),
            sender,
        }
    }

    /// Send a JSON event string to this session.
    pub fn send_event(&self, json: &str) -> bool {
        match self.sender.send(json.to_string()) {
            Ok(_) => true,
            Err(_) => {
                debug!(
                    "WebSocket session '{}' send failed (disconnected)",
                    self.session_id
                );
                false
            }
        }
    }

    /// Check if this session is subscribed to the given application.
    pub fn is_subscribed_to(&self, app_name: &str) -> bool {
        self.app_names.read().iter().any(|n| n == app_name)
    }

    /// Add an application subscription.
    pub fn subscribe_app(&self, app_name: &str) {
        let mut names = self.app_names.write();
        if !names.iter().any(|n| n == app_name) {
            names.push(app_name.to_string());
        }
    }

    /// Remove an application subscription.
    pub fn unsubscribe_app(&self, app_name: &str) {
        self.app_names.write().retain(|n| n != app_name);
    }
}

/// Manages all active WebSocket sessions for ARI event streaming.
///
/// Sessions are keyed by session ID. Events are broadcast to all sessions
/// that are subscribed to the relevant application name.
pub struct WebSocketSessionManager {
    /// Active sessions indexed by session ID.
    sessions: DashMap<String, Arc<WebSocketSession>>,
    /// Reverse index: app name -> set of session IDs subscribed to that app.
    app_sessions: DashMap<String, Vec<String>>,
}

impl WebSocketSessionManager {
    /// Create a new session manager.
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            app_sessions: DashMap::new(),
        }
    }

    /// Register a new WebSocket session.
    ///
    /// Returns a receiver that the WebSocket handler should read from
    /// to get JSON event strings to send to the client.
    pub fn register_session(
        &self,
        session_id: String,
        app_names: Vec<String>,
    ) -> (Arc<WebSocketSession>, EventReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        let session = Arc::new(WebSocketSession::new(session_id.clone(), app_names.clone(), tx));

        // Add to the session map
        self.sessions.insert(session_id.clone(), session.clone());

        // Add to the app->sessions reverse index
        for app_name in &app_names {
            self.app_sessions
                .entry(app_name.clone())
                .or_insert_with(Vec::new)
                .push(session_id.clone());
        }

        info!(
            "WebSocket session '{}' registered for apps: {:?}",
            session_id, app_names
        );

        (session, rx)
    }

    /// Unregister a WebSocket session (on disconnect).
    pub fn unregister_session(&self, session_id: &str) {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            // Remove from the app->sessions reverse index
            let app_names = session.app_names.read().clone();
            for app_name in &app_names {
                if let Some(mut sessions) = self.app_sessions.get_mut(app_name) {
                    sessions.retain(|id| id != session_id);
                }
            }
            info!("WebSocket session '{}' unregistered", session_id);
        }
    }

    /// Send a JSON event string to all sessions subscribed to the given application.
    pub fn send_to_app(&self, app_name: &str, json: &str) {
        // Get session IDs for this app
        let session_ids: Vec<String> = match self.app_sessions.get(app_name) {
            Some(ids) => ids.clone(),
            None => return,
        };

        let mut disconnected = Vec::new();

        for session_id in &session_ids {
            if let Some(session) = self.sessions.get(session_id) {
                if !session.send_event(json) {
                    disconnected.push(session_id.clone());
                }
            } else {
                disconnected.push(session_id.clone());
            }
        }

        // Clean up disconnected sessions
        if !disconnected.is_empty() {
            if let Some(mut ids) = self.app_sessions.get_mut(app_name) {
                ids.retain(|id| !disconnected.contains(id));
            }
            for id in &disconnected {
                self.sessions.remove(id);
            }
        }
    }

    /// Broadcast a JSON event string to all connected sessions (all apps).
    pub fn broadcast_all(&self, json: &str) {
        let mut disconnected = Vec::new();
        for entry in self.sessions.iter() {
            if !entry.value().send_event(json) {
                disconnected.push(entry.key().clone());
            }
        }
        for id in disconnected {
            self.unregister_session(&id);
        }
    }

    /// Get the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get the number of sessions subscribed to a specific application.
    pub fn app_session_count(&self, app_name: &str) -> usize {
        self.app_sessions
            .get(app_name)
            .map(|ids| ids.len())
            .unwrap_or(0)
    }
}

impl Default for WebSocketSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WebSocketSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketSessionManager")
            .field("session_count", &self.sessions.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_register_unregister() {
        let manager = WebSocketSessionManager::new();
        let (session, _rx) = manager.register_session(
            "test-session-1".to_string(),
            vec!["my-app".to_string()],
        );
        assert_eq!(session.session_id, "test-session-1");
        assert_eq!(manager.session_count(), 1);
        assert_eq!(manager.app_session_count("my-app"), 1);

        manager.unregister_session("test-session-1");
        assert_eq!(manager.session_count(), 0);
    }

    #[test]
    fn test_send_to_app() {
        let manager = WebSocketSessionManager::new();
        let (_session, mut rx) = manager.register_session(
            "s1".to_string(),
            vec!["app1".to_string()],
        );

        manager.send_to_app("app1", r#"{"type":"StasisStart"}"#);

        // The event should be in the receiver
        let msg = rx.try_recv().unwrap();
        assert!(msg.contains("StasisStart"));
    }

    #[test]
    fn test_send_to_unsubscribed_app() {
        let manager = WebSocketSessionManager::new();
        let (_session, mut rx) = manager.register_session(
            "s1".to_string(),
            vec!["app1".to_string()],
        );

        // Send to a different app -- should not be received
        manager.send_to_app("app2", r#"{"type":"StasisStart"}"#);

        assert!(rx.try_recv().is_err());
    }
}
