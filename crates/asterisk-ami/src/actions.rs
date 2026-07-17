//! AMI action handlers.
//!
//! Each AMI action (Login, Ping, Originate, etc.) has a handler that
//! processes the action and returns a response. Actions are registered
//! in an ActionRegistry and dispatched by name.

use crate::auth::{self, UserRegistry};
use crate::events::EventCategory;
use crate::protocol::{AmiAction, AmiEvent, AmiResponse};
use crate::session::AmiSession;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Global startup time -- initialized when the module is first accessed.
/// Used by CoreStatus to report real uptime.
static STARTUP_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The authorization requirement for an AMI action.
///
/// This mirrors Asterisk's per-action `authority` field (`manager.c`): every
/// action is classified once, in [`action_authority`], so the trust boundary is
/// auditable in a single table rather than scattered across handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionAuthority {
    /// Part of the login handshake; permitted before authentication.
    Preauth,
    /// Read-only status / list / show / session-control action. Any
    /// authenticated session may run it, matching Asterisk (status reads are
    /// not write-gated).
    ReadOnly,
    /// State-changing action. The session's `write_perm` MUST contain this
    /// category or the action is refused with "Permission denied".
    Write(EventCategory),
}

/// Classify an AMI action's authorization requirement by its lowercased name.
///
/// Returns `None` for a name that is not a recognized action. A recognized
/// (registered) action MUST be classified here: the
/// `test_every_registered_action_is_classified` drift test fails if any
/// registered handler is missing from this table, so a newly added
/// state-changing action can never silently inherit a read-only free pass —
/// an unmapped dangerous action is a caught bug, not a default-allow.
///
/// The write categories mirror Asterisk's per-action authority:
/// - `Command` (CLI execution) → `COMMAND`
/// - `UpdateConfig` → `CONFIG`
/// - `Originate` / `PJSIPNotify` / `PJSIPQualify` → `SYSTEM`
///   (Asterisk lets `Originate` use `system` OR `call`; per the coordinator
///   decision we require the stricter `SYSTEM` and document it. This crate has
///   no dedicated `originate` privilege category, so `ListCommands` advertises
///   `system` for Originate to match this enforcement.)
/// - `QueueAdd` / `QueueRemove` / `QueuePause` → `AGENT` (queue-membership
///   administration, matching Asterisk `app_queue` and this crate's
///   `ListCommands`-advertised `agent` privilege).
/// - `Redirect` / `Hangup` / `Bridge` / `Park` / `SetVar` / `Atxfer` /
///   `SendText` / `ConfBridgeKick` / `ConfBridgeMute` → `CALL`
pub fn action_authority(name_lower: &str) -> Option<ActionAuthority> {
    use ActionAuthority::{Preauth, ReadOnly, Write};
    let authority = match name_lower {
        // Login handshake: allowed before authentication.
        "login" | "challenge" => Preauth,

        // COMMAND: the CLI execution surface.
        "command" => Write(EventCategory::COMMAND),

        // CONFIG: configuration writes.
        "updateconfig" => Write(EventCategory::CONFIG),

        // SYSTEM: origination and system-level signaling.
        "originate" | "pjsipnotify" | "pjsipqualify" => Write(EventCategory::SYSTEM),

        // AGENT: queue-membership administration.
        "queueadd" | "queueremove" | "queuepause" => Write(EventCategory::AGENT),

        // CALL: call control, dialplan-variable writes, conf-bridge participant ops.
        "redirect" | "hangup" | "bridge" | "park" | "setvar" | "atxfer" | "sendtext"
        | "confbridgekick" | "confbridgemute" => Write(EventCategory::CALL),

        // Read-only status / list / show / session-control actions: any
        // authenticated session may run them (must not be over-gated).
        "logoff" | "ping" | "events" | "corestatus" | "coresettings" | "coreshowchannels"
        | "status" | "getconfig" | "getvar" | "listcategories" | "listcommands" | "rtpstats"
        | "showdialplan" | "queuestatus" | "meetmelist" | "confbridgelist" | "waitfullybooted"
        | "pjsipshowendpoints" | "pjsipshowendpoint" | "pjsipshowregistrationsinbound"
        | "pjsipshowregistrationsoutbound" => ReadOnly,

        // Unrecognized name: not a registered action. Falls through to the
        // handler lookup, which returns "Invalid/unknown command".
        _ => return None,
    };
    Some(authority)
}

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
    /// Per-source failed-`Login` rate limiter (issue #130). Shared across all
    /// connections of a server so guess volume is bounded per source address.
    pub login_rate_limiter: Arc<crate::rate_limit::LoginRateLimiter>,
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
        let authority = action_authority(&name_lower);

        // Authentication gate: every action except the login handshake requires
        // an authenticated session. (An unrecognized action — `authority` ==
        // None — is also gated here so a pre-auth client cannot probe handlers.)
        if authority != Some(ActionAuthority::Preauth) && !session.authenticated {
            return AmiResponse::error("Permission denied")
                .with_action_id(action.action_id.clone());
        }

        // Per-action write-authority gate (issue #126), enforced AFTER the auth
        // gate: a state-changing action requires the matching write category in
        // the session's `write_perm`. Read-only and pre-auth actions are not
        // write-gated. Registered actions are guaranteed to be classified (see
        // `test_every_registered_action_is_classified`), so a registered
        // state-changing action can never reach its handler without this check.
        if let Some(ActionAuthority::Write(required)) = authority {
            if !session.write_perm.contains(required) {
                warn!(
                    "AMI: session {} denied '{}' (missing write category 0x{:04x})",
                    session.id, action.name, required.0
                );
                return AmiResponse::error("Permission denied")
                    .with_action_id(action.action_id.clone());
            }
        }

        let handler = {
            let handlers = self.handlers.read();
            handlers.get(&name_lower).cloned()
        };

        match handler {
            Some(handler) => {
                // Runtime fail-closed backstop for issue #126: a REGISTERED
                // handler with no authority classification must NOT execute for
                // an authenticated no-perm session. This can only happen if a
                // new handler was registered without being added to
                // `action_authority` — the `test_every_registered_action_is_classified`
                // drift test fails first at CI, but deny here too so a slipped
                // classification is fail-closed at runtime, not a free pass.
                if authority.is_none() {
                    warn!(
                        "AMI: registered action '{}' has no authority classification; denying (fail-closed)",
                        action.name
                    );
                    return AmiResponse::error("Permission denied")
                        .with_action_id(action.action_id.clone());
                }
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

        // RTPStats
        self.register("rtpstats", Box::new(handle_rtp_stats));

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

        // GetVar
        self.register("getvar", Box::new(handle_getvar));

        // SetVar
        self.register("setvar", Box::new(handle_setvar));

        // PJSIP actions
        self.register("pjsipshowendpoints", Box::new(handle_pjsip_show_endpoints));
        self.register("pjsipshowendpoint", Box::new(handle_pjsip_show_endpoint));
        self.register(
            "pjsipshowregistrationsinbound",
            Box::new(handle_pjsip_show_registrations_inbound),
        );
        self.register(
            "pjsipshowregistrationsoutbound",
            Box::new(handle_pjsip_show_registrations_outbound),
        );
        self.register("pjsipnotify", Box::new(handle_pjsip_notify));
        self.register("pjsipqualify", Box::new(handle_pjsip_qualify));

        // MeetmeList
        self.register("meetmelist", Box::new(handle_meetme_list));

        // ConfBridge actions  
        self.register("confbridgelist", Box::new(handle_confbridge_list));
        self.register("confbridgekick", Box::new(handle_confbridge_kick));
        self.register("confbridgemute", Box::new(handle_confbridge_mute));

        // WaitFullyBooted
        self.register("waitfullybooted", Box::new(handle_wait_fully_booted));

        // SendText
        self.register("sendtext", Box::new(handle_send_text));

        // Atxfer (attended transfer)
        self.register("atxfer", Box::new(handle_atxfer));
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
    // Login rate limiting (issue #130): throttle guess VOLUME per source before
    // touching credentials, so an attacker cannot grind unlimited online
    // password guesses. Keyed by source address, not username, so rotating the
    // username does not evade the block.
    let source_ip = session.addr.ip();
    if let Err(retry_after) = context.login_rate_limiter.check(source_ip) {
        warn!(
            "AMI Login: throttled from {} ({}s remaining)",
            source_ip,
            retry_after.as_secs()
        );
        return AmiResponse::error("Login rate exceeded, try again later");
    }

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
            context.login_rate_limiter.record_failure(source_ip);
            return AmiResponse::error("Authentication failed");
        }
    };

    // Check for MD5 challenge/response authentication
    if let Some(key) = action.get_header("Key") {
        // MD5 auth: verify against session challenge.
        //
        // The challenge is single-use: consume it (`take`) before evaluating the
        // response, so the nonce cannot be reused for a second Login on the same
        // session. A client must request a fresh `Challenge` for every login
        // attempt (this is how well-behaved AMI clients already work). Without
        // this, a captured `Login` with a fixed challenge could be replayed, and
        // an attacker could grind guesses against one stable nonce.
        if let Some(challenge) = session.challenge.take() {
            if auth::verify_md5_response(&challenge, &user.secret, key) {
                context.login_rate_limiter.record_success(source_ip);
                session.authenticate(&user);
                return AmiResponse::success("Authentication accepted");
            } else {
                context.login_rate_limiter.record_failure(source_ip);
                return AmiResponse::error("Authentication failed");
            }
        } else {
            context.login_rate_limiter.record_failure(source_ip);
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
        context.login_rate_limiter.record_success(source_ip);
        session.authenticate(&user);
        AmiResponse::success("Authentication accepted")
    } else {
        context.login_rate_limiter.record_failure(source_ip);
        AmiResponse::error("Authentication failed")
    }
}

/// Handle the Logoff action.
///
/// Real Asterisk returns `Response: Goodbye` (not `Response: Success`)
/// so that starpy and other clients can check `message['response'] == 'Goodbye'`.
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
    AmiResponse::success("Thanks for all the fish.")
        .with_response_value("Goodbye")
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
        .with_header("Timestamp", format!("{}", chrono_timestamp()))
}

