//! SIP session management.
//!
//! A SipSession represents an INVITE dialog/media session. It manages
//! the full lifecycle: INVITE -> 1xx -> 200 OK -> ACK -> BYE.

use std::net::SocketAddr;

use tracing::{debug, info};
use uuid::Uuid;

use crate::dialog::Dialog;
use crate::parser::{extract_tag, extract_uri, SipMessage, SipMethod, SipUri, StartLine, RequestLine, SipHeader, header_names};
use crate::rtp::RtpSession;
use crate::sdp::SessionDescription;

/// Session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// INVITE sent/received, waiting for response.
    Initiated,
    /// 1xx received, ringing.
    Early,
    /// 200 OK received/sent, media flowing.
    Established,
    /// BYE sent/received.
    Terminating,
    /// Session ended.
    Terminated,
}

/// Configuration for early media fork handling.
#[derive(Debug, Clone)]
pub struct EarlyMediaConfig {
    /// Whether to accept early media from any fork.
    pub follow_early_media_fork: bool,
    /// Whether to accept multiple SDP answers from different forks.
    pub accept_multiple_sdp_answers: bool,
}

impl Default for EarlyMediaConfig {
    fn default() -> Self {
        Self {
            follow_early_media_fork: true,
            accept_multiple_sdp_answers: false,
        }
    }
}

/// State tracking for early media from forked INVITEs.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct EarlyMediaState {
    /// Whether the INVITE was forked (1xx from multiple UASs).
    pub forked: bool,
    /// URIs of UASs that have sent provisional responses.
    pub forked_from: Vec<SipUri>,
    /// Index of the currently selected fork for media.
    pub selected_fork: Option<usize>,
    /// To-tags from different forks (for dialog disambiguation).
    pub fork_tags: Vec<String>,
}


impl EarlyMediaState {
    /// Record a provisional response from a fork.
    ///
    /// Returns `true` if this is a new fork (not seen before).
    pub fn on_provisional(&mut self, to_tag: &str, contact_uri: Option<&SipUri>) -> bool {
        if self.fork_tags.contains(&to_tag.to_string()) {
            return false;
        }

        self.fork_tags.push(to_tag.to_string());
        if let Some(uri) = contact_uri {
            self.forked_from.push(uri.clone());
        }

        if self.fork_tags.len() > 1 {
            self.forked = true;
        }

        // Select the first fork by default.
        if self.selected_fork.is_none() {
            self.selected_fork = Some(0);
        }

        true
    }

    /// Select a specific fork for early media.
    pub fn select_fork(&mut self, index: usize) {
        if index < self.fork_tags.len() {
            self.selected_fork = Some(index);
        }
    }

    /// Check if a given To-tag is the currently selected fork.
    pub fn is_selected_fork(&self, to_tag: &str) -> bool {
        match self.selected_fork {
            Some(idx) => self.fork_tags.get(idx).map(|t| t.as_str()) == Some(to_tag),
            None => false,
        }
    }

    /// Get the number of forks detected.
    pub fn fork_count(&self) -> usize {
        self.fork_tags.len()
    }
}

/// A SIP media session.
#[derive(Debug)]
pub struct SipSession {
    /// Unique session identifier.
    pub id: String,
    /// Current session state.
    pub state: SessionState,
    /// The underlying SIP dialog.
    pub dialog: Option<Dialog>,
    /// Local SDP description.
    pub local_sdp: Option<SessionDescription>,
    /// The initial local SDP answer (before any re-INVITEs), used by SFU.
    pub initial_local_sdp: Option<SessionDescription>,
    /// Remote SDP description.
    pub remote_sdp: Option<SessionDescription>,
    /// RTP session for media.
    pub rtp: Option<RtpSession>,
    /// Local SIP address.
    pub local_addr: SocketAddr,
    /// Remote SIP address.
    pub remote_addr: SocketAddr,
    /// The original INVITE request (for reference).
    pub invite: Option<SipMessage>,
    /// Whether we are the caller (UAC) or callee (UAS).
    pub is_outbound: bool,
    /// Call-ID for this session.
    pub call_id: String,
    /// Our From tag.
    pub local_tag: String,
    /// Early media fork state.
    pub early_media: EarlyMediaState,
    /// Early media configuration.
    pub early_media_config: EarlyMediaConfig,
    /// Outbound digest credentials to answer a 401/407 challenge on this
    /// origination leg (M-f). `None` for legs whose endpoint has no
    /// `outbound_auth`, in which case a challenge is a hard failure (no retry).
    pub outbound_auth: Option<crate::authenticator::AuthCredentials>,
    /// How many credentialed INVITE retries this leg has already sent. Bounds
    /// the challenge/response loop so a carrier that keeps challenging cannot
    /// drive an unbounded resend (M-f "bounded retries").
    pub auth_attempts: u32,
    /// Outbound From user-part (`from_user`). A carrier that authorizes calls by
    /// caller identity (e.g. Chime rejects a From that is not a DID we own)
    /// requires a specific user here. `None` falls back to `asterisk`.
    pub from_user: Option<String>,
    /// Outbound From host-part (`from_domain`). `None` falls back to the
    /// signalling host:port advertised toward the peer.
    pub from_domain: Option<String>,
}

