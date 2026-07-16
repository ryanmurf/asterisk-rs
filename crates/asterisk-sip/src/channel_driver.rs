//! SIP Channel Driver.
//!
//! Integrates the SIP stack with the Asterisk channel model, implementing
//! the ChannelDriver trait for SIP/PJSIP-style channel operations.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::Mutex;
use tracing::{debug, info};

use asterisk_codecs::{codecs, Codec};
use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_res::dns_srv::{CachingResolver, SipUriTarget, TransportType};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

use crate::media_stats::{complete_channel_media_stats, register_channel_media_stats};
use crate::rtp::{RtpPortAllocator, RtpPortRange, RtpSession};
use crate::sdp::SessionDescription;
use crate::session::{SessionState, SipSession};
use crate::transport::{SipTransport, UdpTransport};

/// Per-channel SIP private data.
struct SipChannelPrivate {
    /// The SIP session.
    session: Mutex<SipSession>,
    /// The RTP session for media.
    ///
    /// Held as an `Arc` so frame I/O clones the session handle out and
    /// releases the mutex BEFORE awaiting the socket. Holding the guard
    /// across `recv_frame().await` (as a naive implementation would) makes
    /// a blocked reader starve concurrent writers on the same leg: a
    /// mixing bridge (ConfBridge softmix) writes from its own task, and a
    /// listen-only participant would otherwise only receive audio when
    /// their own inbound packets released the lock.
    rtp: Mutex<Option<Arc<RtpSession>>>,
    /// SIP transport to use.
    transport: Arc<dyn SipTransport>,
    /// Canonical Request-URI selected while resolving the destination.
    request_uri: String,
    /// Every address returned for the destination, in resolver order.
    remote_targets: Vec<SocketAddr>,
    /// Endpoint-filtered codecs used to create and validate this dialog's SDP.
    codecs: Vec<Codec>,
}

impl fmt::Debug for SipChannelPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipChannelPrivate").finish()
    }
}

/// Global counter for channel naming (like Asterisk's chan_pjsip counter).
/// Shared by both the outbound `request()` path and the inbound INVITE path so
/// that channel-name suffixes are process-globally unique and monotonic.
static CHANNEL_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Allocate the next process-global, monotonically increasing channel-name
/// suffix. Used to build `PJSIP/<label>-<suffix>` names for both inbound and
/// outbound calls; guarantees no two concurrent calls collide on a name.
pub fn next_channel_suffix() -> u32 {
    CHANNEL_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The SIP channel driver.
///
/// Port of chan_pjsip.c. Implements the ChannelDriver trait for SIP calls.
pub struct SipChannelDriver {
    /// Local SIP address.
    local_addr: SocketAddr,
    /// Active channels.
    channels: RwLock<HashMap<String, Arc<SipChannelPrivate>>>,
    /// SIP transport.
    transport: RwLock<Option<Arc<dyn SipTransport>>>,
    /// Shared SIP stack for outbound client transactions. Startup attaches
    /// this alongside the transport; driver-only tests retain the transport
    /// fallback.
    stack: RwLock<Option<Arc<crate::stack::SipStack>>>,
    /// Supported codecs.
    codecs: Vec<Codec>,
    /// Shared bounded allocator used by inbound and outbound RTP legs.
    rtp_allocator: Arc<RtpPortAllocator>,
    /// Inbound registrar, shared from the event handler at startup. When set,
    /// [`Self::request`] prefers a live dynamic contact binding over the
    /// static AoR contact so registered devices are reachable (issue #33).
    registrar: RwLock<Option<Arc<crate::registrar::Registrar>>>,
    /// TTL-aware SIP destination resolver shared by outbound calls.
    resolver: CachingResolver,
}

impl fmt::Debug for SipChannelDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipChannelDriver")
            .field("local_addr", &self.local_addr)
            .field("active_channels", &self.channels.read().len())
            .field("rtp_port_range", &self.rtp_allocator.range())
            .finish()
    }
}

impl SipChannelDriver {
    /// Create a new SIP channel driver.
    pub fn new(local_addr: SocketAddr) -> Self {
        Self::with_rtp_port_range(local_addr, RtpPortRange::default())
    }

    /// Create a SIP channel driver with a validated bounded RTP range.
    pub fn with_rtp_port_range(local_addr: SocketAddr, range: RtpPortRange) -> Self {
        Self {
            local_addr,
            channels: RwLock::new(HashMap::new()),
            transport: RwLock::new(None),
            stack: RwLock::new(None),
            codecs: vec![
                codecs::pcmu(), codecs::pcma(), codecs::telephone_event(),
                codecs::vp8(), codecs::h264(), codecs::vp9(), codecs::h265(),
            ],
            rtp_allocator: Arc::new(RtpPortAllocator::new(range)),
            registrar: RwLock::new(None),
            resolver: CachingResolver::new(),
        }
    }

    /// Allocate an RTP session for either side of the SIP event/driver path.
    pub(crate) async fn allocate_rtp_session(
        &self,
        bind_ip: std::net::IpAddr,
    ) -> AsteriskResult<RtpSession> {
        self.rtp_allocator.allocate(bind_ip).await
    }

    /// Share the inbound registrar so outbound `request()` can resolve a
    /// dynamically-registered contact for an endpoint's AoR (issue #33).
    /// Wired once at startup with the same registrar the event handler owns.
    pub fn set_registrar(&self, registrar: Arc<crate::registrar::Registrar>) {
        *self.registrar.write() = Some(registrar);
    }

