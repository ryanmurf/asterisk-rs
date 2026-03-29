//! ICE-integrated RTP transport.
//!
//! Wraps an RTP session with ICE connectivity. Candidate gathering happens
//! before SDP offer generation, and after ICE completes the RTP flow uses
//! the nominated candidate pair. Falls back to direct RTP if ICE fails.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::ice::{
    IceAgent, IceCandidate, IceMode, IceRole, IceState,
    COMPONENT_RTP, COMPONENT_RTCP,
};
use crate::stun::{self, MessageClass, StunMessage};
use crate::turn::TurnClient;

// ---------------------------------------------------------------------------
// ICE RTP Transport
// ---------------------------------------------------------------------------

/// An RTP transport that uses ICE for connectivity.
///
/// Manages the ICE agent lifecycle alongside the RTP session,
/// performing candidate gathering, connectivity checks, and
/// media path establishment.
pub struct IceRtpTransport {
    /// The ICE agent managing candidates and checks.
    pub agent: IceAgent,
    /// RTP socket.
    pub rtp_socket: Option<Arc<UdpSocket>>,
    /// RTCP socket (if separate from RTP).
    pub rtcp_socket: Option<Arc<UdpSocket>>,
    /// STUN server for srflx candidates.
    pub stun_server: Option<SocketAddr>,
    /// TURN client for relay candidates.
    pub turn_client: Option<TurnClient>,
    /// Whether ICE is enabled.
    pub ice_enabled: bool,
    /// Fallback remote address (from SDP c= line, used if ICE fails).
    pub fallback_addr: Option<SocketAddr>,
    /// The active remote address for RTP (set after ICE completes).
    pub active_remote_addr: Option<SocketAddr>,
    /// The active remote address for RTCP.
    pub active_remote_rtcp_addr: Option<SocketAddr>,
}

impl std::fmt::Debug for IceRtpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IceRtpTransport")
            .field("ice_enabled", &self.ice_enabled)
            .field("state", &self.agent.state)
            .field("active_remote_addr", &self.active_remote_addr)
            .field("fallback_addr", &self.fallback_addr)
            .finish()
    }
}

impl IceRtpTransport {
    /// Create a new ICE RTP transport.
    pub fn new(mode: IceMode, role: IceRole) -> Self {
        let num_components = match mode {
            IceMode::Full => 1, // RTP only (RTCP mux is common now)
            IceMode::Lite => 1,
        };

        Self {
            agent: IceAgent::new(mode, role, num_components),
            rtp_socket: None,
            rtcp_socket: None,
            stun_server: None,
            turn_client: None,
            ice_enabled: true,
            fallback_addr: None,
            active_remote_addr: None,
            active_remote_rtcp_addr: None,
        }
    }

    /// Create a new ICE RTP transport with RTP and RTCP components.
    pub fn new_with_rtcp(mode: IceMode, role: IceRole) -> Self {
        Self {
            agent: IceAgent::new(mode, role, 2),
            rtp_socket: None,
            rtcp_socket: None,
            stun_server: None,
            turn_client: None,
            ice_enabled: true,
            fallback_addr: None,
            active_remote_addr: None,
            active_remote_rtcp_addr: None,
        }
    }

    /// Disable ICE (fall back to direct RTP).
    pub fn disable_ice(&mut self) {
        self.ice_enabled = false;
    }

    /// Set the STUN server for srflx candidate gathering.
    pub fn set_stun_server(&mut self, addr: SocketAddr) {
        self.stun_server = Some(addr);
    }

    /// Set the TURN client for relay candidate gathering.
    pub fn set_turn_client(&mut self, client: TurnClient) {
        self.turn_client = Some(client);
    }

    /// Set the RTP socket.
    pub fn set_rtp_socket(&mut self, socket: Arc<UdpSocket>) {
        self.rtp_socket = Some(socket);
    }

    /// Set the RTCP socket.
    pub fn set_rtcp_socket(&mut self, socket: Arc<UdpSocket>) {
        self.rtcp_socket = Some(socket);
    }

    /// Set the fallback remote address (from SDP).
    pub fn set_fallback_addr(&mut self, addr: SocketAddr) {
        self.fallback_addr = Some(addr);
    }

    /// Get the effective remote address for sending RTP.
    ///
    /// Returns the ICE-nominated address if available, otherwise the
    /// fallback address from SDP.
    pub fn effective_remote_addr(&self) -> Option<SocketAddr> {
        if self.ice_enabled {
            self.active_remote_addr.or(self.fallback_addr)
        } else {
            self.fallback_addr
        }
    }