impl SipSession {
    /// Create a new outbound session.
    pub fn new_outbound(local_addr: SocketAddr, remote_addr: SocketAddr) -> Self {
        let id = Uuid::new_v4().to_string();
        let call_id = format!("{}@{}", Uuid::new_v4(), local_addr.ip());
        let local_tag = Uuid::new_v4().to_string()[..8].to_string();

        Self {
            id,
            state: SessionState::Initiated,
            dialog: None,
            local_sdp: None,
            initial_local_sdp: None,
            remote_sdp: None,
            rtp: None,
            local_addr,
            remote_addr,
            invite: None,
            is_outbound: true,
            call_id,
            local_tag,
            early_media: EarlyMediaState::default(),
            early_media_config: EarlyMediaConfig::default(),
            outbound_auth: None,
            auth_attempts: 0,
            from_user: None,
            from_domain: None,
        }
    }

    /// Create a new inbound session from a received INVITE.
    pub fn new_inbound(
        invite: &SipMessage,
        local_addr: SocketAddr,
        remote_addr: SocketAddr,
    ) -> Option<Self> {
        let call_id = invite.call_id()?.to_string();
        let local_tag = Uuid::new_v4().to_string()[..8].to_string();

        let dialog = Dialog::from_uas_request(invite, &local_tag);

        // Parse SDP from body if present.
        let remote_sdp = if !invite.body.is_empty() {
            SessionDescription::parse(&invite.body).ok()
        } else {
            None
        };

        Some(Self {
            id: Uuid::new_v4().to_string(),
            state: SessionState::Initiated,
            dialog,
            local_sdp: None,
            initial_local_sdp: None,
            remote_sdp,
            rtp: None,
            local_addr,
            remote_addr,
            invite: Some(invite.clone()),
            is_outbound: false,
            call_id,
            local_tag,
            early_media: EarlyMediaState::default(),
            early_media_config: EarlyMediaConfig::default(),
            outbound_auth: None,
            auth_attempts: 0,
            from_user: None,
            from_domain: None,
        })
    }

    /// The `host:port` to advertise in Via/Contact/From toward this session's
    /// remote peer, applying the transport's `external_signaling_address` +
    /// optional `external_signaling_port` (New-3), scoped by `local_net`
    /// exactly like `advertised_media_ip` does for SDP. A peer inside
    /// `local_net` sees the internal bind; an external peer sees the external
    /// address and port. With no NAT config (or a local peer) this is the
    /// unchanged bind `host:port`, so non-NAT deployments are unaffected.
    pub fn signaling_hostport(&self) -> String {
        crate::sdp::advertised_signaling_hostport(self.local_addr, self.remote_addr)
    }

    /// Build an INVITE request for an outbound session.
    pub fn build_invite(&mut self, to_uri: &str) -> SipMessage {
        self.build_invite_with_uri(to_uri, to_uri)
    }

    /// The outbound From URI (`sip:user@domain`) this session presents. Honors
    /// the endpoint's `from_user`/`from_domain` (a carrier expects a specific
    /// caller identity — e.g. a DID it owns), falling back to `asterisk` @ the
    /// signalling host:port. Used for the INVITE and every in-dialog request so
    /// the From identity stays stable across the dialog.
    pub fn from_uri(&self) -> String {
        let user = self.from_user.as_deref().unwrap_or("asterisk");
        match self.from_domain.as_deref() {
            Some(domain) => format!("sip:{user}@{domain}"),
            None => format!("sip:{user}@{}", self.signaling_hostport()),
        }
    }

    /// Build an INVITE with separate Request-URI and To header value.
    /// The request_uri is used as the actual SIP Request-URI (typically the
    /// contact address), while to_uri is used in the To header.
    pub fn build_invite_with_uri(&mut self, request_uri: &str, to_uri: &str) -> SipMessage {
        let sig = self.signaling_hostport();
        let from_uri = self.from_uri();
        let contact_uri = format!("sip:asterisk@{sig}");
        let branch = format!("z9hG4bK{}", &Uuid::new_v4().to_string().replace('-', "")[..16]);

        let uri = SipUri::parse(request_uri).unwrap_or_else(|_| SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: self.remote_addr.ip().to_string(),
            port: Some(self.remote_addr.port()),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let sdp_body = self.local_sdp.as_ref().map(|s| s.to_string()).unwrap_or_default();
        let content_length = sdp_body.len();

        let mut headers = vec![
            SipHeader { name: header_names::VIA.to_string(), value: format!("SIP/2.0/UDP {};branch={}", sig, branch) },
            SipHeader { name: header_names::MAX_FORWARDS.to_string(), value: "70".to_string() },
            SipHeader { name: header_names::FROM.to_string(), value: format!("<{}>;tag={}", from_uri, self.local_tag) },
            SipHeader { name: header_names::TO.to_string(), value: format!("<{}>", to_uri) },
            SipHeader { name: header_names::CALL_ID.to_string(), value: self.call_id.clone() },
            SipHeader { name: header_names::CSEQ.to_string(), value: "1 INVITE".to_string() },
            SipHeader { name: header_names::CONTACT.to_string(), value: format!("<{}>", contact_uri) },
            SipHeader { name: header_names::USER_AGENT.to_string(), value: "Rustisk/0.1.0".to_string() },
            SipHeader { name: header_names::ALLOW.to_string(), value: "INVITE, ACK, CANCEL, BYE, OPTIONS, REFER, NOTIFY".to_string() },
        ];

        if !sdp_body.is_empty() {
            headers.push(SipHeader { name: header_names::CONTENT_TYPE.to_string(), value: "application/sdp".to_string() });
        }
        headers.push(SipHeader { name: header_names::CONTENT_LENGTH.to_string(), value: content_length.to_string() });

        let msg = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: sdp_body,
        };

        self.invite = Some(msg.clone());
        msg
    }