    /// Resolve the contact URI to dial for a bare endpoint name.
    ///
    /// Prefers a live registrar binding for the endpoint's AoR over the
    /// static configured contact, so a phone that REGISTERed from a dynamic
    /// address is reachable via `Dial(PJSIP/<endpoint>)` (issue #33). Returns
    /// `None` when neither a dynamic nor a static contact is available, so the
    /// caller can apply its own last-resort fallback.
    fn resolve_endpoint_contact(
        config: &crate::pjsip_config::PjsipConfig,
        registrar: Option<&crate::registrar::Registrar>,
        dest: &str,
    ) -> Option<String> {
        let ep = config.find_endpoint(dest)?;
        let aor_name = ep.aors.as_deref().unwrap_or(dest);

        // A live registration wins over the static contact.
        if let Some(reg) = registrar {
            if let Some(contact) = reg.best_contact(aor_name) {
                return Some(contact);
            }
        }

        // Fall back to the statically configured AoR contact.
        config
            .find_aor(aor_name)
            .and_then(|a| a.contact.first().cloned())
    }

    /// Initialize the transport layer.
    pub async fn init_transport(&self) -> AsteriskResult<()> {
        let transport = UdpTransport::bind(self.local_addr).await.map_err(|e| {
            AsteriskError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("Failed to bind SIP transport: {}", e),
            ))
        })?;
        *self.transport.write() = Some(Arc::new(transport));
        info!(addr = %self.local_addr, "SIP channel driver initialized");
        Ok(())
    }

    /// Set an externally-created transport (shared with the SIP stack).
    pub fn set_transport(&self, transport: Arc<dyn SipTransport>) {
        *self.transport.write() = Some(transport);
    }

    /// Attach the shared stack so outbound requests use RFC 3261 client
    /// transactions and their retransmission/timeout timers.
    pub fn set_stack(&self, stack: Arc<crate::stack::SipStack>) {
        *self.stack.write() = Some(stack);
    }

    fn get_private(&self, name: &str) -> Option<Arc<SipChannelPrivate>> {
        self.channels.read().get(name).cloned()
    }

    fn remove_private(&self, name: &str) -> Option<Arc<SipChannelPrivate>> {
        let removed = self.channels.write().remove(name);
        if removed.is_some() {
            complete_channel_media_stats(name);
        }
        removed
    }

    /// Attach a pre-bound [`RtpSession`] to an **inbound** channel so the media
    /// plane (`read_frame`/`write_frame` → RTP) is reachable by channel name,
    /// exactly like an outbound channel created via [`Self::request`].
    ///
    /// Inbound INVITEs are handled by [`crate::event_handler::SipEventHandler`],
    /// which builds the channel directly in the global store rather than through
    /// this driver — so without this call an inbound channel has no RTP session
    /// and carries no media (issue #7).
    ///
    /// `session` must be the **real inbound** [`SipSession`] (from the received
    /// INVITE), not a fabricated outbound one. It previously stored
    /// `SipSession::new_outbound(...)`, whose `invite` is `None` and
    /// `is_outbound` is `true`; [`ChannelDriver::indicate`] requires
    /// `session.invite` to be `Some`, so any app/AGI signalling Ringing /
    /// Progress / Busy (180/183/486) on an inbound channel through the driver
    /// hit `None` and silently no-op'd, and `hangup()` skipped the BYE
    /// (state `Initiated`). Storing the inbound session — which carries the
    /// INVITE and `is_outbound = false` — makes those uniform driver paths work
    /// (issue #36).
    pub fn attach_inbound_media(
        &self,
        channel_name: &str,
        session: SipSession,
        transport: Arc<dyn SipTransport>,
        rtp: RtpSession,
    ) {
        self.attach_inbound_media_with_codecs(
            channel_name,
            session,
            transport,
            rtp,
            self.codecs.clone(),
        );
    }

    /// Attach inbound media with the codec policy of the identified endpoint.
    pub fn attach_inbound_media_with_codecs(
        &self,
        channel_name: &str,
        session: SipSession,
        transport: Arc<dyn SipTransport>,
        rtp: RtpSession,
        codecs: Vec<Codec>,
    ) {
        let remote_addr = session.remote_addr;
        let stats = rtp.stats.clone();
        let priv_data = Arc::new(SipChannelPrivate {
            session: Mutex::new(session),
            rtp: Mutex::new(Some(Arc::new(rtp))),
            transport,
            request_uri: String::new(),
            remote_targets: vec![remote_addr],
            codecs,
        });
        self.channels.write().insert(channel_name.to_string(), priv_data);
        let unique_id = asterisk_core::channel_store::find_by_name(channel_name)
            .map(|channel| channel.lock().unique_id.0.clone());
        register_channel_media_stats(channel_name, unique_id.as_deref(), stats);
    }

    /// Remove a channel's private data (and its RTP socket) from the driver.
    ///
    /// Used to tear down the media plane when a call ends, so bound RTP
    /// sockets are not leaked in the driver's channel map. Idempotent — a
    /// no-op if the channel is already gone.
    pub fn remove_channel(&self, name: &str) {
        self.remove_private(name);
    }

    /// Number of channels currently in the driver's map. Each entry owns a
    /// bound RTP socket, so this is the live measure of media-plane resource
    /// usage — used by tests to assert legs are released rather than leaked
    /// (issue #28).
    pub fn active_channel_count(&self) -> usize {
        self.channels.read().len()
    }

    /// The local UDP port currently bound for a channel's media plane, if the
    /// channel exists and has an RTP session attached.
    ///
    /// The re-INVITE path uses this to advertise the *real* media port in its
    /// SDP answer instead of a placeholder, so hold/unhold and other mid-call
    /// renegotiations do not break audio for peers that honor the answer SDP
    /// (mirrors the initial-INVITE fix for issue #8).
    pub async fn channel_rtp_local_port(&self, channel_name: &str) -> Option<u16> {
        let priv_data = self.channels.read().get(channel_name)?.clone();
        let rtp = priv_data.rtp.lock().await;
        rtp.as_ref()
            .and_then(|r| r.local_addr().ok())
            .map(|a| a.port())
    }

    /// Return the channel's negotiated telephone-event payload type.
    pub async fn channel_rtp_dtmf_payload_type(&self, channel_name: &str) -> Option<u8> {
        let priv_data = self.get_private(channel_name)?;
        let rtp = priv_data.rtp.lock().await.clone()?;
        rtp.dtmf_payload_type()
    }

    /// Codecs permitted by the endpoint policy for this dialog.
    pub fn channel_codecs(&self, channel_name: &str) -> Option<Vec<Codec>> {
        Some(self.get_private(channel_name)?.codecs.clone())
    }

    /// Apply an outbound INVITE answer to the driver-owned dialog and media
    /// session used by `hangup`, `write_frame`, and RTP.
    pub(crate) async fn apply_outbound_answer(
        &self,
        channel_name: &str,
        response: &crate::parser::SipMessage,
    ) -> AsteriskResult<()> {
        let priv_data = self.get_private(channel_name)
            .ok_or_else(|| AsteriskError::NotFound(channel_name.to_string()))?;

        let remote_sdp = {
            let mut session = priv_data.session.lock().await;
            session.on_response(response);
            session.remote_sdp.clone().ok_or_else(|| {
                AsteriskError::InvalidArgument("200 OK has no valid SDP answer".into())
            })?
        };

        let payload_type = crate::sdp_rtp::negotiated_audio_payload_type(
            &remote_sdp, &priv_data.codecs,
        ).ok_or_else(|| {
            AsteriskError::InvalidArgument("SDP answer has no common audio codec".into())
        })?;
        if !matches!(payload_type, 0 | 8) {
            return Err(AsteriskError::InvalidArgument(format!(
                "SDP answer selected non-G.711 payload type {payload_type}"
            )));
        }
        let remote_addr = crate::sdp_rtp::remote_rtp_endpoint(&remote_sdp)
            .ok_or_else(|| {
                AsteriskError::InvalidArgument(
                    "SDP answer has no active IP audio endpoint".into(),
                )
            })?;

        let rtp = priv_data.rtp.lock().await.clone()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;
        rtp.set_remote_addr(remote_addr);
        rtp.set_payload_type(payload_type);

        let dtmf = crate::sdp_rtp::negotiated_dtmf_payload_type(
            &remote_sdp, &priv_data.codecs,
        );
        if let Some(dtmf_payload_type) = dtmf {
            rtp.set_dtmf_payload_type(dtmf_payload_type);
        } else {
            rtp.clear_dtmf_payload_type();
        }

        debug!(channel = channel_name, %remote_addr, payload_type,
            dtmf_payload_type = ?dtmf,
            "Applied outbound SDP answer to driver media session");
        Ok(())
    }

    #[cfg(test)]
    async fn channel_rtp_remote_addr(&self, channel_name: &str) -> Option<SocketAddr> {
        let priv_data = self.get_private(channel_name)?;
        let remote_addr = priv_data.rtp.lock().await.clone()?.remote_addr();
        remote_addr
    }

    #[cfg(test)]
    async fn channel_rtp_payload_type(&self, channel_name: &str) -> Option<u8> {
        let priv_data = self.get_private(channel_name)?;
        let payload_type = priv_data.rtp.lock().await.clone()?.payload_type();
        Some(payload_type)
    }

    #[cfg(test)]
    async fn channel_session_state(&self, channel_name: &str) -> Option<SessionState> {
        let priv_data = self.get_private(channel_name)?;
        let state = priv_data.session.lock().await.state;
        Some(state)
    }

    fn get_transport(&self) -> AsteriskResult<Arc<dyn SipTransport>> {
        self.transport.read().clone().ok_or_else(|| {
            AsteriskError::Internal("SIP transport not initialized".into())
        })
    }
}

