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
use crate::rate_limit::{LoginRateLimitConfig, LoginRateLimiter};
use crate::session::{self, AmiSession};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// Default AMI listen port.
pub const DEFAULT_AMI_PORT: u16 = 5038;

/// AMI server banner sent when a client connects.
const AMI_BANNER: &str = "Asterisk Call Manager/1.1\r\n";

/// Maximum bytes buffered for a single un-dispatched AMI message before the
/// connection is dropped (issue #110). Mirrors the SIP parser's
/// `MAX_CONTENT_LENGTH` (64 KiB). Bounds a client that connects and never sends
/// the `\r\n\r\n` terminator (pre-auth memory-exhaustion DoS).
const MAX_AMI_MESSAGE_BYTES: usize = 64 * 1024;

/// Maximum bytes in a single AMI header line before the connection is dropped
/// (issue #110). Mirrors the SIP parser's `MAX_HEADER_VALUE_LEN` (8 KiB); also
/// bounds a client that sends one endless line with no newline.
const MAX_AMI_LINE_BYTES: usize = 8 * 1024;

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
    /// Per-source failed-`Login` rate-limit configuration (issue #130).
    pub login_rate_limit: LoginRateLimitConfig,
}

impl Default for AmiServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], DEFAULT_AMI_PORT)),
            enabled: false,
            auth_timeout: 30,
            auth_limit: 50,
            display_connects: true,
            allow_multiple_login: true,
            login_rate_limit: LoginRateLimitConfig::default(),
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
    /// Count of currently-connected but NOT-yet-authenticated sessions
    /// (issue #130 `auth_limit`). New connections past the cap are refused.
    unauth_sessions: Arc<AtomicUsize>,
    /// Per-source failed-`Login` rate limiter (issue #130), shared by every
    /// connection so guess volume is bounded per source address.
    login_rate_limiter: Arc<LoginRateLimiter>,
    /// The address the listener actually bound to (set by `start`). Lets callers
    /// (and tests) discover the real port when configured with port 0.
    bound_addr: Arc<RwLock<Option<SocketAddr>>>,
}

impl AmiServer {
    /// Create a new AMI server with the given configuration.
    pub fn new(config: AmiServerConfig) -> Self {
        let user_registry = Arc::new(UserRegistry::new());
        let action_registry = Arc::new(ActionRegistry::new(user_registry.clone()));
        let (event_tx, _) = broadcast::channel(1024);
        let login_rate_limiter =
            Arc::new(LoginRateLimiter::with_config(config.login_rate_limit.clone()));

        Self {
            config,
            user_registry,
            action_registry,
            sessions: Arc::new(DashMap::new()),
            event_tx,
            unauth_sessions: Arc::new(AtomicUsize::new(0)),
            login_rate_limiter,
            bound_addr: Arc::new(RwLock::new(None)),
        }
    }

    /// The address the listener bound to (available after `start` succeeds).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self.bound_addr.read()
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
        let mut _last_error = None;

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
                        _last_error = Some(e);
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

        // Record the actually-bound address so callers/tests can find the port
        // (relevant when configured with port 0, or after the busy-port retry).
        *self.bound_addr.write() = Some(listener.local_addr()?);

        let sessions = self.sessions.clone();
        let user_registry = self.user_registry.clone();
        let action_registry = self.action_registry.clone();
        let event_tx = self.event_tx.clone();
        let config = self.config.clone();
        let unauth_sessions = self.unauth_sessions.clone();
        let login_rate_limiter = self.login_rate_limiter.clone();

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
                        let unauth_sessions = unauth_sessions.clone();
                        let login_rate_limiter = login_rate_limiter.clone();

