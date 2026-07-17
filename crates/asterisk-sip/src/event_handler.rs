//! SIP Event Handler.
//!
//! Receives SIP events from the SIP stack and creates/manages Asterisk
//! channels. This is the glue between the SIP protocol layer and the
//! PBX/channel model.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use tracing::{info, warn, debug, trace};

use crate::channel_driver::SipChannelDriver;
use crate::parser::SipMessage;
use crate::authenticator::AuthCredentials;
use crate::registrar::Registrar;
use crate::rtp::{RtpPortAllocator, RtpSession};
use crate::sdp::SessionDescription;
use crate::session::SipSession;
use crate::transport::SipTransport;
use asterisk_codecs::{codecs, Codec};
use asterisk_core::channel::store;
use asterisk_core::channel::softhangup;
use asterisk_core::pbx::Dialplan;
use asterisk_types::{ChannelState, HangupCause};

/// Methods this UA supports, advertised in the initial-INVITE 2xx `Allow`
/// (RFC 3311 §5.1) and in the OPTIONS 200 `Allow` (RFC 3261 §11 / RFC 3311
/// §5.2). `UPDATE` is included because the stack answers in-dialog UPDATE
/// (`handle_update`); omitting it told peers UPDATE was unsupported even though
/// the handler existed (M5 review MAJOR-3).
pub const SUPPORTED_METHODS: &str = "INVITE, ACK, CANCEL, BYE, OPTIONS, REFER, NOTIFY, UPDATE";

/// Build the 200 OK for an OPTIONS ping. Advertises [`SUPPORTED_METHODS`] in
/// `Allow` (which now includes UPDATE) plus `Accept`/`Server`. Centralising the
/// Allow list here keeps the OPTIONS advertisement and the initial-2xx Allow in
/// lockstep and makes the wire contract unit-testable.
pub fn build_options_ok(request: &SipMessage) -> Option<SipMessage> {
    let mut ok = request.create_response(200, "OK").ok()?;
    ok.add_header(crate::parser::header_names::ALLOW, SUPPORTED_METHODS);
    ok.add_header("Accept", "application/sdp");
    ok.add_header("Server", "Rustisk/0.1.0");
    Some(ok)
}

/// RFC 4028 §9 responder-only session-timer policy for an in-dialog request
/// carrying `Session-Expires`.
///
/// rustisk answers UAC-driven refreshes (re-INVITE / UPDATE) but does not yet
/// schedule its own, so it only ever acts as a *responder* with the UAC as the
/// refresher:
///
/// * `refresher=uac`    → echo `uac` (honour the peer's explicit choice).
/// * no refresher param → select `uac` (our policy: the peer refreshes).
/// * `refresher=uas`    → the peer asked us to refresh, which we do NOT
///   implement. RFC 4028 forbids overriding the peer's explicit choice, and we
///   must not claim a role we don't perform (that would let a healthy long call
///   drop at the timer). So the timer is *declined*: return `None`, emit no
///   `Session-Expires`. UAS-side refresh SCHEDULING is deferred to M7.
///
/// Returns `Some((interval, "uac"))` to advertise in the response, or `None` to
/// omit the session timer entirely.
fn session_timer_response(request: &SipMessage) -> Option<(String, &'static str)> {
    let se = request.get_header(crate::parser::header_names::SESSION_EXPIRES)?;
    let mut parts = se.split(';');
    let interval = parts.next()?.trim().to_string();
    // A Session-Expires without a positive integer interval is malformed; do
    // not engage a timer for it.
    if interval.parse::<u32>().is_err() {
        return None;
    }
    let mut refresher = None;
    for p in parts {
        if let Some(v) = p.trim().strip_prefix("refresher=") {
            refresher = Some(v.trim().to_ascii_lowercase());
        }
    }
    match refresher.as_deref() {
        // Never claim the uas role we don't schedule (deferred to M7).
        Some("uas") => None,
        // `uac` explicitly, or unspecified (we select uac): the peer refreshes.
        _ => Some((interval, "uac")),
    }
}

/// Per-call state stored by the event handler for SIP signaling.
struct CallState {
    /// The SIP session (holds INVITE, dialog, SDP, etc.).
    session: SipSession,
    /// Source-validation tuple: the address inbound in-dialog requests and
    /// responses must originate from, and where server responses are sent.
    /// This is the symmetric INVITE source and is NEVER moved by a target
    /// refresh, so forged-request rejection and symmetric-response behavior
    /// are preserved.
    remote_addr: SocketAddr,
    /// Physical next hop for LOCAL in-dialog requests (BYE, re-INVITE).
    /// Initialized to the INVITE source tuple (`remote_addr`) and refreshed to
    /// the resolved Contact when an in-dialog UPDATE / re-INVITE performs an
    /// RFC 3261 §12.2 target refresh. Kept separate from `remote_addr` so the
    /// datagram actually reaches the refreshed target instead of the stale
    /// INVITE source (M5 review MAJOR-3b).
    next_hop: SocketAddr,
    /// Channel name for correlation.
    channel_name: String,
    /// Dial abandoned this leg while its INVITE was still pending. Retain
    /// signaling state long enough to resolve a crossing 2xx with ACK+BYE.
    abandoned: bool,
    /// Bounded socket reservation used only by signaling-only handlers that
    /// were constructed without a channel driver (primarily unit tests).
    _rtp_reservation: Option<RtpSession>,
}

#[cfg(not(test))]
const ABANDONED_SIGNALING_GRACE: std::time::Duration =
    crate::transaction::timers::TIMER_B;
#[cfg(test)]
const ABANDONED_SIGNALING_GRACE: std::time::Duration =
    std::time::Duration::from_millis(50);
#[cfg(not(test))]
const OUTBOUND_BYE_SIGNALING_GRACE: std::time::Duration =
    crate::transaction::timers::TIMER_F;
#[cfg(test)]
const OUTBOUND_BYE_SIGNALING_GRACE: std::time::Duration =
    std::time::Duration::from_millis(50);

/// Exact live-resource counts for SIP call teardown verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SipResourceCounts {
    pub driver_channels: usize,
    pub call_id_mappings: usize,
    pub call_states: usize,
    pub notify_channels: usize,
    pub invite_client_transactions: usize,
    pub invite_server_transactions: usize,
    pub non_invite_client_transactions: usize,
    pub non_invite_server_transactions: usize,
    /// Live RTP sessions (bound UDP ports) — a true gauge of RTP port
    /// allocations, independent of the driver map.
    pub rtp_sessions: usize,
    /// Total contact bindings across every AoR in the registrar.
    pub registrar_bindings: usize,
    /// Scoped channel hangup-callback registrations (issue #121/#122).
    pub hangup_callbacks: usize,
    /// Scoped channel answer-callback registrations (issue #121/#122).
    pub answer_callbacks: usize,
}

/// SIP event handler -- bridges the SIP stack to the Asterisk channel model.
pub struct SipEventHandler {
    dialplan: Arc<Dialplan>,
    /// Configuration snapshot used for endpoint identification and auth.
    pjsip_config: Option<Arc<crate::pjsip_config::PjsipConfig>>,
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
    /// Bounded fallback for signaling-only handler users that do not attach a
    /// channel driver. The reservation is retained in [`CallState`].
    fallback_rtp_allocator: RtpPortAllocator,
    /// Inbound REGISTER handler (contact bindings per AoR).
    registrar: Arc<Registrar>,
    /// The SIP stack whose transaction layer tracks INVITE server
    /// transactions. Set once at startup via [`Self::set_stack`]. Every
    /// final INVITE response the handler sends is first recorded through
    /// [`crate::stack::SipStack::record_invite_final`], which suppresses an
    /// answer racing a CANCEL-sent 487 and arms Timer G retransmission
    /// (issue #55). When absent (handler-level tests), finals are sent
    /// unrecorded.
    stack: OnceLock<Arc<crate::stack::SipStack>>,
    /// RTP inactivity teardown (`rtptimeout`). `Some(d)` reaps an established
    /// inbound call that receives no accepted inbound RTP for `d`; `None`
    /// disables the reaper. Set once at startup via [`Self::set_rtp_timeout`];
    /// defaults to disabled so handler-level tests are unaffected unless they
    /// opt in.
    rtp_timeout: RwLock<Option<std::time::Duration>>,
}

impl SipEventHandler {
    /// Create a new event handler with the given dialplan and transport.
    pub fn new(dialplan: Arc<Dialplan>, transport: Arc<dyn SipTransport>) -> Self {
        Self::new_with_pjsip_config(
            dialplan,
            transport,
            crate::pjsip_config::get_global_pjsip_config(),
        )
    }

    /// Create an event handler with an explicit PJSIP configuration snapshot.
    ///
    /// Startup uses [`Self::new`]; this constructor keeps source-policy tests
    /// independent of the process-global configuration.
    pub fn new_with_pjsip_config(
        dialplan: Arc<Dialplan>,
        transport: Arc<dyn SipTransport>,
        pjsip_config: Option<Arc<crate::pjsip_config::PjsipConfig>>,
    ) -> Self {
        // Register transport with global notify service
        crate::notify_service::global_notify_service().set_transport(transport.clone());
        Self {
            dialplan,
            pjsip_config,
            callid_map: Arc::new(RwLock::new(HashMap::new())),
            transport,
            call_states: Arc::new(RwLock::new(HashMap::new())),
            supported_codecs: vec![
                codecs::pcmu(), codecs::pcma(), codecs::telephone_event(),
                codecs::vp8(), codecs::h264(), codecs::vp9(), codecs::h265(),
            ],
            channel_driver: OnceLock::new(),
            fallback_rtp_allocator: RtpPortAllocator::default(),
            registrar: Arc::new(Registrar::new()),
            stack: OnceLock::new(),
            rtp_timeout: RwLock::new(None),
        }
    }

    /// Configure the RTP inactivity teardown (`rtptimeout`). `Some(d)` arms the
    /// reaper for every subsequently answered inbound call; `None` disables it.
    /// Startup passes the value parsed from `rtp.conf`.
    pub fn set_rtp_timeout(&self, timeout: Option<std::time::Duration>) {
        *self.rtp_timeout.write() = timeout;
    }

    /// The currently configured RTP inactivity teardown, if any.
    pub fn rtp_timeout(&self) -> Option<std::time::Duration> {
        *self.rtp_timeout.read()
    }

    /// Attach the SIP stack so final INVITE responses are recorded in (and
    /// gated by) its transaction layer. See the `stack` field docs.
    pub fn set_stack(&self, stack: Arc<crate::stack::SipStack>) {
        let _ = self.stack.set(stack);
    }

    /// Record `response` as the final response for `request`'s INVITE server
    /// transaction. Returns whether the caller may put it on the wire —
    /// `false` means the transaction already got a final (e.g. a
    /// CANCEL-triggered 487) and the response must be suppressed.
    fn may_send_invite_final(&self, request: &SipMessage, response: &SipMessage) -> bool {
        match self.stack.get() {
            Some(stack) => stack.record_invite_final(request, response),
            None => true,
        }
    }

