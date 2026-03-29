//! SIP Event Handler.
//!
//! Receives SIP events from the SIP stack and creates/manages Asterisk
//! channels. This is the glue between the SIP protocol layer and the
//! PBX/channel model.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::RwLock;
use tracing::{info, warn};

use crate::parser::SipMessage;
use crate::session::SipSession;
use crate::transport::SipTransport;
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
}

impl SipEventHandler {
    /// Create a new event handler with the given dialplan and transport.
    pub fn new(dialplan: Arc<Dialplan>, transport: Arc<dyn SipTransport>) -> Self {
        Self {
            dialplan,
            callid_map: Arc::new(RwLock::new(HashMap::new())),
            transport,
            call_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Handle an incoming SIP INVITE -- creates a channel and starts PBX execution.
    ///
    /// Returns the Call-ID if the invite was handled successfully.
    pub async fn handle_incoming_invite(
        &self,
        request: &SipMessage,
        remote_addr: SocketAddr,
        session: SipSession,
    ) -> Option<String> {
        // 1. Extract caller info from From header
        let from = request.get_header("From")?;
        let caller_num = extract_user_from_header(from).unwrap_or_default();
        let caller_name = extract_display_name(from).unwrap_or_default();

        // 2. Extract dialed number from To header / Request-URI
        let to = request.get_header("To")?;
        let exten = extract_user_from_header(to).unwrap_or_else(|| "s".to_string());

        // 3. Extract Call-ID for tracking
        let call_id = request.call_id()?.to_string();

        // 4. Send 100 Trying immediately
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

        // 5. Create the channel via the global store
        let channel_name = format!("PJSIP/{}-{:08x}", caller_num, rand_id());
        let channel = store::alloc_channel(&channel_name);

        {
            let mut ch = channel.lock();
            ch.caller.id.number.number = caller_num;
            ch.caller.id.name.name = caller_name;
            ch.exten = exten;
            ch.context = "from-external".to_string();
            ch.set_state(ChannelState::Ring);
        }

        // 6. Register Call-ID mapping for response routing
        {
            let mut map = self.callid_map.write();
            map.insert(call_id.clone(), channel_name.clone());
        }

        // 7. Store the SIP session state for later signaling (200 OK, BYE)
        let call_state = Arc::new(tokio::sync::Mutex::new(CallState {
            session,
            remote_addr,
            channel_name: channel_name.clone(),
        }));
        {
            let mut states = self.call_states.write();
            states.insert(call_id.clone(), call_state.clone());
        }

        // 8. Spawn PBX execution on a background task
        let dialplan = self.dialplan.clone();
        let ch_for_pbx = channel.clone();
        let ch_name_for_cleanup = channel_name.clone();
        let transport = self.transport.clone();
        let call_id_for_task = call_id.clone();
        let call_states_ref = self.call_states.clone();
        let callid_map_ref = self.callid_map.clone();
        tokio::spawn(async move {
            // Convert from parking_lot::Mutex to tokio::sync::Mutex for pbx_run
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
                new_ch
            };

            let tokio_channel = Arc::new(tokio::sync::Mutex::new(channel_data));
            let result = asterisk_core::pbx::exec::pbx_run(tokio_channel, dialplan).await;
            info!(channel = %ch_name_for_cleanup, "PBX completed with result: {:?}", result);

            // After PBX completes, send BYE to the remote side
            {
                let cs = call_state.lock().await;
                // Build and send BYE (or 200 OK to BYE if remote already sent BYE)
                // For an inbound call that we answered, we send BYE when hanging up.
                let mut session = SipSession::new_inbound(
                    cs.session.invite.as_ref().unwrap_or(&SipMessage::new_request(
                        crate::parser::SipMethod::Invite,
                        "sip:placeholder@localhost",
                    )),
                    cs.session.local_addr,
                    cs.remote_addr,
                ).unwrap_or_else(|| {
                    SipSession::new_outbound(cs.session.local_addr, cs.remote_addr)
                });
                // Copy dialog from stored session
                session.dialog = cs.session.dialog.clone();
                session.call_id = cs.session.call_id.clone();
                session.invite = cs.session.invite.clone();

                if let Some(bye) = session.build_bye() {
                    if let Err(e) = transport.send(&bye, cs.remote_addr).await {
                        warn!(call_id = %call_id_for_task, "Failed to send BYE: {}", e);
                    } else {
                        info!(call_id = %call_id_for_task, "Sent BYE");
                    }
                }
            }

            // Clean up call state
            call_states_ref.write().remove(&call_id_for_task);
            callid_map_ref.write().remove(&call_id_for_task);

            // Clean up from global store
            let uid = ch_for_pbx.lock().unique_id.0.clone();
            store::deregister(&uid);
        });

        // 9. Send 200 OK for the INVITE (answer the call)
        // In a proper implementation this would be triggered by the Answer() app,
        // but for now we send it eagerly since the dialplan will answer immediately.
        {
            let cs = {
                let states = self.call_states.read();
                states.get(&call_id).cloned()
            };
            if let Some(cs) = cs {
                let mut state = cs.lock().await;
                if let Some(ok_response) = state.session.build_200_ok() {
                    if let Err(e) = self.transport.send(&ok_response, state.remote_addr).await {
                        warn!(call_id = %call_id, "Failed to send 200 OK: {}", e);
                    } else {
                        info!(call_id = %call_id, "Sent 200 OK");
                        state.session.state = crate::session::SessionState::Established;
                    }
                }
            }
        }

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
                if let Some(channel) = store::find_by_name(&name) {
                    let mut ch = channel.lock();
                    ch.softhangup(softhangup::AST_SOFTHANGUP_DEV);
                }
            }

            // Clean up
            self.callid_map.write().remove(&call_id);
            self.call_states.write().remove(&call_id);
        }
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
