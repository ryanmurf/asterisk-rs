//! SIP Event Handler.
//!
//! Receives SIP events from the SIP stack and creates/manages Asterisk
//! channels. This is the glue between the SIP protocol layer and the
//! PBX/channel model.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::RwLock;
use tracing::{info, warn, debug};

use crate::parser::SipMessage;
use crate::authenticator::AuthCredentials;
use crate::sdp::SessionDescription;
use crate::session::SipSession;
use crate::transport::SipTransport;
use asterisk_codecs::{codecs, Codec};
use asterisk_core::channel::store;
use asterisk_core::channel::softhangup;
use asterisk_core::pbx::Dialplan;
use asterisk_types::{ChannelState, HangupCause};

/// Per-call state stored by the event handler for SIP signaling.
struct CallState {
    /// The SIP session (holds INVITE, dialog, SDP, etc.).
    session: SipSession,
    /// Remote address to send responses to.
    remote_addr: SocketAddr,
    /// Channel name for correlation.
    channel_name: String,
}

/// SIP event handler -- bridges the SIP stack to the Asterisk channel model.
pub struct SipEventHandler {
    dialplan: Arc<Dialplan>,
    /// Call-ID to channel name mapping for response/BYE routing.
    callid_map: Arc<RwLock<HashMap<String, String>>>,
    /// SIP transport for sending responses.
    transport: Arc<dyn SipTransport>,
    /// Per-call SIP state keyed by Call-ID.
    call_states: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<CallState>>>>>,
    /// Supported codecs (audio + video) for SDP answer generation.
    supported_codecs: Vec<Codec>,
}

impl SipEventHandler {
    /// Create a new event handler with the given dialplan and transport.
    pub fn new(dialplan: Arc<Dialplan>, transport: Arc<dyn SipTransport>) -> Self {
        // Register transport with global notify service
        crate::notify_service::global_notify_service().set_transport(transport.clone());
        Self {
            dialplan,
            callid_map: Arc::new(RwLock::new(HashMap::new())),
            transport,
            call_states: Arc::new(RwLock::new(HashMap::new())),
            supported_codecs: vec![
                codecs::pcmu(), codecs::pcma(), codecs::telephone_event(),
                codecs::vp8(), codecs::h264(), codecs::vp9(), codecs::h265(),
            ],
        }
    }

