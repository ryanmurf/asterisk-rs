//! AMI TCP server.
//!
//! The AmiServer listens for TCP connections on the configured port
//! (default 5038) and spawns a handler task for each connection.
//! Each connection gets an AmiSession and reads AMI actions line by
//! line, dispatching them to the action registry and sending responses
//! back over the socket.

use crate::actions::{ActionContext, ActionRegistry};
use crate::auth::{AmiUser, UserRegistry};
use crate::event_bus::AMI_EVENT_BUS;
use crate::protocol::{self, AmiAction, AmiEvent};
use crate::session::{self, AmiSession};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info};

/// Default AMI listen port.
pub const DEFAULT_AMI_PORT: u16 = 5038;

/// AMI server banner sent when a client connects.
const AMI_BANNER: &str = "Asterisk Call Manager/1.1\r\n";

/// Configuration for the AMI server.
#[derive(Debug, Clone)]
pub struct AmiServerConfig {
    /// Address and port to listen on.
    pub bind_addr: SocketAddr,
    /// Whether the AMI server is enabled.
    pub enabled: bool,
    /// Authentication timeout in seconds.
    pub auth_timeout: u64,
    /// Maximum number of unauthenticated sessions.
    pub auth_limit: usize,
    /// Whether to display connection messages.
    pub display_connects: bool,
    /// Whether to allow multiple logins from the same user.
    pub allow_multiple_login: bool,
}

impl Default for AmiServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_AMI_PORT)),
            enabled: true,
            auth_timeout: 30,
            auth_limit: 50,
            display_connects: true,
            allow_multiple_login: true,
        }
    }
}

/// The AMI server.
///
/// Manages the TCP listener, active sessions, user registry, and event
/// broadcasting.
pub struct AmiServer {
    /// Server configuration.
    pub config: AmiServerConfig,
    /// Registry of configured AMI users.
    pub user_registry: Arc<UserRegistry>,
    /// Registry of action handlers.
    pub action_registry: Arc<ActionRegistry>,
    /// Active sessions indexed by session ID.
    pub sessions: Arc<DashMap<String, Arc<RwLock<AmiSession>>>>,
    /// Broadcast channel for sending events to all sessions.
    event_tx: broadcast::Sender<AmiEvent>,
}

impl AmiServer {
    /// Create a new AMI server with the given configuration.
    pub fn new(config: AmiServerConfig) -> Self {
        let user_registry = Arc::new(UserRegistry::new());
        let action_registry = Arc::new(ActionRegistry::new(user_registry.clone()));
        let (event_tx, _) = broadcast::channel(1024);

        Self {
            config,
            user_registry,
            action_registry,
            sessions: Arc::new(DashMap::new()),
            event_tx,
        }
    }

    /// Add a user to the server's user registry.
    pub fn add_user(&self, user: AmiUser) {
        self.user_registry.add_user(user);
    }