    /// Gather candidates for this transport.
    ///
    /// Should be called before generating the SDP offer.
    pub async fn gather_candidates(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.ice_enabled {
            return Ok(());
        }

        // Gather host candidates from the RTP socket
        if let Some(ref socket) = self.rtp_socket {
            let local_addr = socket.local_addr()?;
            self.agent.gather_host_candidates(&[local_addr]);
        }

        // Gather RTCP host candidates
        if let Some(ref socket) = self.rtcp_socket {
            if self.agent.num_components >= 2 {
                let local_addr = socket.local_addr()?;
                let candidate = IceCandidate::new_host(local_addr, COMPONENT_RTCP, 65535);
                self.agent.local_candidates.push(candidate);
            }
        }

        // Gather srflx candidates
        if let (Some(stun_server), Some(ref socket)) = (self.stun_server, &self.rtp_socket) {
            match self.agent.gather_srflx_candidates(stun_server, socket, COMPONENT_RTP).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(error = %e, "failed to gather srflx candidates");
                }
            }
        }

        // Gather relay candidates
        if let Some(ref mut turn_client) = self.turn_client {
            match self.agent.gather_relay_candidates(turn_client, COMPONENT_RTP).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(error = %e, "failed to gather relay candidates");
                }
            }
        }

        debug!(
            candidates = self.agent.local_candidates.len(),
            "ICE candidate gathering complete"
        );

        Ok(())
    }

    /// Start ICE connectivity checks.
    pub fn start_checks(&mut self) {
        if !self.ice_enabled {
            return;
        }
        self.agent.start_checking();
    }

    /// Run one round of connectivity checks.
    ///
    /// Returns true if there are more checks to perform.
    pub async fn run_check_cycle(&mut self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.ice_enabled || self.agent.state != IceState::Checking {
            return Ok(false);
        }

        let socket = match &self.rtp_socket {
            Some(s) => s.clone(),
            None => return Ok(false),
        };

        // Get the next pair to check
        let pair_idx = match self.agent.next_check_pair() {
            Some(idx) => idx,
            None => return Ok(false),
        };

        // Build and send the connectivity check
        let pair = &self.agent.check_list[pair_idx];
        let request = self.agent.build_check_request(pair);
        let tid = request.transaction_id.clone();
        let remote_addr = pair.remote.address;

        // Serialize with integrity using remote password
        let key = self.agent.remote_pwd.as_bytes();
        let bytes = request.to_bytes_with_integrity_and_fingerprint(key);

        socket.send_to(&bytes, remote_addr).await?;
        self.agent.mark_in_progress(pair_idx, tid);

        debug!(
            remote = %remote_addr,
            pair_idx = pair_idx,
            "ICE sent connectivity check"
        );

        // Wait for response with timeout
        let mut buf = vec![0u8; stun::MAX_MESSAGE_SIZE];
        let timeout = Duration::from_millis(crate::ice::CHECK_TIMEOUT_MS);

        match tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, _from))) => {
                if stun::is_stun_message(&buf[..len]) {
                    let response = StunMessage::parse(&buf[..len])?;
                    if let Some(idx) = self.agent.find_pair_by_transaction(&response.transaction_id)
                    {
                        self.agent.process_check_response(&response, idx);
                    }
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "ICE check recv error");
                self.agent.check_list[pair_idx].state = crate::ice::PairState::Failed;
            }
            Err(_) => {
                // Timeout
                let pair = &mut self.agent.check_list[pair_idx];
                pair.retransmissions += 1;
                if pair.retransmissions >= crate::ice::MAX_CHECK_RETRANSMISSIONS {
                    pair.state = crate::ice::PairState::Failed;
                    debug!(pair_idx = pair_idx, "ICE check timed out (max retries)");
                } else {
                    pair.state = crate::ice::PairState::Waiting;
                }
            }
        }

        // Update active address if ICE completed
        if self.agent.state == IceState::Completed {
            self.apply_nominated_pair();
        }

        let has_more = self.agent.check_list.iter().any(|p| {
            p.state == crate::ice::PairState::Waiting || p.state == crate::ice::PairState::Frozen
        });

        Ok(has_more)
    }

    /// Process an incoming STUN message on the RTP socket.
    ///
    /// Should be called when a STUN message is received instead of an RTP packet.
    pub async fn process_incoming_stun(
        &mut self,
        data: &[u8],
        source: SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.ice_enabled {
            return Ok(());
        }

        let msg = StunMessage::parse(data)?;

        match msg.class {
            MessageClass::Request => {
                // Verify integrity with local password
                if let Err(e) = msg.verify_integrity(data, self.agent.local_pwd.as_bytes()) {
                    debug!(error = %e, "ICE incoming check failed integrity");
                    return Ok(());
                }

                let socket = match &self.rtp_socket {
                    Some(s) => s.clone(),
                    None => return Ok(()),
                };

                let local_addr = socket.local_addr()?;
                let (response, _triggered_idx) =
                    self.agent.process_incoming_check(&msg, source, local_addr);

                // Send response with integrity using local password
                let key = self.agent.local_pwd.as_bytes();
                let response_bytes = response.to_bytes_with_integrity_and_fingerprint(key);
                socket.send_to(&response_bytes, source).await?;

                if self.agent.state == IceState::Completed {
                    self.apply_nominated_pair();
                }
            }
            MessageClass::SuccessResponse | MessageClass::ErrorResponse => {
                // This is a response to one of our checks
                if let Some(idx) = self.agent.find_pair_by_transaction(&msg.transaction_id) {
                    self.agent.process_check_response(&msg, idx);

                    if self.agent.state == IceState::Completed {
                        self.apply_nominated_pair();
                    }
                }
            }
            MessageClass::Indication => {
                // Binding Indication: keepalive, no response needed
            }
        }

        Ok(())
    }

    /// Apply the nominated pair as the active remote address.
    fn apply_nominated_pair(&mut self) {
        if let Some(pair) = self.agent.nominated_pair(COMPONENT_RTP) {
            let old = self.active_remote_addr;
            self.active_remote_addr = Some(pair.remote.address);
            if old != self.active_remote_addr {
                info!(
                    remote = %pair.remote.address,
                    local = %pair.local.address,
                    "ICE nominated RTP path"
                );
            }
        }
        if let Some(pair) = self.agent.nominated_pair(COMPONENT_RTCP) {
            self.active_remote_rtcp_addr = Some(pair.remote.address);
        }
    }

    /// Check if ICE has completed.
    pub fn is_completed(&self) -> bool {
        !self.ice_enabled || self.agent.state == IceState::Completed
    }

    /// Check if ICE has failed.
    pub fn is_failed(&self) -> bool {
        self.agent.state == IceState::Failed
    }

    /// Get the ICE state.
    pub fn state(&self) -> IceState {
        self.agent.state
    }

    /// Get local ICE credentials.
    pub fn local_credentials(&self) -> (&str, &str) {
        (&self.agent.local_ufrag, &self.agent.local_pwd)
    }

    /// Get local candidates.
    pub fn local_candidates(&self) -> &[IceCandidate] {
        &self.agent.local_candidates
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ice_transport_creation() {
        let transport = IceRtpTransport::new(IceMode::Full, IceRole::Controlling);
        assert!(transport.ice_enabled);
        assert_eq!(transport.agent.state, IceState::Idle);
        assert!(transport.active_remote_addr.is_none());
    }

    #[test]
    fn test_ice_transport_disabled() {
        let mut transport = IceRtpTransport::new(IceMode::Full, IceRole::Controlling);
        transport.disable_ice();
        transport.set_fallback_addr("10.0.0.1:5000".parse().unwrap());

        assert_eq!(
            transport.effective_remote_addr(),
            Some("10.0.0.1:5000".parse().unwrap())
        );
    }

    #[test]
    fn test_ice_transport_effective_addr() {
        let mut transport = IceRtpTransport::new(IceMode::Full, IceRole::Controlling);
        transport.set_fallback_addr("10.0.0.1:5000".parse().unwrap());

        // Before ICE completes, should use fallback
        assert_eq!(
            transport.effective_remote_addr(),
            Some("10.0.0.1:5000".parse().unwrap())
        );

        // After ICE sets active addr
        transport.active_remote_addr = Some("10.0.0.2:6000".parse().unwrap());
        assert_eq!(
            transport.effective_remote_addr(),
            Some("10.0.0.2:6000".parse().unwrap())
        );
    }

    #[test]
    fn test_ice_transport_with_rtcp() {
        let transport = IceRtpTransport::new_with_rtcp(IceMode::Full, IceRole::Controlled);
        assert_eq!(transport.agent.num_components, 2);
    }
}