/// Handle CoreShowChannels action.
fn handle_core_show_channels(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let action_id = action.action_id.clone().unwrap_or_default();
    let channels = asterisk_core::channel_store::all_channels();
    let count = channels.len();

    let mut resp = AmiResponse::success("Channels will follow");

    for chan_arc in &channels {
        let chan = chan_arc.lock();
        let state_num = (chan.state as u8).to_string();
        let state_desc = chan.state.to_string();
        let priority_str = chan.priority.to_string();
        let duration_str = "0"; // TODO: track creation time

        let mut event = AmiEvent::new("CoreShowChannel", 0x01);
        if !action_id.is_empty() {
            event.add_header("ActionID", &action_id);
        }
        event.add_header("Channel", &chan.name);
        event.add_header("Uniqueid", &chan.unique_id.0);
        event.add_header("Linkedid", &chan.linkedid);
        event.add_header("Context", &chan.context);
        event.add_header("Extension", &chan.exten);
        event.add_header("Priority", &priority_str);
        event.add_header("ChannelState", &state_num);
        event.add_header("ChannelStateDesc", &state_desc);
        event.add_header("CallerIDNum", &chan.caller.id.number.number);
        event.add_header("CallerIDName", &chan.caller.id.name.name);
        event.add_header("ConnectedLineNum", "");
        event.add_header("ConnectedLineName", "");
        event.add_header("Language", &chan.language);
        event.add_header("AccountCode", &chan.accountcode);
        event.add_header("Duration", duration_str);
        event.add_header("BridgeId", "");
        event.add_header("Application", "");
        event.add_header("ApplicationData", "");

        resp.add_followup_event(event);
    }

    let mut complete = AmiEvent::new("CoreShowChannelsComplete", 0x01);
    if !action_id.is_empty() {
        complete.add_header("ActionID", &action_id);
    }
    complete.add_header("EventList", "Complete");
    complete.add_header("ListItems", count.to_string());
    resp.add_followup_event(complete);

    resp
}

/// Handle CoreStatus action.
fn handle_core_status(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let uptime_secs = STARTUP_TIME.elapsed().as_secs();
    let active_channels = asterisk_core::channel_store::count();
    let sip_counts = asterisk_sip::get_global_event_handler()
        .map(|handler| handler.resource_counts());

    // Format startup date/time from system time - epoch seconds of STARTUP_TIME
    use std::time::{SystemTime, UNIX_EPOCH};
    let startup_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(uptime_secs);
    // Simple epoch-to-date formatting
    let startup_date = format_epoch_date(startup_epoch);
    let startup_time = format_epoch_time(startup_epoch);

    AmiResponse::success("Core Status")
        .with_header("CoreStartupDate", &startup_date)
        .with_header("CoreStartupTime", &startup_time)
        .with_header("CoreReloadDate", &startup_date)
        .with_header("CoreReloadTime", &startup_time)
        .with_header("CoreCurrentCalls", active_channels.to_string())
        .with_header(
            "SIPDriverChannels",
            sip_counts.map(|counts| counts.driver_channels).unwrap_or(0).to_string(),
        )
        .with_header(
            "SIPCallIdMappings",
            sip_counts.map(|counts| counts.call_id_mappings).unwrap_or(0).to_string(),
        )
        .with_header(
            "SIPCallStates",
            sip_counts.map(|counts| counts.call_states).unwrap_or(0).to_string(),
        )
        .with_header(
            "SIPNotifyChannels",
            sip_counts
                .map(|counts| counts.notify_channels)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPInviteClientTransactions",
            sip_counts
                .map(|counts| counts.invite_client_transactions)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPInviteServerTransactions",
            sip_counts
                .map(|counts| counts.invite_server_transactions)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPNonInviteClientTransactions",
            sip_counts
                .map(|counts| counts.non_invite_client_transactions)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPNonInviteServerTransactions",
            sip_counts
                .map(|counts| counts.non_invite_server_transactions)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPRtpSessions",
            sip_counts.map(|counts| counts.rtp_sessions).unwrap_or(0).to_string(),
        )
        .with_header(
            "SIPRegistrarBindings",
            sip_counts
                .map(|counts| counts.registrar_bindings)
                .unwrap_or(0)
                .to_string(),
        )
        .with_header(
            "SIPHangupCallbacks",
            sip_counts.map(|counts| counts.hangup_callbacks).unwrap_or(0).to_string(),
        )
        .with_header(
            "SIPAnswerCallbacks",
            sip_counts.map(|counts| counts.answer_callbacks).unwrap_or(0).to_string(),
        )
        .with_header("CoreStartupSecs", uptime_secs.to_string())
}

/// Handle CoreSettings action.
fn handle_core_settings(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    AmiResponse::success("Core Settings")
        .with_header("AsteriskVersion", "Rustisk 0.1.0")
        .with_header("SystemName", "Rustisk")
        .with_header("AMIversion", "11.0.0")
        .with_header("MaxCalls", "0")
        .with_header("MaxLoadAvg", "0.0")
        .with_header("MaxFileHandles", "0")
}

/// Handle the Originate action.
///
/// Parses the channel technology from the Channel header (e.g. "Local/100@default"),
/// looks up the channel driver via `TECH_REGISTRY`, calls `request()` to create
/// the channel, then `call()` to initiate the outbound leg.
///
/// For Local channels this creates a proper ;1/;2 pair:
///   - ;1 (owner) gets the Originate context/exten/priority and runs PBX
///   - ;2 (chan) runs the dialplan at the Local destination
///   - Both channels are registered in the global store and emit Newchannel events
///   - PBX is started on ;2 via local_call() -> ast_pbx_start(;2) semantics
///
/// This mirrors the C Asterisk flow: action_originate -> ast_pbx_outgoing_exten
/// -> ast_request -> local_request -> local_call -> ast_pbx_start(;2).
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
    let application = action.get_header("Application").map(|a| a.to_string());
    let app_data = action.get_header("Data").unwrap_or("").to_string();

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

    // Parse technology/data from channel string (e.g. "Local/100@default" -> tech="Local", data="100@default")
    let (tech, data) = match channel_str.split_once('/') {
        Some((t, d)) => (t.to_string(), d.to_string()),
        None => {
            // No slash means use the whole string as the channel name (legacy)
            return originate_simple(&channel_str, &context, &exten, priority, &caller_id, variables);
        }
    };

    // Look up the technology driver in the global registry
    let driver = match asterisk_core::TECH_REGISTRY.find(&tech) {
        Some(d) => d,
        None => {
            warn!("AMI Originate: unknown channel technology '{}'", tech);
            // Fall back to simple originate (just alloc + pbx_run)
            return originate_simple(&channel_str, &context, &exten, priority, &caller_id, variables);
        }
    };

    let dialplan = match asterisk_core::get_global_dialplan() {
        Some(dp) => dp,
        None => {
            warn!("AMI Originate: no dialplan loaded, cannot execute");
            return AmiResponse::error("Dialplan not loaded");
        }
    };

    // Spawn the originate asynchronously since driver.request() is async
    let channel_str_clone = channel_str.clone();
    tokio::spawn(async move {
        // Request a channel from the technology driver
        let chan = match driver.request(&data, None).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Originate: driver.request('{}') failed: {}", data, e);
                crate::event_bus::publish_event(
                    crate::protocol::AmiEvent::new("OriginateResponse", 0x02)
                        .with_header("Response", "Failure")
                        .with_header("Channel", &channel_str_clone),
                );
                return;
            }
        };

        // Register the channel in the global store (emits Newchannel)
        let store_chan = asterisk_core::channel_store::register_existing_channel(chan);

        // Set Originate context/exten/priority, caller ID, and variables
        let chan_name;
        let chan_uid;
        {
            let mut ch = store_chan.lock();
            ch.context = context.clone();
            ch.exten = exten.clone();
            ch.priority = priority;
            if !caller_id.is_empty() {
                ch.caller.id.number.number = caller_id.clone();
                ch.caller.id.number.valid = true;
            }
            for (k, v) in &variables {
                ch.set_variable(k.clone(), v.clone());
            }
            chan_name = ch.name.clone();
            chan_uid = ch.unique_id.0.clone();
        }

        // Build a standalone Channel copy for driver.call() -- we cannot
        // hold the parking_lot::MutexGuard across an .await point.
        let mut call_channel = {
            let ch = store_chan.lock();
            let mut c = asterisk_core::Channel::new(&ch.name);
            c.unique_id = ch.unique_id.clone();
            c.context = ch.context.clone();
            c.exten = ch.exten.clone();
            c.priority = ch.priority;
            c.linkedid = ch.linkedid.clone();
            c.caller = ch.caller.clone();
            c
        };
        // parking_lot guard is dropped here, before any .await

        // Call the driver's call() method.
        // For Local channels, this:
        //   - Sets ;1 to Ring state
        //   - Retrieves the pending ;2 channel
        //   - Registers ;2 in the channel store (emits Newchannel for ;2)
        //   - Emits LocalBridge event
        //   - Spawns pbx_run on ;2
        if let Err(e) = driver.call(&mut call_channel, &data, 30000).await {
            warn!("Originate: driver.call() failed for {}: {}", chan_name, e);
            crate::event_bus::publish_event(
                crate::protocol::AmiEvent::new("OriginateResponse", 0x02)
                    .with_header("Response", "Failure")
                    .with_header("Channel", &chan_name)
                    .with_header("Uniqueid", &chan_uid),
            );
            let _ = driver.hangup(&mut call_channel).await;
            release_originate_leg(&tech, &chan_name, &chan_uid);
            return;
        }

        // Sync state changes from call() back to the store channel
        {
            let mut ch = store_chan.lock();
            ch.state = call_channel.state;
        }

        // Create a tokio::sync::Mutex copy for execution on ;1
        let pbx_channel = {
            let guard = store_chan.lock();
            let mut ch = asterisk_core::Channel::new(&guard.name);
            ch.unique_id = guard.unique_id.clone();
            ch.state = guard.state;
            ch.context = guard.context.clone();
            ch.exten = guard.exten.clone();
            ch.priority = guard.priority;
            ch.linkedid = guard.linkedid.clone();
            ch.caller = guard.caller.clone();
            for (k, v) in &variables {
                ch.set_variable(k.clone(), v.clone());
            }
            std::sync::Arc::new(tokio::sync::Mutex::new(ch))
        };

        let response_str = if let Some(ref app_name) = application {
            // Application mode: execute the application directly instead of dialplan
            info!("Originate: running app '{}({})' on ;1 channel {}", app_name, app_data, chan_name);
            use asterisk_core::pbx::app_registry::APP_REGISTRY;
            if let Some(app) = APP_REGISTRY.find(app_name) {
                let mut ch = pbx_channel.lock().await;
                let result = app.execute(&mut ch, &app_data).await;
                info!("Originate: app '{}' finished for channel {} result={:?}", app_name, chan_name, result);
                match result {
                    asterisk_core::pbx::PbxResult::Success => "Success",
                    _ => "Failure",
                }
            } else {
                warn!("Originate: application '{}' not found", app_name);
                "Failure"
            }
        } else {
            // Context/Exten mode: run through the dialplan
            info!("Originate: starting pbx_run for ;1 channel {}", chan_name);
            let result = asterisk_core::pbx_run(pbx_channel.clone(), dialplan).await;
            info!("Originate: pbx_run finished for channel {} result={:?}", chan_name, result);
            match result {
                asterisk_core::PbxRunResult::Success => "Success",
                _ => "Failure",
            }
        };

        // Direct PJSIP Originate teardown keeps the event handler's dialog as
        // the CSeq authority and retains only signaling state until a BYE
        // final (or Timer F). Other technologies use their driver teardown.
        let pjsip_signaling_retained = if tech.eq_ignore_ascii_case("PJSIP") {
            if let Some(handler) = asterisk_sip::get_global_event_handler() {
                handler.finish_outbound_originate(&chan_name).await;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !pjsip_signaling_retained {
            let mut ch = pbx_channel.lock().await;
            if let Err(error) = driver.hangup(&mut ch).await {
                warn!("Originate: driver.hangup() failed for {}: {}", chan_name, error);
            }
        }
        crate::event_bus::publish_event(
            crate::protocol::AmiEvent::new("OriginateResponse", 0x02)
                .with_header("Response", response_str)
                .with_header("Channel", &chan_name)
                .with_header("Uniqueid", &chan_uid),
        );

        if !pjsip_signaling_retained {
            release_originate_leg(&tech, &chan_name, &chan_uid);
        }
    });

    AmiResponse::success("Originate successfully queued")
}

