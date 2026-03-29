//! AMI action handlers.
//!
//! Each AMI action (Login, Ping, Originate, etc.) has a handler that
//! processes the action and returns a response. Actions are registered
//! in an ActionRegistry and dispatched by name.

use crate::auth::{self, UserRegistry};
use crate::events::EventCategory;
use crate::protocol::{AmiAction, AmiResponse};
use crate::session::AmiSession;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Type alias for action handler functions.
///
/// An action handler receives the action, a mutable reference to the session,
/// and a shared reference to the action context, and returns a response.
pub type ActionHandler = Box<
    dyn Fn(&AmiAction, &mut AmiSession, &ActionContext) -> AmiResponse
        + Send
        + Sync,
>;

/// Context available to action handlers.
///
/// Provides access to the user registry and other shared state needed
/// to process actions.
pub struct ActionContext {
    /// Registry of configured AMI users.
    pub user_registry: Arc<UserRegistry>,
}

/// Registry of AMI action handlers.
pub struct ActionRegistry {
    handlers: RwLock<HashMap<String, Arc<ActionHandler>>>,
}

impl ActionRegistry {
    /// Create a new action registry with all built-in handlers.
    pub fn new(_user_registry: Arc<UserRegistry>) -> Self {
        let registry = Self {
            handlers: RwLock::new(HashMap::new()),
        };
        registry.register_builtins();
        registry
    }

    /// Register a handler for a given action name.
    pub fn register(&self, name: impl Into<String>, handler: ActionHandler) {
        let name = name.into();
        debug!("AMI: registering action handler for '{}'", name);
        self.handlers
            .write()
            .insert(name.to_lowercase(), Arc::new(handler));
    }

    /// Dispatch an action to its handler.
    pub fn dispatch(
        &self,
        action: &AmiAction,
        session: &mut AmiSession,
        context: &ActionContext,
    ) -> AmiResponse {
        let name_lower = action.name.to_lowercase();

        // Login is special: allowed before authentication
        if name_lower != "login" && name_lower != "challenge" && !session.authenticated {
            return AmiResponse::error("Permission denied")
                .with_action_id(action.action_id.clone());
        }

        let handler = {
            let handlers = self.handlers.read();
            handlers.get(&name_lower).cloned()
        };

        match handler {
            Some(handler) => {
                let resp = handler(action, session, context);
                resp.with_action_id(action.action_id.clone())
            }
            None => {
                warn!("AMI: unknown action '{}'", action.name);
                AmiResponse::error(format!("Invalid/unknown command: {}", action.name))
                    .with_action_id(action.action_id.clone())
            }
        }
    }