#[async_trait]
impl ChannelDriver for SipChannelDriver {
    fn name(&self) -> &str {
        "PJSIP"
    }

    fn description(&self) -> &str {
        "PJSIP SIP Channel Driver"
    }

    /// Request an outbound SIP channel.
    ///
    /// `dest` format: `endpoint_name` or `sip:user@host[:port]`
    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let transport = self.get_transport()?;

        // Select the canonical destination URI. Resolution happens once here;
        // call() retains and tries the complete returned address set.
        let endpoint_config = crate::pjsip_config::get_global_pjsip_config();
        let request_uri = if dest.starts_with("sip:") || dest.starts_with("sips:") {
            dest.to_string()
        } else if dest.contains('@') || dest.contains(':') {
            format!("sip:{}", dest)
        } else {
            // Treat as endpoint name -- resolve its AOR contact, preferring a
            // live registration over the static config contact (issue #33).
            let config = endpoint_config.as_ref()
                .ok_or_else(|| AsteriskError::NotFound(format!("No PJSIP config loaded for endpoint '{}'", dest)))?;
            if config.find_endpoint(dest).is_none() {
                return Err(AsteriskError::NotFound(format!("Endpoint '{}' not found", dest)));
            }
            {
                let reg_guard = self.registrar.read();
                Self::resolve_endpoint_contact(config, reg_guard.as_deref(), dest)
            }
            .unwrap_or_else(|| format!("sip:{}@127.0.0.1:5060", dest))
        };

