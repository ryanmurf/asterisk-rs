//! SIP Event Handler.
//!
//! Receives SIP events from the SIP stack and creates/manages Asterisk
//! channels. This is the glue between the SIP protocol layer and the
//! PBX/channel model.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use tracing::{info, warn, debug};

use crate::channel_driver::SipChannelDriver;
use crate::parser::SipMessage;
use crate::authenticator::AuthCredentials;
use crate::registrar::Registrar;
use crate::rtp::RtpSession;
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
    /// SIP channel driver used to attach the inbound media plane (RTP) to
    /// answered channels, mirroring the outbound `request()` wiring. Set once
    /// at startup via [`Self::set_channel_driver`]; when absent, inbound calls
    /// still signal but carry no media.
    channel_driver: OnceLock<Arc<SipChannelDriver>>,
    /// Inbound REGISTER handler (contact bindings per AoR).
    registrar: Arc<Registrar>,
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
            channel_driver: OnceLock::new(),
            registrar: Arc::new(Registrar::new()),
        }
    }

    /// Attach the SIP channel driver used to wire the inbound media plane.
    ///
    /// Called once at startup with the same `SipChannelDriver` that is
    /// registered in the tech registry, so inbound RTP sessions land in the
    /// driver's channel map (keyed by channel name) and are reachable by
    /// `Echo()` and other apps via `read_frame`/`write_frame`.
    pub fn set_channel_driver(&self, driver: Arc<SipChannelDriver>) {
        let _ = self.channel_driver.set(driver);
    }

    /// The inbound registrar, exposed for status/introspection and tests.
    pub fn registrar(&self) -> Arc<Registrar> {
        self.registrar.clone()
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

        // 3b. Check if this is a re-INVITE (in-dialog INVITE for hold/unhold/media update).
        //     A re-INVITE has a To tag (established dialog), while a new INVITE does not.
        //     We must check the To tag to avoid misidentifying a new inbound INVITE as a
        //     re-INVITE when Asterisk calls itself (same Call-ID for outbound and inbound).
        {
            let has_to_tag = request.get_header("To")
                .and_then(crate::parser::extract_tag)
                .is_some();
            let existing = self.call_states.read().contains_key(&call_id);
            if existing && has_to_tag {
                eprintln!("[DEBUG] re-INVITE detected for call_id={}", call_id);
                return self.handle_reinvite_request(request, remote_addr, session).await;
            }
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

        // 5a. Delayed-offer INVITE (no SDP body). RFC 3261 §13.2.1 requires
        //     the UAS to place its OWN offer in the 2xx and take the peer's
        //     answer from the ACK. We do not yet implement offer-in-2xx /
        //     answer-in-ACK (it needs the ACK body plumbed from the stack to
        //     this handler), and the previous behaviour was worse than a
        //     rejection: build_200_ok emitted a 200 OK with no SDP and the
        //     ACK's SDP was silently discarded, so the gateway either dropped
        //     the call or brought it up with no media. Until the negotiated
        //     path exists, reject cleanly with 488 Not Acceptable Here so the
        //     peer gets an unambiguous, spec-legal final response instead of a
        //     broken session (RFC 3261 §21.4.26). Re-INVITEs are handled
        //     earlier (step 3b), so this only affects initial INVITEs.
        if session.remote_sdp.is_none() {
            if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                let _ = self.transport.send(&resp, remote_addr).await;
                info!(
                    call_id = %call_id,
                    "Sent 488 Not Acceptable Here (delayed-offer INVITE with no SDP not supported)"
                );
            }
            return None;
        }

        // 5b. If the INVITE carries an SDP offer we cannot answer (no codec in
        //     common on any stream), reject with 488 Not Acceptable Here rather
        //     than sending a 200 OK whose media is entirely rejected — the
        //     latter brings the call "up" with guaranteed silence and a leaked
        //     RTP socket (RFC 3264 §6 / RFC 3261 §21.4.26). We probe by building
        //     a trial answer with a dummy non-zero port so accepted streams
        //     (port != 0) are distinguishable from rejected ones (port 0).
        if let Some(ref offer) = session.remote_sdp {
            let trial = SessionDescription::create_answer(
                offer,
                &session.local_addr.ip().to_string(),
                1,
                &self.supported_codecs,
            );
            let any_accepted = trial
                .media_descriptions
                .iter()
                .any(|m| m.port != 0);
            if !any_accepted {
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    let _ = self.transport.send(&resp, remote_addr).await;
                    debug!(call_id = %call_id, "Sent 488 Not Acceptable Here (no common codec)");
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
        //    In real Asterisk, the inbound PJSIP channel is named after the
        //    matched endpoint (e.g. PJSIP/alice-00000001), not the caller.
        let matched_endpoint_name = pjsip_config.as_ref()
            .and_then(|cfg| cfg.identify_endpoint_by_ip(&remote_addr.ip().to_string()))
            .map(|s| s.to_string());
        let chan_label = matched_endpoint_name.as_deref().unwrap_or(&caller_num);
        let channel_name = format!(
            "PJSIP/{}-{:08}",
            chan_label,
            crate::channel_driver::next_channel_suffix()
        );
        let mut new_ch = asterisk_core::channel::Channel::new(&channel_name);
        new_ch.caller.id.number.number = caller_num.clone();
        new_ch.caller.id.name.name = caller_name;
        new_ch.exten = exten;
        new_ch.context = endpoint_context.clone();
        new_ch.set_state(ChannelState::Ring);

        // Look up accountcode from the matched endpoint
        if let Some(ref cfg) = pjsip_config {
            if let Some(ref ep_name) = matched_endpoint_name {
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

        // 7. Bind the inbound media plane (RTP) and generate the SDP answer.
        //
        //    Mirrors the outbound `SipChannelDriver::request()` wiring: bind a
        //    local RTP socket, point it at the caller's advertised RTP endpoint,
        //    select the negotiated payload type, then attach it to the channel
        //    via the driver so `read_frame`/`write_frame` (and thus `Echo`) can
        //    move media. The SDP answer advertises the socket's REAL port —
        //    never the old hardcoded 10000 (issues #7, #8, #9).
        let local_ip = session.local_addr.ip().to_string();
        if let Some(remote_sdp) = session.remote_sdp.clone() {
            let mut answer_port: u16 = 0;

            let rtp_bind = SocketAddr::new(session.local_addr.ip(), 0);
            match RtpSession::bind(rtp_bind).await {
                Ok(mut rtp) => {
                    if let Some(remote_rtp) = remote_rtp_endpoint(&remote_sdp) {
                        rtp.set_remote_addr(remote_rtp);
                    }
                    if let Some(pt) = negotiated_payload_type(&remote_sdp, &self.supported_codecs) {
                        rtp.payload_type = pt;
                    }
                    answer_port = rtp.local_addr().map(|a| a.port()).unwrap_or(0);

                    if let Some(driver) = self.channel_driver.get() {
                        // Store the REAL inbound session (carries the INVITE,
                        // is_outbound = false) so driver.indicate()/hangup()
                        // work on this channel instead of silently no-opping on
                        // a fabricated outbound placeholder (issue #36). Built
                        // fresh from the same INVITE; the event handler keeps
                        // its own `session` for the 200 OK / BYE path.
                        let driver_session =
                            SipSession::new_inbound(request, session.local_addr, remote_addr)
                                .unwrap_or_else(|| {
                                    let mut s = SipSession::new_outbound(
                                        session.local_addr,
                                        remote_addr,
                                    );
                                    s.is_outbound = false;
                                    s.invite = Some(request.clone());
                                    s
                                });
                        driver.attach_inbound_media(
                            &channel_name,
                            driver_session,
                            self.transport.clone(),
                            rtp,
                        );
                        debug!(
                            call_id = %call_id,
                            channel = %channel_name,
                            port = answer_port,
                            "Bound inbound RTP and attached media plane"
                        );
                    } else {
                        warn!(
                            call_id = %call_id,
                            "No channel driver set -- inbound call will carry no media"
                        );
                    }
                }
                Err(e) => {
                    warn!(call_id = %call_id, "Failed to bind inbound RTP socket: {}", e);
                }
            }

            // Advertise the socket's real port. Only if the bind failed do we
            // fall back to a placeholder so the SDP still parses.
            let sdp_port = if answer_port != 0 { answer_port } else { 10000 };
            let answer_sdp = SessionDescription::create_answer(
                &remote_sdp,
                &local_ip,
                sdp_port,
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
        // Tear down the inbound media plane (drop the RTP socket) when the call
        // ends, so bound sockets are not leaked in the driver's channel map.
        let driver_for_cleanup = self.channel_driver.get().cloned();
        let channel_name_for_media = channel_name.clone();

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

            // Send BYE to the remote endpoint to tear down the SIP dialog.
            {
                let cs_arc_opt = {
                    let states = call_states_ref.read();
                    states.get(&call_id_for_task).cloned()
                };
                if let Some(cs_arc) = cs_arc_opt {
                    let mut cs = cs_arc.lock().await;
                    if let Some(bye) = cs.session.build_bye() {
                        if let Err(e) = transport.send(&bye, cs.remote_addr).await {
                            warn!(call_id = %call_id_for_task, "Failed to send BYE: {}", e);
                        } else {
                            eprintln!("[DEBUG] Sent BYE for call_id={}", call_id_for_task);
                        }
                    }
                }
            }

            // Wait for remote 200 OK to our BYE, or clean up after timeout.
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

                // Finalize teardown: deregister from the store, unregister from
                // the NOTIFY service, and drop the media plane. Sharing one
                // helper with handle_bye's path guarantees the NOTIFY
                // registration is released on the dialog-timeout path too — it
                // was previously leaked there (only the remote-BYE path in
                // handle_bye called unregister_channel).
                let uid = ch_for_cleanup.lock().unique_id.0.clone();
                finalize_inbound_teardown(&channel_name_for_media, &uid, driver_for_cleanup.as_ref());
            });
        });

        Some(call_id)
    }

    /// Register a Call-ID → channel name mapping for outbound calls.
    /// This allows SIP responses to be routed back to the correct channel.
    pub fn register_outbound_callid(&self, call_id: &str, channel_name: &str) {
        self.callid_map.write().insert(call_id.to_string(), channel_name.to_string());
        tracing::debug!(call_id, channel_name, "registered outbound Call-ID mapping");
    }

    /// Register an outbound call's session so we can send ACK/BYE later.
    pub fn register_outbound_session(&self, call_id: &str, channel_name: &str, session: SipSession, remote_addr: SocketAddr) {
        let call_state = Arc::new(tokio::sync::Mutex::new(CallState {
            session,
            remote_addr,
            channel_name: channel_name.to_string(),
        }));
        self.call_states.write().insert(call_id.to_string(), call_state);
    }

    /// Handle a SIP response (180/200/4xx/5xx) for outbound calls.
    pub async fn handle_response(&self, response: &SipMessage, remote_addr: SocketAddr) {
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
            let cseq_method = response.get_header("CSeq")
                .and_then(|c| c.split_whitespace().last())
                .map(|m| m.to_uppercase())
                .unwrap_or_default();
            // Handle channel state update only for INVITE responses
            if cseq_method == "INVITE" {
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
            // Update NOTIFY service with remote tag from response
            if let Some(to_tag) = response.get_header("To")
                .and_then(crate::parser::extract_tag) {
                let notify_svc = crate::notify_service::global_notify_service();
                notify_svc.update_remote_tag(&channel_name, &to_tag);
            }
            // Send ACK for 200 OK on outbound INVITE only (not NOTIFY 200 OK)
            let cseq_is_invite = response.get_header("CSeq")
                .map(|c| c.to_uppercase().contains("INVITE"))
                .unwrap_or(false);
            if status_code == 200 && cseq_is_invite {
                let cs_arc = {
                    let states = self.call_states.read();
                    states.get(&call_id).cloned()
                };
                if let Some(cs_arc) = cs_arc {
                    let mut cs = cs_arc.lock().await;
                    if cs.session.is_outbound {
                        cs.session.on_response(response);
                        if let Some(ack) = cs.session.build_ack() {
                            if let Err(e) = self.transport.send(&ack, remote_addr).await {
                                warn!(call_id = %call_id, "Failed to send ACK: {}", e);
                            } else {
                                eprintln!("[DEBUG] Sent ACK for 200 OK call_id={}", call_id);
                            }
                        } else {
                            eprintln!("[DEBUG] Failed to build ACK for call_id={}", call_id);
                        }
                    }
                }
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
                // Release the driver media plane so a remote-initiated BYE on
                // an OUTBOUND leg does not leak its RTP socket / channel-map
                // entry (issue #28). Idempotent, so the inbound path — whose
                // media plane is finalized by the spawned cleanup task — is
                // unaffected (the later finalize call is a no-op).
                if let Some(driver) = self.channel_driver.get() {
                    driver.remove_channel(&name);
                }
            }

            // Clean up
            self.callid_map.write().remove(&call_id);
            self.call_states.write().remove(&call_id);

            // Notify any SFU conferences that this SIP call was hung up.
            crate::notify_sip_hangup(&call_id);
        }
    }

    /// Handle an incoming SIP REGISTER request.
    ///
    /// Routes the request to the registrar (contact binding + expiry) and sends
    /// the resulting response. When endpoints have auth configured, the client
    /// must present a valid digest first — otherwise we return a 401 challenge,
    /// consistent with the INVITE auth flow (issue #11). Without this routing,
    /// inbound REGISTER received no response at all.
    pub async fn handle_register(&self, request: &SipMessage, remote_addr: SocketAddr) {
        let call_id = request.call_id().unwrap_or("").to_string();

        // Enforce digest auth if any endpoint has credentials configured.
        if let Some(cfg) = crate::pjsip_config::get_global_pjsip_config() {
            let mut creds: Vec<AuthCredentials> = Vec::new();
            for ep in &cfg.endpoints {
                if let Some(ref auth_name) = ep.auth {
                    if let Some(auth) = cfg.find_auth(auth_name) {
                        creds.push(AuthCredentials::new(&auth.username, &auth.password, ""));
                    }
                }
            }

            if !creds.is_empty() {
                let authenticator = crate::authenticator::InboundAuthenticator::new();
                if let Err(challenge) = authenticator.verify(request, &creds, false) {
                    if let Err(e) = self.transport.send(&challenge, remote_addr).await {
                        warn!(call_id = %call_id, "Failed to send REGISTER 401 challenge: {}", e);
                    } else {
                        debug!(call_id = %call_id, "Sent 401 challenge for REGISTER");
                    }
                    return;
                }

                // Auth succeeded. Scope the binding: the authenticated user
                // may only (de)register an AoR owned by an endpoint it
                // authenticates for. Without this, ANY configured user's
                // valid credentials could bind ANY AoR — e.g. alice REGISTERs
                // `To: <sip:bob@…>` and hijacks bob's inbound calls the moment
                // routing consults the registrar. Mirrors res_pjsip_registrar's
                // `find_aor_name` against the identified endpoint's configured
                // `aors` (RFC 3261 §10.3 — a registrar authorizes the binding,
                // it does not merely authenticate the transaction).
                let authed_user = request
                    .get_header(crate::parser::header_names::AUTHORIZATION)
                    .and_then(crate::authenticator::parse_authorization)
                    .map(|p| p.username);
                let requested_aor = crate::registrar::aor_name_from_request(request);

                let allowed = match (authed_user.as_deref(), requested_aor.as_deref()) {
                    (Some(user), Some(aor)) => user_may_register_aor(&cfg, user, aor),
                    // Missing identity or target AoR: refuse rather than bind
                    // blindly.
                    _ => false,
                };
                if !allowed {
                    warn!(
                        call_id = %call_id,
                        user = authed_user.as_deref().unwrap_or("<unknown>"),
                        aor = requested_aor.as_deref().unwrap_or("<unknown>"),
                        "REGISTER denied: authenticated user may not bind this AoR"
                    );
                    if let Ok(resp) = request.create_response(403, "Forbidden") {
                        let _ = self.transport.send(&resp, remote_addr).await;
                    }
                    return;
                }
            }
        }

        // Authenticated (or no auth required): perform the registration.
        let response = self.registrar.handle_register(request);
        let status = response.status_code().unwrap_or(0);
        if let Err(e) = self.transport.send(&response, remote_addr).await {
            warn!(call_id = %call_id, "Failed to send REGISTER response: {}", e);
        } else {
            info!(call_id = %call_id, status, "Handled REGISTER");
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
        // Skip outbound calls — handle_response already sends their ACK
        if cs.session.is_outbound {
            return;
        }
        if let Some(ack) = cs.session.build_reinvite_ack(response) {
            if let Err(e) = self.transport.send(&ack, remote_addr).await {
                warn!(call_id = %call_id, "Failed to send ACK for re-INVITE 200 OK: {}", e);
            } else {
                debug!(call_id = %call_id, "Sent ACK for re-INVITE 200 OK");
            }
        }
    }

    /// Handle an incoming re-INVITE (in-dialog INVITE for hold/unhold/media update).
    async fn handle_reinvite_request(
        &self,
        request: &SipMessage,
        remote_addr: SocketAddr,
        session: SipSession,
    ) -> Option<String> {
        let call_id = request.call_id()?.to_string();

        // Get existing call state to update session SDP
        let cs_arc = {
            let states = self.call_states.read();
            states.get(&call_id)?.clone()
        };

        // Parse the re-INVITE's SDP offer
        let remote_sdp = session.remote_sdp.clone();

        // Check if this is a hold (a=sendonly or a=inactive in the SDP)
        let is_hold = if let Some(ref sdp) = remote_sdp {
            let sdp_str = sdp.to_string();
            sdp_str.contains("a=sendonly") || sdp_str.contains("a=inactive")
        } else {
            false
        };

        // Advertise the media plane's REAL bound RTP port in the answer, not a
        // placeholder. Binding a re-INVITE answer to a bogus port (the old
        // hardcoded 10000) breaks audio after every hold/unhold/renegotiation
        // for peers that honor the answer SDP -- the same defect #8 fixed for
        // the initial INVITE, which had been left unfixed on this path.
        let channel_name = { cs_arc.lock().await.channel_name.clone() };
        let media_port = match self.channel_driver.get() {
            Some(driver) => driver
                .channel_rtp_local_port(&channel_name)
                .await
                .unwrap_or(10000),
            None => 10000,
        };

        // Generate SDP answer
        let local_ip = session.local_addr.ip().to_string();
        let answer_sdp = if let Some(ref offer) = remote_sdp {
            let answer = SessionDescription::create_answer(
                offer,
                &local_ip,
                media_port,
                &self.supported_codecs,
            );
            Some(answer)
        } else {
            // No SDP in re-INVITE; use the existing local SDP
            let cs = cs_arc.lock().await;
            cs.session.local_sdp.clone()
        };

        // Build 200 OK response
        let mut ok_resp = request.create_response(200, "OK").ok()?;

        // Add Contact header
        ok_resp.add_header("Contact", &format!("<sip:asterisk@{}>", session.local_addr));

        // Add SDP body
        if let Some(ref sdp) = answer_sdp {
            let sdp_str = sdp.to_string();
            ok_resp.add_header("Content-Type", "application/sdp");
            ok_resp.add_header("Content-Length", &sdp_str.len().to_string());
            ok_resp.body = sdp_str;
        }

        // Send 200 OK
        if let Err(e) = self.transport.send(&ok_resp, remote_addr).await {
            warn!(call_id = %call_id, "Failed to send 200 OK for re-INVITE: {}", e);
            return None;
        }
        eprintln!("[DEBUG] Sent 200 OK for re-INVITE call_id={}", call_id);

        // Emit Hold/Unhold AMI event
        let _channel_name = {
            let cs = cs_arc.lock().await;
            cs.channel_name.clone()
        };
        if is_hold {
            eprintln!("[DEBUG] Hold detected on channel {}", _channel_name);
            // Find the bridged peer channel and emit DeviceStateChange for its endpoint
            if let Some(store_chan) = asterisk_core::channel_store::find_by_name(&_channel_name) {
                let ch = store_chan.lock();
                if let Some(peer_name) = ch.variables.get("BRIDGEPEER") {
                    // Extract device name from peer channel name (PJSIP/bob-00000001 → PJSIP/bob)
                    let device = peer_name.rsplit_once('-')
                        .map(|(prefix, _)| prefix.to_string())
                        .unwrap_or_else(|| peer_name.clone());
                    eprintln!("[DEBUG] Emitting DeviceStateChange for {} = ONHOLD", device);
                    asterisk_core::channel::publish_channel_event("DeviceStateChange", &[
                        ("Device", &device),
                        ("State", "ONHOLD"),
                    ]);
                }
            }
        }

        Some(call_id)
    }

    /// Send BYE for a channel by looking up its Call-ID in the callid_map,
    /// then release every local resource the leg held.
    pub async fn send_bye_for_channel(&self, channel_name: &str) {
        // Find Call-ID for this channel and send a BYE if we have session
        // state for it. A leg with no Call-ID (never reached `call()`) still
        // has a bound RTP socket in the driver map, so we fall through to the
        // release step below regardless.
        let call_id = {
            let map = self.callid_map.read();
            map.iter().find(|(_, name)| name.as_str() == channel_name)
                .map(|(cid, _)| cid.clone())
        };
        if let Some(ref call_id) = call_id {
            let cs_arc = {
                let states = self.call_states.read();
                states.get(call_id).cloned()
            };
            if let Some(cs_arc) = cs_arc {
                let mut cs = cs_arc.lock().await;
                if let Some(bye) = cs.session.build_bye() {
                    if let Err(e) = self.transport.send(&bye, cs.remote_addr).await {
                        warn!(call_id = %call_id, "Failed to send BYE for {}: {}", channel_name, e);
                    } else {
                        eprintln!("[DEBUG] Sent BYE for channel {} call_id={}", channel_name, call_id);
                    }
                }
            }
        }
        // Release the driver media plane (RTP socket), NOTIFY registration,
        // and Call-ID/state bookkeeping. Before this, only the two maps were
        // cleared and the driver channel entry + its bound socket leaked for
        // the lifetime of the process (issue #28).
        self.release_outbound_leg(channel_name);
    }

    /// Release an outbound leg's local resources — its driver channel-map
    /// entry (and thus the bound RTP socket), NOTIFY registration, and
    /// Call-ID/state bookkeeping — WITHOUT sending any SIP.
    ///
    /// Used to reclaim abandoned Dial legs (losing legs in a parallel dial,
    /// and every leg of a failed dial) so their sockets are not leaked
    /// (issue #28). Sending the appropriate CANCEL/BYE for such legs is the
    /// caller's concern; this only frees resources. Idempotent.
    pub fn release_outbound_leg(&self, channel_name: &str) {
        let call_id = {
            let map = self.callid_map.read();
            map.iter()
                .find(|(_, name)| name.as_str() == channel_name)
                .map(|(cid, _)| cid.clone())
        };
        if let Some(call_id) = call_id {
            self.callid_map.write().remove(&call_id);
            self.call_states.write().remove(&call_id);
        }
        crate::notify_service::global_notify_service().unregister_channel(channel_name);
        if let Some(driver) = self.channel_driver.get() {
            driver.remove_channel(channel_name);
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

    /// Get the remote SDP asynchronously (waits for the lock).
    pub async fn get_remote_sdp_async(&self, call_id: &str) -> Option<SessionDescription> {
        let cs_arc = {
            let states = self.call_states.read();
            states.get(call_id)?.clone()
        };
        let cs = cs_arc.lock().await;
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

    /// Get the initial local SDP asynchronously (waits for the lock).
    pub async fn get_initial_local_sdp_async(&self, call_id: &str) -> Option<SessionDescription> {
        let cs_arc = {
            let states = self.call_states.read();
            states.get(call_id)?.clone()
        };
        let cs = cs_arc.lock().await;
        cs.session.initial_local_sdp.clone().or_else(|| cs.session.local_sdp.clone())
    }

    /// Get the local address for generating SDP.
    pub fn local_addr_for_call(&self, call_id: &str) -> Option<String> {
        let states = self.call_states.read();
        let cs_arc = states.get(call_id)?;
        let cs = cs_arc.try_lock().ok()?;
        Some(cs.session.local_addr.ip().to_string())
    }

    /// Get the local address asynchronously (waits for the lock).
    pub async fn local_addr_for_call_async(&self, call_id: &str) -> Option<String> {
        let cs_arc = {
            let states = self.call_states.read();
            states.get(call_id)?.clone()
        };
        let cs = cs_arc.lock().await;
        Some(cs.session.local_addr.ip().to_string())
    }

    /// Get the current count of active call-id mappings.
    pub fn active_calls(&self) -> usize {
        self.callid_map.read().len()
    }
}

/// Finalize teardown of an inbound call's server-side resources:
///   1. deregister the channel from the global store,
///   2. unregister it from the NOTIFY service, and
///   3. drop its media plane (RTP socket) in the driver.
///
/// Idempotent — each step is a no-op if already done — so it is safe to call
/// after `handle_bye` has cleaned up some of the same state. Shared by the
/// remote-BYE and dialog-timeout teardown paths so the NOTIFY registration is
/// released consistently on both (it was previously leaked on the timeout path).
fn finalize_inbound_teardown(
    channel_name: &str,
    unique_id: &str,
    driver: Option<&Arc<SipChannelDriver>>,
) {
    store::deregister(unique_id);
    crate::notify_service::global_notify_service().unregister_channel(channel_name);
    if let Some(driver) = driver {
        driver.remove_channel(channel_name);
    }
}

/// Extract the caller's advertised RTP endpoint (IP + port) from an offer SDP.
///
/// Prefers a media-level `c=` line, falling back to the session-level `c=`.
/// Returns `None` when there is no active audio stream (port 0) or the address
/// cannot be parsed — in which case we still bind a socket but leave the remote
/// unset until the first packet teaches us the source.
fn remote_rtp_endpoint(sdp: &SessionDescription) -> Option<SocketAddr> {
    let media = sdp
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")?;
    if media.port == 0 {
        return None;
    }
    let addr_str = media
        .connection
        .as_ref()
        .map(|c| c.addr.as_str())
        .or_else(|| sdp.connection.as_ref().map(|c| c.addr.as_str()))?;
    let ip: std::net::IpAddr = addr_str.parse().ok()?;
    Some(SocketAddr::new(ip, media.port))
}

/// Determine the outbound RTP payload type for the answer: the first codec the
/// offer and our supported list share (so we echo/transmit with a PT the caller
/// understands). Falls back to the offer's first advertised format.
fn negotiated_payload_type(sdp: &SessionDescription, supported: &[Codec]) -> Option<u8> {
    let media = sdp
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")?;
    for oc in media.codecs() {
        for sc in supported {
            if oc.name.eq_ignore_ascii_case(&sc.name) && oc.sample_rate == sc.sample_rate {
                return Some(oc.payload_type);
            }
        }
    }
    media.formats.first().copied()
}

/// Authorize a REGISTER: is `username` (a digest identity that has already
/// authenticated) permitted to bind the Address-of-Record `aor`?
///
/// The rule mirrors real Asterisk: a REGISTER may only bind an AoR that
/// belongs to the endpoint whose credentials authenticated the request. For
/// each endpoint we check two things about the *same* endpoint:
///   1. it authenticates the caller — it references an auth section whose
///      `username` equals the authenticated user; and
///   2. it owns `aor` — `aor` is in its configured `aors` list
///      (comma-separated), or, when the endpoint configures no `aors`, `aor`
///      matches the endpoint's own identity (its section name or its auth
///      username). The no-`aors` fallback is the standard minimal-config
///      convention (`endpoint == auth == aor`) and only ever grants an
///      endpoint its *own* identity.
///
/// All comparisons are case-insensitive, consistent with the other config
/// lookups. Because ownership is always evaluated against the very endpoint
/// the caller authenticated as, the granted AoR can never be a *different*
/// endpoint's — keying on the cryptographically-verified digest username
/// (not the spoofable From/To) is what stops alice's valid credentials from
/// binding bob's AoR (issue #33).
fn user_may_register_aor(
    cfg: &crate::pjsip_config::PjsipConfig,
    username: &str,
    aor: &str,
) -> bool {
    cfg.endpoints.iter().any(|ep| {
        let auth_username = ep
            .auth
            .as_ref()
            .and_then(|auth_name| cfg.find_auth(auth_name))
            .map(|auth| auth.username.as_str());

        // (1) This endpoint must authenticate the caller.
        if !auth_username.is_some_and(|u| u.eq_ignore_ascii_case(username)) {
            return false;
        }

        // (2) ...and own the requested AoR.
        match ep
            .aors
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(aors) => aors.split(',').any(|a| a.trim().eq_ignore_ascii_case(aor)),
            None => {
                ep.name.eq_ignore_ascii_case(aor)
                    || auth_username.is_some_and(|u| u.eq_ignore_ascii_case(aor))
            }
        }
    })
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
    if let Some(inner) = header.strip_prefix('"') {
        if let Some(end_quote) = inner.find('"') {
            let name = &inner[..end_quote];
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
    fn test_remote_rtp_endpoint_from_offer() {
        let offer = SessionDescription::create_offer(
            "203.0.113.7",
            40000,
            &[codecs::pcmu()],
        );
        let ep = remote_rtp_endpoint(&offer).expect("offer has an audio endpoint");
        assert_eq!(ep.ip().to_string(), "203.0.113.7");
        assert_eq!(ep.port(), 40000);
    }

    #[test]
    fn test_remote_rtp_endpoint_rejected_stream_is_none() {
        // A media stream with port 0 (rejected/held) has no live endpoint.
        let mut offer = SessionDescription::create_offer("203.0.113.7", 0, &[codecs::pcmu()]);
        offer.media_descriptions[0].port = 0;
        assert!(remote_rtp_endpoint(&offer).is_none());
    }

    // --- issue #33: REGISTER AoR authorization scoping ---------------------

    fn scoping_config() -> crate::pjsip_config::PjsipConfig {
        use crate::pjsip_config::{AuthConfig, EndpointConfig, PjsipConfig};

        let mk_auth = |name: &str, user: &str| AuthConfig {
            name: name.to_string(),
            username: user.to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let mk_ep = |name: &str, auth: &str, aors: Option<&str>| EndpointConfig {
            name: name.to_string(),
            auth: Some(auth.to_string()),
            aors: aors.map(|s| s.to_string()),
            ..Default::default()
        };

        PjsipConfig {
            endpoints: vec![
                mk_ep("alice", "alice-auth", Some("alice")),
                mk_ep("bob", "bob-auth", Some("bob")),
                // An endpoint with no AoR cannot register anything.
                mk_ep("carol", "carol-auth", None),
                // An endpoint owning several AoRs (comma list).
                mk_ep("multi", "multi-auth", Some("m1, m2")),
            ],
            auths: vec![
                mk_auth("alice-auth", "alice"),
                mk_auth("bob-auth", "bob"),
                mk_auth("carol-auth", "carol"),
                mk_auth("multi-auth", "multi"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn test_user_may_register_own_aor() {
        let cfg = scoping_config();
        assert!(user_may_register_aor(&cfg, "alice", "alice"));
    }

    #[test]
    fn test_user_may_not_register_another_users_aor() {
        // The core hijack: alice's valid credentials must NOT bind bob's AoR.
        let cfg = scoping_config();
        assert!(!user_may_register_aor(&cfg, "alice", "bob"));
    }

    #[test]
    fn test_aor_membership_is_case_insensitive() {
        let cfg = scoping_config();
        assert!(user_may_register_aor(&cfg, "ALICE", "Alice"));
    }

    #[test]
    fn test_unknown_user_may_register_nothing() {
        let cfg = scoping_config();
        assert!(!user_may_register_aor(&cfg, "eve", "alice"));
    }

    #[test]
    fn test_endpoint_without_explicit_aor_owns_its_own_identity() {
        // carol has no `aors=` configured. Under the minimal-config
        // convention it implicitly owns the AoR matching its own identity...
        let cfg = scoping_config();
        assert!(user_may_register_aor(&cfg, "carol", "carol"));
        // ...but still nothing else — no cross-endpoint binding.
        assert!(!user_may_register_aor(&cfg, "carol", "alice"));
        assert!(!user_may_register_aor(&cfg, "carol", "dave"));
    }

    #[test]
    fn test_comma_separated_aor_list_membership() {
        let cfg = scoping_config();
        assert!(user_may_register_aor(&cfg, "multi", "m1"));
        assert!(user_may_register_aor(&cfg, "multi", "m2"));
        assert!(!user_may_register_aor(&cfg, "multi", "m3"));
    }

    // --- issue #28: outbound legs release their RTP socket / driver entry --

    // `request()` is a ChannelDriver trait method; bring the trait into scope.
    use asterisk_core::channel::ChannelDriver;

    async fn driver_and_handler() -> (Arc<SipChannelDriver>, Arc<SipEventHandler>) {
        let transport: Arc<dyn crate::transport::SipTransport> = Arc::new(
            crate::transport::UdpTransport::bind("127.0.0.1:0".parse().unwrap())
                .await
                .unwrap(),
        );
        let driver = Arc::new(SipChannelDriver::new("127.0.0.1:0".parse().unwrap()));
        driver.set_transport(transport.clone());
        let handler = Arc::new(SipEventHandler::new(
            Arc::new(asterisk_core::pbx::Dialplan::new()),
            transport,
        ));
        handler.set_channel_driver(driver.clone());
        (driver, handler)
    }

    #[tokio::test]
    async fn send_bye_for_channel_releases_driver_entry() {
        let (driver, handler) = driver_and_handler().await;

        // An outbound leg: request() binds an RTP socket and inserts a
        // driver channel-map entry.
        let ch = driver
            .request("sip:bob@127.0.0.1:5060", None)
            .await
            .expect("outbound request");
        // Register its Call-ID/session as call() would, so the BYE path runs.
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        handler.register_outbound_callid("out-1", &ch.name);
        handler.register_outbound_session(
            "out-1",
            &ch.name,
            SipSession::new_outbound("127.0.0.1:0".parse().unwrap(), remote),
            remote,
        );
        assert_eq!(driver.active_channel_count(), 1);

        handler.send_bye_for_channel(&ch.name).await;

        assert_eq!(
            driver.active_channel_count(),
            0,
            "outbound leg's RTP socket / driver entry must be released on hangup"
        );
        assert_eq!(handler.active_calls(), 0, "Call-ID bookkeeping must be cleared");
    }

    #[tokio::test]
    async fn release_outbound_leg_frees_driver_entry_without_session() {
        let (driver, handler) = driver_and_handler().await;

        // A leg that never reached call() (no Call-ID/session) still holds a
        // bound RTP socket — abandoning it must free that socket.
        let ch = driver
            .request("sip:carol@127.0.0.1:5060", None)
            .await
            .expect("outbound request");
        assert_eq!(driver.active_channel_count(), 1);

        handler.release_outbound_leg(&ch.name);

        assert_eq!(
            driver.active_channel_count(),
            0,
            "abandoned leg's RTP socket / driver entry must be released"
        );
    }

    #[tokio::test]
    async fn handle_bye_releases_outbound_driver_entry() {
        let (driver, handler) = driver_and_handler().await;

        let ch = driver
            .request("sip:dave@127.0.0.1:5060", None)
            .await
            .expect("outbound request");
        handler.register_outbound_callid("bye-call-1", &ch.name);
        assert_eq!(driver.active_channel_count(), 1);

        // Remote sends a BYE for this call.
        let bye_raw = "BYE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKbye1\r\n\
             From: <sip:dave@127.0.0.1>;tag=f1\r\n\
             To: <sip:asterisk@127.0.0.1>;tag=t1\r\n\
             Call-ID: bye-call-1\r\n\
             CSeq: 2 BYE\r\n\
             Content-Length: 0\r\n\r\n";
        let bye = SipMessage::parse(bye_raw.as_bytes()).unwrap();
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        handler.handle_bye(&bye, remote).await;

        assert_eq!(
            driver.active_channel_count(),
            0,
            "remote BYE on an outbound leg must release its RTP socket / driver entry"
        );
    }

    #[test]
    fn test_negotiated_payload_type_prefers_common_codec() {
        // Offer PCMA (8); our supported list includes it -> PT 8.
        let offer = SessionDescription::create_offer("203.0.113.7", 40000, &[codecs::pcma()]);
        let supported = vec![codecs::pcmu(), codecs::pcma()];
        assert_eq!(negotiated_payload_type(&offer, &supported), Some(8));
    }

    #[test]
    fn test_finalize_inbound_teardown_unregisters_notify_and_store() {
        // Regression: the dialog-timeout cleanup path previously deregistered
        // the store channel but never unregistered the NOTIFY-service state,
        // leaking a ChannelSipState per timed-out call. finalize_inbound_teardown
        // must clear both.
        let notify = crate::notify_service::global_notify_service();
        let name = "PJSIP/teardown-test-0000ffff";

        let ch = asterisk_core::channel::Channel::new(name);
        let registered = store::register_existing_channel(ch);
        let uid = registered.lock().unique_id.0.clone();
        assert!(store::find_by_name(name).is_some(), "channel registered");

        notify.register_channel(
            name,
            crate::notify_service::ChannelSipState {
                call_id: "teardown-call".to_string(),
                local_tag: "ltag".to_string(),
                remote_tag: "rtag".to_string(),
                local_uri: "sip:asterisk@127.0.0.1".to_string(),
                remote_target: "sip:caller@127.0.0.1".to_string(),
                remote_addr: "127.0.0.1:5062".parse().unwrap(),
                local_seq: 100,
            },
        );
        assert!(notify.is_registered(name), "notify state registered");

        // driver=None exercises the store + notify teardown without needing a
        // live SipChannelDriver.
        finalize_inbound_teardown(name, &uid, None);

        assert!(
            !notify.is_registered(name),
            "NOTIFY registration must be released on teardown (the leaked path)"
        );
        assert!(
            store::find_by_name(name).is_none(),
            "store channel must be deregistered on teardown"
        );
    }
}
