//! XMPP/Jabber client integration.
//!
//! Port of `res/res_xmpp.c`. Provides XMPP client connectivity for
//! sending/receiving instant messages, presence, and MWI notifications
//! over Jabber. Implements JabberSend(), JABBER_STATUS(), JABBER_RECEIVE().

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
pub enum XmppError {
    #[error("XMPP connection failed: {0}")]
    ConnectionError(String),
    #[error("XMPP client not found: {0}")]
    ClientNotFound(String),
    #[error("XMPP send failed: {0}")]
    SendError(String),
    #[error("XMPP error: {0}")]
    Other(String),
}

pub type XmppResult<T> = Result<T, XmppError>;

// ---------------------------------------------------------------------------
// Presence state
// ---------------------------------------------------------------------------

/// XMPP presence status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmppPresence {
    /// Available (online).
    Available,
    /// Chat (free for chat).
    Chat,
    /// Away.
    Away,
    /// Extended away (xa).
    ExtendedAway,
    /// Do not disturb.
    DoNotDisturb,
    /// Unavailable (offline).
    Unavailable,
}

impl XmppPresence {
    /// Parse from XMPP show element value.
    pub fn from_show(show: &str) -> Self {
        match show.to_lowercase().as_str() {
            "chat" => Self::Chat,
            "away" => Self::Away,
            "xa" => Self::ExtendedAway,
            "dnd" => Self::DoNotDisturb,
            _ => Self::Available,
        }
    }

    /// Convert to numeric status for JABBER_STATUS() function.
    pub fn as_status_number(&self) -> i32 {
        match self {
            Self::Available | Self::Chat => 1,
            Self::Away => 2,
            Self::ExtendedAway => 3,
            Self::DoNotDisturb => 4,
            Self::Unavailable => 5,
        }
    }
}

impl fmt::Display for XmppPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available => write!(f, "available"),
            Self::Chat => write!(f, "chat"),
            Self::Away => write!(f, "away"),
            Self::ExtendedAway => write!(f, "xa"),
            Self::DoNotDisturb => write!(f, "dnd"),
            Self::Unavailable => write!(f, "unavailable"),
        }
    }
}

// ---------------------------------------------------------------------------
// XMPP message
// ---------------------------------------------------------------------------

/// An XMPP instant message.
#[derive(Debug, Clone)]
pub struct XmppMessage {
    /// Sender JID.
    pub from: String,
    /// Recipient JID.
    pub to: String,
    /// Message body text.
    pub body: String,
    /// Timestamp.
    pub timestamp: u64,
}

impl XmppMessage {
    pub fn new(from: &str, to: &str, body: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            from: from.to_string(),
            to: to.to_string(),
            body: body.to_string(),
            timestamp: now,
        }
    }
}

// ---------------------------------------------------------------------------
// Buddy (contact) tracking
// ---------------------------------------------------------------------------

/// An XMPP buddy (roster entry) with presence.
#[derive(Debug, Clone)]
pub struct XmppBuddy {
    /// Full JID (user@domain/resource).
    pub jid: String,
    /// Current presence status.
    pub presence: XmppPresence,
    /// Status message text.
    pub status_message: String,
}

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

/// XMPP client connection configuration (from `xmpp.conf`).
#[derive(Debug, Clone)]
pub struct XmppClientConfig {
    /// Account name (section name in xmpp.conf).
    pub name: String,
    /// XMPP server hostname.
    pub server: String,
    /// XMPP server port.
    pub port: u16,
    /// Username (local part of JID).
    pub username: String,
    /// Password.
    pub password: String,
    /// Use TLS.
    pub use_tls: bool,
    /// Use SASL authentication.
    pub use_sasl: bool,
    /// Status message to set on connect.
    pub status_message: String,
    /// Priority value for presence.
    pub priority: i32,
    /// Operate as XMPP component (instead of client).
    pub component: bool,
}

impl Default for XmppClientConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            server: String::new(),
            port: 5222,
            username: String::new(),
            password: String::new(),
            use_tls: true,
            use_sasl: true,
            status_message: String::new(),
            priority: 1,
            component: false,
        }
    }
}

impl XmppClientConfig {
    /// Build the full JID from username and server.
    pub fn jid(&self) -> String {
        format!("{}@{}", self.username, self.server)
    }
}

// ---------------------------------------------------------------------------
// XMPP client
// ---------------------------------------------------------------------------

/// An XMPP client connection.
///
/// Port of the XMPP client from `res_xmpp.c`. Manages connection state,
/// roster/buddy tracking, and message send/receive.
#[derive(Debug)]
pub struct XmppClient {
    pub config: XmppClientConfig,
    /// Whether currently connected.
    connected: RwLock<bool>,
    /// Buddy roster with presence tracking.
    buddies: RwLock<HashMap<String, XmppBuddy>>,
    /// Queued received messages.
    message_queue: RwLock<Vec<XmppMessage>>,
}

impl XmppClient {
    pub fn new(config: XmppClientConfig) -> Self {
        Self {
            config,
            connected: RwLock::new(false),
            buddies: RwLock::new(HashMap::new()),
            message_queue: RwLock::new(Vec::new()),
        }
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        *self.connected.read()
    }

    /// Connect to the XMPP server (stub).
    pub fn connect(&self) -> XmppResult<()> {
        debug!(
            server = %self.config.server,
            jid = %self.config.jid(),
            "XMPP connect (stub)"
        );
        *self.connected.write() = true;
        info!(jid = %self.config.jid(), "XMPP connected (stub)");
        Ok(())
    }

    /// Disconnect from the XMPP server.
    pub fn disconnect(&self) {
        *self.connected.write() = false;
        info!(jid = %self.config.jid(), "XMPP disconnected");
    }

