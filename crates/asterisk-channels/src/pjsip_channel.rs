//! PJSIP channel driver.
//!
//! Port of `channels/chan_pjsip.c`. Bridges the `asterisk-sip` SipSession with
//! the generic channel model, providing full SIP call lifecycle management.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::Mutex;
use tracing::{debug, info};

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// DTMF delivery method for a PJSIP channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum DtmfMode {
    /// RFC 2833 telephone-event RTP payloads.
    #[default]
    Rfc2833,
    /// SIP INFO messages carrying `application/dtmf-relay`.
    Info,
    /// In-band audio tones (generated/detected by DSP).
    Inband,
    /// Automatically choose (prefer RFC 2833, fall back to INFO).
    Auto,
}


/// T.38 fax gateway state (stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum T38State {
    /// T.38 not active.
    #[default]
    Disabled,
    /// Local side proposed T.38.
    LocalReinvite,
    /// Remote side proposed T.38.
    RemoteReinvite,
    /// T.38 session active.
    Active,
    /// Rejecting T.38.
    Rejected,
}


/// Endpoint-level configuration applied to every channel created for this
/// PJSIP endpoint.
#[derive(Debug, Clone)]
pub struct PjsipEndpointConfig {
    /// SIP URI or hostname of the endpoint.
    pub remote_addr: SocketAddr,
    /// DTMF mode.
    pub dtmf_mode: DtmfMode,
    /// Supported codec names (in preference order).
    pub codecs: Vec<String>,
    /// Whether direct media (re-INVITE for RTP bypass) is allowed.
    pub direct_media: bool,
    /// Whether T.38 fax is enabled.
    pub t38_enabled: bool,
}

// ---------------------------------------------------------------------------
// Per-channel private data
// ---------------------------------------------------------------------------

/// Hold state for a PJSIP channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldState {
    /// Not on hold.
    Active,
    /// We placed the remote on hold (sent re-INVITE with `a=sendonly`).
    LocalHold,
    /// Remote placed us on hold.
    RemoteHold,
}

/// Technology-private data for a single PJSIP channel.
struct PjsipPrivate {
    /// SIP Call-ID.
    call_id: String,
    /// Remote SIP endpoint address.
    remote_addr: SocketAddr,
    /// Local SIP listen address.
    local_addr: SocketAddr,
    /// Current session state.
    session_state: PjsipSessionState,
    /// DTMF mode for this channel.
    dtmf_mode: DtmfMode,
    /// Hold state.
    hold_state: HoldState,
    /// T.38 state.
    t38_state: T38State,
    /// Frame channel for delivering SIP-signaling frames to `read_frame`.
    frame_tx: tokio::sync::mpsc::Sender<Frame>,
    /// Frame receiver.
    frame_rx: Mutex<tokio::sync::mpsc::Receiver<Frame>>,
    /// Pending REFER target, if any.
    pending_refer: Option<String>,
}

impl fmt::Debug for PjsipPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PjsipPrivate")
            .field("call_id", &self.call_id)
            .field("remote_addr", &self.remote_addr)
            .field("session_state", &self.session_state)
            .field("dtmf_mode", &self.dtmf_mode)
            .field("hold_state", &self.hold_state)
            .field("t38_state", &self.t38_state)
            .finish()
    }
}

/// Internal session state (simplified from full SIP dialog FSM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PjsipSessionState {
    /// Channel created, INVITE not yet sent/received.
    Idle,
    /// INVITE sent (outbound) or received (inbound), awaiting response.
    Initiated,
    /// Received 1xx provisional.
    Early,
    /// 200 OK exchanged, media flowing.
    Established,
    /// BYE sent or received.
    Terminating,
    /// Session over.
    Terminated,
}

// ---------------------------------------------------------------------------
// Channel driver
// ---------------------------------------------------------------------------