    /// Process a response to our INVITE.
    pub fn on_response(&mut self, response: &SipMessage) {
        let status = response.status_code().unwrap_or(0);

        match status {
            100..=199 => {
                self.state = SessionState::Early;

                // Track early media forks: detect 1xx from different UASs via To-tag
                if let Some(to_hdr) = response.to_header() {
                    if let Some(to_tag) = extract_tag(to_hdr) {
                        let contact_uri = response
                            .get_header(header_names::CONTACT)
                            .and_then(extract_uri)
                            .and_then(|u| SipUri::parse(&u).ok());

                        self.early_media.on_provisional(
                            &to_tag,
                            contact_uri.as_ref(),
                        );

                        // Only process SDP from the selected fork
                        if (self.early_media_config.follow_early_media_fork
                            || self.early_media.is_selected_fork(&to_tag))
                            && !response.body.is_empty()
                                && (self.early_media_config.accept_multiple_sdp_answers
                                    || self.remote_sdp.is_none())
                                {
                                    self.remote_sdp =
                                        SessionDescription::parse(&response.body).ok();
                                }
                    }
                }

                // Create early dialog if To tag is present
                if let (Some(invite), None) = (&self.invite, &self.dialog) {
                    self.dialog = Dialog::from_uac_response(invite, response);
                }
                debug!(call_id = %self.call_id, status, "Session early");
            }
            200..=299 => {
                self.state = SessionState::Established;
                // Create or confirm dialog
                if let Some(invite) = &self.invite {
                    if let Some(ref mut dialog) = self.dialog {
                        dialog.confirm();
                    } else {
                        self.dialog = Dialog::from_uac_response(invite, response);
                    }
                }
                // Parse SDP from body
                if !response.body.is_empty() {
                    self.remote_sdp = SessionDescription::parse(&response.body).ok();
                }
                info!(call_id = %self.call_id, "Session established");
            }
            300..=699 => {
                self.state = SessionState::Terminated;
                if let Some(ref mut dialog) = self.dialog {
                    dialog.terminate();
                }
                info!(call_id = %self.call_id, status, "Session failed");
            }
            _ => {}
        }
    }

    /// Build a 200 OK response (for UAS).
    pub fn build_200_ok(&self) -> Option<SipMessage> {
        let invite = self.invite.as_ref()?;
        let mut response = invite.create_response(200, "OK").ok()?;

        // Add Contact (NAT-scoped toward the peer: external addr/port for a
        // peer outside local_net, internal otherwise — New-3).
        let contact = format!("<sip:asterisk@{}>", self.signaling_hostport());
        response.headers.push(SipHeader {
            name: header_names::CONTACT.to_string(),
            value: contact,
        });

        // RFC 3311 §5.1: advertise our supported methods (incl. UPDATE) in the
        // initial 2xx Allow so peers know in-dialog UPDATE is supported. Absent
        // this, a peer is told UPDATE is unsupported though the handler exists
        // (M5 review MAJOR-3).
        response.headers.push(SipHeader {
            name: header_names::ALLOW.to_string(),
            value: crate::event_handler::SUPPORTED_METHODS.to_string(),
        });

        // Add To tag
        for h in &mut response.headers {
            if h.name.eq_ignore_ascii_case(header_names::TO) && !h.value.contains("tag=") {
                h.value = format!("{};tag={}", h.value, self.local_tag);
            }
        }

        // Add SDP body
        if let Some(ref sdp) = self.local_sdp {
            let body = sdp.to_string();
            response.body = body.clone();
            // Update Content-Length and add Content-Type
            for h in &mut response.headers {
                if h.name.eq_ignore_ascii_case(header_names::CONTENT_LENGTH) {
                    h.value = body.len().to_string();
                }
            }
            response.headers.push(SipHeader {
                name: header_names::CONTENT_TYPE.to_string(),
                value: "application/sdp".to_string(),
            });
        }

        Some(response)
    }