    /// Start the AMI server with retry logic for port binding.
    ///
    /// This spawns the TCP listener task and returns immediately.
    /// The server runs until the returned handle is dropped.
    pub async fn start(&self) -> Result<(), std::io::Error> {
        if !self.config.enabled {
            info!("AMI: server is disabled");
            return Ok(());
        }

        const MAX_PORT_ATTEMPTS: usize = 10;
        let original_port = self.config.bind_addr.port();
        let mut current_addr = self.config.bind_addr;
        let mut last_error = None;

        let listener: TcpListener = loop {
            match TcpListener::bind(current_addr).await {
                Ok(listener) => {
                    let actual_addr = listener.local_addr()?;
                    if actual_addr.port() != original_port {
                        info!(
                            "AMI: Port {} was busy, successfully bound to port {} instead",
                            original_port, actual_addr.port()
                        );
                    }
                    info!("AMI: listening on {}", actual_addr);
                    break listener;
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::AddrInUse {
                        last_error = Some(e);
                        current_addr.set_port(current_addr.port() + 1);
                        debug!(
                            "AMI: Port {} busy, trying port {}",
                            current_addr.port() - 1,
                            current_addr.port()
                        );
                        
                        // Check if we've exceeded max attempts
                        if current_addr.port() > original_port + (MAX_PORT_ATTEMPTS as u16) {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::AddrInUse, 
                                "All attempted AMI ports are in use"
                            ));
                        }
                    } else {
                        // Non-port-conflict error, fail immediately
                        return Err(e);
                    }
                }
            }
        };

        let sessions = self.sessions.clone();
        let user_registry = self.user_registry.clone();
        let action_registry = self.action_registry.clone();
        let event_tx = self.event_tx.clone();
        let config = self.config.clone();

        // Also spawn a task that forwards events from the global AMI_EVENT_BUS
        // into the server's internal broadcast channel so that sessions get them.
        let event_tx_for_bus = event_tx.clone();
        tokio::spawn(async move {
            let mut bus_rx = AMI_EVENT_BUS.subscribe();
            loop {
                match bus_rx.recv().await {
                    Ok(event) => {
                        let _ = event_tx_for_bus.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("AMI server: global bus lagged by {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("AMI: new connection from {}", addr);

                        let sessions = sessions.clone();
                        let user_registry = user_registry.clone();
                        let action_registry = action_registry.clone();
                        let event_rx = event_tx.subscribe();
                        let config = config.clone();

                        tokio::spawn(async move {
                            Self::handle_connection(
                                stream,
                                addr,
                                sessions,
                                user_registry,
                                action_registry,
                                event_rx,
                                config,
                            )
                            .await;
                        });
                    }
                    Err(e) => {
                        error!("AMI: accept error: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    /// Handle a single AMI connection.
    async fn handle_connection(
        stream: TcpStream,
        addr: SocketAddr,
        sessions: Arc<DashMap<String, Arc<RwLock<AmiSession>>>>,
        user_registry: Arc<UserRegistry>,
        action_registry: Arc<ActionRegistry>,
        mut event_rx: broadcast::Receiver<AmiEvent>,
        _config: AmiServerConfig,
    ) {
        let (reader, writer) = stream.into_split();

        // Create the session's outbound channel
        let (send_tx, send_rx) = mpsc::channel::<String>(256);

        // Create the session
        let session = AmiSession::new(addr, send_tx.clone());
        let session_id = session.id.clone();
        let session = Arc::new(RwLock::new(session));
        sessions.insert(session_id.to_string(), session.clone());

        // Spawn the writer task
        let writer_handle = tokio::spawn(session::session_writer(send_rx, writer));

        // Send the AMI banner
        if let Err(e) = send_tx.send(AMI_BANNER.to_string()).await {
            debug!("AMI: failed to send banner: {}", e);
            sessions.remove(&session_id.to_string());
            return;
        }

        // Spawn event forwarding task
        let event_session = session.clone();
        let event_send_tx = send_tx.clone();
        let event_session_id = session_id.to_string();
        let event_handle = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        // Acquire and release the lock before awaiting send
                        let data = {
                            let sess = event_session.read();
                            if sess.should_receive_event(&event) {
                                Some(event.serialize())
                            } else {
                                None
                            }
                        };
                        if let Some(data) = data {
                            if event_send_tx.send(data).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("AMI session {}: lagged by {} events", event_session_id, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        // Create the action context
        let context = ActionContext {
            user_registry,
        };

        // Read and process actions
        let mut buf_reader = BufReader::new(reader);
        let mut message_buf = String::new();
        let mut line_buf = String::new();

        loop {
            line_buf.clear();
            match buf_reader.read_line(&mut line_buf).await {
                Ok(0) => {
                    // Connection closed
                    debug!("AMI session {}: connection closed", session_id);
                    break;
                }
                Ok(_) => {
                    message_buf.push_str(&line_buf);

                    // Check if we have a complete message (blank line)
                    if protocol::read_message(&message_buf).is_some() {
                        // Parse and dispatch the action
                        if let Some(action) = AmiAction::parse(&message_buf) {
                            debug!(
                                "AMI session {}: received action '{}'",
                                session_id, action.name
                            );

                            let response = {
                                let mut sess = session.write();
                                action_registry.dispatch(&action, &mut sess, &context)
                            };

                            let resp_data = response.serialize();
                            if send_tx.send(resp_data).await.is_err() {
                                break;
                            }

                            // Check if this was a Logoff
                            if action.name.eq_ignore_ascii_case("Logoff") {
                                break;
                            }
                        }
                        message_buf.clear();
                    }
                }
                Err(e) => {
                    debug!("AMI session {}: read error: {}", session_id, e);
                    break;
                }
            }
        }

        // Clean up
        info!("AMI session {}: disconnected from {}", session_id, addr);
        sessions.remove(&session_id.to_string());
        event_handle.abort();
        drop(send_tx);
        let _ = writer_handle.await;
    }

    /// Broadcast an event to all connected and authenticated sessions.
    ///
    /// Events are published to both the server's internal broadcast channel
    /// and the global `AMI_EVENT_BUS` so that sessions connected to any
    /// server instance can receive them.
    pub fn broadcast_event(&self, event: AmiEvent) {
        // Publish on the global bus (reaches all servers in the process)
        crate::event_bus::publish_event(event.clone());
        // Also send directly on this server's internal channel
        let _ = self.event_tx.send(event);
    }

    /// Get the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Get the number of authenticated sessions.
    pub fn authenticated_session_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|entry| entry.value().read().authenticated)
            .count()
    }

    /// List active sessions (session ID, username, remote address).
    pub fn list_sessions(&self) -> Vec<(String, Option<String>, SocketAddr)> {
        self.sessions
            .iter()
            .map(|entry| {
                let sess = entry.value().read();
                (
                    entry.key().clone(),
                    sess.username.clone(),
                    sess.addr,
                )
            })
            .collect()
    }

    /// Kick a session by ID.
    pub fn kick_session(&self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }
}

impl std::fmt::Debug for AmiServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmiServer")
            .field("config", &self.config)
            .field("sessions", &self.sessions.len())
            .field("users", &self.user_registry.count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AmiUser;
    use crate::events::EventCategory;

    #[test]
    fn test_server_config_default() {
        let config = AmiServerConfig::default();
        assert_eq!(config.bind_addr.port(), 5038);
        assert!(config.enabled);
        assert_eq!(config.auth_timeout, 30);
    }

    #[test]
    fn test_server_creation() {
        let server = AmiServer::new(AmiServerConfig::default());
        assert_eq!(server.session_count(), 0);
        assert_eq!(server.user_registry.count(), 0);
    }

    #[test]
    fn test_server_add_user() {
        let server = AmiServer::new(AmiServerConfig::default());
        server.add_user(AmiUser::new("admin", "secret"));
        assert_eq!(server.user_registry.count(), 1);
    }

    #[test]
    fn test_broadcast_event() {
        let server = AmiServer::new(AmiServerConfig::default());
        // Broadcasting with no sessions should not panic
        let event = AmiEvent::new("Test", EventCategory::SYSTEM.0);
        server.broadcast_event(event);
    }

    #[test]
    fn test_list_sessions_empty() {
        let server = AmiServer::new(AmiServerConfig::default());
        assert!(server.list_sessions().is_empty());
    }
}