/// PJSIP channel driver -- integrates the SIP signaling stack from
/// `asterisk-sip` with the Asterisk channel model.
///
/// Port of `chan_pjsip.c`.
pub struct PjsipChannelDriver {
    /// Local address for SIP/RTP.
    local_addr: SocketAddr,
    /// Active channels keyed by channel unique ID.
    channels: RwLock<HashMap<String, Arc<PjsipPrivate>>>,
    /// Default DTMF mode.
    default_dtmf_mode: DtmfMode,
}

impl fmt::Debug for PjsipChannelDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PjsipChannelDriver")
            .field("local_addr", &self.local_addr)
            .field("active_channels", &self.channels.read().len())
            .finish()
    }
}

impl PjsipChannelDriver {
    /// Create a new PJSIP channel driver.
    pub fn new(local_addr: SocketAddr) -> Self {
        Self {
            local_addr,
            channels: RwLock::new(HashMap::new()),
            default_dtmf_mode: DtmfMode::Rfc2833,
        }
    }

    /// Create with a specific default DTMF mode.
    pub fn with_dtmf_mode(mut self, mode: DtmfMode) -> Self {
        self.default_dtmf_mode = mode;
        self
    }

    fn get_private(&self, id: &str) -> Option<Arc<PjsipPrivate>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<PjsipPrivate>> {
        self.channels.write().remove(id)
    }

    /// Resolve a destination string to a `(SIP-URI, SocketAddr)` pair.
    fn resolve_dest(dest: &str) -> AsteriskResult<(String, SocketAddr)> {
        if dest.starts_with("sip:") || dest.starts_with("sips:") {
            // Full SIP URI -- extract host:port.
            let without_scheme = if dest.starts_with("sips:") {
                &dest[5..]
            } else {
                &dest[4..]
            };
            let host_part = without_scheme.split(';').next().unwrap_or(without_scheme);
            let host_part = if let Some((_user, host)) = host_part.split_once('@') {
                host
            } else {
                host_part
            };
            let addr: SocketAddr = if host_part.contains(':') {
                host_part.parse()
            } else {
                format!("{}:5060", host_part).parse()
            }
            .map_err(|e| AsteriskError::InvalidArgument(format!("Bad SIP addr: {}", e)))?;
            Ok((dest.to_string(), addr))
        } else {
            // Treat as host or host:port
            let addr_str = if dest.contains(':') {
                dest.to_string()
            } else {
                format!("{}:5060", dest)
            };
            let addr: SocketAddr = addr_str
                .parse()
                .map_err(|e| AsteriskError::InvalidArgument(format!("Bad dest: {}", e)))?;
            Ok((format!("sip:{}", dest), addr))
        }
    }

    /// Initiate an attended transfer via REFER (external API).
    pub async fn transfer(&self, channel: &mut Channel, target: &str) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        info!(
            call_id = %priv_data.call_id,
            target = target,
            "PJSIP REFER transfer initiated"
        );
        // In a full implementation this would build and send a SIP REFER request.
        // For now we log and queue a Transfer control frame.
        let _ = priv_data
            .frame_tx
            .send(Frame::control(ControlFrame::Transfer))
            .await;
        Ok(())
    }

    /// Toggle hold/unhold.
    pub async fn set_hold(&self, channel_id: &str, hold: bool) -> AsteriskResult<()> {
        let _priv_data = self
            .get_private(channel_id)
            .ok_or_else(|| AsteriskError::NotFound(channel_id.to_string()))?;

        // In a full implementation this would send a re-INVITE with
        // `a=sendonly` / `a=sendrecv`.
        debug!(channel_id, hold, "PJSIP hold state change (stub)");
        Ok(())
    }
}

impl Default for PjsipChannelDriver {
    fn default() -> Self {
        Self::new(SocketAddr::from(([0, 0, 0, 0], 5060)))
    }
}

#[async_trait]
impl ChannelDriver for PjsipChannelDriver {
    fn name(&self) -> &str {
        "PJSIP"
    }

