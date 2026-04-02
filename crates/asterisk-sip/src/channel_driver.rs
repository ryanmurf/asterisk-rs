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
    rtp: Mutex<Option<RtpSession>>,
    /// SIP transport to use.
    transport: Arc<dyn SipTransport>,
}

impl fmt::Debug for SipChannelPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipChannelPrivate").finish()
    }
}

/// Global counter for outbound channel naming (like Asterisk's chan_pjsip counter).
static CHANNEL_COUNTER: AtomicU32 = AtomicU32::new(1);

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
        }
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
            // Treat as endpoint name -- look up AOR contact from config
            let config = endpoint_config.as_ref()
                .ok_or_else(|| AsteriskError::NotFound(format!("No PJSIP config loaded for endpoint '{}'", dest)))?;
            let ep = config.find_endpoint(dest)
                .ok_or_else(|| AsteriskError::NotFound(format!("Endpoint '{}' not found", dest)))?;
            let aor_name = ep.aors.as_deref().unwrap_or(dest);
            let aor = config.find_aor(aor_name);
            let contact_uri = aor.and_then(|a| a.contact.first()).cloned()
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

        // Create SDP offer
        let sdp = SessionDescription::create_offer(
            &self.local_addr.ip().to_string(),
            rtp_port,
            &self.codecs,
        );
        sip_session.local_sdp = Some(sdp);

        let counter = CHANNEL_COUNTER.fetch_add(1, Ordering::Relaxed);
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
            rtp: Mutex::new(Some(rtp_session)),
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

        let rtp_guard = priv_data.rtp.lock().await;
        let rtp = rtp_guard
            .as_ref()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;

        rtp.recv_frame().await
    }

    /// Write a frame (to RTP).
    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let rtp_guard = priv_data.rtp.lock().await;
        let rtp = rtp_guard
            .as_ref()
            .ok_or_else(|| AsteriskError::Internal("No RTP session".into()))?;

        rtp.send_frame(frame).await
    }

    /// Send DTMF via RFC 2833.
    async fn send_digit_end(&self, channel: &mut Channel, digit: char, duration: u32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(&channel.name)
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        let rtp_guard = priv_data.rtp.lock().await;
        let rtp = rtp_guard
            .as_ref()
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