    /// Send a TU response through the stack when available so non-INVITE
    /// server transactions cache it, enter Completed, and reap on Timer J.
    async fn send_server_response(
        &self,
        response: &SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<(), crate::transport::TransportError> {
        match self.stack.get() {
            Some(stack) => stack.send_response(response.clone(), remote_addr).await,
            None => self.transport.send(response, remote_addr).await,
        }
    }

    /// Send a dialog request through the non-INVITE client transaction when
    /// the production stack is attached. Handler-only tests retain their
    /// transport fallback.
    async fn send_client_request(
        &self,
        request: SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<(), crate::transport::TransportError> {
        match self.stack.get() {
            Some(stack) => stack.send_request(request, remote_addr).await.map(|_| ()),
            None => self.transport.send(&request, remote_addr).await,
        }
    }

    async fn validate_in_dialog_request(
        &self,
        request: &SipMessage,
        remote_addr: SocketAddr,
    ) -> Result<(), &'static str> {
        let call_id = request.call_id().ok_or("missing Call-ID")?;
        let call_state = self
            .call_states
            .read()
            .get(call_id)
            .cloned()
            .ok_or("unknown Call-ID")?;
        let mut call_state = call_state.lock().await;
        if call_state.remote_addr != remote_addr {
            return Err("source tuple mismatch");
        }
        let dialog = call_state
            .session
            .dialog
            .as_mut()
            .ok_or("session has no dialog")?;
        dialog
            .validate_remote_request(request)
            .map_err(|_| "dialog identity, route set, or CSeq mismatch")
    }

    async fn reject_in_dialog_request(
        &self,
        request: &SipMessage,
        remote_addr: SocketAddr,
        reason: &'static str,
    ) {
        let call_id = request.call_id().unwrap_or("<missing>");
        warn!(call_id, source = %remote_addr, reason, "Rejected forged in-dialog request");
        let Ok(response) = request.create_response(481, "Call/Transaction Does Not Exist") else {
            return;
        };
        if request.method() == Some(crate::parser::SipMethod::Invite) {
            if self.may_send_invite_final(request, &response) {
                let _ = self.transport.send(&response, remote_addr).await;
            }
        } else {
            let _ = self.send_server_response(&response, remote_addr).await;
        }
    }

    async fn response_matches_dialog(
        &self,
        response: &SipMessage,
        remote_addr: SocketAddr,
    ) -> bool {
        let Some(call_id) = response.call_id() else {
            return false;
        };
        let Some(call_state) = self.call_states.read().get(call_id).cloned() else {
            return false;
        };
        let call_state = call_state.lock().await;
        if call_state.remote_addr != remote_addr {
            return false;
        }

        let Some(response_cseq) = response
            .cseq()
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return false;
        };

        if let Some(dialog) = call_state.session.dialog.as_ref() {
            return dialog
                .validate_remote_response(response, dialog.local_seq)
                .is_ok();
        }

        let Some(invite) = call_state.session.invite.as_ref() else {
            return false;
        };
        invite.call_id() == response.call_id()
            && invite.from_header().and_then(crate::parser::extract_tag)
                == response.from_header().and_then(crate::parser::extract_tag)
            && invite
                .cseq()
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u32>().ok())
                == Some(response_cseq)
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
        // Treat configured IP/CIDR identify rules as an allowlist. This must be
        // the first policy decision: an untrusted source must not receive an
        // auth challenge, probe dialplan existence, or create channel state.
        // The transport source is authoritative here, never Via or From (RFC
        // 3261 section 18.2.1).
        let pjsip_config = self.pjsip_config.clone();
        let matched_endpoint_name = match source_acl_endpoint(
            pjsip_config.as_deref(),
            remote_addr.ip(),
        ) {
            Ok(endpoint) => endpoint,
            Err(SourceAclDenied) => {
                let call_id = request.call_id().unwrap_or("<missing>");
                if let Ok(forbidden) = request.create_response(403, "Forbidden") {
                    if self.may_send_invite_final(request, &forbidden) {
                        if let Err(e) = self.transport.send(&forbidden, remote_addr).await {
                            warn!(
                                call_id,
                                source = %remote_addr,
                                "Failed to send source ACL rejection: {}",
                                e
                            );
                        } else {
                            warn!(
                                call_id,
                                source = %remote_addr,
                                "Rejected INVITE from source outside configured identify CIDRs"
                            );
                        }
                    }
                }
                return None;
            }
        };

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
        // Issue #129: this used to be an unconditional `eprintln!` dump of the
        // caller number and EVERY header value — including `Authorization`
        // (digest credential material) and `From`/`Contact`/
        // `P-Asserted-Identity` (caller PII) — on the normal call path at any
        // verbosity. The summary and the header dump are now level-gated, the
        // summary carries no caller number, and sensitive header VALUES are
        // never printed (see `redact_invite_header_value`). Full traffic
        // inspection belongs to the explicitly-enabled SIP logger, not here.
        debug!(call_id = %call_id, exten = %exten, "handle_incoming_invite");
        for h in &request.headers {
            trace!(header = %h.name, value = %redact_invite_header_value(&h.name, &h.value), "INVITE header");
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
                debug!(call_id = %call_id, "re-INVITE detected");
                return self.handle_reinvite_request(request, remote_addr, session).await;
            }
        }

        // 4. Authenticate the request against configured endpoints.
        //    Build credentials from all endpoints that have auth configured.
        let mut identified_endpoint_name = matched_endpoint_name.clone();
        let mut endpoint_context = identified_endpoint_name
            .as_deref()
            .and_then(|name| pjsip_config.as_ref()?.find_endpoint(name))
            .map(|endpoint| endpoint.context.clone())
            .unwrap_or_else(|| "default".to_string());
        let mut allow_overlap = identified_endpoint_name
            .as_deref()
            .and_then(|name| pjsip_config.as_ref()?.find_endpoint(name))
            .map(|endpoint| endpoint.allow_overlap)
            .unwrap_or(true);

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
                        debug!(call_id = %call_id, "Auth succeeded");
                        // Auth succeeded -- identify the endpoint from the auth username.
                        // Extract username from the Authorization header to find the matching endpoint.
                        if let Some(auth_hdr) = request.get_header(crate::parser::header_names::AUTHORIZATION) {
                            if let Some(parsed) = crate::authenticator::parse_authorization(auth_hdr) {
                                for (ep_name, cred) in &all_creds {
                                    if cred.username == parsed.username {
                                        if let Some(ep) = cfg.find_endpoint(ep_name) {
                                            identified_endpoint_name = Some(ep_name.clone());
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
                        debug!(call_id = %call_id, "Auth failed, sending 401");
                        // Send 401 challenge
                        if self.may_send_invite_final(request, &challenge) {
                            if let Err(e) = self.transport.send(&challenge, remote_addr).await {
                                warn!(call_id = %call_id, "Failed to send 401 challenge: {}", e);
                            } else {
                                debug!(call_id = %call_id, "Sent 401 Unauthorized challenge");
                            }
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
        debug!(context = %endpoint_context, exten = %exten, exists = extension_exists, allow_overlap, "Extension lookup");
        if !extension_exists {
            if allow_overlap && self.dialplan.could_match(&endpoint_context, &exten) {
                // Overlap enabled and extension could match with more digits -> 484
                if let Ok(resp) = request.create_response(484, "Address Incomplete") {
                    if self.may_send_invite_final(request, &resp) {
                        let _ = self.transport.send(&resp, remote_addr).await;
                        debug!(call_id = %call_id, exten = %exten, "Sent 484 Address Incomplete (overlap enabled)");
                    }
                }
                return None;
            } else {
                // No match possible -> 404
                if let Ok(resp) = request.create_response(404, "Not Found") {
                    if self.may_send_invite_final(request, &resp) {
                        let _ = self.transport.send(&resp, remote_addr).await;
                        debug!(call_id = %call_id, exten = %exten, "Sent 404 Not Found");
                    }
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
                if self.may_send_invite_final(request, &resp) {
                    let _ = self.transport.send(&resp, remote_addr).await;
                    info!(
                        call_id = %call_id,
                        "Sent 488 Not Acceptable Here (delayed-offer INVITE with no SDP not supported)"
                    );
                }
            }
            return None;
        }

        let endpoint_codecs = identified_endpoint_name
            .as_deref()
            .and_then(|name| pjsip_config.as_ref()?.find_endpoint(name))
            .map(|endpoint| endpoint.media_codecs(&self.supported_codecs))
            .unwrap_or_else(|| self.supported_codecs.clone());

        // 5b. M2 bridges raw G.711 bytes without transcoding. Reject any offer
        //     that does not negotiate the endpoint-pinned static PCMU or PCMA
        //     payload before allocating a channel or RTP socket.
        if let Some(ref offer) = session.remote_sdp {
            let negotiated = crate::sdp_rtp::negotiated_audio_payload_type(
                offer,
                &endpoint_codecs,
            );
            if !matches!(negotiated, Some(0 | 8)) {
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    if self.may_send_invite_final(request, &resp) {
                        let _ = self.transport.send(&resp, remote_addr).await;
                        warn!(
                            call_id = %call_id,
                            ?negotiated,
                            "Sent 488 Not Acceptable Here (no endpoint-pinned G.711 codec)"
                        );
                    }
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
        let chan_label = identified_endpoint_name.as_deref().unwrap_or(&caller_num);
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
            if let Some(ref ep_name) = identified_endpoint_name {
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
        //    move media. The SDP answer advertises the socket's real,
        //    bounded port (issues #7, #8, #9, #70) and a
        //    concrete, routable connection address: external_media_address
        //    when configured, else the interface routed toward the caller.
        //    Advertising a raw INADDR_ANY bind as `c=IN IP4 0.0.0.0`
        //    blackholed audio for peers without symmetric RTP (issue #56).
        let mut rtp_reservation = None;
        if let Some(remote_sdp) = session.remote_sdp.clone() {
            // Route/NAT selection targets the peer that will actually send
            // RTP — the offer's media endpoint — not the SIP signaling
            // source; they differ behind proxies/SBCs and with third-party
            // media. Signaling source is the fallback for an unresolvable
            // offer address.
            let media_peer = crate::sdp_rtp::remote_rtp_endpoint(&remote_sdp)
                .unwrap_or(remote_addr);
            // Fail closed (CP3): if a configured external_media_address FQDN does
            // not resolve, do NOT answer with a bogus/internal c=/o= address —
            // reject the INVITE (488) rather than leak or blackhole the media.
            let Some(local_ip) = crate::sdp::advertised_media_ip(session.local_addr, media_peer)
            else {
                let unique_id = channel.lock().unique_id.0.clone();
                store::deregister(&unique_id);
                self.callid_map.write().remove(&call_id);
                warn!(call_id = %call_id, "Fail-closed: external_media_address did not resolve; rejecting INVITE 488");
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    if self.may_send_invite_final(request, &resp) {
                        let _ = self.transport.send(&resp, remote_addr).await;
                    }
                }
                return None;
            };
            let rtp_result = match self.channel_driver.get() {
                Some(driver) => driver.allocate_rtp_session(session.local_addr.ip()).await,
                None => self
                    .fallback_rtp_allocator
                    .allocate(session.local_addr.ip())
                    .await,
            }
            .and_then(|rtp| {
                let port = rtp.local_addr()?.port();
                Ok((rtp, port))
            });
            match rtp_result {
                Ok((rtp, answer_port)) => {
                    if let Some(remote_rtp) =
                        crate::sdp_rtp::remote_rtp_endpoint(&remote_sdp)
                    {
                        rtp.set_remote_addr(remote_rtp);
                    }
                    if let Some(pt) = crate::sdp_rtp::negotiated_audio_payload_type(
                        &remote_sdp, &endpoint_codecs,
                    ) {
                        rtp.set_payload_type(pt);
                    }
                    if let Some(pt) = crate::sdp_rtp::negotiated_dtmf_payload_type(
                        &remote_sdp,
                        &endpoint_codecs,
                    ) {
                        rtp.set_dtmf_payload_type(pt);
                    }
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
                    if let Some(driver) = self.channel_driver.get() {
                        driver.attach_inbound_media_with_codecs(
                            &channel_name,
                            driver_session,
                            self.transport.clone(),
                            rtp,
                            endpoint_codecs.clone(),
                        );
                    } else {
                        warn!(
                            call_id = %call_id,
                            "No channel driver set -- retaining bounded RTP socket for signaling-only call"
                        );
                        rtp_reservation = Some(rtp);
                    }
                    debug!(
                        call_id = %call_id,
                        channel = %channel_name,
                        port = answer_port,
                        "Bound inbound RTP media plane"
                    );

                    let answer_sdp = SessionDescription::create_answer(
                        &remote_sdp,
                        &local_ip,
                        answer_port,
                        &endpoint_codecs,
                    );
                    session.local_sdp = Some(answer_sdp.clone());
                    session.initial_local_sdp = Some(answer_sdp);
                }
                Err(e) => {
                    let unique_id = channel.lock().unique_id.0.clone();
                    store::deregister(&unique_id);
                    self.callid_map.write().remove(&call_id);
                    warn!(call_id = %call_id, "Failed to allocate inbound RTP socket: {}", e);
                    if let Ok(resp) = request.create_response(503, "Service Unavailable") {
                        if self.may_send_invite_final(request, &resp) {
                            let _ = self.transport.send(&resp, remote_addr).await;
                        }
                    }
                    return None;
                }
            }
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
            // Local in-dialog requests start out addressed to the INVITE
            // source tuple; a later UPDATE/re-INVITE target refresh moves it.
            next_hop: remote_addr,
            channel_name: channel_name.clone(),
            abandoned: false,
            _rtp_reservation: rtp_reservation,
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
        // For gating the Answer-triggered 200 OK against a racing CANCEL:
        // the transaction layer atomically decides which final wins.
        let stack_for_answer = self.stack.get().cloned();
        // Tear down the inbound media plane (drop the RTP socket) when the call
        // ends, so bound sockets are not leaked in the driver's channel map.
        let driver_for_cleanup = self.channel_driver.get().cloned();
        let channel_name_for_media = channel_name.clone();
        // Snapshot the RTP inactivity teardown for this call. Captured here (not
        // read inside the task) so the value is fixed for the call's lifetime.
        let rtp_timeout_for_task = *self.rtp_timeout.read();

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
        // The returned handle unregisters the closure when the per-call task
        // below ends, so the global answer registry returns to baseline instead
        // of leaking one closure per inbound call (issue #121).
        let answer_cb_handle =
            asterisk_core::channel::register_answer_callback_scoped(Box::new(move |uid| {
                if uid == unique_id_for_answer_cb {
                    answer_notify_for_cb.notify_one();
                }
            }));

        // Register a hangup callback -- fires when Channel::hangup() is called.
        // Scoped for the same reason as the answer callback above (issue #121).
        let hangup_cb_handle =
            asterisk_core::channel::register_hangup_callback_scoped(Box::new(move |uid, _cause| {
                if uid == unique_id_for_cb {
                    hangup_notify_for_cb.notify_one();
                }
            }));

        tokio::spawn(async move {
            // Keep the per-call answer/hangup callback registrations alive for
            // the whole task. Both callbacks have already fired (or become
            // moot) by the time this task returns; dropping the handles here
            // unregisters the closures so the global registries do not grow
            // without bound over a soak (issue #121).
            let _answer_cb_handle = answer_cb_handle;
            let _hangup_cb_handle = hangup_cb_handle;
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
            let mut pbx_handle = tokio::spawn(async move {
                asterisk_core::pbx::exec::pbx_run(tokio_channel_clone, dialplan_clone).await
            });

            // Wait for Answer() to be called, for the dialplan to finish
            // WITHOUT answering (failed/unknown app, pre-answer hangup), or
            // for the 30s answer timeout. The old flat 30s wait left an
            // early-aborting call in limbo for the full window (issue #57).
            let mut early_pbx_result = None;
            let answered = tokio::select! {
                // Poll in declared order: Answer() and dialplan completion
                // become ready near-simultaneously when Answer() is the last
                // priority — the stored Notify permit must win so the call
                // is answered, not torn down as "never answered".
                biased;
                _ = answer_notify.notified() => true,
                res = &mut pbx_handle => {
                    debug!(call_id = %call_id_for_task, "Dialplan finished before Answer()");
                    early_pbx_result = Some(res);
                    false
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    info!(
                        call_id = %call_id_for_task,
                        "Answer() not called within timeout; unwinding dialplan"
                    );
                    // Give up on the call: soft-hangup both channel copies so
                    // the dialplan unwinds and the pending INVITE gets its
                    // final failure response now, instead of staying open
                    // indefinitely (and instead of a late Answer() racing a
                    // failure final — post-timeout the call is unanswerable).
                    {
                        let mut ch = tokio_channel.lock().await;
                        ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                    }
                    if let Some(store_chan) = store::find_by_name(&ch_name_for_cleanup) {
                        let mut ch = store_chan.lock();
                        ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                    }
                    false
                }
            };

            // Whether OUR 200 OK actually went on the wire (vs. suppressed
            // by a racing CANCEL or never attempted): selects BYE vs.
            // final-failure teardown below.
            let mut established = false;

            if answered {
                // Answer() was called -- send 200 OK now.
                let still_active = call_states_ref.read().contains_key(&call_id_for_task);
                if still_active {
                    let mut cs = call_state.lock().await;
                    if let Some(ok_response) = cs.session.build_200_ok() {
                        // Atomically record the 200 as the INVITE's final
                        // response. If a CANCEL-triggered 487 won the race,
                        // the answer must never hit the wire (RFC 3261 §9.2).
                        let allowed = match (stack_for_answer.as_ref(), cs.session.invite.as_ref())
                        {
                            (Some(stack), Some(invite)) => {
                                stack.record_invite_final(invite, &ok_response)
                            }
                            _ => true,
                        };
                        if !allowed {
                            info!(
                                call_id = %call_id_for_task,
                                "Suppressing 200 OK: INVITE already has a final response (cancelled)"
                            );
                        } else if let Err(e) = transport.send(&ok_response, cs.remote_addr).await {
                            warn!(call_id = %call_id_for_task, "Failed to send 200 OK: {}", e);
                        } else {
                            info!(call_id = %call_id_for_task, "Sent 200 OK (triggered by Answer app)");
                            cs.session.state = crate::session::SessionState::Established;
                            established = true;
                        }
                    }
                }
            }

            // Reflect the answered state in the GLOBAL STORE copy. pbx_run
            // mutates a detached tokio copy of the channel, so without this the
            // store's channel — what `core show channels` / AMI CoreStatus and
            // every status consumer observes — stays at its pre-answer state
            // for the entire established call. The answer callback only wakes a
            // Notify; it does not touch the store copy for inbound calls.
            // (M5, store.rs baseline.)
            if established {
                if let Some(store_chan) = store::find_by_name(&ch_name_for_cleanup) {
                    let mut ch = store_chan.lock();
                    if ch.state != ChannelState::Up && ch.state != ChannelState::Down {
                        ch.set_state(ChannelState::Up);
                    }
                }
            }

            // Wait for pbx_run to finish (unless it already has). On an
            // established call, enforce the RTP inactivity timeout
            // (`rtptimeout`): if no accepted inbound RTP arrives for the
            // configured window, tear the call down. This is the load-bearing
            // reaper for a media-silent established call — nothing else bounds
            // it (the SIP dialog would otherwise stay up until the peer sends
            // BYE, and a long-running app such as Echo()/Wait() never returns).
            let result = match early_pbx_result {
                Some(r) => r,
                None => match (established, rtp_timeout_for_task, driver_for_cleanup.as_ref()) {
                    (true, Some(timeout), Some(driver)) => {
                        let watchdog_start = tokio::time::Instant::now();
                        // Poll fine enough to reap near the deadline without
                        // busy-looping; bounded to [250 ms, 1 s].
                        let poll = (timeout / 10).clamp(
                            std::time::Duration::from_millis(250),
                            std::time::Duration::from_secs(1),
                        );
                        loop {
                            tokio::select! {
                                biased;
                                r = &mut pbx_handle => break r,
                                _ = tokio::time::sleep(poll) => {
                                    let Some(idle) =
                                        driver.channel_rtp_idle(&ch_name_for_cleanup).await
                                    else {
                                        // No media plane attached: nothing to
                                        // police. Silence here is not an
                                        // inactivity signal, so never reap.
                                        continue;
                                    };
                                    // Discount pre-answer silence: measure from
                                    // the later of last-activity and watchdog
                                    // start (idle.min(elapsed-since-arming)).
                                    let effective = idle.min(watchdog_start.elapsed());
                                    if effective >= timeout {
                                        info!(
                                            call_id = %call_id_for_task,
                                            ?timeout,
                                            "RTP inactivity timeout reached; tearing down \
                                             media-silent call"
                                        );
                                        // Signal hangup via the GLOBAL STORE copy
                                        // only — the same signal handle_bye uses
                                        // for an inbound call. Hangup-aware apps
                                        // (Echo, Read, Wait, ConfBridge, ...)
                                        // poll the store copy between reads and
                                        // unwind. Deliberately not touching the
                                        // detached tokio copy the app holds: the
                                        // app owns that lock, and reaching for it
                                        // here would contend with the very read
                                        // loop we need to release.
                                        if let Some(store_chan) =
                                            store::find_by_name(&ch_name_for_cleanup)
                                        {
                                            let mut ch = store_chan.lock();
                                            ch.hangup_cause = HangupCause::NoAnswer;
                                            ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                                        }
                                        // Wait for the app to unwind, then fall
                                        // through to the normal established-call
                                        // teardown below (BYE + finalize).
                                        break (&mut pbx_handle).await;
                                    }
                                }
                            }
                        }
                    }
                    _ => pbx_handle.await,
                },
            };
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

            if established {
                // Send BYE to the remote endpoint to tear down the SIP dialog.
                let cs_arc_opt = {
                    let states = call_states_ref.read();
                    states.get(&call_id_for_task).cloned()
                };
                if let Some(cs_arc) = cs_arc_opt {
                    let mut cs = cs_arc.lock().await;
                    if let Some(bye) = cs.session.build_bye() {
                        // Route to the dialog's (possibly UPDATE-refreshed) next
                        // hop, not the stale INVITE source (M5 review MAJOR-3b).
                        if let Err(e) = transport.send(&bye, cs.next_hop).await {
                            warn!(call_id = %call_id_for_task, "Failed to send BYE: {}", e);
                        } else {
                            debug!(call_id = %call_id_for_task, "Sent BYE");
                        }
                    }
                }
            } else {
                // The dialplan ended without the call ever being answered
                // (unknown app, pre-answer hangup, no Answer() step). The
                // still-open INVITE must get a final response — leaving it
                // unanswered after 100 Trying forces the caller to
                // retransmit and time out (issue #57). The status is mapped
                // from the channel's hangup cause (486/503/...; default
                // 480), and recording it through the transaction layer both
                // arms Timer G and makes this a no-op when a CANCEL's 487
                // already terminated the transaction.
                let cause = { tokio_channel.lock().await.hangup_cause as u32 };
                let (status, reason) = crate::rfc3326::hangup_cause_to_sip_status(cause);
                let cs_arc_opt = {
                    let states = call_states_ref.read();
                    states.get(&call_id_for_task).cloned()
                };
                if let Some(cs_arc) = cs_arc_opt {
                    {
                        let cs = cs_arc.lock().await;
                        if let Some(ref invite) = cs.session.invite {
                            if let Ok(resp) = invite.create_response(status, reason) {
                                let allowed = match stack_for_answer.as_ref() {
                                    Some(stack) => stack.record_invite_final(invite, &resp),
                                    None => true,
                                };
                                if !allowed {
                                    debug!(
                                        call_id = %call_id_for_task,
                                        "INVITE already has a final response; no pre-answer failure sent"
                                    );
                                } else if let Err(e) =
                                    transport.send(&resp, cs.remote_addr).await
                                {
                                    warn!(
                                        call_id = %call_id_for_task,
                                        "Failed to send {} for unanswered INVITE: {}", status, e
                                    );
                                } else {
                                    info!(
                                        call_id = %call_id_for_task,
                                        status,
                                        "Sent final response for INVITE (dialplan ended pre-answer)"
                                    );
                                }
                            }
                        }
                    }
                    // No dialog was ever established: drop the call state now
                    // so the cleanup task finalizes immediately instead of
                    // idling out the 32s dialog timeout.
                    call_states_ref.write().remove(&call_id_for_task);
                    callid_map_ref.write().remove(&call_id_for_task);
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
            next_hop: remote_addr,
            channel_name: channel_name.to_string(),
            abandoned: false,
            _rtp_reservation: None,
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

        let status_code = response.status_code().unwrap_or(0);
        let cseq_method = response.get_header("CSeq")
            .and_then(|c| c.split_whitespace().last())
            .map(|m| m.to_uppercase())
            .unwrap_or_default();

        // The stack deliberately reports stray responses to the TU. Never
        // let one mutate a live session merely because it reused a known
        // Call-ID: source tuple, dialog tags, route set, and local CSeq must
        // all match first.
        if matches!(cseq_method.as_str(), "INVITE" | "BYE")
            && !self.response_matches_dialog(response, remote_addr).await
        {
            warn!(call_id = %call_id, source = %remote_addr,
                "Ignored response that did not match the live dialog");
            return;
        }

        // A final response to our BYE completes the dialog immediately. The
        // per-call cleanup task observes the removed state and releases its
        // store channel, driver entry, and RTP reservation on its next poll
        // instead of idling for the full 32-second stale-dialog deadline.
        if status_code >= 200 && cseq_method == "BYE" {
            self.call_states.write().remove(&call_id);
            self.callid_map.write().remove(&call_id);
            debug!(call_id = %call_id, status_code, "BYE transaction completed");
            return;
        }

        if cseq_method != "INVITE" {
            return;
        }

        let cs_arc = {
            let states = self.call_states.read();
            states.get(&call_id).cloned()
        };
        let Some(cs_arc) = cs_arc else {
            return;
        };

        // This mutex is the serialization point between response handling
        // and Dial abandonment. It deliberately remains held across the
        // signaling awaits: either the 2xx path sends ACK before abandonment
        // observes Established, or abandonment marks the leg first and this
        // path performs the mandatory ACK-then-BYE recovery.
        let mut cs = cs_arc.lock().await;
        if !cs.session.is_outbound {
            return;
        }
        cs.session.on_response(response);

        if let Some(to_tag) = response.get_header("To")
            .and_then(crate::parser::extract_tag) {
            crate::notify_service::global_notify_service()
                .update_remote_tag(&channel_name, &to_tag);
        }

        if (200..300).contains(&status_code) {
            if let Some(ack) = cs.session.build_ack() {
                if let Err(error) = self.transport.send(&ack, remote_addr).await {
                    warn!(call_id = %call_id, %error, "Failed to send ACK");
                } else {
                    debug!(call_id = %call_id, "Sent ACK for 200 OK");
                }
            } else {
                warn!(call_id = %call_id, "Failed to build ACK for 200 OK");
            }

            if cs.abandoned {
                if let Some(bye) = cs.session.build_bye() {
                    if let Err(error) = self.send_client_request(bye, cs.next_hop).await {
                        warn!(call_id = %call_id, %error,
                            "Failed to send BYE after abandoned INVITE received 200");
                    } else {
                        info!(call_id = %call_id,
                            "Resolved abandoned INVITE/200 crossing with ACK then BYE");
                    }
                }
                drop(cs);
                self.release_outbound_leg(&channel_name);
                return;
            }

            let answer_result = match self.channel_driver.get() {
                Some(driver) => driver.apply_outbound_answer(
                    &channel_name, response,
                ).await.map_err(|error| error.to_string()),
                None => Err("SIP channel driver is not attached".to_string()),
            };

            match answer_result {
                Ok(()) => {
                    if let Some(channel) = store::find_by_name(&channel_name) {
                        channel.lock().set_state(ChannelState::Up);
                    }
                }
                Err(error) => {
                    warn!(call_id = %call_id, channel = %channel_name, %error,
                        "Rejecting unusable outbound SDP answer");
                    if let Some(bye) = cs.session.build_bye() {
                        if let Err(send_error) = self.send_client_request(bye, remote_addr).await {
                            warn!(call_id = %call_id,
                                "Failed to send BYE after rejecting answer: {}", send_error);
                        }
                    }
                    if let Some(channel) = store::find_by_name(&channel_name) {
                        let mut ch = channel.lock();
                        ch.hangup_cause = HangupCause::BearerCapNotAvail;
                        ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                    }
                }
            }
            return;
        }

        let abandoned = cs.abandoned;
        drop(cs);
        if abandoned && status_code >= 300 {
            self.release_outbound_leg(&channel_name);
            return;
        }

        if let Some(channel) = store::find_by_name(&channel_name) {
            let mut ch = channel.lock();
            match status_code {
                180 | 183 => ch.set_state(ChannelState::Ringing),
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

            if let Err(reason) = self
                .validate_in_dialog_request(request, remote_addr)
                .await
            {
                self.reject_in_dialog_request(request, remote_addr, reason).await;
                return;
            }

            // Send 200 OK to the BYE
            match request.create_response(200, "OK") {
                Ok(ok_resp) => {
                    if let Err(e) = self.send_server_response(&ok_resp, remote_addr).await {
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
            let is_outbound = {
                let cs_arc = {
                    let states = self.call_states.read();
                    states.get(&call_id).cloned()
                };
                match cs_arc {
                    Some(cs_arc) => cs_arc.lock().await.session.is_outbound,
                    None => false,
                }
            };

            if let Some(name) = channel_name {
                if let Some(channel) = store::find_by_name(&name) {
                    let mut ch = channel.lock();
                    ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                }
                if is_outbound {
                    // Outbound legs are registered in the global store by
                    // Dial(), so use the complete outbound release path.
                    self.release_outbound_leg(&name);
                } else {
                    crate::notify_service::global_notify_service()
                        .unregister_channel(&name);
                    if let Some(driver) = self.channel_driver.get() {
                        driver.remove_channel(&name);
                    }
                }
            }

            // Clean up
            self.callid_map.write().remove(&call_id);
            self.call_states.write().remove(&call_id);

            // Notify any SFU conferences that this SIP call was hung up.
            crate::notify_sip_hangup(&call_id);
        }
    }

    /// Validate a transaction-user ACK. The stack has already checked the
    /// INVITE transaction identity; this enforces the remaining dialog and
    /// route-set constraints before the application accepts it.
    pub async fn handle_ack(&self, request: &SipMessage, remote_addr: SocketAddr) {
        if let Err(reason) = self
            .validate_in_dialog_request(request, remote_addr)
            .await
        {
            warn!(
                call_id = request.call_id().unwrap_or("<missing>"),
                source = %remote_addr,
                reason,
                "Ignored ACK that did not match the live dialog"
            );
        }
    }

    /// Build an SDP answer to an in-dialog offer (re-INVITE / UPDATE). It
    /// advertises the channel's REAL bound RTP port and a routable connection
    /// address (never 0.0.0.0), with route/NAT selection targeting the offer's
    /// media endpoint and falling back to the signaling source.
    /// Build the SDP answer for an in-dialog renegotiation (UPDATE/re-INVITE
    /// with an offer). Returns `None` — fail closed (CP3) — if a configured
    /// `external_media_address` FQDN does not resolve, so the caller rejects the
    /// renegotiation rather than answering with a bogus/internal media address.
    async fn renegotiation_answer(
        &self,
        channel_name: &str,
        offer: &SessionDescription,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
    ) -> Option<SessionDescription> {
        let channel_codecs = self
            .channel_driver
            .get()
            .and_then(|driver| driver.channel_codecs(channel_name))
            .unwrap_or_else(|| self.supported_codecs.clone());
        let media_port = match self.channel_driver.get() {
            Some(driver) => driver
                .channel_rtp_local_port(channel_name)
                .await
                .unwrap_or(10000),
            None => 10000,
        };
        let media_peer = crate::sdp_rtp::remote_rtp_endpoint(offer).unwrap_or(remote_addr);
        let local_ip = crate::sdp::advertised_media_ip(local_addr, media_peer)?;
        Some(SessionDescription::create_answer(offer, &local_ip, media_port, &channel_codecs))
    }

    /// Handle an in-dialog **UPDATE** (RFC 3311). Two shapes:
    ///
    /// * **With an SDP offer** — a mid-dialog media renegotiation. The offer is
    ///   applied to the media plane (remote address + payload type re-installed,
    ///   fail-closed on the pinned G.711 policy), the stored SDP is updated, and
    ///   a 200 OK carrying the SDP answer is returned. Real renegotiation, not a
    ///   sent-claim: the pump immediately sends to the new address and accepts
    ///   the new payload type.
    /// * **Without SDP** — a session-timer refresh / connected-line update. A
    ///   200 OK is returned; when the peer armed a session timer we advertise
    ///   the refresher we actually implement — the UAC — per
    ///   [`session_timer_response`] (RFC 4028 §9). We never claim `refresher=uas`.
    ///
    /// Both shapes are **target-refresh** requests (RFC 3261 §12.2 / RFC 3311):
    /// the request's Contact is applied to the dialog so later local in-dialog
    /// requests (BYE, re-INVITE) use the refreshed target, not the stale INVITE
    /// Contact.
    ///
    /// Previously this was silently answered `501 Not Implemented`, dropping any
    /// mid-call media change or session refresh (`main.rs` UPDATE dispatch).
    pub async fn handle_update(&self, request: &SipMessage, remote_addr: SocketAddr) {
        if let Err(reason) = self
            .validate_in_dialog_request(request, remote_addr)
            .await
        {
            self.reject_in_dialog_request(request, remote_addr, reason).await;
            return;
        }

        let Some(call_id) = request.call_id().map(|c| c.to_string()) else {
            return;
        };
        let cs_arc = {
            let states = self.call_states.read();
            states.get(&call_id).cloned()
        };
        let Some(cs_arc) = cs_arc else {
            return;
        };
        let (channel_name, local_addr) = {
            let cs = cs_arc.lock().await;
            (cs.channel_name.clone(), cs.session.local_addr)
        };

        // RFC 3261 §12.2 / RFC 3311: UPDATE is a target-refresh request. Apply
        // its Contact to the dialog's remote target so subsequent local
        // in-dialog requests (BYE, re-INVITE) address the refreshed target
        // instead of the stale INVITE Contact. (M5 review MAJOR-3.)
        if let Some(contact) = request
            .get_header(crate::parser::header_names::CONTACT)
            .and_then(crate::parser::extract_uri)
        {
            let mut cs = cs_arc.lock().await;
            cs.session.update_remote_target(&contact);
            // The refreshed target is also the physical next hop: move the
            // transport send destination to the resolved Contact so a later
            // local BYE/re-INVITE datagram actually reaches the new target,
            // not just its Request-URI (M5 review MAJOR-3b). Keep the prior
            // next hop when the Contact host is not directly addressable, so
            // symmetric routing stays intact (real NAT/DNS is M6).
            if let Some(addr) = cs.session.remote_target_addr() {
                cs.next_hop = addr;
            }
        }

        let carries_sdp = request
            .get_header(crate::parser::header_names::CONTENT_TYPE)
            .map(|ct| ct.to_ascii_lowercase().contains("application/sdp"))
            .unwrap_or(false)
            && !request.body.trim().is_empty();

        if carries_sdp {
            let Ok(offer) = SessionDescription::parse(&request.body) else {
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    let _ = self.send_server_response(&resp, remote_addr).await;
                }
                warn!(call_id = %call_id, "Rejected UPDATE with unparseable SDP offer");
                return;
            };

            // Fail-closed on the channel's pinned G.711 policy (mirrors the
            // re-INVITE path). A non-PCMU/PCMA offer is answered 488 rather than
            // silently mis-negotiated.
            let channel_codecs = self
                .channel_driver
                .get()
                .and_then(|driver| driver.channel_codecs(&channel_name))
                .unwrap_or_else(|| self.supported_codecs.clone());
            let negotiated =
                crate::sdp_rtp::negotiated_audio_payload_type(&offer, &channel_codecs);
            if !matches!(negotiated, Some(0 | 8)) {
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    let _ = self.send_server_response(&resp, remote_addr).await;
                    warn!(call_id = %call_id, ?negotiated,
                        "Rejected UPDATE outside the channel's pinned G.711 policy");
                }
                return;
            }

            // Apply the offer to the media plane BEFORE answering, so the pump
            // is renegotiated the instant our 200 goes out.
            if let Some(driver) = self.channel_driver.get() {
                if let Err(error) = driver.apply_inbound_offer(&channel_name, &offer).await {
                    warn!(call_id = %call_id, %error, "UPDATE media renegotiation failed");
                    if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                        let _ = self.send_server_response(&resp, remote_addr).await;
                    }
                    return;
                }
            }

            // Fail closed (CP3): if external_media_address does not resolve,
            // reject the UPDATE renegotiation (488) rather than answer with a
            // bogus/internal media address.
            let Some(answer) = self
                .renegotiation_answer(&channel_name, &offer, local_addr, remote_addr)
                .await
            else {
                warn!(call_id = %call_id, "Fail-closed: external_media_address did not resolve; rejecting UPDATE 488");
                if let Ok(resp) = request.create_response(488, "Not Acceptable Here") {
                    let _ = self.send_server_response(&resp, remote_addr).await;
                }
                return;
            };
            {
                let mut cs = cs_arc.lock().await;
                cs.session.remote_sdp = Some(offer);
                cs.session.local_sdp = Some(answer.clone());
            }

            let Ok(mut ok) = request.create_response(200, "OK") else {
                return;
            };
            ok.add_header(
                "Contact",
                &format!(
                    "<sip:asterisk@{}>",
                    crate::sdp::advertised_signaling_hostport(local_addr, remote_addr)
                ),
            );
            let sdp_str = answer.to_string();
            ok.add_header("Content-Type", "application/sdp");
            ok.add_header("Content-Length", &sdp_str.len().to_string());
            ok.body = sdp_str;
            // A media UPDATE may also carry a session timer; honour the same
            // responder-only (uac) refresher policy as the no-SDP branch.
            if let Some((interval, refresher)) = session_timer_response(request) {
                ok.add_header(
                    "Session-Expires",
                    &format!("{};refresher={}", interval, refresher),
                );
                ok.add_header("Require", "timer");
            }
            if let Err(e) = self.send_server_response(&ok, remote_addr).await {
                warn!(call_id = %call_id, "Failed to send 200 OK for UPDATE: {}", e);
            } else {
                info!(call_id = %call_id, "Answered in-dialog UPDATE (media renegotiated)");
            }
        } else {
            // Session-timer refresh / connected-line update: answer 200 and, per
            // RFC 4028 §9, advertise the refresher we actually implement (the
            // UAC). We NEVER emit refresher=uas — UAS-side refresh scheduling is
            // deferred to M7, and claiming that role would let a healthy long
            // call drop at the timer (M5 review MAJOR-3).
            let Ok(mut ok) = request.create_response(200, "OK") else {
                return;
            };
            ok.add_header(
                "Contact",
                &format!(
                    "<sip:asterisk@{}>",
                    crate::sdp::advertised_signaling_hostport(local_addr, remote_addr)
                ),
            );
            if let Some((interval, refresher)) = session_timer_response(request) {
                ok.add_header(
                    "Session-Expires",
                    &format!("{};refresher={}", interval, refresher),
                );
                ok.add_header("Require", "timer");
            }
            if let Err(e) = self.send_server_response(&ok, remote_addr).await {
                warn!(call_id = %call_id, "Failed to send 200 OK for UPDATE refresh: {}", e);
            } else {
                info!(call_id = %call_id, "Answered in-dialog UPDATE (session refresh)");
            }
        }
    }

    /// Handle an incoming CANCEL that terminated a pending INVITE (issue #55).
    ///
    /// The stack's transaction layer has already answered on the wire (200 OK
    /// to the CANCEL, 487 Request Terminated to the INVITE, RFC 3261 §9.2);
    /// this aborts the application side: soft-hangup the channel so the
    /// running dialplan (Wait(), Echo(), ...) unwinds, and drop the call
    /// state so a later Answer() can no longer emit a 200 OK for a call the
    /// caller already abandoned. Mirrors the teardown in `handle_bye`; the
    /// spawned per-call cleanup task finalizes the media plane once the call
    /// state disappears.
    pub async fn handle_cancel(&self, request: &SipMessage, _remote_addr: SocketAddr) {
        let Some(call_id) = request.call_id() else {
            return;
        };
        let call_id = call_id.to_string();

        // A CANCEL has no effect on an already-answered call (RFC 3261
        // §9.2). The transaction layer suppresses the bogus 487 for this
        // case; guard here too so a late-delivered IncomingCancel can never
        // tear down an established session.
        {
            let cs_arc = {
                let states = self.call_states.read();
                states.get(&call_id).cloned()
            };
            if let Some(cs_arc) = cs_arc {
                let cs = cs_arc.lock().await;
                if cs.session.state == crate::session::SessionState::Established {
                    info!(call_id = %call_id, "Ignoring CANCEL for an established call");
                    return;
                }
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
            if let Some(driver) = self.channel_driver.get() {
                driver.remove_channel(&name);
            }
            info!(call_id = %call_id, channel = %name, "CANCEL aborted pending call");
        }

        self.callid_map.write().remove(&call_id);
        self.call_states.write().remove(&call_id);

        // Notify any SFU conferences that this SIP call ended.
        crate::notify_sip_hangup(&call_id);
    }

    /// Tear down a UAS call whose successful INVITE response was never ACKed.
    /// The per-call cleanup task observes the removed state and releases the
    /// store, driver/RTP, and NOTIFY registrations.
    pub fn handle_invite_ack_timeout(&self, call_id: &str) {
        let channel_name = self.callid_map.read().get(call_id).cloned();
        if let Some(name) = channel_name {
            if let Some(channel) = store::find_by_name(&name) {
                channel.lock().softhangup(softhangup::AST_SOFTHANGUP_DEV);
            }
            info!(call_id, channel = %name, "INVITE 2xx ACK timeout tore down call");
        }
        self.callid_map.write().remove(call_id);
        self.call_states.write().remove(call_id);
        crate::notify_sip_hangup(call_id);
    }

    /// Apply abnormal outbound transaction timeouts to the owning call. Timer
    /// B fails an unanswered outbound INVITE; Timer F finishes a BYE whose
    /// final response was lost. Normal D/I/J/K expiry never reaches this path.
    pub fn handle_client_transaction_timeout(
        &self,
        call_id: &str,
        method: crate::parser::SipMethod,
    ) {
        let channel_name = self.callid_map.read().get(call_id).cloned();
        match method {
            crate::parser::SipMethod::Invite => {
                if let Some(ref name) = channel_name {
                    if let Some(channel) = store::find_by_name(name) {
                        let mut channel = channel.lock();
                        channel.hangup_cause = HangupCause::NoAnswer;
                        channel.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                    }
                    self.release_outbound_leg(name);
                } else {
                    self.callid_map.write().remove(call_id);
                    self.call_states.write().remove(call_id);
                }
                info!(call_id, "Timer B tore down unanswered outbound INVITE");
            }
            crate::parser::SipMethod::Bye => {
                self.callid_map.write().remove(call_id);
                self.call_states.write().remove(call_id);
                info!(call_id, "Timer F reaped outbound BYE signaling state");
            }
            _ => {}
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
        let pjsip_config = self.pjsip_config.clone();

        if source_acl_endpoint(pjsip_config.as_deref(), remote_addr.ip()).is_err() {
            warn!(
                call_id = %call_id,
                source = %remote_addr,
                "Rejected REGISTER from source outside configured identify CIDRs"
            );
            if let Ok(resp) = request.create_response(403, "Forbidden") {
                let _ = self.send_server_response(&resp, remote_addr).await;
            }
            return;
        }

        // Enforce digest auth if any endpoint has credentials configured.
        if let Some(cfg) = pjsip_config {
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
                    if let Err(e) = self.send_server_response(&challenge, remote_addr).await {
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
                        let _ = self.send_server_response(&resp, remote_addr).await;
                    }
                    return;
                }
            }
        }

        // Authenticated (or no auth required): perform the registration.
        let response = self.registrar.handle_register(request);
        let status = response.status_code().unwrap_or(0);
        if let Err(e) = self.send_server_response(&response, remote_addr).await {
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
            if let Err(e) = self.transport.send(&reinvite, cs.next_hop).await {
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

        if !self.response_matches_dialog(response, remote_addr).await {
            warn!(call_id = %call_id, source = %remote_addr,
                "Ignored re-INVITE response that did not match the live dialog");
            return;
        }

        let cs_arc = {
            let states = self.call_states.read();
            match states.get(&call_id) {
                Some(cs) => cs.clone(),
                None => return,
            }
        };

        let mut cs = cs_arc.lock().await;

        // A 2xx to a locally initiated re-INVITE is itself a target refresh
        // (RFC 3261 §12.2): the response Contact is the peer's current remote
        // target. Apply it to the dialog remote target AND move the local
        // in-dialog next hop to the resolved Contact, mirroring the re-INVITE
        // REQUEST path (`handle_reinvite_request`) and the UPDATE path
        // (`handle_update`). Without this a later BYE routes to the stale INVITE
        // source port even though the 200 advertised a new Contact (M5 review
        // MAJOR-F1). IP-literal only, via `remote_target_addr()`, so an
        // unresolvable Contact keeps the prior working next hop rather than
        // losing routing. This applies to BOTH inbound and outbound calls, so it
        // runs before the outbound ACK short-circuit below.
        if let Some(contact) = response
            .get_header(crate::parser::header_names::CONTACT)
            .and_then(crate::parser::extract_uri)
        {
            cs.session.update_remote_target(&contact);
            if let Some(addr) = cs.session.remote_target_addr() {
                cs.next_hop = addr;
            }
        }

        // Outbound calls: handle_response drives their ACK, so we only refresh
        // the target above and return here. (That ACK's wire-correctness for a
        // re-INVITE — build_ack hard-codes `CSeq: 1 ACK` and the initial R-URI —
        // is the M7 M-f lifecycle item, out of this carryover's scope; the
        // target refresh itself now applies to outbound too.)
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

        if let Err(reason) = self
            .validate_in_dialog_request(request, remote_addr)
            .await
        {
            self.reject_in_dialog_request(request, remote_addr, reason).await;
            return None;
        }

        // A re-INVITE is a target-refresh request too (RFC 3261 §12.2): apply
        // its Contact to the dialog remote target and move the local-request
        // next hop to the resolved Contact, mirroring the UPDATE path so a
        // later BYE reaches the refreshed target (M5 review MAJOR-3b).
        if let Some(contact) = request
            .get_header(crate::parser::header_names::CONTACT)
            .and_then(crate::parser::extract_uri)
        {
            let mut cs = cs_arc.lock().await;
            cs.session.update_remote_target(&contact);
            if let Some(addr) = cs.session.remote_target_addr() {
                cs.next_hop = addr;
            }
        }

        // Parse the re-INVITE's SDP offer
        let remote_sdp = session.remote_sdp.clone();

        // Check if this is a hold: a=sendonly / a=inactive, or the classic
        // c=0.0.0.0 hold (RFC 2543 / 3264). A hold re-INVITE keeps the dialog
        // but must pause the media pump so the bridged far side hears silence.
        let is_hold = if let Some(ref sdp) = remote_sdp {
            let sdp_str = sdp.to_string();
            sdp_str.contains("a=sendonly")
                || sdp_str.contains("a=inactive")
                || sdp_str.contains("c=IN IP4 0.0.0.0")
                || sdp_str.contains("c=IN IP6 ::")
        } else {
            false
        };

        // Advertise the media plane's REAL bound RTP port in the answer, not a
        // placeholder. Binding a re-INVITE answer to a bogus port (the old
        // hardcoded 10000) breaks audio after every hold/unhold/renegotiation
        // for peers that honor the answer SDP -- the same defect #8 fixed for
        // the initial INVITE, which had been left unfixed on this path.
        let channel_name = { cs_arc.lock().await.channel_name.clone() };
        let channel_codecs = self
            .channel_driver
            .get()
            .and_then(|driver| driver.channel_codecs(&channel_name))
            .unwrap_or_else(|| self.supported_codecs.clone());
        if let Some(ref offer) = remote_sdp {
            let negotiated = crate::sdp_rtp::negotiated_audio_payload_type(
                offer,
                &channel_codecs,
            );
            if !matches!(negotiated, Some(0 | 8)) {
                let response = request
                    .create_response(488, "Not Acceptable Here")
                    .ok()?;
                if self.may_send_invite_final(request, &response) {
                    let _ = self.transport.send(&response, remote_addr).await;
                    warn!(
                        call_id = %call_id,
                        ?negotiated,
                        "Rejected re-INVITE outside the channel's pinned G.711 policy"
                    );
                }
                return None;
            }
        }

        // ACTUALLY renegotiate the media plane (the load-bearing half of the
        // re-INVITE — previously this handler answered but never touched the
        // media session, so a codec/port change or hold was a sent-claim only):
        //   * hold offer  -> pause the pump (far side hears silence), keep the
        //                    existing remote so un-hold can resume.
        //   * media offer -> un-hold and re-install the remote address +
        //                    payload types from the offer, so the pump sends to
        //                    the new endpoint and accepts the new codec.
        if let (Some(offer), Some(driver)) = (remote_sdp.as_ref(), self.channel_driver.get()) {
            if is_hold {
                driver.set_channel_hold(&channel_name, true);
            } else {
                driver.set_channel_hold(&channel_name, false);
                if let Err(error) = driver.apply_inbound_offer(&channel_name, offer).await {
                    warn!(call_id = %call_id, %error, "re-INVITE media renegotiation failed");
                    let response = request.create_response(488, "Not Acceptable Here").ok()?;
                    if self.may_send_invite_final(request, &response) {
                        let _ = self.transport.send(&response, remote_addr).await;
                    }
                    return None;
                }
            }
        }

        let media_port = match self.channel_driver.get() {
            Some(driver) => driver
                .channel_rtp_local_port(&channel_name)
                .await
                .unwrap_or(10000),
            None => 10000,
        };

        // Generate SDP answer with a concrete, routable connection address
        // (external_media_address / routed interface — never 0.0.0.0,
        // issue #56). Route/NAT selection targets the re-INVITE's media
        // endpoint, falling back to the signaling source.
        let answer_sdp = if let Some(ref offer) = remote_sdp {
            let media_peer = crate::sdp_rtp::remote_rtp_endpoint(offer)
                .unwrap_or(remote_addr);
            // Fail closed (CP3): reject the re-INVITE (488) if a configured
            // external_media_address FQDN does not resolve, rather than answer
            // with a bogus/internal media address.
            let Some(local_ip) = crate::sdp::advertised_media_ip(session.local_addr, media_peer)
            else {
                warn!(call_id = %call_id, "Fail-closed: external_media_address did not resolve; rejecting re-INVITE 488");
                let response = request.create_response(488, "Not Acceptable Here").ok()?;
                if self.may_send_invite_final(request, &response) {
                    let _ = self.transport.send(&response, remote_addr).await;
                }
                return None;
            };
            let answer = SessionDescription::create_answer(
                offer,
                &local_ip,
                media_port,
                &channel_codecs,
            );
            Some(answer)
        } else {
            // No SDP in re-INVITE; use the existing local SDP
            let cs = cs_arc.lock().await;
            cs.session.local_sdp.clone()
        };

        // Build 200 OK response
        let mut ok_resp = request.create_response(200, "OK").ok()?;

        // Add Contact header (NAT-scoped toward the peer — New-3).
        ok_resp.add_header(
            "Contact",
            &format!(
                "<sip:asterisk@{}>",
                crate::sdp::advertised_signaling_hostport(session.local_addr, remote_addr)
            ),
        );

        // Add SDP body
        if let Some(ref sdp) = answer_sdp {
            let sdp_str = sdp.to_string();
            ok_resp.add_header("Content-Type", "application/sdp");
            ok_resp.add_header("Content-Length", &sdp_str.len().to_string());
            ok_resp.body = sdp_str;
        }

        // Send 200 OK (recorded in the re-INVITE's own server transaction,
        // so a CANCEL racing this answer is resolved atomically).
        if !self.may_send_invite_final(request, &ok_resp) {
            info!(call_id = %call_id, "Suppressing re-INVITE 200 OK: transaction already completed");
            return None;
        }
        if let Err(e) = self.transport.send(&ok_resp, remote_addr).await {
            warn!(call_id = %call_id, "Failed to send 200 OK for re-INVITE: {}", e);
            return None;
        }
        debug!(call_id = %call_id, "Sent 200 OK for re-INVITE");

        // Emit Hold/Unhold AMI event
        let _channel_name = {
            let cs = cs_arc.lock().await;
            cs.channel_name.clone()
        };
        if is_hold {
            debug!(channel = %_channel_name, "Hold detected");
            // Find the bridged peer channel and emit DeviceStateChange for its endpoint
            if let Some(store_chan) = asterisk_core::channel_store::find_by_name(&_channel_name) {
                let ch = store_chan.lock();
                if let Some(peer_name) = ch.variables.get("BRIDGEPEER") {
                    // Extract device name from peer channel name (PJSIP/bob-00000001 → PJSIP/bob)
                    let device = peer_name.rsplit_once('-')
                        .map(|(prefix, _)| prefix.to_string())
                        .unwrap_or_else(|| peer_name.clone());
                    debug!(device = %device, "Emitting DeviceStateChange ONHOLD");
                    asterisk_core::channel::publish_channel_event("DeviceStateChange", &[
                        ("Device", &device),
                        ("State", "ONHOLD"),
                    ]);
                }
            }
        }

        // Persist the renegotiated SDP so the stored session reflects the new
        // offer/answer (later in-dialog requests, SFU, and a subsequent
        // renegotiation all read from here). Previously the stored SDP kept the
        // ORIGINAL INVITE's media even after a hold/unhold/codec change.
        {
            let mut cs = cs_arc.lock().await;
            if let Some(offer) = remote_sdp {
                cs.session.remote_sdp = Some(offer);
            }
            if let Some(answer) = answer_sdp {
                cs.session.local_sdp = Some(answer);
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
                    if let Err(e) = self.send_client_request(bye, cs.next_hop).await {
                        warn!(call_id = %call_id, "Failed to send BYE for {}: {}", channel_name, e);
                    } else {
                        debug!(channel = %channel_name, call_id = %call_id, "Sent BYE");
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

    /// Finish a direct outbound Originate while retaining only the signaling
    /// state needed to consume the BYE final response.
    ///
    /// The event-handler session is authoritative for dialog CSeq. Building
    /// the BYE here keeps response validation aligned, while media, store, and
    /// NOTIFY resources are released immediately. A lost final is bounded by
    /// Timer F instead of stranding the two signaling maps indefinitely.
    pub async fn finish_outbound_originate(&self, channel_name: &str) {
        let call_id = {
            let map = self.callid_map.read();
            map.iter().find(|(_, name)| name.as_str() == channel_name)
                .map(|(call_id, _)| call_id.clone())
        };
        let Some(call_id) = call_id else {
            self.release_outbound_leg(channel_name);
            return;
        };
        let call_state = {
            let states = self.call_states.read();
            states.get(&call_id).cloned()
        };
        let Some(call_state) = call_state else {
            self.release_outbound_leg(channel_name);
            return;
        };

        let sent_bye = {
            let mut state = call_state.lock().await;
            match state.session.build_bye() {
                Some(bye) => {
                    if let Err(error) = self.send_client_request(bye, state.next_hop).await {
                        warn!(call_id = %call_id, channel = channel_name, %error,
                            "Failed to send BYE for completed Originate");
                    }
                    true
                }
                None => false,
            }
        };

        if !sent_bye {
            self.release_outbound_leg(channel_name);
            return;
        }

        self.release_outbound_media(channel_name);
        self.schedule_outbound_bye_signaling_reap(call_id, call_state);
    }

    fn schedule_outbound_bye_signaling_reap(
        &self,
        call_id: String,
        call_state: Arc<tokio::sync::Mutex<CallState>>,
    ) {
        let callid_map = self.callid_map.clone();
        let call_states = self.call_states.clone();
        tokio::spawn(async move {
            tokio::time::sleep(OUTBOUND_BYE_SIGNALING_GRACE).await;
            let still_current = call_states
                .read()
                .get(&call_id)
                .is_some_and(|current| Arc::ptr_eq(current, &call_state));
            if still_current {
                call_states.write().remove(&call_id);
                callid_map.write().remove(&call_id);
                debug!(call_id = %call_id,
                    "Reaped outbound BYE signaling state after Timer F");
            }
        });
    }

    /// Put the correct SIP request on the wire for an abandoned outbound Dial
    /// leg, then release its local resources.
    ///
    /// A pending or early INVITE is cancelled in-transaction. An established
    /// dialog is terminated with BYE. A leg that already received a final
    /// failure needs no additional request.
    pub async fn cancel_or_bye_outbound_leg(&self, channel_name: &str) {
        let call_id = {
            let map = self.callid_map.read();
            map.iter().find(|(_, name)| name.as_str() == channel_name)
                .map(|(call_id, _)| call_id.clone())
        };
        let mut preserve_signaling = None;
        let mut released = false;
        if let Some(ref call_id) = call_id {
            let cs_arc = {
                let states = self.call_states.read();
                states.get(call_id).cloned()
            };
            if let Some(cs_arc) = cs_arc {
                let mut cs = cs_arc.lock().await;
                let pending = matches!(
                    cs.session.state,
                    crate::session::SessionState::Initiated
                        | crate::session::SessionState::Early
                );
                cs.abandoned = true;
                let request = match cs.session.state {
                    crate::session::SessionState::Initiated
                    | crate::session::SessionState::Early => cs.session.build_cancel(),
                    crate::session::SessionState::Established => cs.session.build_bye(),
                    crate::session::SessionState::Terminating
                    | crate::session::SessionState::Terminated => None,
                };
                if let Some(request) = request {
                    let method = request.method();
                    if let Err(error) = self.send_client_request(request, cs.next_hop).await {
                        warn!(call_id = %call_id, channel = channel_name,
                            ?method, %error, "Failed to signal abandoned Dial leg");
                    } else {
                        info!(call_id = %call_id, channel = channel_name,
                            ?method, "Signaled abandoned Dial leg");
                    }
                }
                if pending {
                    preserve_signaling = Some((call_id.clone(), cs_arc.clone()));
                } else {
                    // Keep the per-call mutex held through final release so
                    // a response that already cloned this state can only run
                    // afterward and will observe `abandoned = true`.
                    self.release_outbound_leg(channel_name);
                    released = true;
                }
            }
        }
        if let Some((call_id, call_state)) = preserve_signaling {
            self.release_outbound_media(channel_name);
            self.schedule_abandoned_signaling_reap(call_id, call_state);
        } else if !released {
            self.release_outbound_leg(channel_name);
        }
    }

    fn schedule_abandoned_signaling_reap(
        &self,
        call_id: String,
        call_state: Arc<tokio::sync::Mutex<CallState>>,
    ) {
        let callid_map = self.callid_map.clone();
        let call_states = self.call_states.clone();
        tokio::spawn(async move {
            // A final response may legitimately arrive until the INVITE client
            // transaction's Timer B expires. Keep just the routing tombstone
            // for that full window so a late crossing 2xx still gets ACK+BYE.
            tokio::time::sleep(ABANDONED_SIGNALING_GRACE).await;
            let still_current = call_states
                .read()
                .get(&call_id)
                .is_some_and(|current| Arc::ptr_eq(current, &call_state));
            if still_current {
                call_states.write().remove(&call_id);
                callid_map.write().remove(&call_id);
                debug!(call_id = %call_id,
                    "Reaped abandoned INVITE after final-response grace period");
            }
        });
    }

    fn release_outbound_media(&self, channel_name: &str) {
        crate::notify_service::global_notify_service().unregister_channel(channel_name);
        if let Some(driver) = self.channel_driver.get() {
            driver.remove_channel(channel_name);
        }
        if let Some(channel) = store::find_by_name(channel_name) {
            let unique_id = channel.lock().unique_id.0.clone();
            store::deregister(&unique_id);
        }
    }

    /// Release an outbound leg's local resources — its global store channel,
    /// driver channel-map entry (and thus the bound RTP socket), NOTIFY
    /// registration, and Call-ID/state bookkeeping — WITHOUT sending SIP.
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
        self.release_outbound_media(channel_name);
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

    /// Snapshot the resource owners that must return to baseline after a call.
    pub fn resource_counts(&self) -> SipResourceCounts {
        let transaction_counts = self.stack.get()
            .map(|stack| stack.transaction_counts())
            .unwrap_or_default();
        SipResourceCounts {
            driver_channels: self.channel_driver.get()
                .map(|driver| driver.active_channel_count())
                .unwrap_or(0),
            call_id_mappings: self.callid_map.read().len(),
            call_states: self.call_states.read().len(),
            notify_channels: crate::notify_service::global_notify_service().active_channel_count(),
            invite_client_transactions: transaction_counts.invite_client,
            invite_server_transactions: transaction_counts.invite_server,
            non_invite_client_transactions: transaction_counts.non_invite_client,
            non_invite_server_transactions: transaction_counts.non_invite_server,
            rtp_sessions: crate::rtp::active_rtp_sessions(),
            registrar_bindings: self.registrar.total_bindings(),
            hangup_callbacks: asterisk_core::channel::registered_hangup_callbacks(),
            answer_callbacks: asterisk_core::channel::registered_answer_callbacks(),
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceAclDenied;

/// Apply configured `type=identify` IP/CIDR rules as a source allowlist.
///
/// A deployment that configures at least one `match=` entry has explicitly
/// enabled source identification, so no match is a denial. Configurations
/// without any IP match entries retain authenticated endpoint behavior; a
/// `match_header` entry alone is not an IP source ACL.
fn source_acl_endpoint(
    config: Option<&crate::pjsip_config::PjsipConfig>,
    source_ip: std::net::IpAddr,
) -> Result<Option<String>, SourceAclDenied> {
    let Some(config) = config else {
        return Ok(None);
    };
    let acl_configured = config
        .identifies
        .iter()
        .any(|identify| !identify.matches.is_empty());
    if !acl_configured {
        return Ok(None);
    }

    config
        .identify_endpoint_by_ip(&source_ip.to_string())
        .map(|endpoint| Some(endpoint.to_string()))
        .ok_or(SourceAclDenied)
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
            // '>' before '<' yields start > end; slicing the reversed range
            // `header[start + 1..end]` panics ("byte range starts at N but
            // ends at M"), a remotely triggerable crash on a malformed
            // From/To header. Only slice when the brackets are ordered.
            if start < end {
                &header[start + 1..end]
            } else {
                header
            }
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

/// Allowlist filter for the trace-level INVITE header dump (issue #129).
///
/// `Authorization`/`Proxy-Authorization` carry digest credential material,
/// `From`/`To`/`Contact`/`P-Asserted-Identity`/`P-Preferred-Identity`/
/// `Remote-Party-ID`/... carry caller PII, and an unknown or custom header
/// can carry anything — so the dump is default-deny: only structurally
/// boring, non-identity headers print their raw value; every other header
/// prints its NAME with a redacted placeholder. Full traffic inspection
/// belongs to the explicitly-enabled SIP logger (`logger.rs`), never to an
/// always-on debug trace.
fn redact_invite_header_value<'a>(name: &str, value: &'a str) -> &'a str {
    const VALUE_SAFE_HEADERS: &[&str] = &[
        "accept",
        "allow",
        "call-id",
        "content-length",
        "content-type",
        "cseq",
        "expires",
        "max-forwards",
        "min-se",
        "require",
        "session-expires",
        "supported",
        "user-agent",
        "via",
    ];
    if VALUE_SAFE_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        value
    } else {
        "<redacted>"
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

    #[derive(Debug)]
    struct RecordingTransport {
        local_addr: SocketAddr,
        sent: std::sync::Mutex<Vec<SipMessage>>,
    }

    impl RecordingTransport {
        fn new() -> Self {
            Self {
                local_addr: "127.0.0.1:5060".parse().unwrap(),
                sent: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn statuses(&self) -> Vec<u16> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(SipMessage::status_code)
                .collect()
        }

        fn methods(&self) -> Vec<crate::parser::SipMethod> {
            self.sent.lock().unwrap().iter()
                .filter_map(SipMessage::method).collect()
        }
    }

    #[derive(Debug)]
    struct BlockingCancelTransport {
        local_addr: SocketAddr,
        sent: std::sync::Mutex<Vec<SipMessage>>,
        cancel_started: tokio::sync::Notify,
        release_cancel: tokio::sync::Notify,
    }

    impl BlockingCancelTransport {
        fn new() -> Self {
            Self {
                local_addr: "127.0.0.1:5060".parse().unwrap(),
                sent: std::sync::Mutex::new(Vec::new()),
                cancel_started: tokio::sync::Notify::new(),
                release_cancel: tokio::sync::Notify::new(),
            }
        }

        fn methods(&self) -> Vec<crate::parser::SipMethod> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(SipMessage::method)
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl SipTransport for RecordingTransport {
        async fn send(
            &self,
            msg: &SipMessage,
            _addr: SocketAddr,
        ) -> Result<(), crate::transport::TransportError> {
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        fn local_addr(&self) -> Result<SocketAddr, crate::transport::TransportError> {
            Ok(self.local_addr)
        }

        fn protocol(&self) -> &str {
            "UDP"
        }
    }

    #[async_trait::async_trait]
    impl SipTransport for BlockingCancelTransport {
        async fn send(
            &self,
            msg: &SipMessage,
            _addr: SocketAddr,
        ) -> Result<(), crate::transport::TransportError> {
            self.sent.lock().unwrap().push(msg.clone());
            if msg.method() == Some(crate::parser::SipMethod::Cancel) {
                self.cancel_started.notify_one();
                self.release_cancel.notified().await;
            }
            Ok(())
        }

        fn local_addr(&self) -> Result<SocketAddr, crate::transport::TransportError> {
            Ok(self.local_addr)
        }

        fn protocol(&self) -> &str {
            "UDP"
        }
    }

    fn source_acl_test_config() -> crate::pjsip_config::PjsipConfig {
        use crate::pjsip_config::{
            AuthConfig, EndpointConfig, IdentifyConfig, PjsipConfig,
        };

        PjsipConfig {
            endpoints: vec![EndpointConfig {
                name: "carrier".to_string(),
                auth: Some("carrier-auth".to_string()),
                ..Default::default()
            }],
            auths: vec![AuthConfig {
                name: "carrier-auth".to_string(),
                username: "carrier".to_string(),
                password: "secret".to_string(),
                ..Default::default()
            }],
            identifies: vec![IdentifyConfig {
                name: "carrier-identify".to_string(),
                endpoint: "carrier".to_string(),
                matches: vec!["192.0.2.0/24".to_string()],
                match_header: None,
            }],
            ..Default::default()
        }
    }

    fn source_acl_test_invite(call_id: &str) -> SipMessage {
        let wire = format!(
            "INVITE sip:100@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP client.example.com:5060;branch=z9hG4bK-acl\r\n\
             From: <sip:caller@example.net>;tag=acl-test\r\n\
             To: <sip:100@example.com>\r\n\
             Call-ID: {}\r\n\
             CSeq: 1 INVITE\r\n\
             Content-Length: 0\r\n\r\n",
            call_id
        );
        SipMessage::parse(wire.as_bytes()).unwrap()
    }

    fn source_acl_test_register(call_id: &str) -> SipMessage {
        let wire = format!(
            "REGISTER sip:example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP client.example.com:5060;branch=z9hG4bK-register-acl\r\n\
             From: <sip:carrier@example.com>;tag=register-acl\r\n\
             To: <sip:carrier@example.com>\r\n\
             Call-ID: {}\r\n\
             CSeq: 1 REGISTER\r\n\
             Contact: <sip:carrier@client.example.com>\r\n\
             Content-Length: 0\r\n\r\n",
            call_id
        );
        SipMessage::parse(wire.as_bytes()).unwrap()
    }

    #[tokio::test]
    async fn inbound_rtp_exhaustion_returns_503_without_live_call_state() {
        use asterisk_core::pbx::{Context, Extension, Priority};

        let occupied = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rtp_port = occupied.local_addr().unwrap().port();
        let range = crate::rtp::RtpPortRange::new(rtp_port, rtp_port).unwrap();

        let mut dialplan = Dialplan::new();
        let mut context = Context::new("default");
        let mut extension = Extension::new("100");
        extension.add_priority(Priority {
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });
        context.add_extension(extension);
        dialplan.add_context(context);

        let transport = Arc::new(RecordingTransport::new());
        let driver = Arc::new(SipChannelDriver::with_rtp_port_range(
            transport.local_addr,
            range,
        ));
        driver.set_transport(transport.clone());
        let handler = SipEventHandler::new(Arc::new(dialplan), transport.clone());
        handler.set_channel_driver(driver.clone());

        let sdp = SessionDescription::create_offer(
            "127.0.0.1",
            40000,
            &[codecs::pcmu()],
        )
        .to_string();
        let wire = format!(
            "INVITE sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:5090;branch=z9hG4bK-rtp-full\r\n\
             From: <sip:caller@127.0.0.1>;tag=rtp-full\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: rtp-range-exhausted\r\n\
             CSeq: 1 INVITE\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: {}\r\n\r\n{}",
            sdp.len(), sdp
        );
        let invite = SipMessage::parse(wire.as_bytes()).unwrap();
        let remote_addr = "127.0.0.1:5090".parse().unwrap();
        let session = SipSession::new_inbound(&invite, transport.local_addr, remote_addr).unwrap();

        let accepted = handler
            .handle_incoming_invite(&invite, remote_addr, session)
            .await;

        assert!(accepted.is_none());
        assert_eq!(transport.statuses(), vec![100, 503]);
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(driver.active_channel_count(), 0);
    }

    #[tokio::test]
    async fn source_acl_rejects_before_auth_and_allows_configured_cidr() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new_with_pjsip_config(
            Arc::new(Dialplan::new()),
            transport.clone(),
            Some(Arc::new(source_acl_test_config())),
        );

        let denied_addr = "198.51.100.10:5060".parse().unwrap();
        let denied_invite = source_acl_test_invite("source-acl-denied");
        let denied_session = SipSession::new_outbound(transport.local_addr, denied_addr);
        let denied = handler
            .handle_incoming_invite(&denied_invite, denied_addr, denied_session)
            .await;

        assert!(denied.is_none());
        assert_eq!(transport.statuses(), vec![403]);
        assert_eq!(handler.active_calls(), 0);

        let allowed_addr = "192.0.2.42:5060".parse().unwrap();
        let allowed_invite = source_acl_test_invite("source-acl-allowed");
        let allowed_session = SipSession::new_outbound(transport.local_addr, allowed_addr);
        let allowed = handler
            .handle_incoming_invite(&allowed_invite, allowed_addr, allowed_session)
            .await;

        assert!(allowed.is_none());
        assert_eq!(transport.statuses(), vec![403, 401]);
        assert_eq!(handler.active_calls(), 0);

        let denied_register = source_acl_test_register("source-acl-register-denied");
        handler.handle_register(&denied_register, denied_addr).await;

        assert_eq!(transport.statuses(), vec![403, 401, 403]);
    }

    #[test]
    fn source_acl_is_inactive_without_ip_match_rules() {
        let config = crate::pjsip_config::PjsipConfig::default();
        let source = "203.0.113.10".parse().unwrap();

        assert_eq!(source_acl_endpoint(Some(&config), source), Ok(None));
        assert_eq!(source_acl_endpoint(None, source), Ok(None));
    }

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

    /// Regression: a From/To header where '>' precedes '<' must NOT panic.
    /// `header[start + 1..end]` used to abort with "byte range starts at N but
    /// ends at M" on the reversed range — a remote DoS, since this runs on the
    /// From/To of every incoming request in the main SIP event loop.
    #[test]
    fn test_extract_user_from_header_reversed_brackets_does_not_panic() {
        // '>' at index 0, '<' at index 6: reversed range if sliced blindly.
        assert_eq!(
            extract_user_from_header(">sip:x<"),
            Some(">sip:x<".to_string())
        );
        // Well-formed brackets still extract the user part.
        assert_eq!(
            extract_user_from_header(r#""Alice" <sip:alice@atlanta.example.com>"#),
            Some("alice".to_string())
        );
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
        let ep = crate::sdp_rtp::remote_rtp_endpoint(&offer)
            .expect("offer has an audio endpoint");
        assert_eq!(ep.ip().to_string(), "203.0.113.7");
        assert_eq!(ep.port(), 40000);
    }

    #[test]
    fn test_remote_rtp_endpoint_rejected_stream_is_none() {
        // A media stream with port 0 (rejected/held) has no live endpoint.
        let mut offer = SessionDescription::create_offer("203.0.113.7", 0, &[codecs::pcmu()]);
        offer.media_descriptions[0].port = 0;
        assert!(crate::sdp_rtp::remote_rtp_endpoint(&offer).is_none());
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
    async fn every_bye_final_removes_call_maps_without_waiting_for_timer_f() {
        let (_driver, handler) = driver_and_handler().await;
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        for (index, status, reason) in [
            (1, 200, "OK"),
            (2, 408, "Request Timeout"),
            (3, 481, "Call/Transaction Does Not Exist"),
        ] {
            let channel_name = format!("PJSIP/bye-final-{index:08}");
            let mut session = SipSession::new_outbound(
                "127.0.0.1:5061".parse().unwrap(), remote,
            );
            let invite = session.build_invite("sip:peer@127.0.0.1:5060");
            let mut answer = invite.create_response(200, "OK").unwrap();
            answer
                .headers
                .iter_mut()
                .find(|header| header.name.eq_ignore_ascii_case("To"))
                .unwrap()
                .value
                .push_str(";tag=remote");
            answer.add_header("Contact", "<sip:peer@127.0.0.1:5060>");
            session.on_response(&answer);
            let call_id = session.call_id.clone();
            let bye = session.build_bye().unwrap();
            let response = bye.create_response(status, reason).unwrap();
            handler.register_outbound_callid(&call_id, &channel_name);
            handler.register_outbound_session(
                &call_id, &channel_name, session, remote,
            );
            assert_eq!(handler.active_calls(), 1);
            assert_eq!(handler.call_states.read().len(), 1);
            handler.handle_response(&response, remote).await;

            assert_eq!(handler.active_calls(), 0,
                "Call-ID map must clear immediately for BYE status {status}");
            assert_eq!(handler.call_states.read().len(), 0,
                "call state must clear immediately for BYE status {status}");
        }
    }

    #[tokio::test]
    async fn direct_originate_bye_has_bounded_signaling_fallback() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        );
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = SipSession::new_outbound(transport.local_addr, remote);
        let invite = session.build_invite("sip:peer@127.0.0.1:5060");
        let mut answer = invite.create_response(200, "OK").unwrap();
        answer
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=remote");
        answer.add_header("Contact", "<sip:peer@127.0.0.1:5060>");
        session.on_response(&answer);
        let call_id = session.call_id.clone();
        let channel_name = "PJSIP/originate-bye-fallback";
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(
            &call_id, channel_name, session, remote,
        );

        handler.finish_outbound_originate(channel_name).await;
        assert_eq!(transport.methods(), vec![crate::parser::SipMethod::Bye]);
        assert_eq!(handler.active_calls(), 1,
            "only signaling routing is retained until a final or Timer F");
        assert_eq!(handler.call_states.read().len(), 1);

        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(handler.call_states.read().len(), 0);
    }

    fn outbound_session_ready_for_cancel(
        local: SocketAddr,
        remote: SocketAddr,
        state: crate::session::SessionState,
    ) -> SipSession {
        let mut session = SipSession::new_outbound(local, remote);
        session.build_invite_with_uri(
            "sip:peer@127.0.0.1:5060",
            "sip:peer@127.0.0.1:5060",
        );
        session.state = state;
        session
    }

    #[tokio::test]
    async fn abandoned_pending_and_early_legs_send_cancel_before_release() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        );
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();

        for (index, state) in [
            crate::session::SessionState::Initiated,
            crate::session::SessionState::Early,
        ].into_iter().enumerate() {
            let session = outbound_session_ready_for_cancel(
                transport.local_addr, remote, state,
            );
            let failure = session
                .invite
                .as_ref()
                .unwrap()
                .create_response(487, "Request Terminated")
                .unwrap();
            let call_id = session.call_id.clone();
            let channel_name = format!("PJSIP/cancel-{index}");
            handler.register_outbound_callid(&call_id, &channel_name);
            handler.register_outbound_session(
                &call_id, &channel_name, session, remote,
            );
            handler.cancel_or_bye_outbound_leg(&channel_name).await;
            handler.handle_response(&failure, remote).await;
        }

        assert_eq!(transport.methods(), vec![
            crate::parser::SipMethod::Cancel,
            crate::parser::SipMethod::Cancel,
        ]);
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(handler.call_states.read().len(), 0);
    }

    #[tokio::test]
    async fn cancel_200_crossing_is_serialized_as_cancel_ack_bye() {
        let transport = Arc::new(BlockingCancelTransport::new());
        let handler = Arc::new(SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        ));
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let session = outbound_session_ready_for_cancel(
            transport.local_addr,
            remote,
            crate::session::SessionState::Early,
        );
        let invite = session.invite.clone().unwrap();
        let mut answer = invite.create_response(200, "OK").unwrap();
        answer
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=peer-tag");
        answer.add_header("Contact", "<sip:peer@127.0.0.1:5060>");

        let call_id = session.call_id.clone();
        let channel_name = "PJSIP/cancel-200-race";
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(&call_id, channel_name, session, remote);

        let abandon = {
            let handler = handler.clone();
            tokio::spawn(async move {
                handler.cancel_or_bye_outbound_leg(channel_name).await;
            })
        };
        transport.cancel_started.notified().await;

        let response = {
            let handler = handler.clone();
            tokio::spawn(async move {
                handler.handle_response(&answer, remote).await;
            })
        };
        tokio::task::yield_now().await;
        transport.release_cancel.notify_one();
        abandon.await.unwrap();
        response.await.unwrap();

        assert_eq!(
            transport.methods(),
            vec![
                crate::parser::SipMethod::Cancel,
                crate::parser::SipMethod::Ack,
                crate::parser::SipMethod::Bye,
            ],
            "a crossing 2xx must be acknowledged before the abandoned dialog is terminated"
        );
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(handler.call_states.read().len(), 0);
    }

    #[tokio::test]
    async fn abandoned_invite_signaling_state_has_bounded_fallback_reap() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        );
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let session = outbound_session_ready_for_cancel(
            transport.local_addr,
            remote,
            crate::session::SessionState::Early,
        );
        let call_id = session.call_id.clone();
        let channel_name = "PJSIP/abandon-fallback";
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(&call_id, channel_name, session, remote);

        handler.cancel_or_bye_outbound_leg(channel_name).await;
        assert_eq!(handler.active_calls(), 1, "crossing-final grace must retain routing");
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(handler.call_states.read().len(), 0);
    }

    #[tokio::test]
    async fn abandoned_established_leg_sends_bye_before_release() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        );
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = outbound_session_ready_for_cancel(
            transport.local_addr,
            remote,
            crate::session::SessionState::Initiated,
        );
        let invite = session.invite.clone().unwrap();
        let mut answer = invite.create_response(200, "OK").unwrap();
        for header in &mut answer.headers {
            if header.name.eq_ignore_ascii_case(crate::parser::header_names::TO) {
                header.value.push_str(";tag=peer-tag");
            }
        }
        answer.headers.push(crate::parser::SipHeader {
            name: crate::parser::header_names::CONTACT.to_string(),
            value: "<sip:peer@127.0.0.1:5060>".to_string(),
        });
        session.on_response(&answer);

        let call_id = session.call_id.clone();
        let channel_name = "PJSIP/bye-established";
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(
            &call_id, channel_name, session, remote,
        );
        handler.cancel_or_bye_outbound_leg(channel_name).await;

        assert_eq!(transport.methods(), vec![crate::parser::SipMethod::Bye]);
        assert_eq!(handler.active_calls(), 0);
        assert_eq!(handler.call_states.read().len(), 0);
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
        let channel_name = ch.name.clone();
        store::register_existing_channel(ch);
        assert_eq!(driver.active_channel_count(), 1);
        assert!(store::find_by_name(&channel_name).is_some());

        handler.release_outbound_leg(&channel_name);

        assert_eq!(
            driver.active_channel_count(),
            0,
            "abandoned leg's RTP socket / driver entry must be released"
        );
        assert!(store::find_by_name(&channel_name).is_none(),
            "abandoned leg's global store channel must be deregistered");
    }

    #[tokio::test]
    async fn handle_bye_releases_outbound_driver_entry() {
        let (driver, handler) = driver_and_handler().await;

        let ch = driver
            .request("sip:dave@127.0.0.1:5060", None)
            .await
            .expect("outbound request");
        let channel_name = ch.name.clone();
        store::register_existing_channel(ch);
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = SipSession::new_outbound("127.0.0.1:5061".parse().unwrap(), remote);
        let invite = session.build_invite("sip:dave@127.0.0.1:5060");
        let mut answer = invite.create_response(200, "OK").unwrap();
        answer
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=remote-tag");
        answer.add_header("Contact", "<sip:dave@127.0.0.1:5060>");
        session.on_response(&answer);
        let call_id = session.call_id.clone();
        let local_tag = session.local_tag.clone();
        handler.register_outbound_callid(&call_id, &channel_name);
        handler.register_outbound_session(&call_id, &channel_name, session, remote);
        assert_eq!(driver.active_channel_count(), 1);
        assert!(store::find_by_name(&channel_name).is_some());

        // Remote sends a BYE for this call.
        let bye_raw = format!("BYE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKbye1\r\n\
             From: <sip:dave@127.0.0.1>;tag=remote-tag\r\n\
             To: <sip:asterisk@127.0.0.1>;tag={local_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 2 BYE\r\n\
             Content-Length: 0\r\n\r\n");
        let bye = SipMessage::parse(bye_raw.as_bytes()).unwrap();
        handler.handle_bye(&bye, remote).await;

        assert_eq!(
            driver.active_channel_count(),
            0,
            "remote BYE on an outbound leg must release its RTP socket / driver entry"
        );
        assert!(store::find_by_name(&channel_name).is_none(),
            "remote BYE must deregister the outbound global store channel");
    }

    #[tokio::test]
    async fn forged_in_dialog_byes_from_allowed_source_do_not_tear_down_call() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(Arc::new(Dialplan::new()), transport.clone());
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = SipSession::new_outbound(transport.local_addr, remote);
        let invite = session.build_invite("sip:peer@127.0.0.1:5060");
        let mut answer = invite.create_response(200, "OK").unwrap();
        answer
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=peer-tag");
        answer.add_header("Contact", "<sip:peer@127.0.0.1:5060>");
        session.on_response(&answer);
        session.dialog.as_mut().unwrap().remote_seq = Some(10);
        let call_id = session.call_id.clone();
        let local_tag = session.local_tag.clone();
        let channel_name = "PJSIP/dialog-forgery";
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(&call_id, channel_name, session, remote);

        let make_bye = |candidate_call_id: &str, remote_tag: &str, cseq: u32, branch: &str| {
            SipMessage::parse(
                format!(
                    "BYE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
                     Via: SIP/2.0/UDP 127.0.0.1:5060;branch={branch}\r\n\
                     From: <sip:peer@127.0.0.1>;tag={remote_tag}\r\n\
                     To: <sip:asterisk@127.0.0.1>;tag={local_tag}\r\n\
                     Call-ID: {candidate_call_id}\r\n\
                     CSeq: {cseq} BYE\r\n\
                     Content-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap()
        };

        for (forged, source) in [
            (make_bye("unknown-call", "peer-tag", 11, "z9hG4bKcall"), remote),
            (make_bye(&call_id, "wrong-tag", 11, "z9hG4bKtag"), remote),
            (make_bye(&call_id, "peer-tag", 10, "z9hG4bKcseq"), remote),
            (
                make_bye(&call_id, "peer-tag", 11, "z9hG4bKsource"),
                "127.0.0.1:5062".parse().unwrap(),
            ),
        ] {
            handler.handle_bye(&forged, source).await;
            assert_eq!(handler.active_calls(), 1);
            assert_eq!(handler.call_states.read().len(), 1);
        }
        assert_eq!(transport.statuses(), vec![481, 481, 481, 481]);

        handler.release_outbound_leg(channel_name);
    }

    #[tokio::test]
    async fn stray_finals_cannot_mutate_established_dialog() {
        let transport = Arc::new(RecordingTransport::new());
        let handler = SipEventHandler::new(Arc::new(Dialplan::new()), transport.clone());
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = SipSession::new_outbound(transport.local_addr, remote);
        let invite = session.build_invite("sip:peer@127.0.0.1:5060");
        let mut answer = invite.create_response(200, "OK").unwrap();
        answer
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=peer-tag");
        answer.add_header("Contact", "<sip:peer@127.0.0.1:5060>");
        session.on_response(&answer);
        let call_id = session.call_id.clone();
        let channel_name = "PJSIP/forged-final";
        let channel = store::register_existing_channel(
            asterisk_core::channel::Channel::new(channel_name),
        );
        channel.lock().set_state(ChannelState::Up);
        handler.register_outbound_callid(&call_id, channel_name);
        handler.register_outbound_session(&call_id, channel_name, session, remote);

        let mut failure = invite.create_response(486, "Busy Here").unwrap();
        failure
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value
            .push_str(";tag=peer-tag");

        let mut wrong_tag = failure.clone();
        wrong_tag
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("To"))
            .unwrap()
            .value = "<sip:peer@127.0.0.1:5060>;tag=forged".to_string();
        let mut wrong_cseq = failure.clone();
        wrong_cseq
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case("CSeq"))
            .unwrap()
            .value = "2 INVITE".to_string();

        handler
            .handle_response(&failure, "127.0.0.1:5062".parse().unwrap())
            .await;
        handler.handle_response(&wrong_tag, remote).await;
        handler.handle_response(&wrong_cseq, remote).await;

        assert!(!channel.lock().check_hangup());
        let state = handler.call_states.read().get(&call_id).cloned().unwrap();
        assert_eq!(
            state.lock().await.session.state,
            crate::session::SessionState::Established
        );

        handler.release_outbound_leg(channel_name);
    }

    #[tokio::test]
    async fn handle_cancel_releases_call_state_and_driver_entry() {
        let (driver, handler) = driver_and_handler().await;

        // A leg with registered call state, as an in-flight call would have.
        let ch = driver
            .request("sip:erin@127.0.0.1:5060", None)
            .await
            .expect("request");
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        handler.register_outbound_callid("cancel-call-1", &ch.name);
        handler.register_outbound_session(
            "cancel-call-1",
            &ch.name,
            SipSession::new_outbound("127.0.0.1:0".parse().unwrap(), remote),
            remote,
        );
        assert_eq!(driver.active_channel_count(), 1);

        let cancel_raw = "CANCEL sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKcxl1\r\n\
             From: <sip:erin@127.0.0.1>;tag=f1\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: cancel-call-1\r\n\
             CSeq: 1 CANCEL\r\n\
             Content-Length: 0\r\n\r\n";
        let cancel = SipMessage::parse(cancel_raw.as_bytes()).unwrap();
        handler.handle_cancel(&cancel, remote).await;

        assert_eq!(
            driver.active_channel_count(),
            0,
            "CANCEL must release the leg's RTP socket / driver entry"
        );
        assert_eq!(
            handler.active_calls(),
            0,
            "call state must be dropped so a later Answer() cannot send 200 OK"
        );
    }

    #[tokio::test]
    async fn handle_cancel_ignores_established_call() {
        let (driver, handler) = driver_and_handler().await;

        let ch = driver
            .request("sip:frank@127.0.0.1:5060", None)
            .await
            .expect("request");
        let remote: SocketAddr = "127.0.0.1:5060".parse().unwrap();
        let mut session = SipSession::new_outbound("127.0.0.1:0".parse().unwrap(), remote);
        // The call was answered: a CANCEL has no effect (RFC 3261 §9.2).
        session.state = crate::session::SessionState::Established;
        handler.register_outbound_callid("cancel-est-1", &ch.name);
        handler.register_outbound_session("cancel-est-1", &ch.name, session, remote);
        assert_eq!(driver.active_channel_count(), 1);

        let cancel_raw = "CANCEL sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKcxl2\r\n\
             From: <sip:frank@127.0.0.1>;tag=f1\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: cancel-est-1\r\n\
             CSeq: 1 CANCEL\r\n\
             Content-Length: 0\r\n\r\n";
        let cancel = SipMessage::parse(cancel_raw.as_bytes()).unwrap();
        handler.handle_cancel(&cancel, remote).await;

        assert_eq!(
            driver.active_channel_count(),
            1,
            "a CANCEL must not tear down an established call"
        );
        assert_eq!(handler.active_calls(), 1, "established call state must survive");
    }

    #[test]
    fn test_negotiated_payload_type_prefers_common_codec() {
        // Offer PCMA (8); our supported list includes it -> PT 8.
        let offer = SessionDescription::create_offer("203.0.113.7", 40000, &[codecs::pcma()]);
        let supported = vec![codecs::pcmu(), codecs::pcma()];
        assert_eq!(
            crate::sdp_rtp::negotiated_audio_payload_type(&offer, &supported),
            Some(8)
        );
    }

    #[tokio::test]
    async fn no_common_codec_answer_is_acked_then_failed_with_bye() {
        use asterisk_core::channel::ChannelDriver;

        let transport = Arc::new(RecordingTransport::new());
        let driver = Arc::new(SipChannelDriver::new(
            "127.0.0.1:0".parse().unwrap(),
        ));
        driver.set_transport(transport.clone());
        let handler = SipEventHandler::new(
            Arc::new(Dialplan::new()), transport.clone(),
        );
        handler.set_channel_driver(driver.clone());

        let channel = driver.request("sip:listener@127.0.0.1:5060", None)
            .await.unwrap();
        let channel_name = channel.name.clone();
        let registered = store::register_existing_channel(channel);
        let unique_id = registered.lock().unique_id.0.clone();

        let remote = "127.0.0.1:5060".parse().unwrap();
        let mut signaling = SipSession::new_outbound(
            "127.0.0.1:5060".parse().unwrap(), remote,
        );
        let invite = signaling.build_invite_with_uri(
            "sip:listener@127.0.0.1:5060",
            "sip:listener@127.0.0.1:5060",
        );
        let call_id = signaling.call_id.clone();
        let mut answer = invite.create_response(200, "OK").unwrap();
        for header in &mut answer.headers {
            if header.name.eq_ignore_ascii_case(crate::parser::header_names::TO) {
                header.value.push_str(";tag=listener");
            }
        }
        answer.headers.push(crate::parser::SipHeader {
            name: crate::parser::header_names::CONTACT.to_string(),
            value: "<sip:listener@127.0.0.1:5060>".to_string(),
        });
        answer.body = SessionDescription::create_offer(
            "127.0.0.1", 40000,
            &[Codec::new("opus", 111, 48000)],
        ).to_string();

        handler.register_outbound_callid(&call_id, &channel_name);
        handler.register_outbound_session(
            &call_id, &channel_name, signaling, remote,
        );
        handler.handle_response(&answer, remote).await;

        assert_eq!(transport.methods(), vec![
            crate::parser::SipMethod::Ack,
            crate::parser::SipMethod::Bye,
        ]);
        let stored = store::find_by_name(&channel_name).unwrap();
        let stored = stored.lock();
        assert_ne!(stored.state, ChannelState::Up);
        assert!(stored.check_hangup());
        assert_eq!(stored.hangup_cause, HangupCause::BearerCapNotAvail);
        drop(stored);

        handler.release_outbound_leg(&channel_name);
        store::deregister(&unique_id);
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

    #[test]
    fn invite_header_dump_redacts_credentials_and_pii() {
        // Issue #129: the header dump must never print credential material or
        // caller PII. Default-deny — anything not on the allowlist is
        // redacted, including unknown/custom headers.
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "authorization",
            "From",
            "To",
            "Contact",
            "P-Asserted-Identity",
            "P-Preferred-Identity",
            "Remote-Party-ID",
            "X-Custom-Anything",
        ] {
            assert_eq!(
                redact_invite_header_value(name, "Digest username=\"u\", response=\"deadbeef\""),
                "<redacted>",
                "{name} must be redacted"
            );
        }
        // Structurally boring headers keep their value so the dump stays
        // useful for signaling debugging.
        assert_eq!(
            redact_invite_header_value("Via", "SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK1"),
            "SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK1"
        );
        assert_eq!(redact_invite_header_value("CSeq", "1 INVITE"), "1 INVITE");
        assert_eq!(redact_invite_header_value("content-type", "application/sdp"), "application/sdp");
    }
}