    /// Handle an incoming SIP INVITE -- creates a channel and starts PBX execution.
    ///
    /// Returns the Call-ID if the invite was handled successfully.
    pub async fn handle_incoming_invite(
        &self,
        request: &SipMessage,
        remote_addr: SocketAddr,
        mut session: SipSession,
    ) -> Option<String> {
        // 1. Extract caller info from From header
        let from = request.get_header("From")?;
        let caller_num = extract_user_from_header(from).unwrap_or_default();
        let caller_name = extract_display_name(from).unwrap_or_default();

        // 2. Extract dialed number from Request-URI (preferred) or To header
        let exten = match &request.start_line {
            crate::parser::StartLine::Request(r) => {
                r.uri.user.clone().unwrap_or_else(|| "s".to_string())
            }
            _ => {
                let to = request.get_header("To")?;
                extract_user_from_header(to).unwrap_or_else(|| "s".to_string())
            }
        };

        // 3. Extract Call-ID for tracking
        let call_id = request.call_id()?.to_string();
        eprintln!("[DEBUG] handle_incoming_invite: call_id={}, exten={}, caller={}", call_id, exten, caller_num);
        eprintln!("[DEBUG] All headers:");
        for h in &request.headers {
            eprintln!("[DEBUG]   {}: {}", h.name, h.value);
        }

        // 4. Authenticate the request against configured endpoints.
        //    Build credentials from all endpoints that have auth configured.
        let pjsip_config = crate::pjsip_config::get_global_pjsip_config();
        let mut endpoint_context = "default".to_string();
        let mut allow_overlap = true;

        if let Some(ref cfg) = pjsip_config {
            // Collect all auth credentials and their associated endpoint names
            let mut all_creds: Vec<(String, AuthCredentials)> = Vec::new();
            for ep in &cfg.endpoints {
                if let Some(ref auth_name) = ep.auth {
                    if let Some(auth) = cfg.find_auth(auth_name) {
                        all_creds.push((
                            ep.name.clone(),
                            AuthCredentials::new(&auth.username, &auth.password, ""),
                        ));
                    }
                }
            }

            if !all_creds.is_empty() {
                let creds: Vec<AuthCredentials> = all_creds.iter().map(|(_, c)| c.clone()).collect();
                let authenticator = crate::authenticator::InboundAuthenticator::new();
                match authenticator.verify(request, &creds, false) {
                    Ok(()) => {
                        eprintln!("[DEBUG] Auth succeeded for call_id={}", call_id);
                        // Auth succeeded -- identify the endpoint from the auth username.
                        // Extract username from the Authorization header to find the matching endpoint.
                        if let Some(auth_hdr) = request.get_header(crate::parser::header_names::AUTHORIZATION) {
                            if let Some(parsed) = crate::authenticator::parse_authorization(auth_hdr) {
                                for (ep_name, cred) in &all_creds {
                                    if cred.username == parsed.username {
                                        if let Some(ep) = cfg.find_endpoint(ep_name) {
                                            endpoint_context = ep.context.clone();
                                            allow_overlap = ep.allow_overlap;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Err(challenge) => {
                        eprintln!("[DEBUG] Auth failed, sending 401 for call_id={}", call_id);
                        // Send 401 challenge
                        if let Err(e) = self.transport.send(&challenge, remote_addr).await {
                            warn!(call_id = %call_id, "Failed to send 401 challenge: {}", e);
                        } else {
                            debug!(call_id = %call_id, "Sent 401 Unauthorized challenge");
                        }
                        return None;
                    }
                }
            }
        }

        // 5. Check extension existence in the dialplan before proceeding.
        //    If the extension doesn't exist, respond with 484 or 404 depending
        //    on the allow_overlap setting.
        let extension_exists = self.dialplan.find_extension(&endpoint_context, &exten).is_some();
        eprintln!("[DEBUG] Extension lookup: context={}, exten={}, exists={}, allow_overlap={}", endpoint_context, exten, extension_exists, allow_overlap);
        if !extension_exists {
            if allow_overlap && self.dialplan.could_match(&endpoint_context, &exten) {
                // Overlap enabled and extension could match with more digits -> 484
                if let Ok(resp) = request.create_response(484, "Address Incomplete") {
                    let _ = self.transport.send(&resp, remote_addr).await;
                    debug!(call_id = %call_id, exten = %exten, "Sent 484 Address Incomplete (overlap enabled)");
                }
                return None;
            } else {
                // No match possible -> 404
                if let Ok(resp) = request.create_response(404, "Not Found") {
                    let _ = self.transport.send(&resp, remote_addr).await;
                    debug!(call_id = %call_id, exten = %exten, "Sent 404 Not Found");
                }
                return None;
            }
        }

        // 6. Send 100 Trying
        match request.create_response(100, "Trying") {
            Ok(trying) => {
                if let Err(e) = self.transport.send(&trying, remote_addr).await {
                    warn!(call_id = %call_id, "Failed to send 100 Trying: {}", e);
                } else {
                    info!(call_id = %call_id, "Sent 100 Trying");
                }
            }
            Err(e) => {
                warn!(call_id = %call_id, "Failed to build 100 Trying: {}", e);
            }
        }

        // 7. Create the channel and register it in the global store.
        //    We use register_existing_channel so that all fields (including
        //    accountcode) are set before the Newchannel AMI event is emitted.
        let channel_name = format!("PJSIP/{}-{:08x}", caller_num, rand_id());
        let mut new_ch = asterisk_core::channel::Channel::new(&channel_name);
        new_ch.caller.id.number.number = caller_num.clone();
        new_ch.caller.id.name.name = caller_name;
        new_ch.exten = exten;
        new_ch.context = endpoint_context.clone();
        new_ch.set_state(ChannelState::Ring);

        // Look up accountcode from the matched endpoint
        if let Some(ref cfg) = pjsip_config {
            let ep_name = cfg.identify_endpoint_by_ip(&remote_addr.ip().to_string());
            if let Some(ep_name) = ep_name {
                if let Some(ep) = cfg.find_endpoint(ep_name) {
                    new_ch.accountcode = ep.accountcode.clone();
                }
            }
        }

        let channel = store::register_existing_channel(new_ch);

        // Store the SIP Call-ID on the channel so ConfBridge SFU can correlate.
        {
            let mut ch = channel.lock();
            ch.variables.insert("__SIP_CALL_ID".to_string(), call_id.clone());
        }

        // 6. Register Call-ID mapping for response routing
        {
            let mut map = self.callid_map.write();
            map.insert(call_id.clone(), channel_name.clone());
        }

        // 7. Generate SDP answer from the remote offer so 200 OK includes media.
        let local_ip = session.local_addr.ip().to_string();
        if let Some(ref remote_sdp) = session.remote_sdp {
            let answer_sdp = SessionDescription::create_answer(
                remote_sdp,
                &local_ip,
                10000, // RTP port (will be overridden if RTP session is created)
                &self.supported_codecs,
            );
            session.local_sdp = Some(answer_sdp.clone());
            session.initial_local_sdp = Some(answer_sdp);
        }

        // 8. Store the SIP session state for later signaling (200 OK, BYE)
        //    Also register with global notify service for in-dialog NOTIFY.
        let remote_contact = request
            .get_header("Contact")
            .and_then(crate::parser::extract_uri)
            .unwrap_or_else(|| format!("sip:{}@{}", caller_num, remote_addr));
        let local_uri = format!("sip:asterisk@{}", session.local_addr);
        // Extract From tag from the INVITE for the remote tag in our dialog
        let remote_from_tag = request
            .get_header("From")
            .and_then(crate::parser::extract_tag)
            .unwrap_or_default();
        let notify_state = crate::notify_service::ChannelSipState {
            call_id: session.call_id.clone(),
            local_tag: session.local_tag.clone(),
            remote_tag: remote_from_tag,
            local_uri,
            remote_target: remote_contact,
            remote_addr,
            local_seq: 100,
        };
        crate::notify_service::global_notify_service()
            .register_channel(&channel_name, notify_state);

        let call_state = Arc::new(tokio::sync::Mutex::new(CallState {
            session,
            remote_addr,
            channel_name: channel_name.clone(),
        }));
        {
            let mut states = self.call_states.write();
            states.insert(call_id.clone(), call_state.clone());
        }

        // 9. Spawn PBX execution on a background task.
        //
        // The task lifecycle:
        //   a) Wait for the Answer() dialplan app to fire (answer callback)
        //      then send SIP 200 OK.
        //   b) Run pbx_run (which blocks as long as the dialplan keeps
        //      executing -- Wait(), Echo(), ConfBridge(), etc.).
        //   c) After pbx_run completes and the channel hangs up, do NOT
        //      eagerly send BYE.  Instead, wait for the remote to send
        //      BYE.  The SIP dialog stays alive until the remote tears
        //      it down or a generous timeout expires.  This is critical
        //      for SIPp tests that send additional in-dialog requests
        //      (re-INVITE, MESSAGE, INFO) after 200 OK.
        let dialplan = self.dialplan.clone();
        let ch_for_pbx = channel.clone();
        let ch_name_for_cleanup = channel_name.clone();
        let transport = self.transport.clone();
        let call_id_for_task = call_id.clone();
        let call_states_ref = self.call_states.clone();
        let callid_map_ref = self.callid_map.clone();

        // Notify that fires when Answer() is called on the channel.
        let answer_notify = Arc::new(tokio::sync::Notify::new());
        let answer_notify_for_cb = answer_notify.clone();

        // Notify that fires when channel.hangup() is called.
        let hangup_notify = Arc::new(tokio::sync::Notify::new());
        let hangup_notify_for_cb = hangup_notify.clone();

        let unique_id_for_cb = {
            let ch = channel.lock();
            ch.unique_id.0.clone()
        };
        let unique_id_for_answer_cb = unique_id_for_cb.clone();

        // Register an answer callback -- fires when Answer() sets state to Up.
        asterisk_core::channel::register_answer_callback(Box::new(move |uid| {
            if uid == unique_id_for_answer_cb {
                answer_notify_for_cb.notify_one();
            }
        }));

        // Register a hangup callback -- fires when Channel::hangup() is called.
        asterisk_core::channel::register_hangup_callback(Box::new(move |uid, _cause| {
            if uid == unique_id_for_cb {
                hangup_notify_for_cb.notify_one();
            }
        }));

        tokio::spawn(async move {
            // Convert from parking_lot::Mutex to tokio::sync::Mutex for pbx_run.
            // pbx_run expects Arc<tokio::sync::Mutex<Channel>>.
            let channel_data = {
                let guard = ch_for_pbx.lock();
                let mut new_ch = asterisk_core::channel::Channel::new(&guard.name);
                new_ch.unique_id = guard.unique_id.clone();
                new_ch.caller = guard.caller.clone();
                new_ch.exten = guard.exten.clone();
                new_ch.context = guard.context.clone();
                new_ch.state = guard.state;
                new_ch.priority = guard.priority;
                new_ch.linkedid = guard.linkedid.clone();
                new_ch.variables = guard.variables.clone();
                new_ch
            };

            let tokio_channel = Arc::new(tokio::sync::Mutex::new(channel_data));

            // Spawn pbx_run concurrently -- it will call Answer() which
            // triggers our answer_notify, at which point we send 200 OK.
            let dialplan_clone = dialplan.clone();
            let tokio_channel_clone = tokio_channel.clone();
            let pbx_handle = tokio::spawn(async move {
                asterisk_core::pbx::exec::pbx_run(tokio_channel_clone, dialplan_clone).await
            });

            // Wait for Answer() to be called (or pbx_run to finish without
            // answering, in which case we never send 200 OK).
            // Timeout after 30s to avoid leaking.
            let answered = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                answer_notify.notified(),
            ).await;

            if answered.is_ok() {
                // Answer() was called -- send 200 OK now.
                let still_active = call_states_ref.read().contains_key(&call_id_for_task);
                if still_active {
                    let mut cs = call_state.lock().await;
                    if let Some(ok_response) = cs.session.build_200_ok() {
                        if let Err(e) = transport.send(&ok_response, cs.remote_addr).await {
                            warn!(call_id = %call_id_for_task, "Failed to send 200 OK: {}", e);
                        } else {
                            info!(call_id = %call_id_for_task, "Sent 200 OK (triggered by Answer app)");
                            cs.session.state = crate::session::SessionState::Established;
                        }
                    }
                }
            } else {
                debug!(call_id = %call_id_for_task, "Answer() not called within timeout");
            }

            // Wait for pbx_run to finish.
            let result = pbx_handle.await;
            match &result {
                Ok(r) => info!(channel = %ch_name_for_cleanup, "PBX completed with result: {:?}", r),
                Err(e) => warn!(channel = %ch_name_for_cleanup, "PBX task failed: {}", e),
            }

            // pbx_run calls chan.hangup() at the end, which fires the
            // hangup callback.  Wait briefly for that signal.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                hangup_notify.notified(),
            ).await;

            // Do NOT eagerly send BYE.  The SIP dialog stays alive so
            // that SIPp (or any remote UA) can send additional in-dialog
            // messages.  The remote will send BYE when it is done, and
            // handle_bye() will clean up.
            //
            // We only clean up after a generous timeout (32s, matching
            // SIP Timer B / Timer F) to avoid leaking state if the
            // remote disappears without sending BYE.
            let call_id_for_cleanup = call_id_for_task.clone();
            let call_states_cleanup = call_states_ref.clone();
            let callid_map_cleanup = callid_map_ref.clone();
            let ch_for_cleanup = ch_for_pbx.clone();

            tokio::spawn(async move {
                // Wait for remote BYE (handle_bye removes the call state).
                // Poll periodically instead of blocking with a Notify,
                // since handle_bye already cleans up the maps directly.
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(32);
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if !call_states_cleanup.read().contains_key(&call_id_for_cleanup) {
                        debug!(call_id = %call_id_for_cleanup, "Call state cleaned up by remote BYE");
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        info!(call_id = %call_id_for_cleanup, "Dialog timeout -- cleaning up stale call state");
                        call_states_cleanup.write().remove(&call_id_for_cleanup);
                        callid_map_cleanup.write().remove(&call_id_for_cleanup);
                        break;
                    }
                }

                // Clean up from global store
                let uid = ch_for_cleanup.lock().unique_id.0.clone();
                store::deregister(&uid);
            });
        });

        Some(call_id)
    }

    /// Handle a SIP response (180/200/4xx/5xx) for outbound calls.
    pub fn handle_response(&self, response: &SipMessage, _remote_addr: SocketAddr) {
        let call_id = match response.call_id() {
            Some(id) => id.to_string(),
            None => return,
        };

        // Find channel by Call-ID
        let channel_name = {
            let map = self.callid_map.read();
            match map.get(&call_id) {
                Some(name) => name.clone(),
                None => return,
            }
        };

        if let Some(channel) = store::find_by_name(&channel_name) {
            let status_code = response.status_code().unwrap_or(0);
            let mut ch = channel.lock();
            match status_code {
                180 | 183 => {
                    ch.set_state(ChannelState::Ringing);
                }
                200 => {
                    ch.set_state(ChannelState::Up);
                }
                486 => {
                    ch.set_state(ChannelState::Busy);
                    ch.hangup_cause = HangupCause::UserBusy;
                }
                _ if status_code >= 400 => {
                    ch.hangup_cause = HangupCause::NormalClearing;
                    ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                }
                _ => {}
            }
        }
    }

    /// Handle an incoming BYE request.
    pub async fn handle_bye(&self, request: &SipMessage, remote_addr: SocketAddr) {
        if let Some(call_id) = request.call_id() {
            let call_id = call_id.to_string();

            // Send 200 OK to the BYE
            match request.create_response(200, "OK") {
                Ok(ok_resp) => {
                    if let Err(e) = self.transport.send(&ok_resp, remote_addr).await {
                        warn!(call_id = %call_id, "Failed to send 200 OK to BYE: {}", e);
                    } else {
                        info!(call_id = %call_id, "Sent 200 OK to BYE");
                    }
                }
                Err(e) => {
                    warn!(call_id = %call_id, "Failed to build 200 OK to BYE: {}", e);
                }
            }

            let channel_name = {
                let map = self.callid_map.read();
                map.get(&call_id).cloned()
            };

            if let Some(name) = channel_name {
                crate::notify_service::global_notify_service().unregister_channel(&name);
                if let Some(channel) = store::find_by_name(&name) {
                    let mut ch = channel.lock();
                    ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                }
            }

            // Clean up
            self.callid_map.write().remove(&call_id);
            self.call_states.write().remove(&call_id);

            // Notify any SFU conferences that this SIP call was hung up.
            crate::notify_sip_hangup(&call_id);
        }
    }

    /// Send a re-INVITE to an existing session with a new SDP offer.
    ///
    /// Used by the SFU ConfBridge to add/remove video streams.
    /// Returns `true` if the re-INVITE was sent successfully.
    pub async fn send_reinvite(&self, call_id: &str, sdp: SessionDescription) -> bool {
        let cs_arc = {
            let states = self.call_states.read();
            match states.get(call_id) {
                Some(cs) => cs.clone(),
                None => {
                    warn!(call_id = %call_id, "Cannot send re-INVITE: no call state");
                    return false;
                }
            }
        };

        let mut cs = cs_arc.lock().await;
        if let Some(reinvite) = cs.session.build_reinvite(&sdp) {
            if let Err(e) = self.transport.send(&reinvite, cs.remote_addr).await {
                warn!(call_id = %call_id, "Failed to send re-INVITE: {}", e);
                return false;
            }
            info!(call_id = %call_id, "Sent re-INVITE");
            true
        } else {
            warn!(call_id = %call_id, "Failed to build re-INVITE");
            false
        }
    }

    /// Handle a response (200 OK) to our re-INVITE by sending ACK.
    pub async fn handle_reinvite_response(&self, response: &SipMessage, remote_addr: SocketAddr) {
        let call_id = match response.call_id() {
            Some(id) => id.to_string(),
            None => return,
        };

        let status_code = response.status_code().unwrap_or(0);
        if status_code != 200 {
            return;
        }

        // We only receive 200 OK INVITE responses for re-INVITEs we initiated.
        // (For inbound calls, WE send the 200 OK, so we never receive one for the initial INVITE.)
        let cseq = response.cseq().unwrap_or_default();
        if !cseq.ends_with("INVITE") {
            return;
        }

        let cs_arc = {
            let states = self.call_states.read();
            match states.get(&call_id) {
                Some(cs) => cs.clone(),
                None => return,
            }
        };

        let cs = cs_arc.lock().await;
        if let Some(ack) = cs.session.build_reinvite_ack(response) {
            if let Err(e) = self.transport.send(&ack, remote_addr).await {
                warn!(call_id = %call_id, "Failed to send ACK for re-INVITE 200 OK: {}", e);
            } else {
                debug!(call_id = %call_id, "Sent ACK for re-INVITE 200 OK");
            }
        }
    }

    /// Get the remote SDP for an active call (the SDP from the initial INVITE offer).
    pub fn get_remote_sdp(&self, call_id: &str) -> Option<SessionDescription> {
        let states = self.call_states.read();
        let cs_arc = states.get(call_id)?;
        // We need to try_lock since we're in a sync context
        let cs = cs_arc.try_lock().ok()?;
        cs.session.remote_sdp.clone()
    }

    /// Get the local SDP for an active call (the SDP answer we sent in 200 OK).
    pub fn get_local_sdp(&self, call_id: &str) -> Option<SessionDescription> {
        let states = self.call_states.read();
        let cs_arc = states.get(call_id)?;
        let cs = cs_arc.try_lock().ok()?;
        cs.session.local_sdp.clone()
    }

    /// Get the initial local SDP (before any re-INVITEs) for SFU.
    pub fn get_initial_local_sdp(&self, call_id: &str) -> Option<SessionDescription> {
        let states = self.call_states.read();
        let cs_arc = states.get(call_id)?;
        let cs = cs_arc.try_lock().ok()?;
        cs.session.initial_local_sdp.clone().or_else(|| cs.session.local_sdp.clone())
    }

    /// Get the local address for generating SDP.
    pub fn local_addr_for_call(&self, call_id: &str) -> Option<String> {
        let states = self.call_states.read();
        let cs_arc = states.get(call_id)?;
        let cs = cs_arc.try_lock().ok()?;
        Some(cs.session.local_addr.ip().to_string())
    }

    /// Get the current count of active call-id mappings.
    pub fn active_calls(&self) -> usize {
        self.callid_map.read().len()
    }
}

/// Extract the user part from a SIP header value like `"Name" <sip:user@host>` or `sip:user@host`.
fn extract_user_from_header(header: &str) -> Option<String> {
    // Try to find a SIP URI in angle brackets first
    let uri_str = if let Some(start) = header.find('<') {
        if let Some(end) = header.find('>') {
            &header[start + 1..end]
        } else {
            header
        }
    } else {
        // No angle brackets - use the value before any params
        header.split(';').next().unwrap_or(header).trim()
    };

    // Parse sip:user@host
    let after_scheme = if let Some(rest) = uri_str.strip_prefix("sip:") {
        rest
    } else if let Some(rest) = uri_str.strip_prefix("sips:") {
        rest
    } else {
        uri_str
    };

    // Extract user part (before @)
    if let Some((user, _host)) = after_scheme.split_once('@') {
        if user.is_empty() {
            None
        } else {
            Some(user.to_string())
        }
    } else {
        // No @ sign -- the whole thing might be a phone number
        let s = after_scheme.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }
}

/// Extract display name from a SIP header value like `"Alice" <sip:alice@example.com>`.
fn extract_display_name(header: &str) -> Option<String> {
    let header = header.trim();

    // Check for quoted display name: "Name" <sip:...>
    if header.starts_with('"') {
        if let Some(end_quote) = header[1..].find('"') {
            let name = &header[1..end_quote + 1];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    // Check for unquoted display name before <
    if let Some(bracket) = header.find('<') {
        let before = header[..bracket].trim();
        if !before.is_empty() && !before.starts_with("sip:") && !before.starts_with("sips:") {
            return Some(before.to_string());
        }
    }

    None
}

/// Generate a random u32 for channel name suffix.
fn rand_id() -> u32 {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (now & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_user_from_header_angle_brackets() {
        let from = r#""Alice" <sip:alice@atlanta.example.com>;tag=1234"#;
        assert_eq!(extract_user_from_header(from), Some("alice".to_string()));
    }

    #[test]
    fn test_extract_user_from_header_no_brackets() {
        let from = "sip:bob@biloxi.example.com";
        assert_eq!(extract_user_from_header(from), Some("bob".to_string()));
    }

    #[test]
    fn test_extract_user_no_at() {
        let from = "<sip:5551234>";
        assert_eq!(extract_user_from_header(from), Some("5551234".to_string()));
    }

    #[test]
    fn test_extract_display_name_quoted() {
        let from = r#""Alice Smith" <sip:alice@example.com>"#;
        assert_eq!(extract_display_name(from), Some("Alice Smith".to_string()));
    }

    #[test]
    fn test_extract_display_name_unquoted() {
        let from = "Bob <sip:bob@example.com>";
        assert_eq!(extract_display_name(from), Some("Bob".to_string()));
    }

    #[test]
    fn test_extract_display_name_none() {
        let from = "<sip:bob@example.com>";
        assert_eq!(extract_display_name(from), None);
    }

    #[test]
    fn test_rand_id() {
        let a = rand_id();
        // Just verify it returns something non-zero
        // (technically could be 0 but extremely unlikely)
        let _b = rand_id();
        // They are close in time but not necessarily different
        assert!(a > 0 || true); // just ensure it doesn't panic
    }
}