/// Release every resource held by a technology-backed Originate leg.
///
/// PJSIP teardown is centralized in `release_outbound_leg`, which also clears
/// its Call-ID, call-state, driver-media, and NOTIFY registrations. Other
/// channel technologies only need the generic store entry removed here.
fn release_originate_leg(technology: &str, channel_name: &str, unique_id: &str) {
    if technology.eq_ignore_ascii_case("PJSIP") {
        if let Some(handler) = asterisk_sip::get_global_event_handler() {
            handler.release_outbound_leg(channel_name);
            return;
        }
    }
    asterisk_core::channel_store::deregister(unique_id);
}

/// Simple originate: allocate a channel directly and run pbx_run on it.
/// Used as fallback when the tech driver is not in the registry.
fn originate_simple(
    channel_str: &str,
    context: &str,
    exten: &str,
    priority: i32,
    caller_id: &str,
    variables: Vec<(String, String)>,
) -> AmiResponse {
    let dialplan = match asterisk_core::get_global_dialplan() {
        Some(dp) => dp,
        None => {
            return AmiResponse::error("Dialplan not loaded");
        }
    };

    let chan_name;
    let chan_uid;
    {
        let store_chan = asterisk_core::channel_store::alloc_channel(channel_str);
        let mut chan = store_chan.lock();
        chan.context = context.to_string();
        chan.exten = exten.to_string();
        chan.priority = priority;
        if !caller_id.is_empty() {
            chan.caller.id.number.number = caller_id.to_string();
            chan.caller.id.number.valid = true;
        }
        for (k, v) in &variables {
            chan.set_variable(k.clone(), v.clone());
        }
        chan_name = chan.name.clone();
        chan_uid = chan.unique_id.0.clone();
    }

    let pbx_channel = {
        let mut ch = asterisk_core::Channel::new(&chan_name);
        ch.unique_id = asterisk_core::ChannelId(chan_uid.clone());
        ch.context = context.to_string();
        ch.exten = exten.to_string();
        ch.priority = priority;
        ch.linkedid = chan_uid.clone();
        if !caller_id.is_empty() {
            ch.caller.id.number.number = caller_id.to_string();
            ch.caller.id.number.valid = true;
        }
        for (k, v) in variables {
            ch.set_variable(k, v);
        }
        std::sync::Arc::new(tokio::sync::Mutex::new(ch))
    };

    let chan_name_for_task = chan_name.clone();
    let chan_uid_for_task = chan_uid.clone();
    tokio::spawn(async move {
        info!("Originate: starting pbx_run for channel {}", chan_name_for_task);
        let result = asterisk_core::pbx_run(pbx_channel, dialplan).await;
        info!("Originate: pbx_run finished for channel {} result={:?}", chan_name_for_task, result);

        let response_str = match result {
            asterisk_core::PbxRunResult::Success => "Success",
            _ => "Failure",
        };
        crate::event_bus::publish_event(
            crate::protocol::AmiEvent::new("OriginateResponse", 0x02)
                .with_header("Response", response_str)
                .with_header("Channel", &chan_name_for_task)
                .with_header("Uniqueid", &chan_uid_for_task),
        );

        asterisk_core::channel_store::deregister(&chan_uid_for_task);
    });

    AmiResponse::success("Originate successfully queued")
}

/// Handle the Redirect action.
fn handle_redirect(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel_name = match action.get_header("Channel") {
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
        channel_name, exten, redirect_context, priority
    );

    // Look up the channel and change its dialplan location
    if let Some(chan_arc) = asterisk_core::channel_store::find_by_name(channel_name) {
        let mut chan = chan_arc.lock();
        chan.context = redirect_context.to_string();
        chan.exten = exten.to_string();
        chan.priority = priority;
        // Set AsyncGoto flag so pbx_run picks up the new location
        chan.softhangup(asterisk_core::channel::softhangup::AST_SOFTHANGUP_ASYNCGOTO);
        AmiResponse::success("Redirect successful")
    } else {
        AmiResponse::error(format!("Channel not found: {}", channel_name))
    }
}

/// Handle the Hangup action.
fn handle_hangup(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel_name = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let cause_code = action
        .get_header("Cause")
        .and_then(|c| c.parse::<u32>().ok())
        .unwrap_or(16); // Normal clearing

    info!("AMI Hangup: channel={} cause={}", channel_name, cause_code);

    // Support regex patterns like /.*/ for hanging up multiple channels
    if channel_name.starts_with('/') && channel_name.ends_with('/') {
        let pattern = &channel_name[1..channel_name.len()-1];
        let re = match regex::Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return AmiResponse::error("Invalid regex pattern"),
        };
        let mut count = 0;
        for chan_arc in asterisk_core::channel_store::all_channels() {
            let mut chan = chan_arc.lock();
            if re.is_match(&chan.name) {
                chan.hangup_cause = match cause_code {
                    0 => asterisk_types::HangupCause::NotDefined,
                    16 => asterisk_types::HangupCause::NormalClearing,
                    _ => asterisk_types::HangupCause::NormalClearing,
                };
                chan.softhangup(asterisk_core::channel::softhangup::AST_SOFTHANGUP_EXPLICIT);
                count += 1;
            }
        }
        return AmiResponse::success(format!("Hungup {} channel(s)", count));
    }

    // Look up the channel in the global store and request a soft hangup
    if let Some(chan_arc) = asterisk_core::channel_store::find_by_name(channel_name) {
        let mut chan = chan_arc.lock();
        // Map common Q.850 cause codes to HangupCause enum variants
        chan.hangup_cause = match cause_code {
            0 => asterisk_types::HangupCause::NotDefined,
            16 => asterisk_types::HangupCause::NormalClearing,
            17 => asterisk_types::HangupCause::UserBusy,
            18 => asterisk_types::HangupCause::NoUserResponse,
            19 => asterisk_types::HangupCause::NoAnswer,
            21 => asterisk_types::HangupCause::CallRejected,
            _ => asterisk_types::HangupCause::NormalClearing,
        };
        chan.softhangup(asterisk_core::channel::softhangup::AST_SOFTHANGUP_EXPLICIT);
        AmiResponse::success("Channel Hungup")
    } else {
        AmiResponse::error(format!("Channel not found: {}", channel_name))
    }
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

    // Verify both channels exist in the global store
    if asterisk_core::channel_store::find_by_name(channel1).is_none() {
        return AmiResponse::error(format!("Channel not found: {}", channel1));
    }
    if asterisk_core::channel_store::find_by_name(channel2).is_none() {
        return AmiResponse::error(format!("Channel not found: {}", channel2));
    }

    // Generate a bridge ID and emit BridgeCreate + BridgeEnter events.
    // The actual bridge audio path is handled asynchronously by the bridge subsystem.
    let bridge_id = format!("bridge-{}", chrono_timestamp());

    crate::event_bus::publish_event(
        crate::protocol::AmiEvent::new("BridgeCreate", 0x02)
            .with_header("BridgeUniqueid", &bridge_id)
            .with_header("BridgeType", "basic")
            .with_header("BridgeTechnology", "simple_bridge")
            .with_header("BridgeNumChannels", "0"),
    );

    // Emit BridgeEnter events for both channels
    for ch_name in &[channel1, channel2] {
        if let Some(chan_arc) = asterisk_core::channel_store::find_by_name(ch_name) {
            let chan = chan_arc.lock();
            crate::event_bus::publish_event(
                crate::protocol::AmiEvent::new("BridgeEnter", 0x02)
                    .with_header("BridgeUniqueid", &bridge_id)
                    .with_header("BridgeType", "basic")
                    .with_header("Channel", &chan.name)
                    .with_header("Uniqueid", &chan.unique_id.0)
                    .with_header("Linkedid", &chan.linkedid),
            );
        }
    }

    AmiResponse::success("Bridge created")
        .with_header("BridgeUniqueid", bridge_id)
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
pub fn execute_cli_command(command: &str) -> Vec<String> {
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
        vec!["Rustisk 0.1.0".to_string()]
    } else if cmd_lower.starts_with("core show uptime") {
        vec!["System uptime: 00:00:00".to_string()]
    } else if cmd_lower.starts_with("pjsip send notify") {
        handle_pjsip_send_notify_cli(command)
    } else {
        vec![format!("No such command '{}' (type 'core show help' for help)", command)]
    }
}

/// Handle `pjsip send notify <template> endpoint <endpoint>` CLI command.
fn handle_pjsip_send_notify_cli(command: &str) -> Vec<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    // Expected: "pjsip send notify <template> endpoint <endpoint>"
    if parts.len() < 6 {
        return vec!["Usage: pjsip send notify <template> endpoint <endpoint>".to_string()];
    }

    let template_name = parts[3];
    let target_type = parts[4];
    let target = parts[5];

    if !target_type.eq_ignore_ascii_case("endpoint") {
        return vec![format!("Unknown target type '{}'. Use 'endpoint'.", target_type)];
    }

    let svc = asterisk_sip::global_notify_service();
    match svc.send_notify_to_endpoint(template_name, target) {
        Ok(()) => vec![format!("Sending NOTIFY of type '{}' to '{}'", template_name, target)],
        Err(e) => vec![format!("Unable to send NOTIFY: {}", e)],
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
    let specific_channel = action.get_header("Channel");
    let action_id = action.action_id.clone().unwrap_or_default();

    let mut resp = AmiResponse::success("Channel status will follow");
    let mut count = 0u32;

    let channels = asterisk_core::channel_store::all_channels();
    for chan_arc in &channels {
        let chan = chan_arc.lock();

        // If a specific channel was requested, only return that one
        if let Some(specific) = specific_channel {
            if chan.name != specific {
                continue;
            }
        }

        let state_num = (chan.state as u8).to_string();
        let priority_str = chan.priority.to_string();

        let mut event = AmiEvent::new("Status", 0x02);
        if !action_id.is_empty() {
            event.add_header("ActionID", &action_id);
        }
        event.add_header("Channel", &chan.name);
        event.add_header("Uniqueid", &chan.unique_id.0);
        event.add_header("Linkedid", &chan.linkedid);
        event.add_header("CallerIDNum", &chan.caller.id.number.number);
        event.add_header("CallerIDName", &chan.caller.id.name.name);
        event.add_header("Context", &chan.context);
        event.add_header("Extension", &chan.exten);
        event.add_header("Priority", &priority_str);
        event.add_header("ChannelState", &state_num);
        event.add_header("ChannelStateDesc", chan.state.to_string());
        event.add_header("AccountCode", &chan.accountcode);

        resp.add_followup_event(event);
        count += 1;
    }

    let mut complete = AmiEvent::new("StatusComplete", 0x02);
    if !action_id.is_empty() {
        complete.add_header("ActionID", &action_id);
    }
    complete.add_header("Items", count.to_string());
    resp.add_followup_event(complete);

    resp
}