    /// Send a message to a JID (JabberSend application).
    pub fn send_message(&self, to: &str, _body: &str) -> XmppResult<()> {
        if !self.is_connected() {
            return Err(XmppError::ConnectionError("not connected".to_string()));
        }
        debug!(from = %self.config.jid(), to = to, "XMPP send message (stub)");
        Ok(())
    }

    /// Get the status of a buddy (JABBER_STATUS function).
    pub fn get_buddy_status(&self, jid: &str) -> Option<XmppPresence> {
        self.buddies.read().get(jid).map(|b| b.presence)
    }

    /// Update a buddy's presence (called when presence stanza received).
    pub fn update_buddy_presence(&self, jid: &str, presence: XmppPresence, status: &str) {
        let mut buddies = self.buddies.write();
        let buddy = buddies.entry(jid.to_string()).or_insert_with(|| XmppBuddy {
            jid: jid.to_string(),
            presence: XmppPresence::Unavailable,
            status_message: String::new(),
        });
        buddy.presence = presence;
        buddy.status_message = status.to_string();
    }

    /// Queue a received message.
    pub fn receive_message(&self, msg: XmppMessage) {
        self.message_queue.write().push(msg);
    }

    /// Pop the next received message (JABBER_RECEIVE function).
    pub fn pop_message(&self, from_jid: Option<&str>) -> Option<XmppMessage> {
        let mut queue = self.message_queue.write();
        if let Some(jid) = from_jid {
            if let Some(idx) = queue.iter().position(|m| m.from.starts_with(jid)) {
                return Some(queue.remove(idx));
            }
        } else if !queue.is_empty() {
            return Some(queue.remove(0));
        }
        None
    }

    /// List all tracked buddies.
    pub fn buddies(&self) -> Vec<XmppBuddy> {
        self.buddies.read().values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// XMPP manager (multi-account)
// ---------------------------------------------------------------------------

/// Manager for multiple XMPP client connections.
#[derive(Debug)]
pub struct XmppManager {
    clients: RwLock<HashMap<String, XmppClient>>,
}

impl XmppManager {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// Add a client configuration and create the client.
    pub fn add_client(&self, config: XmppClientConfig) {
        let name = config.name.clone();
        self.clients.write().insert(name, XmppClient::new(config));
    }

    /// Get a reference to a client by name.
    pub fn get_client(&self, name: &str) -> Option<XmppClientConfig> {
        self.clients.read().get(name).map(|c| c.config.clone())
    }

    /// Send a message via a named account (for JabberSend app).
    pub fn jabber_send(&self, account: &str, to: &str, message: &str) -> XmppResult<()> {
        let clients = self.clients.read();
        let client = clients
            .get(account)
            .ok_or_else(|| XmppError::ClientNotFound(account.to_string()))?;
        client.send_message(to, message)
    }

    /// Get buddy status via a named account (for JABBER_STATUS function).
    pub fn jabber_status(&self, account: &str, jid: &str) -> XmppResult<i32> {
        let clients = self.clients.read();
        let client = clients
            .get(account)
            .ok_or_else(|| XmppError::ClientNotFound(account.to_string()))?;
        Ok(client
            .get_buddy_status(jid)
            .unwrap_or(XmppPresence::Unavailable)
            .as_status_number())
    }

    /// List all configured account names.
    pub fn account_names(&self) -> Vec<String> {
        self.clients.read().keys().cloned().collect()
    }
}

impl Default for XmppManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presence_from_show() {
        assert_eq!(XmppPresence::from_show("chat"), XmppPresence::Chat);
        assert_eq!(XmppPresence::from_show("away"), XmppPresence::Away);
        assert_eq!(XmppPresence::from_show("dnd"), XmppPresence::DoNotDisturb);
        assert_eq!(XmppPresence::from_show("xa"), XmppPresence::ExtendedAway);
        assert_eq!(XmppPresence::from_show(""), XmppPresence::Available);
    }

    #[test]
    fn test_presence_status_number() {
        assert_eq!(XmppPresence::Available.as_status_number(), 1);
        assert_eq!(XmppPresence::Away.as_status_number(), 2);
        assert_eq!(XmppPresence::DoNotDisturb.as_status_number(), 4);
        assert_eq!(XmppPresence::Unavailable.as_status_number(), 5);
    }

    #[test]
    fn test_client_config_jid() {
        let config = XmppClientConfig {
            username: "asterisk".to_string(),
            server: "example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(config.jid(), "asterisk@example.com");
    }

    #[test]
    fn test_buddy_tracking() {
        let client = XmppClient::new(XmppClientConfig::default());
        client.update_buddy_presence("bob@example.com", XmppPresence::Away, "lunch");

        assert_eq!(
            client.get_buddy_status("bob@example.com"),
            Some(XmppPresence::Away)
        );
        assert_eq!(client.get_buddy_status("unknown@example.com"), None);
    }

    #[test]
    fn test_message_queue() {
        let client = XmppClient::new(XmppClientConfig::default());
        client.receive_message(XmppMessage::new("bob@example.com", "me@example.com", "hello"));
        client.receive_message(XmppMessage::new("alice@example.com", "me@example.com", "hi"));

        let msg = client.pop_message(Some("bob@example.com")).unwrap();
        assert_eq!(msg.body, "hello");

        let msg2 = client.pop_message(None).unwrap();
        assert_eq!(msg2.body, "hi");

        assert!(client.pop_message(None).is_none());
    }

    #[test]
    fn test_manager() {
        let manager = XmppManager::new();
        manager.add_client(XmppClientConfig {
            name: "office".to_string(),
            username: "asterisk".to_string(),
            server: "jabber.example.com".to_string(),
            ..Default::default()
        });

        assert!(manager.get_client("office").is_some());
        assert!(manager.get_client("missing").is_none());
    }
}
