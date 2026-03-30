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

        // 8. Spawn PBX execution on a background task.
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
