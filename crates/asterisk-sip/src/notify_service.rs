//! Global NOTIFY sending service.
//!
//! Allows AMI and CLI code to send arbitrary SIP NOTIFY messages
//! for active channels by looking up their SIP session state.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::RwLock;
use tracing::{debug, info, warn};

use crate::notify::{build_notify_from_template_with_via, NotifyConfig, NotifyTemplate};
use crate::transport::SipTransport;

/// Per-channel SIP state needed for in-dialog NOTIFY.
#[derive(Debug, Clone)]
pub struct ChannelSipState {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: String,
    pub local_uri: String,
    pub remote_target: String,
    pub remote_addr: SocketAddr,
    pub local_seq: u32,
}

/// Global notify service that enables sending NOTIFY from anywhere.
pub struct NotifyService {
    /// Channel name -> SIP state mapping.
    channel_states: RwLock<HashMap<String, ChannelSipState>>,
    /// SIP transport for sending messages.
    transport: OnceLock<Arc<dyn SipTransport>>,
    /// Local SIP address.
    local_addr: OnceLock<SocketAddr>,
    /// NOTIFY template configuration.
    notify_config: NotifyConfig,
}

impl NotifyService {
    fn new() -> Self {
        Self {
            channel_states: RwLock::new(HashMap::new()),
            transport: OnceLock::new(),
            local_addr: OnceLock::new(),
            notify_config: NotifyConfig::new(),
        }
    }

    /// Set the SIP transport (called during startup).
    pub fn set_transport(&self, transport: Arc<dyn SipTransport>) {
        let _ = self.transport.set(transport);
    }

    /// Set the local SIP address.
    pub fn set_local_addr(&self, addr: SocketAddr) {
        let _ = self.local_addr.set(addr);
    }

    /// Register a channel's SIP state for NOTIFY sending.
    pub fn register_channel(&self, channel_name: &str, state: ChannelSipState) {
        debug!(channel = channel_name, call_id = %state.call_id, "Registered channel for NOTIFY");
        self.channel_states
            .write()
            .insert(channel_name.to_string(), state);
    }

    /// Unregister a channel (on hangup).
    pub fn unregister_channel(&self, channel_name: &str) {
        self.channel_states.write().remove(channel_name);
    }

    /// Update the remote tag for a channel (called when 1xx/2xx response arrives).
    pub fn update_remote_tag(&self, channel_name: &str, remote_tag: &str) {
        if let Some(state) = self.channel_states.write().get_mut(channel_name) {
            state.remote_tag = remote_tag.to_string();
        }
    }

    /// Get the notify config for loading templates.
    pub fn notify_config(&self) -> &NotifyConfig {
        &self.notify_config
    }

    /// Send a NOTIFY for a channel with the given variables.
    pub fn send_notify_for_channel(
        &self,
        channel_name: &str,
        variables: &[(String, String)],
    ) -> Result<(), String> {
        let state = {
            let states = self.channel_states.read();
            states
                .get(channel_name)
                .cloned()
                .ok_or_else(|| format!("Channel '{}' not found", channel_name))?
        };

        let transport = self
            .transport
            .get()
            .ok_or_else(|| "SIP transport not initialized".to_string())?
            .clone();

        // Build the NOTIFY from variables
        let mut template = NotifyTemplate::new("adhoc");
        for (name, value) in variables {
            template.add_item(name, value);
        }

        let via_addr = self.local_addr.get().map(|a| a.to_string()).unwrap_or_else(|| "127.0.0.1:5060".to_string());

        let notify = build_notify_from_template_with_via(
            &template,
            &state.remote_target,
            &state.local_uri,
            &state.call_id,
            &state.local_tag,
            &state.remote_tag,
            state.local_seq + 1,
            &via_addr,
        );

        let remote_addr = state.remote_addr;
        info!(
            channel = channel_name,
            call_id = %state.call_id,
            remote = %remote_addr,
            "Sending in-dialog NOTIFY"
        );

        tokio::spawn(async move {
            if let Err(e) = transport.send(&notify, remote_addr).await {
                warn!("Failed to send NOTIFY: {}", e);
            }
        });

        Ok(())
    }

    /// Send a NOTIFY to an endpoint using a named template.
    pub fn send_notify_to_endpoint(
        &self,
        template_name: &str,
        endpoint_name: &str,
    ) -> Result<(), String> {
        eprintln!("[DEBUG] send_notify_to_endpoint: template={}, endpoint={}", template_name, endpoint_name);
        let template = self
            .notify_config
            .get_template(template_name)
            .ok_or_else(|| format!("Template '{}' not found", template_name))?;

        let transport = self
            .transport
            .get()
            .ok_or_else(|| "SIP transport not initialized".to_string())?
            .clone();

        // Look up endpoint contact from PJSIP config
        let contact_uri = {
            let config = crate::pjsip_config::get_global_pjsip_config()
                .ok_or_else(|| "PJSIP config not loaded".to_string())?;
            let aor = config
                .find_aor(endpoint_name)
                .ok_or_else(|| format!("AOR for endpoint '{}' not found", endpoint_name))?;
            aor.contact
                .first()
                .cloned()
                .ok_or_else(|| format!("No contact for endpoint '{}'", endpoint_name))?
        };

        // Parse the contact URI to get the remote address
        let remote_addr = parse_contact_addr(&contact_uri)?;
        let local_addr = self.local_addr.get().copied().unwrap_or_else(|| "127.0.0.1:5060".parse().unwrap());
        let from_uri = format!("sip:asterisk@{}", local_addr);
        let notify = crate::notify::build_notify_adhoc(
            &contact_uri,
            &from_uri,
            &template
                .items
                .iter()
                .map(|i| (i.name.clone(), i.value.clone()))
                .collect::<Vec<_>>(),
            &local_addr.to_string(),
        );

        let endpoint_name = endpoint_name.to_string();
        info!(
            template = template_name,
            endpoint = %endpoint_name,
            remote = %remote_addr,
            "Sending NOTIFY to endpoint"
        );

        eprintln!("[DEBUG] Spawning NOTIFY send to {} at {}", endpoint_name, remote_addr);
        tokio::spawn(async move {
            if let Err(e) = transport.send(&notify, remote_addr).await {
                warn!("Failed to send NOTIFY to {}: {}", endpoint_name, e);
                eprintln!("[DEBUG] Failed to send NOTIFY: {}", e);
            } else {
                eprintln!("[DEBUG] NOTIFY sent successfully to {}", endpoint_name);
            }
        });

        Ok(())
    }
}

fn parse_contact_addr(uri: &str) -> Result<SocketAddr, String> {
    // Parse "sip:user@host:port" or "sip:user@host"
    let stripped = uri
        .strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri);
    let host_part = if let Some((_user, host)) = stripped.split_once('@') {
        host
    } else {
        stripped
    };
    // Remove any URI parameters
    let host_part = host_part.split(';').next().unwrap_or(host_part);

    if host_part.contains(':') {
        host_part
            .parse()
            .map_err(|e| format!("Invalid address '{}': {}", host_part, e))
    } else {
        format!("{}:5060", host_part)
            .parse()
            .map_err(|e| format!("Invalid address '{}': {}", host_part, e))
    }
}

/// Global notify service instance.
static NOTIFY_SERVICE: LazyLock<NotifyService> = LazyLock::new(NotifyService::new);

/// Get the global notify service.
pub fn global_notify_service() -> &'static NotifyService {
    &NOTIFY_SERVICE
}