    /// Refresh the dialog's remote target (Contact) from an in-dialog
    /// target-refresh request (re-INVITE / UPDATE), RFC 3261 §12.2. No-op if no
    /// dialog is established yet. The remote *URI* (To) is left unchanged — only
    /// the target (Request-URI for subsequent local requests) is refreshed.
    pub fn update_remote_target(&mut self, contact_uri: &str) {
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.update_remote_target(contact_uri);
        }
    }

    /// Resolve the dialog's current remote target (Contact) to the transport
    /// address that local in-dialog requests (BYE, re-INVITE) must be
    /// physically sent to.
    ///
    /// Returns `Some(addr)` only when a dialog is established and the target's
    /// host is an IP literal we can address without DNS. Returns `None`
    /// otherwise, so a caller keeps its existing next hop (e.g. the symmetric
    /// INVITE source tuple) rather than losing routing to an unresolvable
    /// host. Real DNS/NAT next-hop resolution is out of scope here (deferred
    /// to the M6 NAT work); this only makes a directly-addressable target
    /// refresh operational.
    pub fn remote_target_addr(&self) -> Option<SocketAddr> {
        let dialog = self.dialog.as_ref()?;
        let uri = SipUri::parse(&dialog.remote_target).ok()?;
        let ip: std::net::IpAddr = uri.host.parse().ok()?;
        Some(SocketAddr::new(ip, uri.port.unwrap_or(5060)))
    }

    /// The CSeq **number** of the INVITE this session sent (1 on the first
    /// attempt, higher after a credentialed challenge retry). ACK and CANCEL
    /// must carry this exact number, not a hardcoded 1 (M-f).
    pub fn invite_cseq_num(&self) -> u32 {
        self.invite
            .as_ref()
            .and_then(|inv| inv.cseq())
            .and_then(|cs| cs.split_whitespace().next())
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(1)
    }

    /// Resolve loose/strict routing for an in-dialog request (RFC 3261
    /// §12.2.1.1). Returns `(request_uri, route_header_values)`.
    ///
    /// With a Record-Route-established route set of loose-routing proxies
    /// (`;lr`, as Chime / AWS Voice Connector and every RFC 3261 proxy emit),
    /// the Request-URI is the peer's Contact (remote target) and each route set
    /// entry becomes a `Route` header. A strict (non-`lr`) first hop is handled
    /// per spec by promoting it to the Request-URI and appending the target.
    fn in_dialog_routing(&self) -> Option<(String, Vec<String>)> {
        let dialog = self.dialog.as_ref()?;
        let target = if dialog.remote_target.is_empty() {
            dialog.remote_uri.clone()
        } else {
            dialog.remote_target.clone()
        };
        if target.is_empty() {
            return None;
        }
        if dialog.route_set.is_empty() {
            return Some((target, Vec::new()));
        }
        let first_uri = extract_uri(&dialog.route_set[0]).unwrap_or_else(|| dialog.route_set[0].clone());
        let loose = SipUri::parse(&first_uri)
            .map(|u| u.parameters.contains_key("lr"))
            .unwrap_or(true);
        if loose {
            Some((target, dialog.route_set.clone()))
        } else {
            let mut routes: Vec<String> = dialog.route_set[1..].to_vec();
            routes.push(format!("<{}>", target));
            Some((first_uri, routes))
        }
    }

    /// The physical next hop (IP:port) an in-dialog request must be sent to:
    /// the first route-set hop when a route set exists (loose routing keeps the
    /// datagram on the proxy path), otherwise the resolved remote target
    /// (Contact). `None` when neither is an addressable IP literal.
    pub fn in_dialog_next_hop(&self) -> Option<SocketAddr> {
        let dialog = self.dialog.as_ref()?;
        let hop = if let Some(first) = dialog.route_set.first() {
            extract_uri(first)?
        } else if !dialog.remote_target.is_empty() {
            dialog.remote_target.clone()
        } else {
            return None;
        };
        let uri = SipUri::parse(&hop).ok()?;
        let ip: std::net::IpAddr = uri.host.parse().ok()?;
        Some(SocketAddr::new(ip, uri.port.unwrap_or(5060)))
    }

    /// Build the 2xx ACK request (RFC 3261 §13.2.2.4). This is a NEW transaction
    /// (fresh branch), sent through the dialog route set to the remote target
    /// (Contact), carrying the INVITE's actual CSeq number — NOT a hardcoded 1.
    pub fn build_ack(&self) -> Option<SipMessage> {
        let invite = self.invite.as_ref()?;
        let dialog = self.dialog.as_ref()?;

        let (request_uri, route_headers) = self.in_dialog_routing().or_else(|| {
            // No dialog target available — fall back to the original R-URI so an
            // ACK is still emitted rather than silently dropped.
            match &invite.start_line {
                StartLine::Request(r) => Some((r.uri.to_string(), Vec::new())),
                _ => None,
            }
        })?;
        let uri = SipUri::parse(&request_uri).ok().or_else(|| match &invite.start_line {
            StartLine::Request(r) => Some(r.uri.clone()),
            _ => None,
        })?;

        let branch = format!("z9hG4bK{}", &Uuid::new_v4().to_string().replace('-', "")[..16]);
        let sig = self.signaling_hostport();

        let mut headers = vec![
            SipHeader { name: header_names::VIA.to_string(), value: format!("SIP/2.0/UDP {};branch={}", sig, branch) },
            SipHeader { name: header_names::MAX_FORWARDS.to_string(), value: "70".to_string() },
            SipHeader { name: header_names::FROM.to_string(), value: invite.from_header()?.to_string() },
            SipHeader {
                name: header_names::TO.to_string(),
                value: format!("{};tag={}", invite.to_header()?.split(";tag=").next().unwrap_or(""), dialog.remote_tag),
            },
            SipHeader { name: header_names::CALL_ID.to_string(), value: self.call_id.clone() },
            SipHeader { name: header_names::CSEQ.to_string(), value: format!("{} ACK", self.invite_cseq_num()) },
        ];
        for route in &route_headers {
            headers.push(SipHeader { name: header_names::ROUTE.to_string(), value: route.clone() });
        }
        headers.push(SipHeader { name: header_names::CONTENT_LENGTH.to_string(), value: "0".to_string() });

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Ack,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: String::new(),
        })
    }

    /// Build a credentialed retry INVITE answering a 401/407 `challenge`, and
    /// adopt it as this session's current INVITE so a subsequent ACK/CANCEL
    /// derives the right (incremented) CSeq and the response matches. Returns
    /// `None` when the leg has no `outbound_auth` or the challenge is
    /// unparseable. The retry is a NEW client transaction: fresh branch,
    /// incremented CSeq, `Authorization`/`Proxy-Authorization` attached.
    pub fn build_auth_retry_invite(&mut self, challenge: &SipMessage) -> Option<SipMessage> {
        let creds = self.outbound_auth.clone()?;
        let invite = self.invite.as_ref()?;
        let retry = crate::authenticator::OutboundAuthenticator::create_authenticated_request(
            invite,
            challenge,
            std::slice::from_ref(&creds),
        )?;
        self.auth_attempts += 1;
        // Adopt the retry as the live INVITE: its CSeq/branch now govern the
        // transaction, and build_ack/build_cancel read CSeq from here.
        self.invite = Some(retry.clone());
        self.state = SessionState::Initiated;
        Some(retry)
    }

    /// Build a BYE request.
    pub fn build_bye(&mut self) -> Option<SipMessage> {
        // From identity must match the INVITE's (same user@domain + local tag)
        // for the dialog's lifetime — carriers key in-dialog requests on it.
        let from_base = self.from_uri();
        let sig = self.signaling_hostport();
        // Loose/strict routing per the established route set (Contact + Route),
        // so an in-dialog BYE follows the same proxy path as the ACK instead of
        // being sent blind to the original request-URI. Fall back to the
        // symmetric INVITE source tuple when no addressable target exists.
        let (request_uri, route_headers) = self
            .in_dialog_routing()
            .unwrap_or_else(|| (format!("sip:{}", self.remote_addr), Vec::new()));
        let dialog = self.dialog.as_mut()?;
        let cseq = dialog.next_cseq();

        let uri = SipUri::parse(&request_uri).ok().unwrap_or_else(|| SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: self.remote_addr.ip().to_string(),
            port: Some(self.remote_addr.port()),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let branch = format!("z9hG4bK{}", &Uuid::new_v4().to_string().replace('-', "")[..16]);

        let from_value = format!("<{}>;tag={}", from_base, dialog.local_tag);

        let to_value = format!("<{}>;tag={}", dialog.remote_uri, dialog.remote_tag);

        let mut headers = vec![
            SipHeader { name: header_names::VIA.to_string(), value: format!("SIP/2.0/UDP {};branch={}", sig, branch) },
            SipHeader { name: header_names::MAX_FORWARDS.to_string(), value: "70".to_string() },
            SipHeader { name: header_names::FROM.to_string(), value: from_value },
            SipHeader { name: header_names::TO.to_string(), value: to_value },
            SipHeader { name: header_names::CALL_ID.to_string(), value: self.call_id.clone() },
            SipHeader { name: header_names::CSEQ.to_string(), value: format!("{} BYE", cseq) },
        ];
        for route in &route_headers {
            headers.push(SipHeader { name: header_names::ROUTE.to_string(), value: route.clone() });
        }
        headers.push(SipHeader { name: header_names::CONTENT_LENGTH.to_string(), value: "0".to_string() });

        self.state = SessionState::Terminating;

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Bye,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: String::new(),
        })
    }

    /// Build a CANCEL request for the original INVITE.
    ///
    /// Used to cancel remaining forks after 200 OK is received from one UAS.
    pub fn build_cancel(&self) -> Option<SipMessage> {
        let invite = self.invite.as_ref()?;

        let uri = match &invite.start_line {
            StartLine::Request(r) => r.uri.clone(),
            _ => return None,
        };

        // CANCEL reuses the same branch as the INVITE (same transaction)
        let via = invite.get_header(header_names::VIA)?;

        let headers = vec![
            SipHeader {
                name: header_names::VIA.to_string(),
                value: via.to_string(),
            },
            SipHeader {
                name: header_names::MAX_FORWARDS.to_string(),
                value: "70".to_string(),
            },
            SipHeader {
                name: header_names::FROM.to_string(),
                value: invite.from_header()?.to_string(),
            },
            SipHeader {
                name: header_names::TO.to_string(),
                value: invite.to_header()?.to_string(),
            },
            SipHeader {
                name: header_names::CALL_ID.to_string(),
                value: self.call_id.clone(),
            },
            SipHeader {
                // CANCEL matches the INVITE's CSeq NUMBER (RFC 3261 §9.1),
                // method CANCEL — not a hardcoded 1 (M-f).
                name: header_names::CSEQ.to_string(),
                value: format!("{} CANCEL", self.invite_cseq_num()),
            },
            SipHeader {
                name: header_names::CONTENT_LENGTH.to_string(),
                value: "0".to_string(),
            },
        ];

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Cancel,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: String::new(),
        })
    }

    /// Build an in-dialog re-INVITE with a new SDP offer.
    ///
    /// Used by the SFU ConfBridge to add/remove video streams for participants.
    pub fn build_reinvite(&mut self, sdp: &SessionDescription) -> Option<SipMessage> {
        let sig = self.signaling_hostport();
        // From identity must match the INVITE's (same user@domain + local tag)
        // for the dialog's lifetime — a re-INVITE that reverts to the internal
        // identity breaks carriers that key in-dialog requests on From.
        let from_base = self.from_uri();
        let dialog = self.dialog.as_mut()?;
        let cseq = dialog.next_cseq();

        // Build the Request-URI from the remote target (Contact of the remote side).
        let uri = SipUri::parse(&dialog.remote_target).ok().unwrap_or_else(|| SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: self.remote_addr.ip().to_string(),
            port: Some(self.remote_addr.port()),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let branch = format!("z9hG4bK{}", &Uuid::new_v4().to_string().replace('-', "")[..16]);

        // From = our identity + local tag, To = remote URI + remote tag.
        let from_value = format!("<{}>;tag={}", from_base, dialog.local_tag);
        let to_value = format!("<{}>;tag={}", dialog.remote_uri, dialog.remote_tag);

        let body = sdp.to_string();

        let headers = vec![
            SipHeader { name: header_names::VIA.to_string(), value: format!("SIP/2.0/UDP {};branch={}", sig, branch) },
            SipHeader { name: header_names::MAX_FORWARDS.to_string(), value: "70".to_string() },
            SipHeader { name: header_names::FROM.to_string(), value: from_value },
            SipHeader { name: header_names::TO.to_string(), value: to_value },
            SipHeader { name: header_names::CALL_ID.to_string(), value: self.call_id.clone() },
            SipHeader { name: header_names::CSEQ.to_string(), value: format!("{} INVITE", cseq) },
            SipHeader { name: header_names::CONTACT.to_string(), value: format!("<sip:asterisk@{}>", sig) },
            SipHeader { name: header_names::CONTENT_TYPE.to_string(), value: "application/sdp".to_string() },
            SipHeader { name: header_names::CONTENT_LENGTH.to_string(), value: body.len().to_string() },
        ];

        // Store the new local SDP.
        self.local_sdp = Some(sdp.clone());

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body,
        })
    }

    /// Build an ACK for a received 200 OK to our re-INVITE.
    pub fn build_reinvite_ack(&self, response: &SipMessage) -> Option<SipMessage> {
        let dialog = self.dialog.as_ref()?;

        let uri = SipUri::parse(&dialog.remote_target).ok().unwrap_or_else(|| SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: self.remote_addr.ip().to_string(),
            port: Some(self.remote_addr.port()),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let branch = format!("z9hG4bK{}", &Uuid::new_v4().to_string().replace('-', "")[..16]);
        let sig = self.signaling_hostport();

        // Same From identity as the re-INVITE this ACKs (dialog-stable).
        let from_value = format!("<{}>;tag={}", self.from_uri(), dialog.local_tag);
        let to_value = format!("<{}>;tag={}", dialog.remote_uri, dialog.remote_tag);

        // CSeq from the response we're ACKing.
        let cseq_num = response.cseq()
            .and_then(|cs| cs.split_whitespace().next())
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(1);

        let headers = vec![
            SipHeader { name: header_names::VIA.to_string(), value: format!("SIP/2.0/UDP {};branch={}", sig, branch) },
            SipHeader { name: header_names::MAX_FORWARDS.to_string(), value: "70".to_string() },
            SipHeader { name: header_names::FROM.to_string(), value: from_value },
            SipHeader { name: header_names::TO.to_string(), value: to_value },
            SipHeader { name: header_names::CALL_ID.to_string(), value: self.call_id.clone() },
            SipHeader { name: header_names::CSEQ.to_string(), value: format!("{} ACK", cseq_num) },
            SipHeader { name: header_names::CONTENT_LENGTH.to_string(), value: "0".to_string() },
        ];

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Ack,
                uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: String::new(),
        })
    }

    /// Terminate the session.
    pub fn terminate(&mut self) {
        self.state = SessionState::Terminated;
        if let Some(ref mut dialog) = self.dialog {
            dialog.terminate();
        }
    }
}