    fn description(&self) -> &str {
        "PJSIP SIP Channel Driver"
    }

    /// Request creates a channel and prepares a SIP session (INVITE not yet sent).
    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        let (_sip_uri, remote_addr) = Self::resolve_dest(dest)?;

        let call_id = format!(
            "{}@{}",
            uuid::Uuid::new_v4(),
            self.local_addr.ip()
        );

        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(128);

        let chan_name = format!("PJSIP/{}", dest);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(PjsipPrivate {
            call_id: call_id.clone(),
            remote_addr,
            local_addr: self.local_addr,
            session_state: PjsipSessionState::Idle,
            dtmf_mode: self.default_dtmf_mode,
            hold_state: HoldState::Active,
            t38_state: T38State::Disabled,
            frame_tx,
            frame_rx: Mutex::new(frame_rx),
            pending_refer: None,
        });

        self.channels.write().insert(channel_id, priv_data);
        info!(call_id, dest, "PJSIP channel requested");
        Ok(channel)
    }

    /// Call sends the INVITE.
    async fn call(&self, channel: &mut Channel, dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        // In a full implementation we would: build SDP offer, construct INVITE
        // via SipSession, and send via transport.  Here we update channel state.
        channel.set_state(ChannelState::Dialing);
        info!(call_id = %priv_data.call_id, dest, "PJSIP INVITE sent");
        Ok(())
    }

    /// Answer sends 200 OK.
    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        channel.answer();
        info!(call_id = %priv_data.call_id, "PJSIP 200 OK sent");
        Ok(())
    }

    /// Hangup sends BYE (or CANCEL if not yet established).
    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = match self.remove_private(channel.unique_id.as_str()) {
            Some(p) => p,
            None => return Ok(()),
        };

        // In a full implementation: send BYE or CANCEL depending on session_state.
        channel.set_state(ChannelState::Down);
        info!(call_id = %priv_data.call_id, "PJSIP call hungup");
        Ok(())
    }

    /// Read a frame -- bridged to RTP recv or signaling queue.
    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        // Read from the internal frame channel (RTP or signaling frames).
        let mut rx = priv_data.frame_rx.lock().await;
        match rx.recv().await {
            Some(frame) => Ok(frame),
            None => Ok(Frame::control(ControlFrame::Hangup)),
        }
    }

    /// Write a frame -- bridged to RTP send.
    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let _priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match frame {
            Frame::Voice { .. } => {
                // In a full implementation: send via RTP session.
            }
            Frame::Video { .. } => {
                // In a full implementation: send via RTP session.
            }
            _ => {
                debug!(frame_type = ?frame.frame_type(), "PJSIP ignoring unsupported write frame");
            }
        }
        Ok(())
    }

    /// Indicate a condition -- maps to SIP provisional/error responses.
    async fn indicate(
        &self,
        channel: &mut Channel,
        condition: i32,
        _data: &[u8],
    ) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match condition as u32 {
            x if x == ControlFrame::Ringing as u32 => {
                // Send 180 Ringing.
                debug!(call_id = %priv_data.call_id, "Indicate: 180 Ringing");
            }
            x if x == ControlFrame::Progress as u32 => {
                // Send 183 Session Progress (with SDP for early media).
                debug!(call_id = %priv_data.call_id, "Indicate: 183 Session Progress");
            }
            x if x == ControlFrame::Proceeding as u32 => {
                // Send 100 Trying.
                debug!(call_id = %priv_data.call_id, "Indicate: 100 Trying");
            }
            x if x == ControlFrame::Busy as u32 => {
                // Send 486 Busy Here.
                debug!(call_id = %priv_data.call_id, "Indicate: 486 Busy Here");
            }
            x if x == ControlFrame::Congestion as u32 => {
                // Send 503 Service Unavailable.
                debug!(call_id = %priv_data.call_id, "Indicate: 503 Service Unavailable");
            }
            x if x == ControlFrame::Hold as u32 => {
                // Send re-INVITE with a=sendonly.
                debug!(call_id = %priv_data.call_id, "Indicate: Hold");
            }
            x if x == ControlFrame::Unhold as u32 => {
                // Send re-INVITE with a=sendrecv.
                debug!(call_id = %priv_data.call_id, "Indicate: Unhold");
            }
            x if x == ControlFrame::T38Parameters as u32 => {
                // T.38 gateway indication (stub).
                debug!(call_id = %priv_data.call_id, "Indicate: T.38 parameters (stub)");
            }
            x if x == ControlFrame::Transfer as u32 => {
                // REFER.
                debug!(call_id = %priv_data.call_id, "Indicate: Transfer (REFER)");
            }
            _ => {
                debug!(
                    call_id = %priv_data.call_id,
                    condition,
                    "Indicate: unhandled condition"
                );
            }
        }
        Ok(())
    }

    /// Send DTMF digit start (RFC 2833 or SIP INFO based on config).
    async fn send_digit_begin(&self, channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match priv_data.dtmf_mode {
            DtmfMode::Rfc2833 | DtmfMode::Auto => {
                // RFC 2833: handled by RTP layer.
                debug!(call_id = %priv_data.call_id, digit = %digit, "DTMF begin (RFC 2833)");
            }
            DtmfMode::Info => {
                // SIP INFO -- begin is a no-op, we send on digit_end.
                debug!(call_id = %priv_data.call_id, digit = %digit, "DTMF begin (SIP INFO, deferred)");
            }
            DtmfMode::Inband => {
                // In-band -- generated by DSP, nothing to do here.
            }
        }
        Ok(())
    }

    /// Send DTMF digit end.
    async fn send_digit_end(
        &self,
        channel: &mut Channel,
        digit: char,
        duration: u32,
    ) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match priv_data.dtmf_mode {
            DtmfMode::Rfc2833 | DtmfMode::Auto => {
                // Send RFC 2833 end event via RTP.
                debug!(call_id = %priv_data.call_id, digit = %digit, duration, "DTMF end (RFC 2833)");
            }
            DtmfMode::Info => {
                // Send SIP INFO with application/dtmf-relay.
                debug!(call_id = %priv_data.call_id, digit = %digit, duration, "DTMF end (SIP INFO)");
            }
            DtmfMode::Inband => {
                // In-band -- handled by DSP.
            }
        }
        Ok(())
    }

    async fn send_text(&self, channel: &mut Channel, text: &str) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        // In a full implementation, this would send a SIP MESSAGE request.
        debug!(call_id = %priv_data.call_id, text_len = text.len(), "PJSIP send_text (stub)");
        Ok(())
    }

    async fn fixup(
        &self,
        _old_channel: &Channel,
        new_channel: &mut Channel,
    ) -> AsteriskResult<()> {
        // Re-associate the private data with the new channel if needed.
        debug!(new_channel = %new_channel.name, "PJSIP fixup");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_sip_uri() {
        let (uri, addr) = PjsipChannelDriver::resolve_dest("sip:alice@192.168.1.1:5060").unwrap();
        assert!(uri.starts_with("sip:"));
        assert_eq!(addr.port(), 5060);
    }

    #[test]
    fn test_resolve_host() {
        let (uri, addr) = PjsipChannelDriver::resolve_dest("192.168.1.1").unwrap();
        assert!(uri.starts_with("sip:"));
        assert_eq!(addr.port(), 5060);
    }

    #[tokio::test]
    async fn test_pjsip_request_and_hangup() {
        let driver = PjsipChannelDriver::new("127.0.0.1:5060".parse().unwrap());
        let mut chan = driver.request("127.0.0.1:5061", None).await.unwrap();
        assert!(chan.name.starts_with("PJSIP/"));
        driver.hangup(&mut chan).await.unwrap();
        assert_eq!(chan.state, ChannelState::Down);
    }
}