        let mut target = SipUriTarget::parse(&request_uri).ok_or_else(|| {
            AsteriskError::InvalidArgument(format!("Invalid SIP URI: {request_uri}"))
        })?;
        if target.scheme == "sips"
            || matches!(target.transport, Some(TransportType::Tcp | TransportType::Tls))
        {
            return Err(AsteriskError::InvalidArgument(
                "SIP channel driver currently supports UDP destinations only".into(),
            ));
        }
        target.port = Some(target.port.unwrap_or(TransportType::Udp.default_port()));
        target.transport = Some(TransportType::Udp);
        let resolved = self.resolver.resolve(&target).await.map_err(|error| {
            AsteriskError::InvalidArgument(format!(
                "Failed to resolve SIP destination {request_uri}: {error}"
            ))
        })?;
        let mut remote_targets = Vec::with_capacity(resolved.len());
        for resolved_target in resolved {
            if resolved_target.transport == TransportType::Udp
                && !remote_targets.contains(&resolved_target.address)
            {
                remote_targets.push(resolved_target.address);
            }
        }
        let remote_addr = *remote_targets.first().ok_or_else(|| {
            AsteriskError::InvalidArgument(format!(
                "SIP destination {request_uri} resolved to no UDP addresses"
            ))
        })?;

        let channel_codecs = endpoint_config
            .as_ref()
            .and_then(|config| config.find_endpoint(dest))
            .map(|endpoint| endpoint.media_codecs(&self.codecs))
            .unwrap_or_else(|| self.codecs.clone());
        if !channel_codecs.iter().any(|codec| {
            codec.name.eq_ignore_ascii_case("PCMU")
                || codec.name.eq_ignore_ascii_case("PCMA")
        }) {
            return Err(AsteriskError::InvalidArgument(format!(
                "Endpoint '{dest}' permits no G.711 audio codec"
            )));
        }

        // Create RTP session
        let rtp_session = self.allocate_rtp_session(self.local_addr.ip()).await?;
        if let Some(codec) = channel_codecs
            .iter()
            .find(|codec| codec.name.eq_ignore_ascii_case("telephone-event"))
        {
            rtp_session.set_dtmf_payload_type(codec.payload_type);
        }
        let rtp_port = rtp_session.local_addr()?.port();

        // Create SIP session
        let mut sip_session = SipSession::new_outbound(self.local_addr, remote_addr);

        // Create SDP offer with a concrete, routable connection address
        // (external_media_address / routed interface — never 0.0.0.0,
        // issue #56).
        let sdp = SessionDescription::create_offer(
            &crate::sdp::advertised_media_ip(self.local_addr, remote_addr),
            rtp_port,
            &channel_codecs,
        );
        sip_session.local_sdp = Some(sdp);

        let counter = next_channel_suffix();
        let chan_name = format!("PJSIP/{}-{:08}", dest, counter);
        let mut channel = Channel::new(chan_name);

        // Apply endpoint config (accountcode, etc.) if available
        if let Some(ref config) = endpoint_config {
            if let Some(ep) = config.find_endpoint(dest) {
                channel.accountcode = ep.accountcode.clone();
            }
        }

        let channel_name = channel.name.clone();
        let stats = rtp_session.stats.clone();

        let priv_data = Arc::new(SipChannelPrivate {
            session: Mutex::new(sip_session),
            rtp: Mutex::new(Some(Arc::new(rtp_session))),
            transport,
            request_uri,
            remote_targets,
            codecs: channel_codecs,
        });