#[cfg(test)]
mod cp1_tests {
    use super::*;
    use crate::authenticator::AuthCredentials;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn cseq_of(msg: &SipMessage) -> String {
        msg.cseq().unwrap().to_string()
    }

    fn branch_of(msg: &SipMessage) -> String {
        msg.get_header(header_names::VIA)
            .unwrap()
            .split(';')
            .find_map(|p| p.trim().strip_prefix("branch="))
            .unwrap()
            .to_string()
    }

    fn request_uri(msg: &SipMessage) -> String {
        match &msg.start_line {
            StartLine::Request(r) => r.uri.to_string(),
            _ => panic!("not a request"),
        }
    }

    /// Drive an outbound session through: INVITE(1) -> 401 challenge ->
    /// credentialed retry INVITE(2) -> 200 with a CHANGED Contact and a
    /// Record-Route. Returns the established session.
    fn established_after_challenge() -> SipSession {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.outbound_auth = Some(AuthCredentials::new("carrier", "s3cr3t", ""));
        let invite = s.build_invite_with_uri("sip:+15550001111@10.0.0.2:5060", "sip:+15550001111@10.0.0.2");
        // The carrier's 401 challenge (To gains a tag).
        let challenge = SipMessage::parse(
            format!(
                "SIP/2.0 401 Unauthorized\r\n\
                 {via}\r\n\
                 {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=carrier401\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: 1 INVITE\r\n\
                 WWW-Authenticate: Digest realm=\"carrier\", nonce=\"abc123\", algorithm=MD5, qop=\"auth\"\r\n\
                 Content-Length: 0\r\n\r\n",
                via = format!("Via: {}", invite.get_header(header_names::VIA).unwrap()),
                from = format!("From: {}", invite.from_header().unwrap()),
                cid = s.call_id,
            )
            .as_bytes(),
        )
        .unwrap();
        let retry = s.build_auth_retry_invite(&challenge).expect("retry built");
        // Sanity: the retry is a new transaction with an incremented CSeq.
        assert_eq!(cseq_of(&retry), "2 INVITE");
        assert_ne!(branch_of(&retry), branch_of(&invite));
        assert!(retry.get_header(header_names::AUTHORIZATION).is_some());
        assert_eq!(s.auth_attempts, 1);

        // The carrier answers the RETRY with a 200 carrying a CHANGED Contact
        // (10.0.0.9:5070, not the request-URI 10.0.0.2) and a Record-Route.
        let ok = SipMessage::parse(
            format!(
                "SIP/2.0 200 OK\r\n\
                 {via}\r\n\
                 {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=carrier200\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: 2 INVITE\r\n\
                 Record-Route: <sip:10.0.0.9:5070;lr>\r\n\
                 Contact: <sip:carrier@10.0.0.9:5070>\r\n\
                 Content-Length: 0\r\n\r\n",
                via = format!("Via: {}", retry.get_header(header_names::VIA).unwrap()),
                from = format!("From: {}", retry.from_header().unwrap()),
                cid = s.call_id,
            )
            .as_bytes(),
        )
        .unwrap();
        s.on_response(&ok);
        assert_eq!(s.state, SessionState::Established);
        s
    }

