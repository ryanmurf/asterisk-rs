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
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

use crate::rtp::RtpSession;
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
    /// Supported codecs.
    codecs: Vec<Codec>,
    /// Inbound registrar, shared from the event handler at startup. When set,
    /// [`Self::request`] prefers a live dynamic contact binding over the
    /// static AoR contact so registered devices are reachable (issue #33).
    registrar: RwLock<Option<Arc<crate::registrar::Registrar>>>,
}

impl fmt::Debug for SipChannelDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipChannelDriver")
            .field("local_addr", &self.local_addr)
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl SipChannelDriver {
    /// Create a new SIP channel driver.
    pub fn new(local_addr: SocketAddr) -> Self {
        Self {
            local_addr,
            channels: RwLock::new(HashMap::new()),
            transport: RwLock::new(None),
            codecs: vec![
                codecs::pcmu(), codecs::pcma(), codecs::telephone_event(),
                codecs::vp8(), codecs::h264(), codecs::vp9(), codecs::h265(),
            ],
            registrar: RwLock::new(None),
        }
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

    fn get_private(&self, name: &str) -> Option<Arc<SipChannelPrivate>> {
        self.channels.read().get(name).cloned()
    }

    fn remove_private(&self, name: &str) -> Option<Arc<SipChannelPrivate>> {
        self.channels.write().remove(name)
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
        let priv_data = Arc::new(SipChannelPrivate {
            session: Mutex::new(session),
            rtp: Mutex::new(Some(Arc::new(rtp))),
            transport,
        });
        self.channels.write().insert(channel_name.to_string(), priv_data);
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

        // Parse destination to determine remote address and endpoint config.
        let endpoint_config = crate::pjsip_config::get_global_pjsip_config();
        let (_to_uri, remote_addr) = if dest.starts_with("sip:") || dest.starts_with("sips:") {
            let uri = crate::parser::SipUri::parse(dest)
                .map_err(|e| AsteriskError::InvalidArgument(format!("Invalid SIP URI: {}", e.0)))?;
            let port = uri.port.unwrap_or(5060);
            let addr: SocketAddr = format!("{}:{}", uri.host, port)
                .parse()
                .map_err(|e| AsteriskError::InvalidArgument(format!("Invalid address: {}", e)))?;
            (dest.to_string(), addr)
        } else if dest.contains('@') || dest.contains(':') {
            // Treat as user@host or host:port
            let addr_str = if dest.contains(':') {
                dest.to_string()
            } else {
                format!("{}:5060", dest)
            };
            let addr: SocketAddr = addr_str
                .parse()
                .map_err(|e| AsteriskError::InvalidArgument(format!("Invalid dest: {}", e)))?;
            (format!("sip:{}", dest), addr)
        } else {
            // Treat as endpoint name -- resolve its AOR contact, preferring a
            // live registration over the static config contact (issue #33).
            let config = endpoint_config.as_ref()
                .ok_or_else(|| AsteriskError::NotFound(format!("No PJSIP config loaded for endpoint '{}'", dest)))?;
            if config.find_endpoint(dest).is_none() {
                return Err(AsteriskError::NotFound(format!("Endpoint '{}' not found", dest)));
            }
            let contact_uri = {
                let reg_guard = self.registrar.read();
                Self::resolve_endpoint_contact(config, reg_guard.as_deref(), dest)
            }
            .unwrap_or_else(|| format!("sip:{}@127.0.0.1:5060", dest));
            // Parse the contact URI to get the remote address
            let uri = crate::parser::SipUri::parse(&contact_uri)
                .map_err(|e| AsteriskError::InvalidArgument(format!("Invalid contact URI: {}", e.0)))?;
            let port = uri.port.unwrap_or(5060);
            let addr: SocketAddr = format!("{}:{}", uri.host, port)
                .parse()
                .map_err(|e| AsteriskError::InvalidArgument(format!("Invalid contact address: {}", e)))?;
            (contact_uri, addr)
        };

        // Create RTP session
        let rtp_bind = SocketAddr::new(self.local_addr.ip(), 0);
        let rtp_session = RtpSession::bind(rtp_bind).await?;
        let rtp_port = rtp_session.local_addr()?.port();

        // Create SIP session
        let mut sip_session = SipSession::new_outbound(self.local_addr, remote_addr);

        // Create SDP offer with a concrete, routable connection address
        // (external_media_address / routed interface — never 0.0.0.0,
        // issue #56).
        let sdp = SessionDescription::create_offer(
            &crate::sdp::advertised_media_ip(self.local_addr, remote_addr),
            rtp_port,
            &self.codecs,
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

        let priv_data = Arc::new(SipChannelPrivate {
            session: Mutex::new(sip_session),
            rtp: Mutex::new(Some(Arc::new(rtp_session))),
            transport,
        });

        self.channels.write().insert(channel_name, priv_data);
        Ok(channel)
    }

    /// Initiate the outbound call (send INVITE).
    async fn call(&self, channel: &mut Channel, dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let mut session = priv_data.session.lock().await;
        // Build the Request-URI. Use the full contact address so the
        // inbound side can extract the user part as the dialed extension.
        let request_uri = if dest.starts_with("sip:") || dest.starts_with("sips:") {
            dest.to_string()
        } else {
            // Look up AOR contact for a proper Request-URI with user@host
            let endpoint_config = crate::pjsip_config::get_global_pjsip_config();
            let contact = endpoint_config.as_ref().and_then(|cfg| {
                let ep = cfg.find_endpoint(dest)?;
                let aor_name = ep.aors.as_deref().unwrap_or(dest);
                let aor = cfg.find_aor(aor_name)?;
                aor.contact.first().cloned()
            });
            contact.unwrap_or_else(|| format!("sip:{}@{}", dest, session.remote_addr))
        };
        let to_uri = if dest.starts_with("sip:") {
            dest.to_string()
        } else {
            format!("sip:{}", dest)
        };

        let invite = session.build_invite_with_uri(&request_uri, &to_uri);

        // Send the INVITE
        priv_data
            .transport
            .send(&invite, session.remote_addr)
            .await
            .map_err(|e| AsteriskError::Internal(format!("Failed to send INVITE: {}", e)))?;

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
                let _ = priv_data.transport.send(&bye, session.remote_addr).await;
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
        rtp_guard.as_ref().map(|rtp| rtp.payload_type as u32)
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
    use std::collections::HashSet;
    use std::thread;
    use std::time::Instant;

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