        self.channels.write().insert(channel_name.clone(), priv_data);
        register_channel_media_stats(
            &channel_name,
            Some(&channel.unique_id.0),
            stats,
        );
        Ok(channel)
    }

    /// Initiate the outbound call (send INVITE).
    async fn call(&self, channel: &mut Channel, dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if let Some(rtp) = priv_data.rtp.lock().await.clone() {
            register_channel_media_stats(
                &channel.name,
                Some(&channel.unique_id.0),
                rtp.stats.clone(),
            );
        }

        let mut session = priv_data.session.lock().await;
        // Build the Request-URI. Use the full contact address so the
        // inbound side can extract the user part as the dialed extension.
        let request_uri = &priv_data.request_uri;
        let to_uri = if dest.starts_with("sip:") {
            dest.to_string()
        } else {
            format!("sip:{}", dest)
        };

        let invite = session.build_invite_with_uri(request_uri, &to_uri);

        // Preserve resolver ordering, but do not pin a call to one address if
        // the transport rejects it. The selected address becomes the dialog's
        // signalling peer for subsequent CANCEL/BYE requests.
        let stack = self.stack.read().clone();
        let mut last_error = None;
        for remote_addr in &priv_data.remote_targets {
            let send_result = match &stack {
                Some(stack) => stack
                    .send_invite(invite.clone(), *remote_addr)
                    .await
                    .map(|_| ()),
                None => priv_data.transport.send(&invite, *remote_addr).await,
            };
            match send_result {
                Ok(()) => {
                    session.remote_addr = *remote_addr;
                    last_error = None;
                    break;
                }
                Err(error) => {
                    debug!(dest, %remote_addr, %error,
                        "SIP destination address failed; trying next DNS result");
                    last_error = Some(error);
                }
            }
        }
        if let Some(error) = last_error {
            return Err(AsteriskError::Internal(format!(
                "Failed to send INVITE to every resolved address: {error}"
            )));
        }

        // Register Call-ID mapping and session so responses can be routed
        // and ACK/BYE can be sent later
        if let Some(handler) = crate::get_global_event_handler() {
            handler.register_outbound_callid(&session.call_id, &channel.name);
            // Create a lightweight session copy for ACK/BYE
            let session_copy = crate::session::SipSession {
                id: session.id.clone(),
                state: session.state,
                dialog: session.dialog.clone(),
                local_sdp: session.local_sdp.clone(),
                initial_local_sdp: None,
                remote_sdp: session.remote_sdp.clone(),
                rtp: None,
                local_addr: session.local_addr,
                remote_addr: session.remote_addr,
                invite: session.invite.clone(),
                is_outbound: session.is_outbound,
                call_id: session.call_id.clone(),
                local_tag: session.local_tag.clone(),
                early_media: session.early_media.clone(),
                early_media_config: session.early_media_config.clone(),
            };
            handler.register_outbound_session(
                &session.call_id,
                &channel.name,
                session_copy,
                session.remote_addr,
            );
        }

        // Register outbound channel in NOTIFY service for in-dialog NOTIFY
        {
            let notify_state = crate::notify_service::ChannelSipState {
                call_id: session.call_id.clone(),
                local_tag: session.local_tag.clone(),
                remote_tag: String::new(), // Updated when 1xx/2xx arrives
                local_uri: format!("sip:asterisk@{}", session.local_addr),
                remote_target: to_uri.clone(),
                remote_addr: session.remote_addr,
                local_seq: 100,
            };
            crate::notify_service::global_notify_service()
                .register_channel(&channel.name, notify_state);
        }

        channel.set_state(ChannelState::Dialing);
        info!(call_id = %session.call_id, dest, "SIP INVITE sent");
        Ok(())
    }

    /// Answer an inbound call (send 200 OK).
    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let mut session = priv_data.session.lock().await;

        let response = session.build_200_ok().ok_or_else(|| {
            AsteriskError::Internal("Failed to build 200 OK".into())
        })?;

        priv_data
            .transport
            .send(&response, session.remote_addr)
            .await
            .map_err(|e| AsteriskError::Internal(format!("Failed to send 200 OK: {}", e)))?;

        session.state = SessionState::Established;
        channel.answer();
        info!(call_id = %session.call_id, "SIP call answered");
        Ok(())
    }

    /// Hang up the call (send BYE).
    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = match self.remove_private(&channel.name) {
            Some(p) => p,
            None => return Ok(()),
        };

        let mut session = priv_data.session.lock().await;

        if session.state == SessionState::Established || session.state == SessionState::Early {
            if let Some(bye) = session.build_bye() {
                let stack = self.stack.read().clone();
                match stack {
                    Some(stack) => {
                        let _ = stack.send_request(bye, session.remote_addr).await;
                    }
                    None => {
                        let _ = priv_data.transport.send(&bye, session.remote_addr).await;
                    }
                }
            }
        }

        session.terminate();
        channel.set_state(ChannelState::Down);
        info!(call_id = %session.call_id, "SIP call hungup");
        Ok(())
    }

    /// Read a frame (from RTP).
    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        // Clone the session handle and release the lock before blocking on
        // the socket, so concurrent write_frame calls are never starved by
        // a reader waiting for inbound media.
        let rtp = priv_data
            .rtp
            .lock()
            .await
            .clone()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;

        rtp.recv_frame().await
    }

    /// Write a frame (to RTP).
    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let rtp = priv_data
            .rtp
            .lock()
            .await
            .clone()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;

        rtp.send_frame(frame).await
    }

    /// The negotiated audio format: the RTP session's payload type (the same
    /// codec id `read_frame` stamps on inbound voice frames).
    async fn audio_format(&self, channel: &Channel) -> Option<u32> {
        let priv_data = self.get_private(&channel.name)?;
        let rtp_guard = priv_data.rtp.lock().await;
        rtp_guard.as_ref().map(|rtp| rtp.payload_type() as u32)
    }

    /// Send DTMF via RFC 2833.
    async fn send_digit_end(&self, channel: &mut Channel, digit: char, duration: u32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let rtp = priv_data
            .rtp
            .lock()
            .await
            .clone()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;

        // Convert ms to samples (8kHz)
        let duration_samples = (duration * 8) as u16;
        rtp.send_dtmf(digit, duration_samples).await
    }

    /// Indicate a condition (send SIP signaling).
    async fn indicate(&self, channel: &mut Channel, condition: i32, _data: &[u8]) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let session = priv_data.session.lock().await;

        match condition as u32 {
            x if x == ControlFrame::Ringing as u32 => {
                if let Some(ref invite) = session.invite {
                    if let Ok(resp) = invite.create_response(180, "Ringing") {
                        let _ = priv_data.transport.send(&resp, session.remote_addr).await;
                    }
                }
            }
            x if x == ControlFrame::Progress as u32 => {
                if let Some(ref invite) = session.invite {
                    if let Ok(resp) = invite.create_response(183, "Session Progress") {
                        let _ = priv_data.transport.send(&resp, session.remote_addr).await;
                    }
                }
            }
            x if x == ControlFrame::Proceeding as u32 => {
                if let Some(ref invite) = session.invite {
                    if let Ok(resp) = invite.create_response(100, "Trying") {
                        let _ = priv_data.transport.send(&resp, session.remote_addr).await;
                    }
                }
            }
            x if x == ControlFrame::Busy as u32 => {
                if let Some(ref invite) = session.invite {
                    if let Ok(resp) = invite.create_response(486, "Busy Here") {
                        let _ = priv_data.transport.send(&resp, session.remote_addr).await;
                    }
                }
            }
            x if x == ControlFrame::Congestion as u32 => {
                if let Some(ref invite) = session.invite {
                    if let Ok(resp) = invite.create_response(503, "Service Unavailable") {
                        let _ = priv_data.transport.send(&resp, session.remote_addr).await;
                    }
                }
            }
            _ => {
                debug!(condition, "Unhandled SIP indication");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{header_names, SipHeader, SipMessage};
    use std::collections::HashSet;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    struct FailAddressTransport {
        local_addr: SocketAddr,
        failed_addr: SocketAddr,
        attempts: StdMutex<Vec<SocketAddr>>,
    }

    #[async_trait]
    impl SipTransport for FailAddressTransport {
        async fn send(
            &self,
            _msg: &SipMessage,
            addr: SocketAddr,
        ) -> Result<(), crate::transport::TransportError> {
            self.attempts.lock().unwrap().push(addr);
            if addr == self.failed_addr {
                Err(crate::transport::TransportError::Connection(
                    "injected address failure".to_string(),
                ))
            } else {
                Ok(())
            }
        }

        fn local_addr(&self) -> Result<SocketAddr, crate::transport::TransportError> {
            Ok(self.local_addr)
        }

        fn protocol(&self) -> &str {
            "UDP"
        }
    }

    // --- issue #33: outbound routing consults the registrar ---------------

    fn routing_config(static_contact: Option<&str>) -> crate::pjsip_config::PjsipConfig {
        use crate::pjsip_config::{AorConfig, EndpointConfig, PjsipConfig};
        PjsipConfig {
            endpoints: vec![EndpointConfig {
                name: "100".to_string(),
                aors: Some("100".to_string()),
                ..Default::default()
            }],
            aors: vec![AorConfig {
                name: "100".to_string(),
                contact: static_contact.map(|c| vec![c.to_string()]).unwrap_or_default(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn resolve_uses_static_contact_when_no_registration() {
        let cfg = routing_config(Some("sip:100@static.example:5060"));
        assert_eq!(
            SipChannelDriver::resolve_endpoint_contact(&cfg, None, "100"),
            Some("sip:100@static.example:5060".to_string())
        );
    }

    #[test]
    fn resolve_prefers_dynamic_registration_over_static() {
        use crate::registrar::{Registrar, Registration};
        let cfg = routing_config(Some("sip:100@static.example:5060"));
        let registrar = Registrar::new();
        registrar.register(Registration {
            aor: "100".to_string(),
            contact_uri: "sip:100@10.0.0.55:5060".to_string(),
            expiration: 3600,
            registered_at: Instant::now(),
            user_agent: "phone".to_string(),
            path: None,
            call_id: "c1".to_string(),
            cseq: 1,
        });
        // The dynamically-registered address wins — without this, a phone
        // that registered from a dynamic address is unreachable via Dial().
        assert_eq!(
            SipChannelDriver::resolve_endpoint_contact(&cfg, Some(&registrar), "100"),
            Some("sip:100@10.0.0.55:5060".to_string())
        );
    }

    #[test]
    fn resolve_none_when_no_contact_anywhere() {
        let cfg = routing_config(None);
        let registrar = crate::registrar::Registrar::new();
        assert_eq!(
            SipChannelDriver::resolve_endpoint_contact(&cfg, Some(&registrar), "100"),
            None
        );
    }

    #[test]
    fn resolve_none_for_unknown_endpoint() {
        let cfg = routing_config(Some("sip:100@static.example:5060"));
        assert_eq!(
            SipChannelDriver::resolve_endpoint_contact(&cfg, None, "999"),
            None
        );
    }

    #[tokio::test]
    async fn fqdn_request_retains_and_tries_the_resolved_address_set() {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let first: SocketAddr = "127.0.0.2:5099".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:5099".parse().unwrap();
        let transport = Arc::new(FailAddressTransport {
            local_addr: local,
            failed_addr: first,
            attempts: StdMutex::new(Vec::new()),
        });
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport.clone());
        driver.resolver.cache().put_addresses(
            "multi.invalid",
            vec![first.ip(), second.ip(), second.ip()],
            Duration::from_secs(60),
        );

        let destination = "sip:listener@multi.invalid:5099";
        let mut channel = driver.request(destination, None).await.unwrap();
        let private = driver.get_private(&channel.name).unwrap();
        assert_eq!(private.remote_targets, vec![first, second]);

        driver.call(&mut channel, destination, 30).await.unwrap();

        assert_eq!(*transport.attempts.lock().unwrap(), vec![first, second]);
        assert_eq!(private.session.lock().await.remote_addr, second);
        driver.remove_channel(&channel.name);
    }

    // --- issue #36: inbound channel stores the real session so indicate works -

    /// Build a minimal inbound INVITE targeting `to`, sent from `from_addr`.
    fn inbound_invite(from_addr: SocketAddr) -> crate::parser::SipMessage {
        let raw = format!(
            "INVITE sip:100@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP {from_addr};branch=z9hG4bKind1\r\n\
             From: <sip:caller@{from_addr}>;tag=caller\r\n\
             To: <sip:100@127.0.0.1>\r\n\
             Call-ID: indicate-call-1\r\n\
             CSeq: 1 INVITE\r\n\
             Content-Length: 0\r\n\r\n"
        );
        crate::parser::SipMessage::parse(raw.as_bytes()).unwrap()
    }

    #[tokio::test]
    async fn indicate_ringing_sends_180_on_inbound_channel() {
        use asterisk_core::channel::ChannelDriver;

        // The "remote" (caller) socket that should receive our 180 Ringing.
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();

        // The driver's send transport.
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind("127.0.0.1:0".parse().unwrap()).await.unwrap(),
        );
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // Attach an inbound channel with the REAL inbound session (carries the
        // INVITE), exactly as the event handler now does.
        let invite = inbound_invite(peer_addr);
        let session = SipSession::new_inbound(&invite, local, peer_addr).expect("inbound session");
        let rtp = RtpSession::bind(local).await.unwrap();
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport.clone());
        let chan_name = "PJSIP/indicate-1";
        driver.attach_inbound_media(chan_name, session, transport, rtp);

        // Signal Ringing via the uniform driver API.
        let mut channel = Channel::new(chan_name);
        driver
            .indicate(&mut channel, ControlFrame::Ringing as i32, &[])
            .await
            .unwrap();

        // The caller must receive a 180 Ringing. Before the fix the driver
        // stored a fabricated outbound session (invite = None), so indicate()
        // silently no-op'd and nothing arrived.
        let mut buf = [0u8; 4096];
        let (n, _src) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            peer.recv_from(&mut buf),
        )
        .await
        .expect("timed out: no 180 was sent (indicate() no-op'd)")
        .unwrap();
        let resp = crate::parser::SipMessage::parse(&buf[..n]).expect("parse response");
        assert_eq!(resp.status_code(), Some(180), "indicate(Ringing) must send 180");
    }

    /// Regression for issue #54: the re-INVITE answer must advertise the media
    /// plane's REAL bound RTP port, not the hardcoded placeholder 10000. The
    /// driver must expose the actual bound port for the channel so the
    /// re-INVITE path can put it in the SDP answer; hold/unhold otherwise
    /// breaks audio for peers that honor the answer.
    #[tokio::test]
    async fn channel_rtp_local_port_reports_real_bound_port() {
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind("127.0.0.1:0".parse().unwrap()).await.unwrap(),
        );
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();

        let invite = inbound_invite(peer);
        let session = SipSession::new_inbound(&invite, local, peer).expect("inbound session");
        let rtp = RtpSession::bind(local).await.unwrap();
        let bound_port = rtp.local_addr().unwrap().port();
        assert_ne!(bound_port, 0, "OS must assign a real port");
        assert_ne!(bound_port, 10000, "bound port must not be the placeholder");

        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport.clone());
        let chan_name = "PJSIP/reinvite-port-1";
        driver.attach_inbound_media(chan_name, session, transport, rtp);

        assert_eq!(
            driver.channel_rtp_local_port(chan_name).await,
            Some(bound_port),
            "must report the real bound RTP port for the re-INVITE answer"
        );
        assert_eq!(
            driver.channel_rtp_local_port("PJSIP/does-not-exist").await,
            None,
            "unknown channel must return None"
        );
    }

    /// Regression for issue #70: call RTP allocation must stay inside the
    /// configured range, report exhaustion, and release the reservation when
    /// channel teardown drops the owning session.
    #[tokio::test]
    async fn bounded_call_rtp_range_exhausts_and_reuses_after_hangup() {
        let probe = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rtp_port = probe.local_addr().unwrap().port();
        drop(probe);

        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind(local).await.unwrap(),
        );
        let range = RtpPortRange::new(rtp_port, rtp_port).unwrap();
        let driver = SipChannelDriver::with_rtp_port_range(local, range);
        driver.set_transport(transport);

        let mut first = driver
            .request("sip:first@127.0.0.1:9", None)
            .await
            .unwrap();
        assert_eq!(
            driver.channel_rtp_local_port(&first.name).await,
            Some(rtp_port)
        );

        let exhausted = driver
            .request("sip:second@127.0.0.1:9", None)
            .await
            .unwrap_err();
        assert!(matches!(
            exhausted,
            AsteriskError::Io(ref error)
                if error.kind() == std::io::ErrorKind::AddrNotAvailable
                    && error.to_string().contains("exhausted")
        ));

        driver.hangup(&mut first).await.unwrap();
        let reused = driver
            .request("sip:third@127.0.0.1:9", None)
            .await
            .unwrap();
        assert_eq!(
            driver.channel_rtp_local_port(&reused.name).await,
            Some(rtp_port)
        );
    }

    #[tokio::test]
    async fn negotiated_dtmf_payload_controls_receiver_detection() {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind(local).await.unwrap(),
        );
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport);
        let mut channel = driver
            .request("sip:dtmf@127.0.0.1:9", None)
            .await
            .unwrap();
        let rtp_port = driver
            .channel_rtp_local_port(&channel.name)
            .await
            .unwrap();

        let negotiated = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 127.0.0.1\r\n\
             s=Test\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0 110\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=rtpmap:110 telephone-event/8000\r\n",
        )
        .unwrap();
        let private = driver.get_private(&channel.name).unwrap();
        let rtp = private.rtp.lock().await.clone().unwrap();
        rtp.set_dtmf_payload_type(
            crate::sdp_rtp::negotiated_dtmf_payload_type(
                &negotiated, &driver.codecs,
            ).unwrap(),
        );
        assert_eq!(
            driver.channel_rtp_dtmf_payload_type(&channel.name).await,
            Some(110)
        );

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target: SocketAddr = format!("127.0.0.1:{}", rtp_port).parse().unwrap();
        let event = crate::rtp::DtmfEvent {
            event: 5,
            end: true,
            volume: 10,
            duration: 800,
        };
        let packet = |payload_type| {
            crate::rtp::build_rtp_packet(
                &crate::rtp::RtpHeader {
                    version: 2,
                    padding: false,
                    extension: false,
                    csrc_count: 0,
                    marker: false,
                    payload_type,
                    sequence: 1,
                    timestamp: 160,
                    ssrc: 0x12345678,
                },
                &event.to_bytes(),
            )
        };

        peer.send_to(&packet(101), target).await.unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(100),
            driver.read_frame(&mut channel),
        ).await.is_err(), "non-negotiated DTMF payload must be discarded");
        assert_eq!(rtp.stats.snapshot().discarded_wrong_payload_type, 1);
        assert_eq!(rtp.stats.snapshot().packets_received, 0);

        peer.send_to(&packet(110), target).await.unwrap();
        assert!(matches!(
            driver.read_frame(&mut channel).await.unwrap(),
            Frame::DtmfEnd {
                digit: '5',
                duration_ms: 100
            }
        ));
    }

    fn answer_for_session(session: &mut SipSession, sdp: &SessionDescription) -> SipMessage {
        let invite = session.build_invite_with_uri(
            "sip:listener@127.0.0.1:5060",
            "sip:listener@127.0.0.1:5060",
        );
        let mut answer = invite.create_response(200, "OK").unwrap();
        for header in &mut answer.headers {
            if header.name.eq_ignore_ascii_case(header_names::TO) {
                header.value.push_str(";tag=listener-tag");
            }
        }
        answer.headers.push(SipHeader {
            name: header_names::CONTACT.to_string(),
            value: "<sip:listener@127.0.0.1:5060>".to_string(),
        });
        answer.body = sdp.to_string();
        answer
    }

    #[tokio::test]
    async fn outbound_answer_drives_listen_only_media_without_symmetric_latch() {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let signaling_peer = tokio::net::UdpSocket::bind(local).await.unwrap();
        let destination = format!("sip:listener@{}", signaling_peer.local_addr().unwrap());
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind(local).await.unwrap(),
        );
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport);
        let mut channel = driver.request(&destination, None).await.unwrap();

        let listener = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let answer_sdp = SessionDescription::create_offer(
            "127.0.0.1", listener.local_addr().unwrap().port(), &[codecs::pcma()],
        );
        let answer = {
            let private = driver.get_private(&channel.name).unwrap();
            let mut session = private.session.lock().await;
            answer_for_session(&mut session, &answer_sdp)
        };
        driver.apply_outbound_answer(&channel.name, &answer).await.unwrap();

        assert_eq!(driver.channel_session_state(&channel.name).await,
            Some(SessionState::Established));
        assert_eq!(driver.channel_rtp_remote_addr(&channel.name).await,
            listener.local_addr().ok());
        assert_eq!(driver.channel_rtp_payload_type(&channel.name).await, Some(8));

        driver.write_frame(
            &mut channel,
            &Frame::voice(8, 160, bytes::Bytes::from(vec![0x55; 160])),
        ).await.expect("SDP-derived remote must permit the first outbound RTP packet");

        let mut packet = [0u8; 2048];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(2), listener.recv_from(&mut packet),
        ).await.expect("listen-only peer did not receive RTP").unwrap();
        let (header, payload) = crate::rtp::parse_rtp_header(&packet[..len]).unwrap();
        assert_eq!(header.payload_type, 8, "PCMA answer must select PT 8");
        assert_eq!(payload, &[0x55; 160]);

        let stats = {
            let private = driver.get_private(&channel.name).unwrap();
            let snapshot = private.rtp.lock().await.clone().unwrap().stats.snapshot();
            snapshot
        };
        assert_eq!(stats.voice_frames_received, 0, "peer remained listen-only");
        assert_eq!(stats.voice_frames_sent, 1, "one voice frame reached the peer");

        driver.hangup(&mut channel).await.unwrap();
        let mut message = [0u8; 2048];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(2), signaling_peer.recv_from(&mut message),
        ).await.expect("established driver session did not send BYE").unwrap();
        let bye = SipMessage::parse(&message[..len]).unwrap();
        assert_eq!(bye.method(), Some(crate::parser::SipMethod::Bye));
    }

    #[tokio::test]
    async fn outbound_answer_without_common_codec_is_rejected() {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport: Arc<dyn SipTransport> = Arc::new(
            UdpTransport::bind(local).await.unwrap(),
        );
        let driver = SipChannelDriver::new(local);
        driver.set_transport(transport);
        let channel = driver.request("sip:listener@127.0.0.1:5060", None)
            .await.unwrap();
        let unsupported = SessionDescription::create_offer(
            "127.0.0.1", 40000, &[Codec::new("opus", 111, 48000)],
        );
        let answer = {
            let private = driver.get_private(&channel.name).unwrap();
            let mut session = private.session.lock().await;
            answer_for_session(&mut session, &unsupported)
        };

        let error = driver.apply_outbound_answer(&channel.name, &answer)
            .await.unwrap_err();
        assert!(error.to_string().contains("no common audio codec"));
        assert_eq!(driver.channel_rtp_remote_addr(&channel.name).await, None);
        assert_eq!(driver.channel_rtp_payload_type(&channel.name).await, Some(0));
    }

    /// Regression for the channel-name collision bug: the inbound INVITE path
    /// previously derived its channel-name suffix from a truncated
    /// `SystemTime::now()` nanosecond value ("rand_id"), which is not random and
    /// collides when two calls land in the same nanosecond window. The shared
    /// monotonic counter must hand out strictly-unique suffixes even under
    /// concurrent allocation from many threads.
    #[test]
    fn next_channel_suffix_is_unique_across_threads() {
        const THREADS: usize = 16;
        const PER_THREAD: usize = 1000;

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                thread::spawn(|| {
                    (0..PER_THREAD)
                        .map(|_| next_channel_suffix())
                        .collect::<Vec<u32>>()
                })
            })
            .collect();

        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().expect("thread panicked"));
        }

        let unique: HashSet<u32> = all.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "channel-name suffixes must be unique; got {} duplicates",
            all.len() - unique.len()
        );
    }
}