                        tokio::spawn(async move {
                            Self::handle_connection(
                                stream,
                                addr,
                                sessions,
                                user_registry,
                                action_registry,
                                event_rx,
                                config,
                                unauth_sessions,
                                login_rate_limiter,
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
    #[allow(clippy::too_many_arguments)]
    async fn handle_connection(
        stream: TcpStream,
        addr: SocketAddr,
        sessions: Arc<DashMap<String, Arc<RwLock<AmiSession>>>>,
        user_registry: Arc<UserRegistry>,
        action_registry: Arc<ActionRegistry>,
        mut event_rx: broadcast::Receiver<AmiEvent>,
        config: AmiServerConfig,
        unauth_sessions: Arc<AtomicUsize>,
        login_rate_limiter: Arc<LoginRateLimiter>,
    ) {
        // auth_limit (issue #130): bound the number of concurrent connections
        // that have NOT yet authenticated. Reserve a pre-auth slot up front;
        // refuse the connection if the cap is already reached. `fetch_add` +
        // rollback keeps this lock-free.
        let prior = unauth_sessions.fetch_add(1, Ordering::SeqCst);
        if prior >= config.auth_limit {
            unauth_sessions.fetch_sub(1, Ordering::SeqCst);
            warn!(
                "AMI: refusing connection from {} — unauthenticated session limit ({}) reached",
                addr, config.auth_limit
            );
            let mut stream = stream;
            let _ = stream
                .write_all(
                    b"Response: Error\r\nMessage: Too many unauthenticated connections\r\n\r\n",
                )
                .await;
            let _ = stream.shutdown().await;
            return;
        }
        // From here the connection holds exactly one pre-auth slot. `slot_held`
        // guarantees we release it exactly once — on authentication OR teardown.
        let mut slot_held = true;

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
            unauth_sessions.fetch_sub(1, Ordering::SeqCst);
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
            login_rate_limiter,
        };

        // auth_timeout (issue #130): a fixed deadline by which the session must
        // authenticate. `sleep_until` on a fixed instant does NOT reset per
        // iteration; the `if !authed` guard disables the branch once the
        // session authenticates, so a logged-in session lives indefinitely.
        let auth_deadline =
            tokio::time::Instant::now() + Duration::from_secs(config.auth_timeout);

        // Bounded reader (issue #110): read the AMI stream as CRLF-framed lines,
        // capping each line at MAX_AMI_LINE_BYTES and the accumulated
        // un-dispatched message at MAX_AMI_MESSAGE_BYTES so a client that never
        // sends the `\r\n\r\n` terminator cannot exhaust memory. `Take::set_limit`
        // re-arms the per-line cap each iteration.
        let mut limited = BufReader::new(reader).take(MAX_AMI_LINE_BYTES as u64);
        let mut message_buf = String::new();
        let mut line_bytes: Vec<u8> = Vec::new();

        loop {
            let authenticated = session.read().authenticated;

            // Release the pre-auth slot the moment the session authenticates.
            if authenticated && slot_held {
                unauth_sessions.fetch_sub(1, Ordering::SeqCst);
                slot_held = false;
            }

            line_bytes.clear();
            limited.set_limit(MAX_AMI_LINE_BYTES as u64);

            let read_result = tokio::select! {
                r = limited.read_until(b'\n', &mut line_bytes) => r,
                _ = tokio::time::sleep_until(auth_deadline), if !authenticated => {
                    warn!(
                        "AMI session {}: authentication timeout ({}s) — dropping unauthenticated connection from {}",
                        session_id, config.auth_timeout, addr
                    );
                    let _ = send_tx
                        .send("Response: Error\r\nMessage: Authentication timeout\r\n\r\n".to_string())
                        .await;
                    break;
                }
            };

            match read_result {
                Ok(0) => {
                    // Connection closed
                    debug!("AMI session {}: connection closed", session_id);
                    break;
                }
                Ok(_) => {
                    // Per-line cap: the take limit hit 0 without a terminating
                    // newline means the line exceeds MAX_AMI_LINE_BYTES.
                    if limited.limit() == 0 && line_bytes.last() != Some(&b'\n') {
                        warn!(
                            "AMI session {}: line exceeded {} bytes — dropping connection from {}",
                            session_id, MAX_AMI_LINE_BYTES, addr
                        );
                        let _ = send_tx
                            .send("Response: Error\r\nMessage: Line too long\r\n\r\n".to_string())
                            .await;
                        break;
                    }

                    message_buf.push_str(&String::from_utf8_lossy(&line_bytes));

                    // Total-message cap: bound the un-dispatched buffer.
                    if message_buf.len() > MAX_AMI_MESSAGE_BYTES {
                        warn!(
                            "AMI session {}: message exceeded {} bytes without terminator — dropping connection from {}",
                            session_id, MAX_AMI_MESSAGE_BYTES, addr
                        );
                        let _ = send_tx
                            .send("Response: Error\r\nMessage: Message too long\r\n\r\n".to_string())
                            .await;
                        break;
                    }

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
        if slot_held {
            // Never authenticated: release the pre-auth slot on teardown.
            unauth_sessions.fetch_sub(1, Ordering::SeqCst);
        }
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
        assert_eq!(config.bind_addr.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert_eq!(config.bind_addr.port(), 5038);
        assert!(!config.enabled);
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

    // -----------------------------------------------------------------------
    // Pre-auth DoS limits: #110 (bounded read buffer) + #130 (auth_timeout,
    // auth_limit, login rate-limit).
    //
    // These drive a real bound server over TCP. Each drop path sends a specific
    // error line before closing, so the tests assert BOTH the correct reason and
    // the drop. Every test is red-capable (neg-control noted on each): defeating
    // the corresponding enforcement makes the awaited message/EOF never arrive,
    // so `drain_until` times out and the assertion fails.
    // -----------------------------------------------------------------------

    async fn spawn_test_server(mut config: AmiServerConfig) -> (AmiServer, SocketAddr) {
        config.enabled = true;
        config.bind_addr = "127.0.0.1:0".parse().unwrap();
        let server = AmiServer::new(config);
        server.add_user(AmiUser::new("admin", "correct-secret"));
        server.start().await.expect("server start");
        let addr = server.local_addr().expect("bound addr");
        (server, addr)
    }

    /// Read from `client` until `needle` appears (Ok) or the deadline elapses
    /// (Err). Returns on EOF too, with whatever was accumulated.
    async fn drain_until(client: &mut TcpStream, needle: &str, dur: Duration) -> Result<String, ()> {
        let fut = async {
            let mut acc: Vec<u8> = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                match client.read(&mut buf).await {
                    Ok(0) => return String::from_utf8_lossy(&acc).into_owned(),
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if String::from_utf8_lossy(&acc).contains(needle) {
                            return String::from_utf8_lossy(&acc).into_owned();
                        }
                    }
                    Err(_) => return String::from_utf8_lossy(&acc).into_owned(),
                }
            }
        };
        tokio::time::timeout(dur, fut).await.map_err(|_| ())
    }

    /// Wait until the server closes the connection (EOF) or the deadline
    /// elapses. `true` = closed within `dur`.
    async fn expect_closed(client: &mut TcpStream, dur: Duration) -> bool {
        let fut = async {
            let mut buf = [0u8; 1024];
            loop {
                match client.read(&mut buf).await {
                    Ok(0) => return true,
                    Ok(_) => continue,
                    Err(_) => return true,
                }
            }
        };
        tokio::time::timeout(dur, fut).await.unwrap_or(false)
    }

    /// #130 auth_timeout: an unauthenticated connection is dropped after the
    /// deadline. Neg-control: don't spawn the deadline branch → the conn lives →
    /// `drain_until` for the timeout message times out → RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_auth_timeout_drops_unauthenticated_connection() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_timeout: 1,
            ..Default::default()
        })
        .await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Never authenticate. Server must send the timeout error and close.
        let got = drain_until(&mut client, "Authentication timeout", Duration::from_secs(4)).await;
        assert!(
            got.is_ok_and(|s| s.contains("Authentication timeout")),
            "unauthenticated connection must be dropped after auth_timeout"
        );
    }

    /// #130 auth_timeout: a session that logs in before the deadline survives it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_authenticated_session_survives_auth_timeout() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_timeout: 1,
            ..Default::default()
        })
        .await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"Action: Login\r\nUsername: admin\r\nSecret: correct-secret\r\n\r\n")
            .await
            .unwrap();
        assert!(
            drain_until(&mut client, "Authentication accepted", Duration::from_secs(2))
                .await
                .is_ok_and(|s| s.contains("Authentication accepted"))
        );

        // Wait well past the (1s) auth deadline; the session must stay alive.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        client.write_all(b"Action: Ping\r\n\r\n").await.unwrap();
        assert!(
            drain_until(&mut client, "Pong", Duration::from_secs(2))
                .await
                .is_ok_and(|s| s.contains("Pong")),
            "authenticated session must survive the auth deadline"
        );
    }

    /// #130 auth_limit: the (cap+1)th concurrent pre-auth connection is refused.
    /// Neg-control: don't enforce the cap → the 3rd is accepted (banner, stays
    /// open) → `drain_until` for the refusal times out → RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_auth_limit_refuses_excess_preauth_connections() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_limit: 2,
            auth_timeout: 30, // isolate from the timeout path
            ..Default::default()
        })
        .await;

        // Hold two unauthenticated connections; reading their banners guarantees
        // each has reserved its pre-auth slot (the counter increments before the
        // banner is sent).
        let mut c1 = TcpStream::connect(addr).await.unwrap();
        let mut c2 = TcpStream::connect(addr).await.unwrap();
        assert!(drain_until(&mut c1, "Call Manager", Duration::from_secs(2)).await.is_ok());
        assert!(drain_until(&mut c2, "Call Manager", Duration::from_secs(2)).await.is_ok());

        // The third pre-auth connection must be refused with the limit message.
        let mut c3 = TcpStream::connect(addr).await.unwrap();
        assert!(
            drain_until(&mut c3, "Too many unauthenticated", Duration::from_secs(3))
                .await
                .is_ok_and(|s| s.contains("Too many unauthenticated")),
            "the (auth_limit+1)th pre-auth connection must be refused"
        );
        assert!(expect_closed(&mut c3, Duration::from_secs(2)).await, "refused conn must close");

        drop((c1, c2));
    }

    /// #110 bounded pre-auth read buffer: a single oversized line (no newline,
    /// exceeding MAX_AMI_LINE_BYTES) drops the connection. Neg-control: remove
    /// the cap → the server keeps buffering with no drop/error → `drain_until`
    /// times out → RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_preauth_oversized_line_drops_connection() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_timeout: 30, // isolate from the timeout path
            ..Default::default()
        })
        .await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        // One giant pre-auth line, no terminator, exceeding the 8 KiB line cap.
        let junk = vec![b'A'; MAX_AMI_LINE_BYTES + 1024];
        let _ = client.write_all(&junk).await; // server may close mid-write

        assert!(
            drain_until(&mut client, "Line too long", Duration::from_secs(3))
                .await
                .is_ok_and(|s| s.contains("Line too long")),
            "an oversized pre-auth line must be rejected and the connection dropped"
        );
    }

    /// #110 bounded pre-auth read buffer: many short lines with no message
    /// terminator cannot grow the buffer without bound — it is dropped once the
    /// total exceeds MAX_AMI_MESSAGE_BYTES. Neg-control: remove the total cap →
    /// no drop → RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_preauth_unterminated_message_is_bounded() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_timeout: 30,
            ..Default::default()
        })
        .await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Short header lines (each < line cap) but never a blank line; total
        // must exceed the message cap and be dropped.
        let mut filler = Vec::new();
        while filler.len() <= MAX_AMI_MESSAGE_BYTES + 2048 {
            filler.extend_from_slice(b"Header: value\r\n");
        }
        let _ = client.write_all(&filler).await;

        assert!(
            drain_until(&mut client, "Message too long", Duration::from_secs(3))
                .await
                .is_ok_and(|s| s.contains("Message too long")),
            "an unterminated pre-auth message must be bounded and dropped"
        );
    }

    /// #130 login rate-limit: after N failed logins from a source, the next is
    /// throttled rather than processed. Bogus wrong passwords only. Neg-control:
    /// disable the limiter → the (N+1)th returns the normal auth failure, never
    /// the throttle message → `drain_until` times out → RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_login_rate_limit_throttles_repeated_failures() {
        let (_server, addr) = spawn_test_server(AmiServerConfig {
            auth_timeout: 30,
            login_rate_limit: LoginRateLimitConfig {
                max_failures: 3,
                window: Duration::from_secs(60),
                block_duration: Duration::from_secs(60),
                enabled: true,
            },
            ..Default::default()
        })
        .await;

        // Three failed logins from this source (obviously-bogus wrong password).
        for _ in 0..3 {
            let mut c = TcpStream::connect(addr).await.unwrap();
            c.write_all(b"Action: Login\r\nUsername: admin\r\nSecret: wrong-guess\r\n\r\n")
                .await
                .unwrap();
            assert!(
                drain_until(&mut c, "Authentication failed", Duration::from_secs(2))
                    .await
                    .is_ok_and(|s| s.contains("Authentication failed")),
                "a wrong-password login should fail normally under the threshold"
            );
        }

        // The fourth attempt from the same source must be throttled.
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"Action: Login\r\nUsername: admin\r\nSecret: wrong-guess\r\n\r\n")
            .await
            .unwrap();
        assert!(
            drain_until(&mut c, "Login rate exceeded", Duration::from_secs(2))
                .await
                .is_ok_and(|s| s.contains("Login rate exceeded")),
            "the (N+1)th failed login from a source must be throttled, not processed"
        );
    }
}
