//! Inbound SIP REGISTER handling (port of res_pjsip_registrar.c).
//!
//! Manages registration of SIP endpoints. Parses REGISTER requests,
//! extracts Contact headers and expiration values, stores contacts
//! keyed by Address-of-Record (AoR), and sends 200 OK responses with
//! the current contact list.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::{debug, info, warn};

use crate::parser::{
    extract_uri, header_names, SipHeader, SipMessage, SipMethod, SipUri, StartLine,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// AOR (Address-of-Record) configuration that governs registration policy.
#[derive(Debug, Clone)]
pub struct AorConfig {
    /// Default expiration when the client does not specify one (seconds).
    pub default_expiration: u32,
    /// Minimum expiration we will accept.
    pub minimum_expiration: u32,
    /// Maximum expiration we will accept.
    pub maximum_expiration: u32,
    /// Maximum number of contacts allowed for this AoR.
    pub max_contacts: usize,
    /// Whether to remove existing contacts when max is reached.
    pub remove_existing: bool,
    /// Whether Path header support is enabled.
    pub support_path: bool,
    /// Whether to remove unavailable contacts on new REGISTER.
    pub remove_unavailable: bool,
}

impl Default for AorConfig {
    fn default() -> Self {
        Self {
            default_expiration: 3600,
            minimum_expiration: 60,
            maximum_expiration: 7200,
            max_contacts: 10,
            remove_existing: false,
            support_path: false,
            remove_unavailable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Contact / Registration
// ---------------------------------------------------------------------------

/// A single registered contact binding.
#[derive(Debug, Clone)]
pub struct Registration {
    /// The AoR this contact belongs to.
    pub aor: String,
    /// Contact URI (e.g. `sip:alice@10.0.0.1:5060`).
    pub contact_uri: String,
    /// Remaining lifetime in seconds at the time of registration.
    pub expiration: u32,
    /// When this registration was created / last refreshed.
    pub registered_at: Instant,
    /// User-Agent string from the REGISTER request.
    pub user_agent: String,
    /// Path header value (if present and supported).
    pub path: Option<String>,
    /// Call-ID of the REGISTER that created/refreshed this contact.
    pub call_id: String,
    /// CSeq number of the last REGISTER.
    pub cseq: u32,
}

impl Registration {
    /// True when the contact has expired.
    pub fn is_expired(&self) -> bool {
        self.registered_at.elapsed() >= Duration::from_secs(self.expiration as u64)
    }

    /// Remaining time-to-live in seconds (clamped to 0).
    pub fn remaining_seconds(&self) -> u32 {
        let elapsed = self.registered_at.elapsed().as_secs() as u32;
        self.expiration.saturating_sub(elapsed)
    }
}

// ---------------------------------------------------------------------------
// Registrar
// ---------------------------------------------------------------------------

/// The inbound SIP registrar.
///
/// Stores contacts per AoR in memory and applies expiration / policy
/// enforcement that mirrors the Asterisk `res_pjsip_registrar` behaviour.
#[derive(Debug)]
pub struct Registrar {
    /// AoR configs keyed by AoR name (e.g. username, extension).
    aor_configs: RwLock<HashMap<String, AorConfig>>,
    /// Contact bindings keyed by AoR name.
    contacts: RwLock<HashMap<String, Vec<Registration>>>,
}

impl Registrar {
    /// Create a new registrar with no configured AoRs.
    pub fn new() -> Self {
        Self {
            aor_configs: RwLock::new(HashMap::new()),
            contacts: RwLock::new(HashMap::new()),
        }
    }

    /// Register (or update) an AoR configuration.
    pub fn add_aor(&self, name: &str, config: AorConfig) {
        self.aor_configs.write().insert(name.to_string(), config);
    }

    /// Retrieve the configuration for an AoR.
    pub fn get_aor_config(&self, name: &str) -> Option<AorConfig> {
        self.aor_configs.read().get(name).cloned()
    }

    /// Get all current contacts for an AoR.
    pub fn get_contacts(&self, aor: &str) -> Vec<Registration> {
        self.contacts
            .read()
            .get(aor)
            .cloned()
            .unwrap_or_default()
    }

    // ----- request handling ------------------------------------------------

    /// Handle an incoming SIP REGISTER request.
    ///
    /// Returns a fully-formed SIP response (200 OK, 400, 403, etc.).
    pub fn handle_register(&self, request: &SipMessage) -> SipMessage {
        // Must be a REGISTER request.
        if request.method() != Some(SipMethod::Register) {
            return self.make_error(request, 405, "Method Not Allowed");
        }

        // Determine AoR from the To header.
        let to_hdr = match request.to_header() {
            Some(v) => v.to_string(),
            None => return self.make_error(request, 400, "Bad Request"),
        };
        let aor_uri = match extract_uri(&to_hdr) {
            Some(u) => u,
            None => return self.make_error(request, 400, "Bad Request"),
        };

        // Derive AoR name (the user-part, or the whole URI if no user).
        let aor_name = SipUri::parse(&aor_uri)
            .ok()
            .and_then(|u| u.user.clone())
            .unwrap_or_else(|| aor_uri.clone());

        // Look up the AoR config; if not configured we use a liberal default.
        let config = self
            .aor_configs
            .read()
            .get(&aor_name)
            .cloned()
            .unwrap_or_default();

        // Extract Contact headers.
        let contact_headers = request.get_headers(header_names::CONTACT);

        // Extract the Expires header (global fallback).
        let global_expires: Option<u32> = request
            .get_header(header_names::EXPIRES)
            .and_then(|v| v.trim().parse().ok());

        // User-Agent.
        let user_agent = request
            .get_header(header_names::USER_AGENT)
            .unwrap_or("unknown")
            .to_string();

        // Call-ID and CSeq.
        let call_id = request.call_id().unwrap_or("").to_string();
        let cseq_num: u32 = request
            .cseq()
            .and_then(|cs| cs.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);

        // Path header.
        let path = request.get_header("Path").map(|s| s.to_string());

        // Handle wildcard unregister: Contact: *
        if contact_headers.iter().any(|c| c.trim() == "*") {
            let expires = global_expires.unwrap_or(0);
            if expires != 0 {
                return self.make_error(request, 400, "Bad Request");
            }
            self.unregister_all(&aor_name);
            info!(aor = %aor_name, "All contacts removed (wildcard)");
            return self.build_200_ok(request, &aor_name, &config);
        }

        if contact_headers.is_empty() {
            // Query-only REGISTER (no Contact headers) -- just return current bindings.
            return self.build_200_ok(request, &aor_name, &config);
        }

        // Process each Contact header.
        for contact_hdr in &contact_headers {
            let (uri, per_contact_expires) = parse_contact_header(contact_hdr);
            let uri = match uri {
                Some(u) => u,
                None => continue,
            };

            let expiration =
                self.determine_expiration(&config, per_contact_expires, global_expires);

            if expiration == 0 {
                self.unregister(&aor_name, &uri);
                debug!(aor = %aor_name, contact = %uri, "Contact removed");
            } else {
                // Enforce max_contacts.
                let current_count = self
                    .contacts
                    .read()
                    .get(&aor_name)
                    .map(|v| v.len())
                    .unwrap_or(0);

                let is_refresh = self
                    .contacts
                    .read()
                    .get(&aor_name)
                    .map(|v| v.iter().any(|r| r.contact_uri == uri))
                    .unwrap_or(false);

                if !is_refresh
                    && current_count >= config.max_contacts
                    && !config.remove_existing
                {
                    warn!(
                        aor = %aor_name,
                        max = config.max_contacts,
                        "Too many contacts"
                    );
                    return self.make_error(request, 403, "Forbidden");
                }

                // Remove oldest if remove_existing is on and we are at capacity.
                if !is_refresh && current_count >= config.max_contacts && config.remove_existing {
                    self.remove_oldest(&aor_name);
                }

                self.register(Registration {
                    aor: aor_name.clone(),
                    contact_uri: uri.clone(),
                    expiration,
                    registered_at: Instant::now(),
                    user_agent: user_agent.clone(),
                    path: path.clone(),
                    call_id: call_id.clone(),
                    cseq: cseq_num,
                });
                info!(aor = %aor_name, contact = %uri, expires = expiration, "Contact registered");
            }
        }

        self.build_200_ok(request, &aor_name, &config)
    }

    /// Add or refresh a contact binding.
    pub fn register(&self, reg: Registration) {
        let mut map = self.contacts.write();
        let list = map.entry(reg.aor.clone()).or_insert_with(Vec::new);

        // If the contact already exists, update it.
        if let Some(existing) = list.iter_mut().find(|r| r.contact_uri == reg.contact_uri) {
            *existing = reg;
        } else {
            list.push(reg);
        }
    }

    /// Remove a specific contact from an AoR.
    pub fn unregister(&self, aor: &str, contact_uri: &str) {
        let mut map = self.contacts.write();
        if let Some(list) = map.get_mut(aor) {
            list.retain(|r| r.contact_uri != contact_uri);
        }
    }

    /// Remove all contacts for an AoR.
    pub fn unregister_all(&self, aor: &str) {
        let mut map = self.contacts.write();
        map.remove(aor);
    }

    /// Remove expired contacts across all AoRs. Returns the number removed.
    pub fn purge_expired(&self) -> usize {
        let mut map = self.contacts.write();
        let mut removed = 0usize;
        for (_aor, list) in map.iter_mut() {
            let before = list.len();
            list.retain(|r| !r.is_expired());
            removed += before - list.len();
        }
        // Remove empty AoR entries.
        map.retain(|_, v| !v.is_empty());
        removed
    }

    // ----- helpers ---------------------------------------------------------

    fn remove_oldest(&self, aor: &str) {
        let mut map = self.contacts.write();
        if let Some(list) = map.get_mut(aor) {
            if !list.is_empty() {
                // Find the contact with the earliest registered_at.
                let oldest_idx = list
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, r)| r.registered_at)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                list.remove(oldest_idx);
            }
        }
    }

    fn determine_expiration(
        &self,
        config: &AorConfig,
        per_contact: Option<u32>,
        global: Option<u32>,
    ) -> u32 {
        let raw = per_contact
            .or(global)
            .unwrap_or(config.default_expiration);

        if raw == 0 {
            return 0;
        }

        raw.max(config.minimum_expiration)
            .min(config.maximum_expiration)
    }

    fn build_200_ok(&self, request: &SipMessage, aor: &str, config: &AorConfig) -> SipMessage {
        let mut response = request
            .create_response(200, "OK")
            .unwrap_or_else(|_| self.make_error(request, 500, "Internal Server Error"));

        // Purge expired contacts for this AoR before responding.
        {
            let mut map = self.contacts.write();
            if let Some(list) = map.get_mut(aor) {
                list.retain(|r| !r.is_expired());
            }
        }

        // Add Contact headers for every current binding.
        let contacts = self.get_contacts(aor);
        for reg in &contacts {
            let remaining = reg.remaining_seconds();
            response.headers.push(SipHeader {
                name: header_names::CONTACT.to_string(),
                value: format!("<{}>;expires={}", reg.contact_uri, remaining),
            });
        }

        // Add an Expires header with the default for this AoR.
        response.headers.push(SipHeader {
            name: header_names::EXPIRES.to_string(),
            value: config.default_expiration.to_string(),
        });

        response
    }

    fn make_error(&self, request: &SipMessage, code: u16, reason: &str) -> SipMessage {
        request
            .create_response(code, reason)
            .unwrap_or_else(|_| SipMessage {
                start_line: StartLine::Response(crate::parser::StatusLine {
                    version: "SIP/2.0".to_string(),
                    status_code: code,
                    reason_phrase: reason.to_string(),
                }),
                headers: Vec::new(),
                body: String::new(),
            })
    }
}

impl Default for Registrar {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Contact header parsing helpers
// ---------------------------------------------------------------------------

/// Parse a single Contact header value into (uri, per-contact-expires).
///
/// Contact header format examples:
///   `<sip:alice@10.0.0.1>;expires=3600`
///   `sip:alice@10.0.0.1`
///   `*`
fn parse_contact_header(value: &str) -> (Option<String>, Option<u32>) {
    let value = value.trim();
    if value == "*" {
        return (None, None);
    }

    let uri = extract_uri(value);

    // Look for ;expires= parameter.
    let expires = value
        .split(';')
        .find_map(|param| {
            let trimmed = param.trim();
            trimmed
                .strip_prefix("expires=")
                .and_then(|v| v.trim().parse::<u32>().ok())
        });

    (uri, expires)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_register(contact: &str, expires: Option<u32>) -> SipMessage {
        let mut hdrs = format!(
            "REGISTER sip:registrar.example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK776\r\n\
             From: Alice <sip:alice@example.com>;tag=abc\r\n\
             To: Alice <sip:alice@example.com>\r\n\
             Call-ID: reg-test-123\r\n\
             CSeq: 1 REGISTER\r\n\
             Contact: {}\r\n",
            contact
        );
        if let Some(exp) = expires {
            hdrs.push_str(&format!("Expires: {}\r\n", exp));
        }
        hdrs.push_str("Content-Length: 0\r\n\r\n");
        SipMessage::parse(hdrs.as_bytes()).unwrap()
    }

    #[test]
    fn test_register_and_query() {
        let registrar = Registrar::new();
        registrar.add_aor("alice", AorConfig::default());

        let reg = make_register("<sip:alice@10.0.0.1>", Some(3600));
        let resp = registrar.handle_register(&reg);
        assert_eq!(resp.status_code(), Some(200));

        let contacts = registrar.get_contacts("alice");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].contact_uri, "sip:alice@10.0.0.1");
    }

    #[test]
    fn test_unregister_with_zero_expires() {
        let registrar = Registrar::new();
        registrar.add_aor("alice", AorConfig::default());

        // Register.
        let reg = make_register("<sip:alice@10.0.0.1>", Some(3600));
        registrar.handle_register(&reg);
        assert_eq!(registrar.get_contacts("alice").len(), 1);

        // Unregister with expires=0.
        let unreg = make_register("<sip:alice@10.0.0.1>;expires=0", None);
        let resp = registrar.handle_register(&unreg);
        assert_eq!(resp.status_code(), Some(200));
        assert_eq!(registrar.get_contacts("alice").len(), 0);
    }

    #[test]
    fn test_wildcard_unregister() {
        let registrar = Registrar::new();
        registrar.add_aor("alice", AorConfig::default());

        let reg = make_register("<sip:alice@10.0.0.1>", Some(3600));
        registrar.handle_register(&reg);

        // Wildcard unregister.
        let unreg = make_register("*", Some(0));
        let resp = registrar.handle_register(&unreg);
        assert_eq!(resp.status_code(), Some(200));
        assert_eq!(registrar.get_contacts("alice").len(), 0);
    }
}