    #[test]
    fn twoxx_ack_targets_route_set_and_contact_with_real_cseq() {
        let s = established_after_challenge();
        let ack = s.build_ack().unwrap();
        // Real CSeq: the RETRY INVITE was CSeq 2, so the ACK is "2 ACK". A
        // hardcoded "1 ACK" (the M-f defect) would fail this.
        assert_eq!(cseq_of(&ack), "2 ACK");
        // Loose routing: Request-URI is the refreshed Contact (10.0.0.9:5070),
        // NOT the original request-URI (10.0.0.2).
        assert_eq!(request_uri(&ack), "sip:carrier@10.0.0.9:5070");
        assert!(request_uri(&ack).contains("10.0.0.9:5070"));
        // The Record-Route became a Route header.
        let routes: Vec<_> = ack.get_headers(header_names::ROUTE);
        assert_eq!(routes.len(), 1);
        assert!(routes[0].contains("10.0.0.9:5070"));
        // Physical next hop is the route-set first hop.
        assert_eq!(s.in_dialog_next_hop(), Some(addr("10.0.0.9:5070")));
    }

    #[test]
    fn bye_follows_route_set_to_target_with_incremented_cseq() {
        let mut s = established_after_challenge();
        let bye = s.build_bye().unwrap();
        // In-dialog CSeq advances beyond the INVITE's 2.
        assert_eq!(cseq_of(&bye), "3 BYE");
        // Request-URI is the Contact target, Route header carries the route set.
        assert_eq!(request_uri(&bye), "sip:carrier@10.0.0.9:5070");
        let routes = bye.get_headers(header_names::ROUTE);
        assert_eq!(routes.len(), 1);
        assert!(routes[0].contains("10.0.0.9:5070"));
    }