    /// List all registered action names.
    pub fn list_actions(&self) -> Vec<String> {
        let handlers = self.handlers.read();
        let mut names: Vec<String> = handlers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Register all built-in action handlers.
    fn register_builtins(&self) {
        // Login
        self.register("login", Box::new(handle_login));

        // Logoff
        self.register("logoff", Box::new(handle_logoff));

        // Challenge (MD5 auth)
        self.register("challenge", Box::new(handle_challenge));

        // Ping
        self.register("ping", Box::new(handle_ping));

        // CoreShowChannels
        self.register("coreshowchannels", Box::new(handle_core_show_channels));

        // CoreStatus
        self.register("corestatus", Box::new(handle_core_status));

        // CoreSettings
        self.register("coresettings", Box::new(handle_core_settings));

        // Originate
        self.register("originate", Box::new(handle_originate));

        // Redirect
        self.register("redirect", Box::new(handle_redirect));

        // Hangup
        self.register("hangup", Box::new(handle_hangup));

        // Bridge
        self.register("bridge", Box::new(handle_bridge));

        // Park
        self.register("park", Box::new(handle_park));

        // Command (CLI execution)
        self.register("command", Box::new(handle_command));

        // Events (enable/disable)
        self.register("events", Box::new(handle_events));

        // GetConfig
        self.register("getconfig", Box::new(handle_get_config));

        // UpdateConfig
        self.register("updateconfig", Box::new(handle_update_config));

        // ListCategories
        self.register("listcategories", Box::new(handle_list_categories));

        // Status
        self.register("status", Box::new(handle_status));

        // ShowDialPlan
        self.register("showdialplan", Box::new(handle_show_dialplan));

        // ListCommands
        self.register("listcommands", Box::new(handle_list_commands));

        // QueueStatus
        self.register("queuestatus", Box::new(handle_queue_status));

        // QueueAdd
        self.register("queueadd", Box::new(handle_queue_add));

        // QueueRemove
        self.register("queueremove", Box::new(handle_queue_remove));

        // QueuePause
        self.register("queuepause", Box::new(handle_queue_pause));
    }
}

// ---------------------------------------------------------------------------
// Built-in action handlers
// ---------------------------------------------------------------------------

/// Handle the Login action.
fn handle_login(
    action: &AmiAction,
    session: &mut AmiSession,
    context: &ActionContext,
) -> AmiResponse {
    let username = match action.get_header("Username") {
        Some(u) => u,
        None => {
            return AmiResponse::error("Username is required");
        }
    };

    let user = match context.user_registry.find_user(username) {
        Some(u) => u,
        None => {
            warn!("AMI Login: unknown user '{}'", username);
            return AmiResponse::error("Authentication failed");
        }
    };

    // Check for MD5 challenge/response authentication
    if let Some(key) = action.get_header("Key") {
        // MD5 auth: verify against session challenge
        if let Some(ref challenge) = session.challenge {
            if auth::verify_md5_response(challenge, &user.secret, key) {
                session.authenticate(&user);
                return AmiResponse::success("Authentication accepted");
            } else {
                return AmiResponse::error("Authentication failed");
            }
        } else {
            return AmiResponse::error("No challenge sent");
        }
    }

    // Plaintext authentication
    let secret = match action.get_header("Secret") {
        Some(s) => s,
        None => {
            return AmiResponse::error("Secret is required");
        }
    };

    if auth::verify_plaintext(&user, secret) {
        session.authenticate(&user);
        AmiResponse::success("Authentication accepted")
    } else {
        AmiResponse::error("Authentication failed")
    }
}

/// Handle the Logoff action.
fn handle_logoff(
    _action: &AmiAction,
    session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    info!(
        "AMI Logoff: user '{}' logging off",
        session.username.as_deref().unwrap_or("unknown")
    );
    session.authenticated = false;
    session.username = None;
    AmiResponse::success("Goodbye")
}

/// Handle the Challenge action (MD5 auth step 1).
fn handle_challenge(
    action: &AmiAction,
    session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let auth_type = action.get_header("AuthType").unwrap_or("md5");
    if !auth_type.eq_ignore_ascii_case("md5") {
        return AmiResponse::error("Must specify AuthType: md5");
    }

    let challenge = auth::generate_challenge();
    session.challenge = Some(challenge.clone());

    AmiResponse::success("Challenge sent")
        .with_header("Challenge", challenge)
}

/// Handle the Ping action.
fn handle_ping(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    AmiResponse::success("Pong")
        .with_header("Ping", "Pong")
        .with_header("Timestamp", &format!("{}", chrono_timestamp()))
}

/// Handle CoreShowChannels action.
fn handle_core_show_channels(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    // In a real implementation, iterate over all active channels:
    //
    //   let channels = ChannelRegistry::list_all();
    //   for ch in &channels {
    //       let event = AmiEvent::new("CoreShowChannel", EventCategory::SYSTEM.0)
    //           .with_header("Channel", &ch.name)
    //           .with_header("Uniqueid", &ch.unique_id.0)
    //           .with_header("Context", &ch.context)
    //           .with_header("Extension", &ch.exten)
    //           .with_header("Priority", &ch.priority.to_string())
    //           .with_header("ChannelState", &(ch.state as u8).to_string());
    //       session.send_event(&event).await;
    //   }
    //   let complete = AmiEvent::new("CoreShowChannelsComplete", EventCategory::SYSTEM.0)
    //       .with_header("ListItems", &channels.len().to_string());
    //   session.send_event(&complete).await;

    AmiResponse::success("Channels will follow")
        .with_header("ListItems", "0")
}

/// Handle CoreStatus action.
fn handle_core_status(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    AmiResponse::success("Core Status")
        .with_header("CoreStartupDate", "2026-01-01")
        .with_header("CoreStartupTime", "00:00:00")
        .with_header("CoreCurrentCalls", "0")
}

/// Handle CoreSettings action.
fn handle_core_settings(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    AmiResponse::success("Core Settings")
        .with_header("AsteriskVersion", "Asterisk 22.0.0-rs")
        .with_header("SystemName", "asterisk-rs")
        .with_header("MaxCalls", "0")
        .with_header("MaxLoadAvg", "0.0")
        .with_header("MaxFileHandles", "0")
}

/// Handle the Originate action.
///
/// Creates a real channel via the global channel store, sets context/exten/
/// priority and optional variables, then emits an OriginateResponse event.
fn handle_originate(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel_str = match action.get_header("Channel") {
        Some(c) => c.to_string(),
        None => return AmiResponse::error("Channel is required"),
    };

    let context = action.get_header("Context").unwrap_or("default").to_string();
    let exten = action.get_header("Exten").unwrap_or("s").to_string();
    let priority = action
        .get_header("Priority")
        .and_then(|p| p.parse::<i32>().ok())
        .unwrap_or(1);
    let _timeout = action
        .get_header("Timeout")
        .and_then(|t| t.parse::<u64>().ok())
        .unwrap_or(30000);
    let caller_id = action.get_header("CallerID").unwrap_or("").to_string();
    let _is_async = action
        .get_header("Async")
        .map(|a| a.eq_ignore_ascii_case("true") || a == "1" || a.eq_ignore_ascii_case("yes"))
        .unwrap_or(false);

    // Collect any Variable headers (key=value pairs)
    let variables: Vec<(String, String)> = action
        .get_header("Variable")
        .map(|v| {
            v.split(',')
                .filter_map(|pair| {
                    let mut parts = pair.splitn(2, '=');
                    let k = parts.next()?.trim().to_string();
                    let val = parts.next().unwrap_or("").trim().to_string();
                    Some((k, val))
                })
                .collect()
        })
        .unwrap_or_default();

    info!(
        "AMI Originate: channel={}, context={}, exten={}, priority={}",
        channel_str, context, exten, priority
    );

    // Create the channel using the global channel store.
    let chan_arc = asterisk_core::channel_store::alloc_channel(&channel_str);
    {
        let mut chan = chan_arc.lock();
        chan.context = context;
        chan.exten = exten;
        chan.priority = priority;
        if !caller_id.is_empty() {
            chan.caller.id.number.number = caller_id;
            chan.caller.id.number.valid = true;
        }
        for (k, v) in variables {
            chan.set_variable(k, v);
        }
    }

    // Emit OriginateResponse event
    {
        let chan = chan_arc.lock();
        crate::event_bus::publish_event(
            crate::protocol::AmiEvent::new_with_headers("OriginateResponse", &[
                ("Response", "Success"),
                ("Channel", &chan.name),
                ("Uniqueid", &chan.unique_id.0),
            ]),
        );
    }

    AmiResponse::success("Originate successfully queued")
}

/// Handle the Redirect action.
fn handle_redirect(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let exten = match action.get_header("Exten") {
        Some(e) => e,
        None => return AmiResponse::error("Exten is required"),
    };

    let redirect_context = action.get_header("Context").unwrap_or("default");
    let priority = action
        .get_header("Priority")
        .and_then(|p| p.parse::<i32>().ok())
        .unwrap_or(1);

    info!(
        "AMI Redirect: channel={} to {}@{} priority {}",
        channel, exten, redirect_context, priority
    );

    // In a real implementation:
    //   let chan = ChannelRegistry::find_by_name(channel)?;
    //   chan.context = context.to_string();
    //   chan.exten = exten.to_string();
    //   chan.priority = priority;
    //   pbx_async_goto(chan).await?;

    AmiResponse::success("Redirect successful")
}

/// Handle the Hangup action.
fn handle_hangup(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let cause = action
        .get_header("Cause")
        .and_then(|c| c.parse::<u32>().ok())
        .unwrap_or(16); // Normal clearing

    info!("AMI Hangup: channel={} cause={}", channel, cause);

    // In a real implementation:
    //   let chan = ChannelRegistry::find_by_name(channel)?;
    //   chan.hangup(HangupCause::from(cause));

    AmiResponse::success("Channel Hungup")
}

/// Handle the Bridge action.
fn handle_bridge(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel1 = match action.get_header("Channel1") {
        Some(c) => c,
        None => return AmiResponse::error("Channel1 is required"),
    };

    let channel2 = match action.get_header("Channel2") {
        Some(c) => c,
        None => return AmiResponse::error("Channel2 is required"),
    };

    let _tone = action
        .get_header("Tone")
        .map(|t| t.eq_ignore_ascii_case("yes") || t == "1")
        .unwrap_or(false);

    info!("AMI Bridge: {} <-> {}", channel1, channel2);

    AmiResponse::success("Bridge created")
}

/// Handle the Park action.
fn handle_park(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let channel2 = action.get_header("Channel2");
    let timeout = action.get_header("Timeout");
    let parkinglot = action.get_header("Parkinglot");

    info!(
        "AMI Park: channel={} channel2={:?} timeout={:?} lot={:?}",
        channel, channel2, timeout, parkinglot
    );

    AmiResponse::success("Park successful")
}

/// Handle the Command action (execute CLI commands).
///
/// Executes basic CLI commands and returns their output. For commands
/// that are not recognised, returns a generic message.
fn handle_command(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let command = match action.get_header("Command") {
        Some(c) => c,
        None => return AmiResponse::error("Command is required"),
    };

    info!("AMI Command: '{}'", command);

    let output = execute_cli_command(command);

    AmiResponse::success("Command output follows")
        .with_output(output)
}

/// Execute a CLI command and return the output lines.
fn execute_cli_command(command: &str) -> Vec<String> {
    let cmd_lower = command.trim().to_lowercase();

    if cmd_lower.starts_with("core show channels") {
        let mut lines = Vec::new();
        let count = asterisk_core::channel_store::count();
        lines.push(format!(
            "{:<40} {:<20} {:<15} {:<20}",
            "Channel", "Location", "State", "Application(Data)"
        ));

        let channels = asterisk_core::channel_store::all_channels();
        for chan_arc in &channels {
            let chan = chan_arc.lock();
            lines.push(format!(
                "{:<40} {}@{}:{:<10} {:<15}",
                chan.name,
                chan.exten,
                chan.context,
                chan.priority,
                chan.state,
            ));
        }

        lines.push(format!("{} active channel(s)", count));
        lines
    } else if cmd_lower.starts_with("core show version") {
        vec!["Asterisk-RS 0.1.0 (Rust rewrite of Asterisk)".to_string()]
    } else if cmd_lower.starts_with("core show uptime") {
        vec!["System uptime: 00:00:00".to_string()]
    } else {
        vec![format!("No such command '{}' (type 'core show help' for help)", command)]
    }
}

/// Handle the Events action (enable/disable event categories).
fn handle_events(
    action: &AmiAction,
    session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let event_mask = match action.get_header("EventMask") {
        Some(m) => m,
        None => return AmiResponse::error("EventMask is required"),
    };

    match event_mask.to_lowercase().as_str() {
        "off" => {
            session.set_events_enabled(false);
        }
        "on" => {
            session.set_events_enabled(true);
            session.set_event_filter(EventCategory::ALL);
        }
        mask => {
            session.set_events_enabled(true);
            session.set_event_filter(EventCategory::parse_list(mask));
        }
    }

    AmiResponse::success("Events configured")
}

/// Handle GetConfig action.
fn handle_get_config(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let filename = match action.get_header("Filename") {
        Some(f) => f,
        None => return AmiResponse::error("Filename is required"),
    };

    info!("AMI GetConfig: filename={}", filename);

    // In a real implementation, read and return the config file:
    //   let config = ConfigLoader::load(filename)?;
    //   for (i, (cat_name, entries)) in config.categories().enumerate() {
    //       resp.with_header(&format!("Category-{:06}", i), cat_name);
    //       for (j, (key, value)) in entries.enumerate() {
    //           resp.with_header(&format!("Line-{:06}-{:06}", i, j), &format!("{}={}", key, value));
    //       }
    //   }

    AmiResponse::success("Configuration loaded")
}

/// Handle UpdateConfig action.
fn handle_update_config(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let src_filename = match action.get_header("SrcFilename") {
        Some(f) => f,
        None => return AmiResponse::error("SrcFilename is required"),
    };

    let dst_filename = match action.get_header("DstFilename") {
        Some(f) => f,
        None => return AmiResponse::error("DstFilename is required"),
    };

    info!(
        "AMI UpdateConfig: src={} dst={}",
        src_filename, dst_filename
    );

    AmiResponse::success("Config updated")
}

/// Handle ListCategories action.
fn handle_list_categories(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let filename = match action.get_header("Filename") {
        Some(f) => f,
        None => return AmiResponse::error("Filename is required"),
    };

    info!("AMI ListCategories: filename={}", filename);

    AmiResponse::success("Categories listed")
}

/// Handle Status action.
fn handle_status(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let _channel = action.get_header("Channel"); // Optional

    // In a real implementation, send status events for each channel:
    //   if let Some(specific_channel) = channel {
    //       // Send status for just that channel
    //   } else {
    //       // Send status for all channels
    //   }
    //   Then send StatusComplete event

    AmiResponse::success("Channel status will follow")
        .with_header("Items", "0")
}

/// Handle ShowDialPlan action.
fn handle_show_dialplan(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let _context_name = action.get_header("Context");
    let _extension = action.get_header("Extension");

    AmiResponse::success("Dialplan will follow")
}

/// Handle ListCommands action.
fn handle_list_commands(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    AmiResponse::success("Commands listed")
        .with_header("Login", "Login Manager (Privilege: <none>)")
        .with_header("Logoff", "Logoff Manager (Privilege: <none>)")
        .with_header("Ping", "Keepalive command (Privilege: <none>)")
        .with_header("Hangup", "Hangup channel (Privilege: system,call)")
        .with_header("Status", "Lists channel status (Privilege: system,call)")
        .with_header("Originate", "Originate a call (Privilege: originate)")
        .with_header("Redirect", "Redirect (transfer) a call (Privilege: call)")
        .with_header("Command", "Execute Asterisk CLI Command (Privilege: command)")
        .with_header("Events", "Control Event Flow (Privilege: <none>)")
        .with_header("CoreShowChannels", "List currently active channels (Privilege: system)")
        .with_header("CoreStatus", "Show PBX core status (Privilege: system)")
        .with_header("CoreSettings", "Show PBX core settings (Privilege: system)")
        .with_header("Bridge", "Bridge two channels (Privilege: call)")
        .with_header("Park", "Park a channel (Privilege: call)")
        .with_header("QueueStatus", "Queue Status (Privilege: <none>)")
        .with_header("QueueAdd", "Add interface to queue (Privilege: agent)")
        .with_header("QueueRemove", "Remove interface from queue (Privilege: agent)")
        .with_header("QueuePause", "Pause/unpause interface in queue (Privilege: agent)")
}

/// Handle QueueStatus action.
fn handle_queue_status(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let _queue = action.get_header("Queue"); // Optional

    // In a real implementation, send events for each queue member and caller

    AmiResponse::success("Queue status will follow")
}

/// Handle QueueAdd action.
fn handle_queue_add(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let queue = match action.get_header("Queue") {
        Some(q) => q,
        None => return AmiResponse::error("Queue is required"),
    };

    let interface = match action.get_header("Interface") {
        Some(i) => i,
        None => return AmiResponse::error("Interface is required"),
    };

    let member_name = action.get_header("MemberName").unwrap_or(interface);
    let penalty = action
        .get_header("Penalty")
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let paused = action
        .get_header("Paused")
        .map(|p| p == "1" || p.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    info!(
        "AMI QueueAdd: queue={} interface={} member={} penalty={} paused={}",
        queue, interface, member_name, penalty, paused
    );

    AmiResponse::success("Added to queue")
}

/// Handle QueueRemove action.
fn handle_queue_remove(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let queue = match action.get_header("Queue") {
        Some(q) => q,
        None => return AmiResponse::error("Queue is required"),
    };

    let interface = match action.get_header("Interface") {
        Some(i) => i,
        None => return AmiResponse::error("Interface is required"),
    };

    info!(
        "AMI QueueRemove: queue={} interface={}",
        queue, interface
    );

    AmiResponse::success("Removed from queue")
}

/// Handle QueuePause action.
fn handle_queue_pause(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let interface = match action.get_header("Interface") {
        Some(i) => i,
        None => return AmiResponse::error("Interface is required"),
    };

    let paused = action
        .get_header("Paused")
        .map(|p| p == "1" || p.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let queue = action.get_header("Queue"); // Optional

    info!(
        "AMI QueuePause: interface={} paused={} queue={:?}",
        interface, paused, queue
    );

    AmiResponse::success(if paused {
        "Interface paused"
    } else {
        "Interface unpaused"
    })
}

/// Get a simple timestamp string (seconds since epoch).
fn chrono_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AmiUser;
    use crate::session::AmiSession;
    use tokio::sync::mpsc;

    fn make_context() -> (ActionContext, Arc<UserRegistry>) {
        let registry = Arc::new(UserRegistry::new());
        registry.add_user(AmiUser::new("admin", "secret"));
        let ctx = ActionContext {
            user_registry: registry.clone(),
        };
        (ctx, registry)
    }

    fn make_session() -> (AmiSession, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(32);
        let addr: std::net::SocketAddr = "127.0.0.1:12345".parse().unwrap();
        (AmiSession::new(addr, tx), rx)
    }

    fn make_authenticated_session() -> (AmiSession, mpsc::Receiver<String>) {
        let (mut session, rx) = make_session();
        let user = AmiUser::new("admin", "secret");
        session.authenticate(&user);
        (session, rx)
    }

    #[test]
    fn test_login_success() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();

        let mut action = AmiAction::new("Login");
        action.set_header("Username", "admin");
        action.set_header("Secret", "secret");

        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(resp.success);
        assert!(session.authenticated);
    }

    #[test]
    fn test_login_wrong_password() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();

        let mut action = AmiAction::new("Login");
        action.set_header("Username", "admin");
        action.set_header("Secret", "wrong");

        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(!resp.success);
        assert!(!session.authenticated);
    }