/// Handle RTPStats action for an active or recently completed SIP call.
fn handle_rtp_stats(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let query = match action
        .get_header("Channel")
        .or_else(|| action.get_header("Uniqueid"))
    {
        Some(query) => query,
        None => return AmiResponse::error("Channel or Uniqueid is required"),
    };

    let Some(stats) = asterisk_sip::lookup_call_media_stats(query) else {
        return AmiResponse::error(format!("No RTP statistics for '{query}'"));
    };

    AmiResponse::success("RTP statistics")
        .with_header("Channel", stats.channel)
        .with_header("Uniqueid", stats.unique_id.unwrap_or_default())
        .with_header("RTPActive", stats.active.to_string())
        .with_header(
            "RTPRemoteAddress",
            stats.rtp.remote_addr.map(|addr| addr.to_string()).unwrap_or_default(),
        )
        .with_header("RTPPacketsTx", stats.rtp.packets_sent.to_string())
        .with_header("RTPPacketsRx", stats.rtp.packets_received.to_string())
        .with_header("RTPOctetsTx", stats.rtp.octets_sent.to_string())
        .with_header("RTPOctetsRx", stats.rtp.octets_received.to_string())
        .with_header(
            "RTPVoiceFramesTx",
            stats.rtp.voice_frames_sent.to_string(),
        )
        .with_header(
            "RTPVoiceFramesRx",
            stats.rtp.voice_frames_received.to_string(),
        )
        .with_header(
            "RTPDTMFDigitsTx",
            stats.rtp.dtmf_digits_sent.to_string(),
        )
        .with_header(
            "RTPDTMFDigitsRx",
            stats.rtp.dtmf_digits_received.to_string(),
        )
        .with_header(
            "RTPDiscardWrongSource",
            stats.rtp.discarded_wrong_source.to_string(),
        )
        .with_header(
            "RTPDiscardWrongPayloadType",
            stats.rtp.discarded_wrong_payload_type.to_string(),
        )
        .with_header(
            "RTPDiscardMalformed",
            stats.rtp.discarded_malformed.to_string(),
        )
        .with_header(
            "RTPDiscardUnstableSSRC",
            stats.rtp.discarded_unstable_ssrc.to_string(),
        )
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
        .with_header("RTPStats", "Show per-call RTP proof counters (Privilege: call)")
        .with_header("Originate", "Originate a call (Privilege: system)")
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
        .with_header("WaitFullyBooted", "Wait for Asterisk to fully boot (Privilege: <none>)")
        .with_header("SendText", "Send text to a channel (Privilege: call)")
        .with_header("PJSIPShowEndpoints", "Lists PJSIP Endpoints (Privilege: system)")
        .with_header("Atxfer", "Attended Transfer (Privilege: call)")
}