    #[test]
    fn cancel_carries_invite_real_cseq_and_branch() {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.outbound_auth = Some(AuthCredentials::new("carrier", "s3cr3t", ""));
        let invite = s.build_invite_with_uri("sip:+15550001111@10.0.0.2:5060", "sip:+15550001111@10.0.0.2");
        let challenge = SipMessage::parse(
            format!(
                "SIP/2.0 401 Unauthorized\r\n\
                 Via: {via}\r\n\
                 From: {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=c1\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: 1 INVITE\r\n\
                 WWW-Authenticate: Digest realm=\"carrier\", nonce=\"n\", algorithm=MD5, qop=\"auth\"\r\n\
                 Content-Length: 0\r\n\r\n",
                via = invite.get_header(header_names::VIA).unwrap(),
                from = invite.from_header().unwrap(),
                cid = s.call_id,
            )
            .as_bytes(),
        )
        .unwrap();
        let retry = s.build_auth_retry_invite(&challenge).unwrap();
        // CANCEL matches the LIVE (retried) INVITE: same branch + same CSeq
        // number (2), method CANCEL. A hardcoded "1 CANCEL" is the M-f defect.
        let cancel = s.build_cancel().unwrap();
        assert_eq!(cseq_of(&cancel), "2 CANCEL");
        assert_eq!(branch_of(&cancel), branch_of(&retry));
    }

    #[test]
    fn no_retry_without_outbound_auth() {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        let _invite = s.build_invite_with_uri("sip:x@10.0.0.2:5060", "sip:x@10.0.0.2");
        let challenge = SipMessage::parse(
            b"SIP/2.0 401 Unauthorized\r\n\
              Via: SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bKx\r\n\
              From: <sip:asterisk@10.0.0.1:5060>;tag=t\r\n\
              To: <sip:x@10.0.0.2>;tag=c\r\n\
              Call-ID: cid\r\n\
              CSeq: 1 INVITE\r\n\
              WWW-Authenticate: Digest realm=\"carrier\", nonce=\"n\", algorithm=MD5\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .unwrap();
        assert!(s.build_auth_retry_invite(&challenge).is_none());
        assert_eq!(s.auth_attempts, 0);
    }
}

#[cfg(test)]
mod cp2_tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn from_user_domain_applied_to_invite_from() {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.from_user = Some("+19995551234".to_string());
        s.from_domain = Some("carrier.example.net".to_string());
        let invite = s.build_invite_with_uri("sip:+15550001111@10.0.0.2:5060", "sip:+15550001111@10.0.0.2");
        let from = invite.from_header().unwrap().to_string();
        assert!(
            from.contains("sip:+19995551234@carrier.example.net"),
            "From must carry the configured user@domain, got: {from}"
        );
        // The internal bind identity must be gone (the CP2 defect).
        assert!(!from.contains("asterisk@"), "From still hardcodes asterisk: {from}");
    }