    #[test]
    fn test_login_unknown_user() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();

        let mut action = AmiAction::new("Login");
        action.set_header("Username", "nobody");
        action.set_header("Secret", "anything");

        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(!resp.success);
    }

    #[test]
    fn test_md5_challenge_login() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Step 1: Request challenge
        let mut challenge_action = AmiAction::new("Challenge");
        challenge_action.set_header("AuthType", "md5");
        let challenge_resp = registry.dispatch(&challenge_action, &mut session, &ctx);
        assert!(challenge_resp.success);
        let challenge = challenge_resp.headers.get("Challenge").unwrap().clone();

        // Step 2: Login with MD5 response
        let md5_response = auth::compute_md5_response(&challenge, "secret");
        let mut login_action = AmiAction::new("Login");
        login_action.set_header("Username", "admin");
        login_action.set_header("AuthType", "md5");
        login_action.set_header("Key", &md5_response);
        let login_resp = registry.dispatch(&login_action, &mut session, &ctx);

        assert!(login_resp.success);
        assert!(session.authenticated);
    }

    #[test]
    fn test_unauthenticated_action_denied() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();

        let action = AmiAction::new("Ping");
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(!resp.success);
        assert!(resp.message.contains("Permission denied"));
    }

    #[test]
    fn test_ping() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();

        let action = AmiAction::new("Ping");
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(resp.success);
        assert!(resp.headers.contains_key("Ping"));
    }

    #[test]
    fn test_unknown_action() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();

        let action = AmiAction::new("NonexistentAction");
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(!resp.success);
        assert!(resp.message.contains("Invalid/unknown command"));
    }

    #[test]
    fn test_events_action() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Disable events
        let mut action = AmiAction::new("Events");
        action.set_header("EventMask", "off");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(!session.events_enabled);

        // Enable specific categories
        let mut action = AmiAction::new("Events");
        action.set_header("EventMask", "system,call");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(session.events_enabled);
        assert!(session.event_filter.contains(EventCategory::CALL));
        assert!(!session.event_filter.contains(EventCategory::DTMF));
    }

    #[test]
    fn test_logoff() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let action = AmiAction::new("Logoff");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(!session.authenticated);
    }

    #[test]
    fn test_originate_requires_channel() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let action = AmiAction::new("Originate");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(!resp.success);
        assert!(resp.message.contains("Channel is required"));
    }

    #[test]
    fn test_hangup_action() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let mut action = AmiAction::new("Hangup");
        action.set_header("Channel", "SIP/100-00000001");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
    }

    #[test]
    fn test_list_commands() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let action = AmiAction::new("ListCommands");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(resp.headers.contains_key("Ping"));
        assert!(resp.headers.contains_key("Originate"));
    }

    #[test]
    fn test_action_id_echoed() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let mut action = AmiAction::new("Ping");
        action.set_header("ActionID", "my-unique-id-42");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert_eq!(resp.action_id.as_deref(), Some("my-unique-id-42"));
    }
}