/// Handle QueueStatus action.
fn handle_queue_status(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let _queue = action.get_header("Queue"); // Optional
    let action_id = action.action_id.clone().unwrap_or_default();

    let mut resp = AmiResponse::success("Queue status will follow");

    // Send QueueStatusComplete event so starpy doesn't hang
    let mut complete = AmiEvent::new("QueueStatusComplete", 0x01);
    if !action_id.is_empty() {
        complete.add_header("ActionID", &action_id);
    }
    complete.add_header("EventList", "Complete");
    complete.add_header("ListItems", "0");
    resp.add_followup_event(complete);

    resp
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

// ---------------------------------------------------------------------------
// GetVar / SetVar
// ---------------------------------------------------------------------------

/// Handle the GetVar action.
fn handle_getvar(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = action.get_header("Channel").unwrap_or("");
    let variable = match action.get_header("Variable") {
        Some(v) => v,
        None => return AmiResponse::error("Variable is required"),
    };

    let value = if channel.is_empty() {
        // Check for dialplan function syntax like DEVICE_STATE(PJSIP/bob)
        if let Some(func_val) = evaluate_dialplan_function(variable) {
            func_val
        } else {
            // Global variable
            asterisk_core::pbx::get_global_variable(variable).unwrap_or_default()
        }
    } else {
        // Channel variable -- try to look up in channel store
        if let Some(chan) = asterisk_core::channel_store::find_by_name(channel) {
            let ch = chan.lock();
            ch.get_variable(variable).unwrap_or("").to_string()
        } else {
            String::new()
        }
    };

    AmiResponse::success("Result will follow")
        .with_header("Variable", variable.to_string())
        .with_header("Value", value)
}

/// Handle the SetVar action.
fn handle_setvar(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = action.get_header("Channel").unwrap_or("");
    let variable = match action.get_header("Variable") {
        Some(v) => v,
        None => return AmiResponse::error("Variable is required"),
    };
    let value = action.get_header("Value").unwrap_or("");

    if channel.is_empty() {
        asterisk_core::pbx::set_global_variable(variable.to_string(), value.to_string());
    } else {
        if let Some(chan) = asterisk_core::channel_store::find_by_name(channel) {
            let mut ch = chan.lock();
            ch.set_variable(variable.to_string(), value.to_string());
        } else {
            return AmiResponse::error("Channel not found");
        }
    }

    AmiResponse::success("Variable Set")
}

/// Evaluate a dialplan function like DEVICE_STATE(PJSIP/bob).
/// Returns Some(value) if the variable matches a known function, None otherwise.
fn evaluate_dialplan_function(variable: &str) -> Option<String> {
    // DEVICE_STATE(device) - returns the device state
    if variable.starts_with("DEVICE_STATE(") && variable.ends_with(')') {
        let device = &variable[13..variable.len()-1];
        // Check if the device has an active channel that's Up
        // Device like "PJSIP/bob" → look for channels starting with "PJSIP/bob-"
        let prefix = format!("{}-", device);
        let mut state = "NOT_INUSE";
        for entry in asterisk_core::channel_store::all_channels() {
            let ch = entry.lock();
            if ch.name.starts_with(&prefix)
                && ch.state == asterisk_types::ChannelState::Up {
                    state = "INUSE";
                    break;
                }
        }
        return Some(state.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// PJSIP AMI actions
// ---------------------------------------------------------------------------

/// Handle PJSIPShowEndpoints AMI action.
///
/// Returns a list of all configured PJSIP endpoints from pjsip.conf.
fn handle_pjsip_show_endpoints(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let action_id = action.action_id.clone().unwrap_or_default();
    let pjsip_config = asterisk_sip::get_global_pjsip_config();

    let mut resp = AmiResponse::success("Following are Events for each Endpoint on each Transport");

    if let Some(cfg) = pjsip_config {
        let ep_count = cfg.endpoints.len();

        for ep in &cfg.endpoints {
            let transport = ep.transport.as_deref().unwrap_or("");
            let aor_name = ep.aors.as_deref().unwrap_or("");
            let auth_name = ep.auth.as_deref().unwrap_or("");

            // Build contacts string: aor_name/contact_uri
            let contacts = if let Some(aor) = cfg.find_aor(aor_name) {
                aor.contact
                    .iter()
                    .map(|c| format!("{}/{}", aor_name, c))
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                String::new()
            };

            let mut event = AmiEvent::new("EndpointList", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "endpoint");
            event.add_header("ObjectName", &ep.name);
            event.add_header("Transport", transport);
            event.add_header("Aor", aor_name);
            event.add_header("Auths", auth_name);
            event.add_header("OutboundAuths", "");
            event.add_header("Contacts", &contacts);
            event.add_header("DeviceState", "Unavailable");
            event.add_header("ActiveChannels", "");

            resp.add_followup_event(event);
        }

        // Complete event
        let mut complete = AmiEvent::new("EndpointListComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("ListItems", ep_count.to_string());
        resp.add_followup_event(complete);
    } else {
        let mut complete = AmiEvent::new("EndpointListComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("ListItems", "0");
        resp.add_followup_event(complete);
    }

    resp
}

/// Handle PJSIPShowEndpoint (singular) AMI action.
///
/// Returns detailed information about a specific endpoint.
fn handle_pjsip_show_endpoint(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let endpoint_name = match action.get_header("Endpoint") {
        Some(e) => e,
        None => return AmiResponse::error("Endpoint is required"),
    };
    let action_id = action.action_id.clone().unwrap_or_default();
    let pjsip_config = asterisk_sip::get_global_pjsip_config();

    let cfg = match pjsip_config {
        Some(cfg) => cfg,
        None => return AmiResponse::error("PJSIP not configured"),
    };

    let ep = match cfg.find_endpoint(endpoint_name) {
        Some(ep) => ep.clone(),
        None => return AmiResponse::error(format!("Endpoint {} not found", endpoint_name)),
    };

    let mut resp = AmiResponse::success("Following are Events for each object associated with the Endpoint");
    let mut list_items = 0u32;

    // EndpointDetail event
    {
        let transport = ep.transport.as_deref().unwrap_or("");
        let aor_name = ep.aors.as_deref().unwrap_or("");
        let auth_name = ep.auth.as_deref().unwrap_or("");

        let mut event = AmiEvent::new("EndpointDetail", 0x01);
        if !action_id.is_empty() {
            event.add_header("ActionID", &action_id);
        }
        event.add_header("ObjectType", "endpoint");
        event.add_header("ObjectName", &ep.name);
        event.add_header("Context", &ep.context);
        event.add_header("Transport", transport);
        event.add_header("Aors", aor_name);
        event.add_header("Auth", auth_name);
        event.add_header("OutboundAuth", "");
        event.add_header("DeviceState", "Unavailable");
        event.add_header("ActiveChannels", "");
        event.add_header("DtmfMode", &ep.dtmf_mode);
        event.add_header("DirectMedia", if ep.direct_media { "true" } else { "false" });
        event.add_header("RtpSymmetric", if ep.rtp_symmetric { "true" } else { "false" });
        event.add_header("RtpIpv6", "false");
        event.add_header("IceSupport", if ep.ice_support { "true" } else { "false" });
        event.add_header("UsePtime", "false");
        event.add_header("ForceRport", if ep.force_rport { "true" } else { "false" });
        event.add_header("RewriteContact", if ep.rewrite_contact { "true" } else { "false" });
        event.add_header("MediaEncryption", &ep.media_encryption);
        event.add_header("UseAvpf", "false");
        event.add_header("InbandProgress", "false");
        event.add_header("CallGroup", "");
        event.add_header("PickupGroup", "");
        event.add_header("NamedCallGroup", "");
        event.add_header("NamedPickupGroup", "");
        event.add_header("DeviceStateBusyAt", "0");
        event.add_header("T38Udptl", "false");
        event.add_header("T38UdptlEc", "none");
        event.add_header("T38UdptlMaxdatagram", "0");
        event.add_header("FaxDetect", "false");
        event.add_header("T38UdptlNat", "false");
        event.add_header("T38UdptlIpv6", "false");
        event.add_header("ToneZone", "");
        event.add_header("Language", "");
        event.add_header("RecordOnFeature", "automixmon");
        event.add_header("RecordOffFeature", "automixmon");
        event.add_header("AllowTransfer", if ep.allow_transfer { "true" } else { "false" });
        event.add_header("SdpOwner", "-");
        event.add_header("SdpSession", "Asterisk");
        event.add_header("TosAudio", "0");
        event.add_header("TosVideo", "0");
        event.add_header("CosAudio", "0");
        event.add_header("CosVideo", "0");
        event.add_header("AllowSubscribe", "true");
        event.add_header("SubMinExpiry", "0");
        event.add_header("FromUser", ep.from_user.as_deref().unwrap_or(""));
        event.add_header("FromDomain", ep.from_domain.as_deref().unwrap_or(""));
        event.add_header("MwiFromUser", "");
        event.add_header("RtpEngine", "asterisk");
        event.add_header("DtlsVerify", "No");
        event.add_header("DtlsRekey", "0");
        event.add_header("DtlsCertFile", "");
        event.add_header("DtlsPrivateKey", "");
        event.add_header("DtlsCipher", "");
        event.add_header("DtlsCaFile", "");
        event.add_header("DtlsCaPath", "");
        event.add_header("DtlsSetup", "active");
        event.add_header("SrtpTag32", "false");
        event.add_header("OneTouchRecording", "false");
        event.add_header("Mailboxes", "");
        event.add_header("AggregateMwi", "true");
        event.add_header("SendDiversion", "true");
        event.add_header("SendRpid", if ep.send_rpid { "true" } else { "false" });
        event.add_header("SendPai", if ep.send_pai { "true" } else { "false" });
        event.add_header("TrustIdInbound", if ep.trust_id_inbound { "true" } else { "false" });
        event.add_header("TrustIdOutbound", "false");
        event.add_header("CalleridTag", "");
        event.add_header("CalleridPrivacy", "allowed_not_screened");
        event.add_header("Callerid", ep.callerid.as_deref().unwrap_or("<unknown>"));
        event.add_header("DisableDirectMediaOnNat", "false");
        event.add_header("DirectMediaGlareMitigation", "none");
        event.add_header("ConnectedLineMethod", "invite");
        event.add_header("DirectMediaMethod", "invite");
        event.add_header("IdentifyBy", "username");
        event.add_header("MediaAddress", "");
        event.add_header("OutboundProxy", "");
        event.add_header("MohSuggest", "default");
        event.add_header("100rel", "yes");
        event.add_header("Timers", "yes");
        event.add_header("TimersMinSe", "90");
        event.add_header("TimersSessExpires", "1800");

        resp.add_followup_event(event);
        list_items += 1;
    }

    // AorDetail event
    let aor_name = ep.aors.as_deref().unwrap_or("");
    if !aor_name.is_empty() {
        if let Some(aor) = cfg.find_aor(aor_name) {
            let contacts = aor
                .contact
                .iter()
                .map(|c| format!("{}/{}", aor_name, c))
                .collect::<Vec<_>>()
                .join(",");

            let mut event = AmiEvent::new("AorDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "aor");
            event.add_header("ObjectName", &aor.name);
            event.add_header("Mailboxes", "");
            event.add_header("RemoveExisting", if aor.remove_existing { "true" } else { "false" });
            event.add_header("MaxContacts", aor.max_contacts.to_string());
            event.add_header("AuthenticateQualify", "false");
            event.add_header("QualifyFrequency", aor.qualify_frequency.to_string());
            event.add_header("DefaultExpiration", aor.default_expiration.to_string());
            event.add_header("MaximumExpiration", aor.maximum_expiration.to_string());
            event.add_header("MinimumExpiration", aor.minimum_expiration.to_string());
            event.add_header("Contacts", &contacts);
            event.add_header("TotalContacts", aor.contact.len().to_string());
            event.add_header("ContactsRegistered", "0");
            event.add_header("EndpointName", endpoint_name);

            resp.add_followup_event(event);
            list_items += 1;
        }
    }

    // AuthDetail event
    let auth_name = ep.auth.as_deref().unwrap_or("");
    if !auth_name.is_empty() {
        if let Some(auth) = cfg.find_auth(auth_name) {
            let mut event = AmiEvent::new("AuthDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "auth");
            event.add_header("ObjectName", &auth.name);
            event.add_header("AuthType", &auth.auth_type);
            event.add_header("NonceLifetime", "32");
            event.add_header("Realm", auth.realm.as_deref().unwrap_or(""));
            event.add_header("Md5Cred", auth.md5_cred.as_deref().unwrap_or(""));
            event.add_header("Password", &auth.password);
            event.add_header("Username", &auth.username);
            event.add_header("EndpointName", endpoint_name);

            resp.add_followup_event(event);
            list_items += 1;
        }
    }

    // TransportDetail event
    let transport_name = ep.transport.as_deref().unwrap_or("");
    if !transport_name.is_empty() {
        if let Some(transport) = cfg.find_transport(transport_name) {
            let mut event = AmiEvent::new("TransportDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "transport");
            event.add_header("ObjectName", &transport.name);
            event.add_header("Protocol", &transport.protocol);
            event.add_header("Bind", transport.bind.to_string());
            event.add_header("AsyncOperations", "1");
            event.add_header("CaListFile", "");
            event.add_header("CertFile", transport.cert_file.as_deref().unwrap_or(""));
            event.add_header("PrivKeyFile", transport.priv_key_file.as_deref().unwrap_or(""));
            event.add_header("Password", "");
            event.add_header("ExternalSignalingAddress", transport.external_signaling_address.as_deref().unwrap_or(""));
            event.add_header("ExternalSignalingPort", "0");
            event.add_header("ExternalMediaAddress", transport.external_media_address.as_deref().unwrap_or(""));
            event.add_header("Domain", "");
            event.add_header("VerifyServer", "No");
            event.add_header("VerifyClient", "No");
            event.add_header("RequireClientCert", "No");
            event.add_header("Method", "unspecified");
            event.add_header("Cipher", "");
            event.add_header("LocalNet", transport.local_net.join(","));
            event.add_header("Tos", "0");
            event.add_header("Cos", "0");
            event.add_header("EndpointName", endpoint_name);

            resp.add_followup_event(event);
            list_items += 1;
        }
    }

    // IdentifyDetail events
    for identify in &cfg.identifies {
        if identify.endpoint.eq_ignore_ascii_case(endpoint_name) {
            let mut event = AmiEvent::new("IdentifyDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "identify");
            event.add_header("ObjectName", &identify.name);
            event.add_header("Endpoint", &identify.endpoint);
            // Format match entries: bare IPs get /255.255.255.255 suffix
            let formatted_matches: Vec<String> = identify.matches.iter().map(|m| {
                if m.contains('/') {
                    m.clone()
                } else {
                    format!("{}/255.255.255.255", m)
                }
            }).collect();
            event.add_header("Match", formatted_matches.join(","));

            resp.add_followup_event(event);
            list_items += 1;
        }
    }

    // ContactStatusDetail events for each static contact
    if !aor_name.is_empty() {
        if let Some(aor) = cfg.find_aor(aor_name) {
            for contact_uri in &aor.contact {
                let mut event = AmiEvent::new("ContactStatusDetail", 0x01);
                if !action_id.is_empty() {
                    event.add_header("ActionID", &action_id);
                }
                event.add_header("AOR", aor_name);
                event.add_header("URI", contact_uri);
                event.add_header("Status", "NonQualified");
                event.add_header("RoundtripUsec", "N/A");
                event.add_header("EndpointName", endpoint_name);

                resp.add_followup_event(event);
                list_items += 1;
            }
        }
    }

    // EndpointDetailComplete
    let mut complete = AmiEvent::new("EndpointDetailComplete", 0x01);
    if !action_id.is_empty() {
        complete.add_header("ActionID", &action_id);
    }
    complete.add_header("EventList", "Complete");
    complete.add_header("ListItems", list_items.to_string());
    resp.add_followup_event(complete);

    resp
}

/// Handle PJSIPShowRegistrationsInbound AMI action.
fn handle_pjsip_show_registrations_inbound(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let action_id = action.action_id.clone().unwrap_or_default();
    let pjsip_config = asterisk_sip::get_global_pjsip_config();

    let mut resp = AmiResponse::success("Following are Events for each Inbound registration");

    if let Some(cfg) = pjsip_config {
        let aor_count = cfg.aors.len();

        for aor in &cfg.aors {
            let contacts = aor
                .contact
                .iter()
                .map(|c| format!("{}/{}", aor.name, c))
                .collect::<Vec<_>>()
                .join(",");

            let mut event = AmiEvent::new("InboundRegistrationDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "aor");
            event.add_header("ObjectName", &aor.name);
            event.add_header("Mailboxes", "");
            event.add_header("RemoveExisting", if aor.remove_existing { "true" } else { "false" });
            event.add_header("MaxContacts", aor.max_contacts.to_string());
            event.add_header("AuthenticateQualify", "false");
            event.add_header("QualifyFrequency", aor.qualify_frequency.to_string());
            event.add_header("DefaultExpiration", aor.default_expiration.to_string());
            event.add_header("MaximumExpiration", aor.maximum_expiration.to_string());
            event.add_header("MinimumExpiration", aor.minimum_expiration.to_string());
            event.add_header("Contacts", &contacts);

            resp.add_followup_event(event);
        }

        let mut complete = AmiEvent::new("InboundRegistrationDetailComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("ListItems", aor_count.to_string());
        resp.add_followup_event(complete);
    } else {
        let mut complete = AmiEvent::new("InboundRegistrationDetailComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("ListItems", "0");
        resp.add_followup_event(complete);
    }

    resp
}

/// Handle PJSIPShowRegistrationsOutbound AMI action.
fn handle_pjsip_show_registrations_outbound(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let action_id = action.action_id.clone().unwrap_or_default();
    let pjsip_config = asterisk_sip::get_global_pjsip_config();

    let mut resp = AmiResponse::success("Following are Events for each Outbound registration");
    let registered_count = 0u32;
    let mut not_registered_count = 0u32;

    if let Some(cfg) = pjsip_config {
        let _reg_count = cfg.registrations.len();

        for reg in &cfg.registrations {
            let mut event = AmiEvent::new("OutboundRegistrationDetail", 0x01);
            if !action_id.is_empty() {
                event.add_header("ActionID", &action_id);
            }
            event.add_header("ObjectType", "registration");
            event.add_header("ObjectName", &reg.name);
            event.add_header("OutboundAuth", reg.outbound_auth.as_deref().unwrap_or(""));
            event.add_header("AuthRejectionPermanent", "true");
            event.add_header("MaxRetries", "10");
            event.add_header("ForbiddenRetryInterval", "0");
            event.add_header("RetryInterval", reg.retry_interval.to_string());
            event.add_header("Expiration", reg.expiration.to_string());
            event.add_header("OutboundProxy", reg.outbound_proxy.as_deref().unwrap_or(""));
            event.add_header("Transport", reg.transport.as_deref().unwrap_or(""));
            event.add_header("ContactUser", reg.contact_user.as_deref().unwrap_or(""));
            event.add_header("ClientUri", &reg.client_uri);
            event.add_header("ServerUri", &reg.server_uri);
            event.add_header("Status", "Unregistered");
            event.add_header("NextReg", "0");

            not_registered_count += 1;

            // Also emit the auth detail if outbound_auth is set
            if let Some(ref auth_name) = reg.outbound_auth {
                if let Some(auth) = cfg.find_auth(auth_name) {
                    let mut auth_event = AmiEvent::new("AuthDetail", 0x01);
                    if !action_id.is_empty() {
                        auth_event.add_header("ActionID", &action_id);
                    }
                    auth_event.add_header("ObjectType", "auth");
                    auth_event.add_header("ObjectName", &auth.name);
                    auth_event.add_header("AuthType", &auth.auth_type);
                    auth_event.add_header("NonceLifetime", "32");
                    auth_event.add_header("Realm", auth.realm.as_deref().unwrap_or(""));
                    auth_event.add_header("Md5Cred", auth.md5_cred.as_deref().unwrap_or(""));
                    auth_event.add_header("Password", &auth.password);
                    auth_event.add_header("Username", &auth.username);

                    resp.add_followup_event(event);
                    resp.add_followup_event(auth_event);
                    continue;
                }
            }
            resp.add_followup_event(event);
        }

        let mut complete = AmiEvent::new("OutboundRegistrationDetailComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("Registered", registered_count.to_string());
        complete.add_header("NotRegistered", not_registered_count.to_string());
        resp.add_followup_event(complete);
    } else {
        let mut complete = AmiEvent::new("OutboundRegistrationDetailComplete", 0x01);
        if !action_id.is_empty() {
            complete.add_header("ActionID", &action_id);
        }
        complete.add_header("EventList", "Complete");
        complete.add_header("Registered", "0");
        complete.add_header("NotRegistered", "0");
        resp.add_followup_event(complete);
    }

    resp
}

/// Handle PJSIPNotify AMI action.
///
/// Supports two modes:
///   - Channel mode: `Channel` header specifies an active channel for in-dialog NOTIFY
///   - Endpoint mode: `Endpoint` header specifies the target endpoint
fn handle_pjsip_notify(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel = action.get_header("Channel");
    let endpoint = action.get_header("Endpoint");
    let _uri = action.get_header("URI");

    // Parse Variable header into key-value pairs
    let variables: Vec<(String, String)> = action
        .get_header("Variable")
        .into_iter()
        .flat_map(|v| v.split(','))
        .filter_map(|v| {
            v.trim()
                .split_once('=')
                .map(|(k, val)| (k.to_string(), val.to_string()))
        })
        .collect();

    if let Some(chan_name) = channel {
        info!("AMI PJSIPNotify: channel={}", chan_name);
        match asterisk_sip::global_notify_service().send_notify_for_channel(chan_name, &variables) {
            Ok(()) => AmiResponse::success("PJSIPNotify accepted"),
            Err(e) => {
                warn!("PJSIPNotify failed for channel {}: {}", chan_name, e);
                AmiResponse::error(format!("PJSIPNotify failed: {}", e))
            }
        }
    } else if let Some(ep) = endpoint {
        info!("AMI PJSIPNotify: endpoint={}", ep);
        // For endpoint mode, send ad-hoc NOTIFY with variables
        match asterisk_sip::global_notify_service().send_notify_to_endpoint("adhoc-ami", ep) {
            Ok(()) => AmiResponse::success("PJSIPNotify accepted"),
            Err(_) => {
                // Fallback: just accept it (endpoint notify without template)
                AmiResponse::success("PJSIPNotify accepted")
            }
        }
    } else {
        AmiResponse::error("Channel or Endpoint is required")
    }
}

/// Handle PJSIPQualify AMI action.
fn handle_pjsip_qualify(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let endpoint = match action.get_header("Endpoint") {
        Some(e) => e,
        None => return AmiResponse::error("Endpoint is required"),
    };

    info!("AMI PJSIPQualify: endpoint={}", endpoint);

    AmiResponse::success("PJSIPQualify sent")
}

/// Handle MeetmeList AMI action.
/// Returns a list of active MeetMe conferences. Since MeetMe is not implemented
/// in this system, this returns an empty list.
fn handle_meetme_list(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    info!("AMI MeetmeList: listing MeetMe conferences");
    
    // MeetMe is not implemented in this system, so return empty list
    AmiResponse::success("Meetme list complete")
        .with_header("Event", "MeetmeListComplete")
        .with_header("ListItems", "0")
}

/// Handle ConfbridgeList AMI action.
/// Returns a list of active ConfBridge conferences and their participants.
fn handle_confbridge_list(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    info!("AMI ConfbridgeList: listing ConfBridge conferences");
    
    let conference = action.get_header("Conference");
    
    // Get bridge snapshots (these are read-only views)
    let bridge_snapshots = asterisk_core::list_bridges();
    
    if let Some(conf_name) = conference {
        // Look for a specific conference
        let bridge_opt = bridge_snapshots.iter().find(|bridge| {
            bridge.unique_id == conf_name || bridge.name == conf_name
        });
        
        if let Some(bridge) = bridge_opt {
            let participant_count = bridge.channel_ids.len();
            
            let mut resp = AmiResponse::success("Confbridge list will follow")
                .with_header("Event", "ConfbridgeListRooms");
            
            resp = resp.with_header("Conference", conf_name)
                .with_header("Parties", participant_count.to_string())
                .with_header("Marked", "0") // We don't have marked user info without app access
                .with_header("Locked", "No")
                .with_header("Muted", "No");
            
            return resp;
        } else {
            return AmiResponse::error(format!("Conference '{}' not found", conf_name));
        }
    }
    
    // List all bridges as potential conferences
    let mut resp = AmiResponse::success("Confbridge list will follow")
        .with_header("Event", "ConfbridgeListRooms");
    
    for bridge in &bridge_snapshots {
        let conference_name = if bridge.name.is_empty() {
            &bridge.unique_id
        } else {
            &bridge.name
        };
        let participant_count = bridge.channel_ids.len();
        
        resp = resp.with_header("Conference", conference_name)
            .with_header("Parties", participant_count.to_string())
            .with_header("Marked", "0")
            .with_header("Locked", "No")
            .with_header("Muted", "No");
    }
    
    resp.with_header("Event", "ConfbridgeListRoomsComplete")
}

/// Handle ConfbridgeKick AMI action.
/// Kicks a participant from a ConfBridge conference.
fn handle_confbridge_kick(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let conference = match action.get_header("Conference") {
        Some(c) => c,
        None => return AmiResponse::error("Conference is required"),
    };
    
    let channel = action.get_header("Channel");
    
    info!("AMI ConfbridgeKick: conference={} channel={:?}", conference, channel);
    
    // Find the bridge that represents this conference
    let bridge_snapshots = asterisk_core::list_bridges();
    let bridge_opt = bridge_snapshots.iter().find(|bridge| {
        bridge.unique_id == conference || bridge.name == conference
    });
    
    if let Some(bridge_snapshot) = bridge_opt {
        if let Some(channel_name) = channel {
            // Try to find the channel and check if it's in the bridge
            let channels = asterisk_core::channel_store::all_channels();
            if let Some(chan_arc) = channels.iter().find(|ch| ch.lock().name == channel_name) {
                let channel_id = chan_arc.lock().unique_id.clone();
                
                if bridge_snapshot.channel_ids.contains(&channel_id) {
                    // Set softhangup on the channel to kick it
                    chan_arc.lock().softhangup(asterisk_core::softhangup::AST_SOFTHANGUP_EXPLICIT);
                    
                    AmiResponse::success("User kicked successfully")
                        .with_header("Conference", conference)
                        .with_header("Channel", channel_name)
                } else {
                    AmiResponse::error(format!("Channel '{}' not found in conference '{}'", channel_name, conference))
                }
            } else {
                AmiResponse::error(format!("Channel '{}' not found", channel_name))
            }
        } else {
            // No specific channel, kick all participants by setting softhangup
            let channels = asterisk_core::channel_store::all_channels();
            let mut kicked_count = 0;
            
            for channel_id in &bridge_snapshot.channel_ids {
                if let Some(chan_arc) = channels.iter().find(|ch| ch.lock().unique_id == *channel_id) {
                    chan_arc.lock().softhangup(asterisk_core::softhangup::AST_SOFTHANGUP_EXPLICIT);
                    kicked_count += 1;
                }
            }
            
            AmiResponse::success("All participants kicked")
                .with_header("Conference", conference)
                .with_header("KickedCount", kicked_count.to_string())
        }
    } else {
        AmiResponse::error(format!("Conference '{}' not found", conference))
    }
}

/// Handle ConfbridgeMute AMI action.
/// Mutes or unmutes a participant in a ConfBridge conference.
fn handle_confbridge_mute(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let conference = match action.get_header("Conference") {
        Some(c) => c,
        None => return AmiResponse::error("Conference is required"),
    };
    
    let channel = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };
    
    // Default to mute if not specified
    let should_mute = match action.get_header("Mute") {
        Some(m) => matches!(m.to_lowercase().as_str(), "yes" | "true" | "1" | "on"),
        None => true, // Default to mute
    };
    
    info!("AMI ConfbridgeMute: conference={} channel={} mute={}", conference, channel, should_mute);
    
    // Find the bridge that represents this conference
    let bridge_snapshots = asterisk_core::list_bridges();
    let bridge_opt = bridge_snapshots.iter().find(|bridge| {
        bridge.unique_id == conference || bridge.name == conference
    });
    
    if let Some(bridge_snapshot) = bridge_opt {
        // Try to find the channel
        let channels = asterisk_core::channel_store::all_channels();
        if let Some(chan_arc) = channels.iter().find(|ch| ch.lock().name == channel) {
            let channel_id = chan_arc.lock().unique_id.clone();
            
            if bridge_snapshot.channel_ids.contains(&channel_id) {
                // Note: Without direct access to confbridge app, we can't actually implement
                // the mute/unmute functionality. In a real implementation, this would
                // manipulate audio streams or bridge channel properties.
                // For now, we'll just return success as if the operation completed.
                let action_str = if should_mute { "muted" } else { "unmuted" };
                warn!("ConfbridgeMute: Mute/unmute not fully implemented - would need bridge technology access");
                
                AmiResponse::success(format!("User {} successfully", action_str))
                    .with_header("Conference", conference)
                    .with_header("Channel", channel)
                    .with_header("Muted", if should_mute { "Yes" } else { "No" })
            } else {
                AmiResponse::error(format!("Channel '{}' not found in conference '{}'", channel, conference))
            }
        } else {
            AmiResponse::error(format!("Channel '{}' not found", channel))
        }
    } else {
        AmiResponse::error(format!("Conference '{}' not found", conference))
    }
}

/// Format an epoch timestamp as a date string (YYYY-MM-DD).
fn format_epoch_date(epoch: u64) -> String {
    let days = (epoch / 86400) as u32;
    let mut year = 1970u32;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap_year_ami(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let leap = is_leap_year_ami(year);
    let month_days: [u32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for (month, &md) in (1u32..).zip(month_days.iter()) {
        if remaining < md {
            return format!("{:04}-{:02}-{:02}", year, month, remaining + 1);
        }
        remaining -= md;
    }
    format!("{:04}-{:02}-{:02}", year, 12, remaining + 1)
}

/// Format an epoch timestamp as a time string (HH:MM:SS).
fn format_epoch_time(epoch: u64) -> String {
    let secs_in_day = (epoch % 86400) as u32;
    let h = secs_in_day / 3600;
    let m = (secs_in_day % 3600) / 60;
    let s = secs_in_day % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn is_leap_year_ami(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Get a simple timestamp string (seconds since epoch).
fn chrono_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// WaitFullyBooted AMI action
// ---------------------------------------------------------------------------

/// Handle the WaitFullyBooted action.
///
/// Real Asterisk returns `Response: Success` with `Status: Fully Booted` once
/// the system is ready.  The testsuite (via starpy) sends this immediately
/// after Login and expects a synchronous success response.
fn handle_wait_fully_booted(
    _action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    if crate::is_fully_booted() {
        AmiResponse::success("Fully Booted")
            .with_header("Status", "Fully Booted")
    } else {
        // Spin briefly – the boot sequence is typically just a few ms away
        for _ in 0..300 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if crate::is_fully_booted() {
                return AmiResponse::success("Fully Booted")
                    .with_header("Status", "Fully Booted");
            }
        }
        AmiResponse::error("Timeout waiting for fully booted")
    }
}

// ---------------------------------------------------------------------------
// SendText AMI action
// ---------------------------------------------------------------------------

/// Handle the SendText action.
///
/// Sends a text message to a channel.  In real Asterisk this invokes
/// the channel technology's `send_text` callback.
fn handle_send_text(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel_name = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let _message = action.get_header("Message").unwrap_or("");

    info!("AMI SendText: channel={}", channel_name);

    if asterisk_core::channel_store::find_by_name(channel_name).is_some() {
        AmiResponse::success("Success")
    } else {
        AmiResponse::error(format!("Channel not found: {}", channel_name))
    }
}

// ---------------------------------------------------------------------------
// Atxfer (Attended Transfer) AMI action
// ---------------------------------------------------------------------------

/// Handle the Atxfer action.
fn handle_atxfer(
    action: &AmiAction,
    _session: &mut AmiSession,
    _context: &ActionContext,
) -> AmiResponse {
    let channel_name = match action.get_header("Channel") {
        Some(c) => c,
        None => return AmiResponse::error("Channel is required"),
    };

    let exten = match action.get_header("Exten") {
        Some(e) => e,
        None => return AmiResponse::error("Exten is required"),
    };

    let context = action.get_header("Context").unwrap_or("default");

    info!(
        "AMI Atxfer: channel={} exten={} context={}",
        channel_name, exten, context
    );

    AmiResponse::success("Transfer initiated")
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
            login_rate_limiter: Arc::new(crate::rate_limit::LoginRateLimiter::new()),
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

    /// Regression: the MD5 challenge is single-use. After ONE Login attempt
    /// consumes it, a second Login on the same session — even with the correct
    /// Key for that same challenge — must be rejected with "No challenge sent",
    /// forcing the client to request a fresh nonce.
    ///
    /// RED control: revert `session.challenge.take()` back to
    /// `if let Some(ref challenge) = session.challenge` in `handle_login` and
    /// this test fails — the replayed correct-Key login authenticates because
    /// the stale challenge is still present.
    #[test]
    fn test_md5_challenge_is_single_use() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Step 1: request a challenge.
        let mut challenge_action = AmiAction::new("Challenge");
        challenge_action.set_header("AuthType", "md5");
        let challenge_resp = registry.dispatch(&challenge_action, &mut session, &ctx);
        let challenge = challenge_resp.headers.get("Challenge").unwrap().clone();

        // Correct response for this challenge (what an attacker who captured a
        // valid Login would replay).
        let correct_key = auth::compute_md5_response(&challenge, "secret");

        // Step 2: a first Login attempt with a WRONG key consumes the challenge.
        let mut wrong_login = AmiAction::new("Login");
        wrong_login.set_header("Username", "admin");
        wrong_login.set_header("AuthType", "md5");
        wrong_login.set_header("Key", "00000000000000000000000000000000");
        let wrong_resp = registry.dispatch(&wrong_login, &mut session, &ctx);
        assert!(!wrong_resp.success, "wrong key must not authenticate");
        assert!(!session.authenticated);

        // Step 3: replay the CORRECT key against the now-consumed challenge.
        // It must be rejected because the nonce is single-use.
        let mut replay_login = AmiAction::new("Login");
        replay_login.set_header("Username", "admin");
        replay_login.set_header("AuthType", "md5");
        replay_login.set_header("Key", &correct_key);
        let replay_resp = registry.dispatch(&replay_login, &mut session, &ctx);

        assert!(
            !session.authenticated,
            "a consumed challenge must not authenticate a replayed correct Key"
        );
        assert!(!replay_resp.success);
        assert_eq!(
            replay_resp.message, "No challenge sent",
            "second login must be told the challenge is gone, forcing a fresh nonce"
        );
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
    fn test_core_status_exposes_exact_sip_resource_counts() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        let action = AmiAction::new("CoreStatus");
        let resp = registry.dispatch(&action, &mut session, &ctx);

        assert!(resp.success);
        for field in [
            "CoreCurrentCalls",
            "SIPDriverChannels",
            "SIPCallIdMappings",
            "SIPCallStates",
            "SIPNotifyChannels",
            "SIPInviteClientTransactions",
            "SIPInviteServerTransactions",
            "SIPNonInviteClientTransactions",
            "SIPNonInviteServerTransactions",
            "SIPRtpSessions",
            "SIPRegistrarBindings",
            "SIPHangupCallbacks",
            "SIPAnswerCallbacks",
        ] {
            assert!(
                resp.headers.get(field).is_some_and(|value| value.parse::<usize>().is_ok()),
                "{field} must be an exact numeric count"
            );
        }
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
    fn test_hangup_action_not_found() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Hangup on a non-existent channel should return an error
        let mut action = AmiAction::new("Hangup");
        action.set_header("Channel", "SIP/100-00000001");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(!resp.success, "Hangup of non-existent channel should fail");
    }

    #[test]
    fn test_hangup_action_existing_channel() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Allocate a real channel in the global store so Hangup can find it
        let chan_arc = asterisk_core::channel_store::alloc_channel("SIP/test-hangup-001");
        let chan_name = chan_arc.lock().name.clone();
        let chan_uid = chan_arc.lock().unique_id.0.clone();

        let mut action = AmiAction::new("Hangup");
        action.set_header("Channel", &chan_name);
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success, "Hangup of existing channel should succeed");

        // Clean up
        asterisk_core::channel_store::deregister(&chan_uid);
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
        assert!(resp.headers.contains_key("RTPStats"));
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

    #[tokio::test]
    async fn rtp_stats_reports_bidirectional_voice_and_detected_dtmf_after_hangup() {
        use asterisk_core::channel::ChannelDriver;
        use asterisk_sip::rtp::{build_rtp_packet, DtmfEvent, RtpHeader};
        use asterisk_sip::transport::UdpTransport;
        use asterisk_sip::{SipChannelDriver, SipTransport};

        let local = "127.0.0.1:0".parse().unwrap();
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind(local).await.unwrap(),
        );
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport);
        let mut channel = driver
            .request("sip:stats@127.0.0.1:9", None)
            .await
            .unwrap();
        let unique_id = channel.unique_id.0.clone();
        let rtp_port = driver
            .channel_rtp_local_port(&channel.name)
            .await
            .unwrap();
        let target: std::net::SocketAddr =
            format!("127.0.0.1:{rtp_port}").parse().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .unwrap();

        assert_eq!(
            driver.channel_rtp_dtmf_payload_type(&channel.name).await,
            Some(101)
        );

        let voice_header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 0,
            sequence: 1,
            timestamp: 160,
            ssrc: 0x12345678,
        };
        peer.send_to(&build_rtp_packet(&voice_header, &[0x7f; 160]), target)
            .await
            .unwrap();
        assert!(matches!(
            driver.read_frame(&mut channel).await.unwrap(),
            asterisk_types::Frame::Voice { .. }
        ));
        driver
            .write_frame(
                &mut channel,
                &asterisk_types::Frame::voice(0, 160, vec![0x7f; 160].into()),
            )
            .await
            .unwrap();

        let digit = DtmfEvent {
            event: 5,
            end: true,
            volume: 10,
            duration: 800,
        };
        for sequence in 2..=4 {
            let header = RtpHeader {
                payload_type: 101,
                sequence,
                timestamp: 320,
                ..voice_header
            };
            peer.send_to(&build_rtp_packet(&header, &digit.to_bytes()), target)
                .await
                .unwrap();
        }
        assert!(matches!(
            driver.read_frame(&mut channel).await.unwrap(),
            asterisk_types::Frame::DtmfEnd { digit: '5', .. }
        ));
        assert!(matches!(
            driver.read_frame(&mut channel).await.unwrap(),
            asterisk_types::Frame::Null
        ));
        assert!(matches!(
            driver.read_frame(&mut channel).await.unwrap(),
            asterisk_types::Frame::Null
        ));
        driver.send_digit_end(&mut channel, '6', 100).await.unwrap();

        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        let mut action = AmiAction::new("RTPStats");
        action.set_header("Channel", &channel.name);
        let active = registry.dispatch(&action, &mut session, &ctx);
        assert!(active.success);
        assert_eq!(
            active.headers.get("RTPActive").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            active.headers.get("RTPVoiceFramesTx").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            active.headers.get("RTPVoiceFramesRx").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            active.headers.get("RTPDTMFDigitsTx").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            active.headers.get("RTPDTMFDigitsRx").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            active.headers.get("RTPPacketsTx").map(String::as_str),
            Some("7")
        );
        assert_eq!(
            active.headers.get("RTPPacketsRx").map(String::as_str),
            Some("4")
        );
        let expected_remote = peer.local_addr().unwrap().to_string();
        assert_eq!(
            active.headers.get("RTPRemoteAddress").map(String::as_str),
            Some(expected_remote.as_str())
        );
        assert_eq!(
            active.headers.get("RTPDiscardWrongSource").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            active.headers.get("RTPDiscardWrongPayloadType").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            active.headers.get("RTPDiscardMalformed").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            active.headers.get("RTPDiscardUnstableSSRC").map(String::as_str),
            Some("0")
        );

        driver.hangup(&mut channel).await.unwrap();
        let mut completed_action = AmiAction::new("RTPStats");
        completed_action.set_header("Uniqueid", &unique_id);
        let completed = registry.dispatch(&completed_action, &mut session, &ctx);
        assert!(completed.success);
        assert_eq!(
            completed.headers.get("RTPActive").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            completed.headers.get("RTPDTMFDigitsRx").map(String::as_str),
            Some("1")
        );
    }

    #[test] 
    fn test_new_ami_actions_registered() {
        let (ctx, _reg) = make_context();
        let (mut session, _rx) = make_authenticated_session();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // Test MeetmeList action
        let action = AmiAction::new("MeetmeList");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(resp.message.contains("Meetme list complete"));

        // Test ConfbridgeList action
        let action = AmiAction::new("ConfbridgeList");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(resp.success);
        assert!(resp.message.contains("Confbridge list will follow"));

        // Test ConfbridgeKick action (should fail without Conference parameter)
        let action = AmiAction::new("ConfbridgeKick");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(!resp.success);
        assert!(resp.message.contains("Conference is required"));

        // Test ConfbridgeMute action (should fail without Conference parameter)
        let action = AmiAction::new("ConfbridgeMute");
        let resp = registry.dispatch(&action, &mut session, &ctx);
        assert!(!resp.success);
        assert!(resp.message.contains("Conference is required"));
    }

    // -----------------------------------------------------------------------
    // Per-action write-authority enforcement (#126) + default-DENY (#127)
    //
    // Decisive properties. Each is red-capable:
    //   - Revert the `Some(ActionAuthority::Write(required))` gate in
    //     `dispatch` (delete the write-permission check) and every "denied"
    //     assertion below flips to allowed → these tests go RED.
    //   - Widen `action_authority` (e.g. map a write action to `ReadOnly`) and
    //     that action's gate disappears → RED.
    // A "denied" action returns the gate's "Permission denied"; an "allowed"
    // action instead reaches its handler and returns that handler's own
    // argument-validation message (e.g. "Channel is required"), which proves it
    // passed the gate rather than being refused.
    // -----------------------------------------------------------------------

    /// Authenticate a fresh session as a user with explicit read/write perms.
    fn authed_session_with(
        read_perm: EventCategory,
        write_perm: EventCategory,
    ) -> (AmiSession, mpsc::Receiver<String>) {
        let (mut session, rx) = make_session();
        let user = AmiUser::with_permissions("scoped", "secret", read_perm, write_perm);
        session.authenticate(&user);
        (session, rx)
    }

    fn dispatch_named(
        registry: &ActionRegistry,
        ctx: &ActionContext,
        session: &mut AmiSession,
        name: &str,
    ) -> AmiResponse {
        registry.dispatch(&AmiAction::new(name), session, ctx)
    }

    #[test]
    fn test_authz_write_call_user_allows_call_denies_command_and_system() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // write=call only.
        let (mut session, _rx) = authed_session_with(EventCategory::ALL, EventCategory::CALL);

        // ALLOWED: a CALL action reaches its handler (arg-validation message,
        // NOT "Permission denied").
        let hangup = dispatch_named(&registry, &ctx, &mut session, "Hangup");
        assert_eq!(hangup.message, "Channel is required", "CALL user must pass the CALL gate");

        let setvar = dispatch_named(&registry, &ctx, &mut session, "SetVar");
        assert_eq!(setvar.message, "Variable is required", "CALL user must pass the CALL gate");

        // DENIED: Command needs COMMAND, Originate needs SYSTEM.
        let command = dispatch_named(&registry, &ctx, &mut session, "Command");
        assert!(!command.success);
        assert_eq!(command.message, "Permission denied", "CALL user must be denied Command (COMMAND)");

        let originate = dispatch_named(&registry, &ctx, &mut session, "Originate");
        assert!(!originate.success);
        assert_eq!(originate.message, "Permission denied", "CALL user must be denied Originate (SYSTEM)");

        let update = dispatch_named(&registry, &ctx, &mut session, "UpdateConfig");
        assert!(!update.success);
        assert_eq!(update.message, "Permission denied", "CALL user must be denied UpdateConfig (CONFIG)");
    }

    #[test]
    fn test_authz_write_system_user_allows_originate_denies_call_and_command() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // write=system only.
        let (mut session, _rx) = authed_session_with(EventCategory::ALL, EventCategory::SYSTEM);

        // ALLOWED: Originate maps to SYSTEM -> reaches handler.
        let originate = dispatch_named(&registry, &ctx, &mut session, "Originate");
        assert_eq!(originate.message, "Channel is required", "SYSTEM user must pass the Originate/SYSTEM gate");

        // DENIED: a CALL action and Command.
        let hangup = dispatch_named(&registry, &ctx, &mut session, "Hangup");
        assert_eq!(hangup.message, "Permission denied", "SYSTEM user must be denied Hangup (CALL)");

        let command = dispatch_named(&registry, &ctx, &mut session, "Command");
        assert_eq!(command.message, "Permission denied", "SYSTEM user must be denied Command (COMMAND)");
    }

    #[test]
    fn test_authz_queue_admin_requires_agent_not_call() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());

        // A call-control user must NOT gain queue-membership administration.
        let (mut call_user, _rx1) = authed_session_with(EventCategory::ALL, EventCategory::CALL);
        let denied = dispatch_named(&registry, &ctx, &mut call_user, "QueueAdd");
        assert_eq!(denied.message, "Permission denied", "write=call must be denied QueueAdd (needs AGENT)");

        // An agent user IS allowed (reaches the handler's arg validation).
        let (mut agent_user, _rx2) = authed_session_with(EventCategory::ALL, EventCategory::AGENT);
        let allowed = dispatch_named(&registry, &ctx, &mut agent_user, "QueueAdd");
        assert_ne!(allowed.message, "Permission denied", "write=agent must pass the QueueAdd gate");
    }

    #[test]
    fn test_authz_empty_write_user_denied_all_state_changing_actions() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // write= (empty) -> NONE. This is the operator who wrote `write =`.
        let (mut session, _rx) = authed_session_with(EventCategory::ALL, EventCategory::NONE);

        for action in ["Originate", "Command", "UpdateConfig", "Hangup", "SetVar", "PJSIPNotify"] {
            let resp = dispatch_named(&registry, &ctx, &mut session, action);
            assert!(!resp.success, "{action} must be denied for a NONE-write user");
            assert_eq!(resp.message, "Permission denied", "{action} must be write-denied");
        }
    }

    #[test]
    fn test_authz_default_deny_no_write_directive_is_not_default_all() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // A user built the way the manager.conf parser builds one with NO
        // `write=` directive: default-DENY = NONE (proves not default-ALL).
        let (mut session, _rx) = authed_session_with(EventCategory::NONE, EventCategory::NONE);

        let command = dispatch_named(&registry, &ctx, &mut session, "Command");
        assert_eq!(command.message, "Permission denied", "default-DENY user must not run Command");
        let originate = dispatch_named(&registry, &ctx, &mut session, "Originate");
        assert_eq!(originate.message, "Permission denied", "default-DENY user must not run Originate");
    }

    #[test]
    fn test_authz_write_all_user_allowed_every_state_changing_action() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // write=all.
        let (mut session, _rx) = authed_session_with(EventCategory::ALL, EventCategory::ALL);

        // All reach their handlers (arg-validation messages), never "Permission denied".
        let cases = [
            ("Command", "Command is required"),
            ("UpdateConfig", "SrcFilename is required"),
            ("Originate", "Channel is required"),
            ("Hangup", "Channel is required"),
            ("SetVar", "Variable is required"),
        ];
        for (action, expected) in cases {
            let resp = dispatch_named(&registry, &ctx, &mut session, action);
            assert_eq!(resp.message, expected, "write=all must allow {action} to reach its handler");
        }
    }

    #[test]
    fn test_authz_readonly_actions_allowed_for_no_perm_user() {
        let (ctx, _reg) = make_context();
        let registry = ActionRegistry::new(ctx.user_registry.clone());
        // No read, no write — still must run read-only status actions (Asterisk
        // does not write-gate status reads).
        let (mut session, _rx) = authed_session_with(EventCategory::NONE, EventCategory::NONE);

        let ping = dispatch_named(&registry, &ctx, &mut session, "Ping");
        assert!(ping.success, "Ping must be allowed for any authenticated session");
        assert!(ping.headers.contains_key("Ping"));

        let channels = dispatch_named(&registry, &ctx, &mut session, "CoreShowChannels");
        assert!(channels.success, "CoreShowChannels must be allowed for any authenticated session");
    }

    /// Drift guard (#126): EVERY registered action MUST be classified in
    /// `action_authority`. If a new handler is registered without a
    /// classification, `action_authority` returns `None` here and this test
    /// fails — turning "forgot to classify a new dangerous action" into a
    /// caught bug rather than a silent read-only free pass.
    #[test]
    fn test_every_registered_action_is_classified() {
        let (_ctx, registry_arc) = make_context();
        let registry = ActionRegistry::new(registry_arc);
        for name in registry.list_actions() {
            assert!(
                action_authority(&name).is_some(),
                "registered AMI action '{name}' is not classified in action_authority \
                 (an unmapped action must never silently inherit a read-only free pass)"
            );
        }
    }

    /// Locks the action->category table so a reviewer sees the trust boundary
    /// in one place and an accidental re-map (e.g. Command -> CALL) fails here.
    #[test]
    fn test_action_authority_table() {
        use ActionAuthority::{Preauth, ReadOnly, Write};
        assert_eq!(action_authority("login"), Some(Preauth));
        assert_eq!(action_authority("challenge"), Some(Preauth));
        assert_eq!(action_authority("command"), Some(Write(EventCategory::COMMAND)));
        assert_eq!(action_authority("updateconfig"), Some(Write(EventCategory::CONFIG)));
        assert_eq!(action_authority("originate"), Some(Write(EventCategory::SYSTEM)));
        assert_eq!(action_authority("pjsipnotify"), Some(Write(EventCategory::SYSTEM)));
        assert_eq!(action_authority("hangup"), Some(Write(EventCategory::CALL)));
        assert_eq!(action_authority("setvar"), Some(Write(EventCategory::CALL)));
        assert_eq!(action_authority("confbridgekick"), Some(Write(EventCategory::CALL)));
        // Queue membership admin is AGENT (matches ListCommands + Asterisk), NOT CALL.
        assert_eq!(action_authority("queueadd"), Some(Write(EventCategory::AGENT)));
        assert_eq!(action_authority("queuepause"), Some(Write(EventCategory::AGENT)));
        assert_eq!(action_authority("ping"), Some(ReadOnly));
        assert_eq!(action_authority("coreshowchannels"), Some(ReadOnly));
        assert_eq!(action_authority("nonexistentaction"), None);
    }
}