    #[test]
    fn from_user_only_keeps_signalling_domain() {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.from_user = Some("+19995551234".to_string());
        let invite = s.build_invite_with_uri("sip:x@10.0.0.2:5060", "sip:x@10.0.0.2");
        let from = invite.from_header().unwrap().to_string();
        assert!(from.contains("sip:+19995551234@"), "user not applied: {from}");
        // Domain falls back to the signalling host:port (the bind), not "asterisk".
        assert!(from.contains("@10.0.0.1:5060"), "domain fallback wrong: {from}");
    }

    #[test]
    fn from_defaults_to_asterisk_when_unset() {
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        let invite = s.build_invite_with_uri("sip:x@10.0.0.2:5060", "sip:x@10.0.0.2");
        assert!(invite.from_header().unwrap().contains("sip:asterisk@10.0.0.1:5060"));
    }

    #[test]
    fn bye_from_matches_configured_identity() {
        // The in-dialog BYE From must carry the SAME user@domain as the INVITE.
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.from_user = Some("+19995551234".to_string());
        s.from_domain = Some("carrier.example.net".to_string());
        let invite = s.build_invite_with_uri("sip:+15550001111@10.0.0.2:5060", "sip:+15550001111@10.0.0.2");
        let ok = SipMessage::parse(
            format!(
                "SIP/2.0 200 OK\r\n\
                 Via: {via}\r\n\
                 From: {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=c200\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: 1 INVITE\r\n\
                 Contact: <sip:carrier@10.0.0.2:5060>\r\n\
                 Content-Length: 0\r\n\r\n",
                via = invite.get_header(header_names::VIA).unwrap(),
                from = invite.from_header().unwrap(),
                cid = s.call_id,
            )
            .as_bytes(),
        )
        .unwrap();
        s.on_response(&ok);
        let bye = s.build_bye().unwrap();
        let from = bye.from_header().unwrap().to_string();
        assert!(from.contains("sip:+19995551234@carrier.example.net"), "BYE From wrong: {from}");
        assert!(!from.contains("asterisk@"), "BYE From still hardcodes asterisk: {from}");
    }

    #[test]
    fn reinvite_and_its_ack_from_match_configured_identity() {
        // M7 follow-up (CMD-MINOR): the in-dialog re-INVITE (ConfBridge SFU
        // video renegotiation) and its ACK must carry the SAME From
        // user@domain as the INVITE/BYE — the From identity is stable for the
        // dialog's lifetime. A build_reinvite that reverts to the hardcoded
        // internal `asterisk@<sig>` identity fails this.
        let mut s = SipSession::new_outbound(addr("10.0.0.1:5060"), addr("10.0.0.2:5060"));
        s.from_user = Some("+19995551234".to_string());
        s.from_domain = Some("carrier.example.net".to_string());
        let invite = s.build_invite_with_uri("sip:+15550001111@10.0.0.2:5060", "sip:+15550001111@10.0.0.2");
        let ok = SipMessage::parse(
            format!(
                "SIP/2.0 200 OK\r\n\
                 Via: {via}\r\n\
                 From: {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=c200\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: 1 INVITE\r\n\
                 Contact: <sip:carrier@10.0.0.2:5060>\r\n\
                 Content-Length: 0\r\n\r\n",
                via = invite.get_header(header_names::VIA).unwrap(),
                from = invite.from_header().unwrap(),
                cid = s.call_id,
            )
            .as_bytes(),
        )
        .unwrap();
        s.on_response(&ok);

        let sdp = SessionDescription::parse(
            "v=0\r\n\
             o=rustisk 0 1 IN IP4 10.0.0.1\r\n\
             s=-\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 30000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .unwrap();
        let reinvite = s.build_reinvite(&sdp).unwrap();
        let from = reinvite.from_header().unwrap().to_string();
        assert!(
            from.contains("sip:+19995551234@carrier.example.net"),
            "re-INVITE From must keep the configured identity, got: {from}"
        );
        assert!(!from.contains("asterisk@"), "re-INVITE From reverts to the internal identity: {from}");

        // The ACK for the re-INVITE's 200 carries the same From identity.
        let reinvite_cseq = reinvite.cseq().unwrap().to_string();
        let reinvite_200 = SipMessage::parse(
            format!(
                "SIP/2.0 200 OK\r\n\
                 Via: {via}\r\n\
                 From: {from}\r\n\
                 To: <sip:+15550001111@10.0.0.2>;tag=c200\r\n\
                 Call-ID: {cid}\r\n\
                 CSeq: {cseq}\r\n\
                 Contact: <sip:carrier@10.0.0.2:5060>\r\n\
                 Content-Length: 0\r\n\r\n",
                via = reinvite.get_header(header_names::VIA).unwrap(),
                from = reinvite.from_header().unwrap(),
                cid = s.call_id,
                cseq = reinvite_cseq,
            )
            .as_bytes(),
        )
        .unwrap();
        let ack = s.build_reinvite_ack(&reinvite_200).unwrap();
        let from = ack.from_header().unwrap().to_string();
        assert!(
            from.contains("sip:+19995551234@carrier.example.net"),
            "re-INVITE ACK From must keep the configured identity, got: {from}"
        );
        assert!(!from.contains("asterisk@"), "re-INVITE ACK From reverts to the internal identity: {from}");
    }
}
