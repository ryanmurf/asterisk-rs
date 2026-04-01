//! PJSIP configuration loader.
//!
//! Parses `pjsip.conf` and produces typed configuration objects for
//! transports, endpoints, AORs, auths, identifies, and registrations.
//! Each configuration section is dispatched by its `type=` field.

use std::net::SocketAddr;

use asterisk_config::AsteriskConfig;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Configuration structs
// ---------------------------------------------------------------------------

/// Top-level container for all parsed PJSIP configuration objects.
#[derive(Debug, Clone, Default)]
pub struct PjsipConfig {
    /// Transport configurations (type=transport).
    pub transports: Vec<TransportConfig>,
    /// Endpoint configurations (type=endpoint).
    pub endpoints: Vec<EndpointConfig>,
    /// Address-of-Record configurations (type=aor).
    pub aors: Vec<AorConfig>,
    /// Authentication configurations (type=auth).
    pub auths: Vec<AuthConfig>,
    /// IP-based endpoint identification (type=identify).
    pub identifies: Vec<IdentifyConfig>,
    /// Outbound registration configurations (type=registration).
    pub registrations: Vec<RegistrationConfig>,
}

/// A SIP transport binding (UDP, TCP, TLS, WS, WSS).
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Section name from pjsip.conf.
    pub name: String,
    /// Protocol: udp, tcp, tls, ws, wss.
    pub protocol: String,
    /// Local bind address.
    pub bind: SocketAddr,
    /// External media address (NAT traversal).
    pub external_media_address: Option<String>,
    /// External signaling address (NAT traversal).
    pub external_signaling_address: Option<String>,
    /// TLS certificate file.
    pub cert_file: Option<String>,
    /// TLS private key file.
    pub priv_key_file: Option<String>,
    /// Local network CIDRs (skip NAT for these).
    pub local_net: Vec<String>,
}

/// A SIP endpoint.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Section name from pjsip.conf.
    pub name: String,
    /// Dialplan context for incoming calls.
    pub context: String,
    /// Codecs to disallow.
    pub disallow: Vec<String>,
    /// Codecs to allow.
    pub allow: Vec<String>,
    /// Reference to an auth section name.
    pub auth: Option<String>,
    /// Reference to an AOR section name.
    pub aors: Option<String>,
    /// Whether to allow direct media (RTP bypass).
    pub direct_media: bool,
    /// Whether to use symmetric RTP.
    pub rtp_symmetric: bool,
    /// DTMF mode: rfc4733, inband, info, auto.
    pub dtmf_mode: String,
    /// Force rport in Via header.
    pub force_rport: bool,
    /// Rewrite the Contact header.
    pub rewrite_contact: bool,
    /// ICE support enabled.
    pub ice_support: bool,
    /// Media encryption mode.
    pub media_encryption: String,
    /// Caller ID name.
    pub callerid: Option<String>,
    /// Caller ID number.
    pub callerid_num: Option<String>,
    /// From user for outbound.
    pub from_user: Option<String>,
    /// From domain for outbound.
    pub from_domain: Option<String>,
    /// Transport reference.
    pub transport: Option<String>,
    /// Send RPID/PAI headers.
    pub send_rpid: bool,
    /// Send P-Asserted-Identity header.
    pub send_pai: bool,
    /// Allow transfer.
    pub allow_transfer: bool,
    /// Trust ID inbound (PAI/RPID).
    pub trust_id_inbound: bool,
    /// Allow overlap dialing (send 484 Address Incomplete for partial matches).
    pub allow_overlap: bool,
    /// Account code for CDR/billing.
    pub accountcode: String,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            context: "default".to_string(),
            disallow: Vec::new(),
            allow: Vec::new(),
            auth: None,
            aors: None,
            direct_media: true,
            rtp_symmetric: false,
            dtmf_mode: "rfc4733".to_string(),
            force_rport: true,
            rewrite_contact: false,
            ice_support: false,
            media_encryption: "no".to_string(),
            callerid: None,
            callerid_num: None,
            from_user: None,
            from_domain: None,
            transport: None,
            send_rpid: false,
            send_pai: false,
            allow_transfer: true,
            trust_id_inbound: false,
            allow_overlap: true,
            accountcode: String::new(),
        }
    }
}

/// Address of Record configuration.
#[derive(Debug, Clone)]
pub struct AorConfig {
    /// Section name.
    pub name: String,
    /// Maximum number of contacts.
    pub max_contacts: u32,
    /// Remove existing contacts when a new one registers.
    pub remove_existing: bool,
    /// Default registration expiration in seconds.
    pub default_expiration: u32,
    /// How often to send OPTIONS qualify pings (0 = disabled).
    pub qualify_frequency: u32,
    /// Maximum registration expiration in seconds.
    pub maximum_expiration: u32,
    /// Minimum registration expiration in seconds.
    pub minimum_expiration: u32,
    /// Static contact URIs.
    pub contact: Vec<String>,
    /// Support outbound (RFC 5626).
    pub support_path: bool,
}

impl Default for AorConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            max_contacts: 1,
            remove_existing: false,
            default_expiration: 3600,
            qualify_frequency: 0,
            maximum_expiration: 7200,
            minimum_expiration: 60,
            contact: Vec::new(),
            support_path: false,
        }
    }
}

/// Authentication configuration.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Section name.
    pub name: String,
    /// Auth type: userpass, md5.
    pub auth_type: String,
    /// Username for authentication.
    pub username: String,
    /// Plaintext password (when auth_type=userpass).
    pub password: String,
    /// MD5 credential hash (when auth_type=md5).
    pub md5_cred: Option<String>,
    /// Digest authentication realm.
    pub realm: Option<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            auth_type: "userpass".to_string(),
            username: String::new(),
            password: String::new(),
            md5_cred: None,
            realm: None,
        }
    }
}

/// IP-based endpoint identification.
#[derive(Debug, Clone)]
pub struct IdentifyConfig {
    /// Section name.
    pub name: String,
    /// Endpoint name to associate with matching IPs.
    pub endpoint: String,
    /// Match patterns (IP addresses, CIDRs, or hostnames).
    pub matches: Vec<String>,
    /// SRV lookups (hostnames that will be resolved).
    pub match_header: Option<String>,
}

impl Default for IdentifyConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            endpoint: String::new(),
            matches: Vec::new(),
            match_header: None,
        }
    }
}

/// Outbound registration configuration.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Section name.
    pub name: String,
    /// Server URI to register with.
    pub server_uri: String,
    /// Client URI (our AOR to register).
    pub client_uri: String,
    /// Auth section reference for outbound auth.
    pub outbound_auth: Option<String>,
    /// Registration retry interval in seconds.
    pub retry_interval: u32,
    /// Registration expiration in seconds.
    pub expiration: u32,
    /// Transport reference.
    pub transport: Option<String>,
    /// Contact user part.
    pub contact_user: Option<String>,
    /// Outbound proxy URI.
    pub outbound_proxy: Option<String>,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            server_uri: String::new(),
            client_uri: String::new(),
            outbound_auth: None,
            retry_interval: 60,
            expiration: 3600,
            transport: None,
            contact_user: None,
            outbound_proxy: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Load and parse a pjsip.conf file into a `PjsipConfig`.
///
/// Each section's `type=` field determines what kind of object it becomes.
pub fn load_pjsip_config(config: &AsteriskConfig) -> PjsipConfig {
    let mut result = PjsipConfig::default();

    for cat in config.get_categories() {
        // Skip template sections -- they're inherited, not instantiated directly.
        if cat.is_template {
            debug!(name = %cat.name, "Skipping template section");
            continue;
        }

        let section_type = get_last_variable(cat, "type")
            .unwrap_or("")
            .to_lowercase();

        match section_type.as_str() {
            "transport" => {
                if let Some(transport) = parse_transport(cat) {
                    info!(
                        name = %transport.name,
                        protocol = %transport.protocol,
                        bind = %transport.bind,
                        "Loaded PJSIP transport"
                    );
                    result.transports.push(transport);
                }
            }
            "endpoint" => {
                let endpoint = parse_endpoint(cat);
                debug!(
                    name = %endpoint.name,
                    context = %endpoint.context,
                    "Loaded PJSIP endpoint"
                );
                result.endpoints.push(endpoint);
            }
            "aor" => {
                let aor = parse_aor(cat);
                debug!(
                    name = %aor.name,
                    max_contacts = %aor.max_contacts,
                    "Loaded PJSIP AOR"
                );
                result.aors.push(aor);
            }
            "auth" => {
                let auth = parse_auth(cat);
                debug!(
                    name = %auth.name,
                    auth_type = %auth.auth_type,
                    username = %auth.username,
                    "Loaded PJSIP auth"
                );
                result.auths.push(auth);
            }
            "identify" => {
                let identify = parse_identify(cat);
                debug!(
                    name = %identify.name,
                    endpoint = %identify.endpoint,
                    "Loaded PJSIP identify"
                );
                result.identifies.push(identify);
            }
            "registration" => {
                let reg = parse_registration(cat);
                debug!(
                    name = %reg.name,
                    server_uri = %reg.server_uri,
                    "Loaded PJSIP registration"
                );
                result.registrations.push(reg);
            }
            "" => {
                // No type= field -- could be [general] or similar; skip silently.
                debug!(name = %cat.name, "Skipping section without type= field");
            }
            other => {
                warn!(
                    name = %cat.name,
                    section_type = other,
                    "Unknown PJSIP section type, skipping"
                );
            }
        }
    }

    info!(
        transports = result.transports.len(),
        endpoints = result.endpoints.len(),
        aors = result.aors.len(),
        auths = result.auths.len(),
        identifies = result.identifies.len(),
        registrations = result.registrations.len(),
        "PJSIP configuration loaded"
    );

    result
}

/// Load pjsip.conf from a path string, returning a `PjsipConfig`.
///
/// Returns `Ok(PjsipConfig)` on success, or the underlying config error.
pub fn load_pjsip_config_from_path(path: &str) -> Result<PjsipConfig, asterisk_config::ConfigError> {
    let config = AsteriskConfig::load(path)?;
    Ok(load_pjsip_config(&config))
}

/// Load pjsip.conf from a string (useful for testing).
pub fn load_pjsip_config_from_str(content: &str) -> Result<PjsipConfig, asterisk_config::ConfigError> {
    let config = AsteriskConfig::from_str(content, "pjsip.conf")?;
    Ok(load_pjsip_config(&config))
}

// ---------------------------------------------------------------------------
// Global PJSIP config store
// ---------------------------------------------------------------------------

use std::sync::{Arc, LazyLock};
use parking_lot::RwLock;

/// Global PJSIP configuration, set at startup and read by AMI actions.
static GLOBAL_PJSIP_CONFIG: LazyLock<RwLock<Option<Arc<PjsipConfig>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Store the loaded PJSIP config globally so AMI handlers can read it.
pub fn set_global_pjsip_config(config: PjsipConfig) {
    *GLOBAL_PJSIP_CONFIG.write() = Some(Arc::new(config));
}

/// Retrieve the global PJSIP config (if loaded).
pub fn get_global_pjsip_config() -> Option<Arc<PjsipConfig>> {
    GLOBAL_PJSIP_CONFIG.read().clone()
}

// ---------------------------------------------------------------------------
// Section parsers
// ---------------------------------------------------------------------------

fn parse_transport(cat: &asterisk_config::Category) -> Option<TransportConfig> {
    let protocol = get_last_variable(cat,"protocol").unwrap_or("udp").to_lowercase();
    let bind_str = get_last_variable(cat,"bind").unwrap_or("0.0.0.0:5060");

    // If bind doesn't include a port, add the default for the protocol.
    let bind_str = if bind_str.contains(':') {
        bind_str.to_string()
    } else {
        let default_port = match protocol.as_str() {
            "tls" | "wss" => 5061,
            _ => 5060,
        };
        format!("{}:{}", bind_str, default_port)
    };

    let bind: SocketAddr = match bind_str.parse() {
        Ok(addr) => addr,
        Err(e) => {
            warn!(
                name = %cat.name,
                bind = %bind_str,
                error = %e,
                "Invalid bind address for transport, skipping"
            );
            return None;
        }
    };

    let mut local_net = Vec::new();
    for val in get_all_variables(cat,"local_net") {
        local_net.push(val.to_string());
    }

    Some(TransportConfig {
        name: cat.name.clone(),
        protocol,
        bind,
        external_media_address: get_last_variable(cat,"external_media_address").map(|s| s.to_string()),
        external_signaling_address: get_last_variable(cat,"external_signaling_address").map(|s| s.to_string()),
        cert_file: get_last_variable(cat,"cert_file").map(|s| s.to_string()),
        priv_key_file: get_last_variable(cat,"priv_key_file").map(|s| s.to_string()),
        local_net,
    })
}

fn parse_endpoint(cat: &asterisk_config::Category) -> EndpointConfig {
    let mut ep = EndpointConfig {
        name: cat.name.clone(),
        ..Default::default()
    };

    if let Some(v) = get_last_variable(cat,"context") {
        ep.context = v.to_string();
    }

    // Collect all disallow/allow in order (multi-value keys).
    for val in get_all_variables(cat,"disallow") {
        for codec in val.split(',') {
            let codec = codec.trim();
            if !codec.is_empty() {
                ep.disallow.push(codec.to_string());
            }
        }
    }
    for val in get_all_variables(cat,"allow") {
        for codec in val.split(',') {
            let codec = codec.trim();
            if !codec.is_empty() {
                ep.allow.push(codec.to_string());
            }
        }
    }

    if let Some(v) = get_last_variable(cat,"auth") {
        ep.auth = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"aors") {
        ep.aors = Some(v.to_string());
    }

    ep.direct_media = parse_bool(get_last_variable(cat,"direct_media"), true);
    ep.rtp_symmetric = parse_bool(get_last_variable(cat,"rtp_symmetric"), false);
    ep.force_rport = parse_bool(get_last_variable(cat,"force_rport"), true);
    ep.rewrite_contact = parse_bool(get_last_variable(cat,"rewrite_contact"), false);
    ep.ice_support = parse_bool(get_last_variable(cat,"ice_support"), false);
    ep.send_rpid = parse_bool(get_last_variable(cat,"send_rpid"), false);
    ep.send_pai = parse_bool(get_last_variable(cat,"send_pai"), false);
    ep.allow_transfer = parse_bool(get_last_variable(cat,"allow_transfer"), true);
    ep.trust_id_inbound = parse_bool(get_last_variable(cat,"trust_id_inbound"), false);
    ep.allow_overlap = parse_bool(get_last_variable(cat,"allow_overlap"), true);

    if let Some(v) = get_last_variable(cat,"dtmf_mode") {
        ep.dtmf_mode = v.to_lowercase();
    }
    if let Some(v) = get_last_variable(cat,"media_encryption") {
        ep.media_encryption = v.to_lowercase();
    }
    if let Some(v) = get_last_variable(cat,"callerid") {
        ep.callerid = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"callerid_num") {
        ep.callerid_num = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"from_user") {
        ep.from_user = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"from_domain") {
        ep.from_domain = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"transport") {
        ep.transport = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"accountcode") {
        ep.accountcode = v.to_string();
    }

    ep
}

fn parse_aor(cat: &asterisk_config::Category) -> AorConfig {
    let mut aor = AorConfig {
        name: cat.name.clone(),
        ..Default::default()
    };

    if let Some(v) = get_last_variable(cat,"max_contacts") {
        aor.max_contacts = v.parse().unwrap_or(1);
    }
    aor.remove_existing = parse_bool(get_last_variable(cat,"remove_existing"), false);
    if let Some(v) = get_last_variable(cat,"default_expiration") {
        aor.default_expiration = v.parse().unwrap_or(3600);
    }
    if let Some(v) = get_last_variable(cat,"qualify_frequency") {
        aor.qualify_frequency = v.parse().unwrap_or(0);
    }
    if let Some(v) = get_last_variable(cat,"maximum_expiration") {
        aor.maximum_expiration = v.parse().unwrap_or(7200);
    }
    if let Some(v) = get_last_variable(cat,"minimum_expiration") {
        aor.minimum_expiration = v.parse().unwrap_or(60);
    }
    aor.support_path = parse_bool(get_last_variable(cat,"support_path"), false);

    for val in get_all_variables(cat,"contact") {
        // Contact values can be comma-separated (e.g., "sip:a@1.2.3.4,sip:b@5.6.7.8")
        for contact in val.split(',') {
            let contact = contact.trim();
            if !contact.is_empty() {
                aor.contact.push(contact.to_string());
            }
        }
    }

    aor
}

fn parse_auth(cat: &asterisk_config::Category) -> AuthConfig {
    let mut auth = AuthConfig {
        name: cat.name.clone(),
        ..Default::default()
    };

    if let Some(v) = get_last_variable(cat,"auth_type") {
        auth.auth_type = v.to_lowercase();
    }
    if let Some(v) = get_last_variable(cat,"username") {
        auth.username = v.to_string();
    }
    if let Some(v) = get_last_variable(cat,"password") {
        auth.password = v.to_string();
    }
    if let Some(v) = get_last_variable(cat,"md5_cred") {
        auth.md5_cred = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"realm") {
        auth.realm = Some(v.to_string());
    }

    auth
}

fn parse_identify(cat: &asterisk_config::Category) -> IdentifyConfig {
    let mut id = IdentifyConfig {
        name: cat.name.clone(),
        ..Default::default()
    };

    if let Some(v) = get_last_variable(cat,"endpoint") {
        id.endpoint = v.to_string();
    }
    for val in get_all_variables(cat,"match") {
        id.matches.push(val.to_string());
    }
    if let Some(v) = get_last_variable(cat,"match_header") {
        id.match_header = Some(v.to_string());
    }

    id
}

fn parse_registration(cat: &asterisk_config::Category) -> RegistrationConfig {
    let mut reg = RegistrationConfig {
        name: cat.name.clone(),
        ..Default::default()
    };

    if let Some(v) = get_last_variable(cat,"server_uri") {
        reg.server_uri = v.to_string();
    }
    if let Some(v) = get_last_variable(cat,"client_uri") {
        reg.client_uri = v.to_string();
    }
    if let Some(v) = get_last_variable(cat,"outbound_auth") {
        reg.outbound_auth = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"retry_interval") {
        reg.retry_interval = v.parse().unwrap_or(60);
    }
    if let Some(v) = get_last_variable(cat,"expiration") {
        reg.expiration = v.parse().unwrap_or(3600);
    }
    if let Some(v) = get_last_variable(cat,"transport") {
        reg.transport = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"contact_user") {
        reg.contact_user = Some(v.to_string());
    }
    if let Some(v) = get_last_variable(cat,"outbound_proxy") {
        reg.outbound_proxy = Some(v.to_string());
    }

    reg
}

// ---------------------------------------------------------------------------
// Endpoint matching
// ---------------------------------------------------------------------------

impl PjsipConfig {
    /// Look up an endpoint by name.
    pub fn find_endpoint(&self, name: &str) -> Option<&EndpointConfig> {
        self.endpoints.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Look up an auth section by name.
    pub fn find_auth(&self, name: &str) -> Option<&AuthConfig> {
        self.auths.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Look up an AOR by name.
    pub fn find_aor(&self, name: &str) -> Option<&AorConfig> {
        self.aors.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Look up a transport by name.
    pub fn find_transport(&self, name: &str) -> Option<&TransportConfig> {
        self.transports.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Look up a registration by name.
    pub fn find_registration(&self, name: &str) -> Option<&RegistrationConfig> {
        self.registrations.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// Match an incoming request's source IP to an endpoint via identify sections.
    ///
    /// Returns the endpoint name if a match is found.
    pub fn identify_endpoint_by_ip(&self, source_ip: &str) -> Option<&str> {
        for id in &self.identifies {
            for pattern in &id.matches {
                if ip_matches(source_ip, pattern) {
                    return Some(&id.endpoint);
                }
            }
        }
        None
    }

    /// Verify digest credentials against stored auth configs.
    ///
    /// Returns `true` if the username/password matches an auth section.
    pub fn verify_credentials(&self, auth_name: &str, username: &str, password: &str) -> bool {
        if let Some(auth) = self.find_auth(auth_name) {
            if auth.auth_type == "userpass" {
                return auth.username == username && auth.password == password;
            }
            // MD5 auth would need to be checked differently (compare hashes)
        }
        false
    }

    /// Get the context for an endpoint, falling back to "default".
    pub fn endpoint_context(&self, endpoint_name: &str) -> &str {
        self.find_endpoint(endpoint_name)
            .map(|e| e.context.as_str())
            .unwrap_or("default")
    }

    /// Get the accountcode for an endpoint, falling back to "".
    pub fn endpoint_accountcode(&self, endpoint_name: &str) -> &str {
        self.find_endpoint(endpoint_name)
            .map(|e| e.accountcode.as_str())
            .unwrap_or("")
    }

    /// Get the allowed codecs for an endpoint.
    pub fn endpoint_codecs(&self, endpoint_name: &str) -> (Vec<&str>, Vec<&str>) {
        if let Some(ep) = self.find_endpoint(endpoint_name) {
            let disallow: Vec<&str> = ep.disallow.iter().map(|s| s.as_str()).collect();
            let allow: Vec<&str> = ep.allow.iter().map(|s| s.as_str()).collect();
            (disallow, allow)
        } else {
            (Vec::new(), Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the last variable with the given name from a category.
///
/// When template inheritance is used, inherited variables are prepended.
/// For single-value fields, the local (last) value should win over inherited ones.
fn get_last_variable<'a>(cat: &'a asterisk_config::Category, name: &str) -> Option<&'a str> {
    cat.variables
        .iter()
        .rev()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .map(|v| v.value.as_str())
}

/// Get all variables with the given name, including inherited ones (for multi-value keys).
fn get_all_variables<'a>(cat: &'a asterisk_config::Category, name: &str) -> Vec<&'a str> {
    cat.variables
        .iter()
        .filter(|v| v.name.eq_ignore_ascii_case(name))
        .map(|v| v.value.as_str())
        .collect()
}

/// Parse a boolean-like config value (yes/no/true/false/1/0).
fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value {
        Some(v) => matches!(v.to_lowercase().as_str(), "yes" | "true" | "1" | "on"),
        None => default,
    }
}

/// Check if a source IP matches a pattern (exact match or CIDR).
fn ip_matches(source_ip: &str, pattern: &str) -> bool {
    // Exact match
    if source_ip == pattern {
        return true;
    }

    // CIDR match
    if let Some((network, prefix_len_str)) = pattern.split_once('/') {
        if let (Ok(src), Ok(net), Ok(prefix_len)) = (
            source_ip.parse::<std::net::IpAddr>(),
            network.parse::<std::net::IpAddr>(),
            prefix_len_str.parse::<u32>(),
        ) {
            return ip_in_cidr(src, net, prefix_len);
        }
    }

    false
}

/// Check if an IP address falls within a CIDR range.
fn ip_in_cidr(ip: std::net::IpAddr, network: std::net::IpAddr, prefix_len: u32) -> bool {
    use std::net::IpAddr;
    match (ip, network) {
        (IpAddr::V4(ip4), IpAddr::V4(net4)) => {
            if prefix_len > 32 {
                return false;
            }
            let mask = if prefix_len == 0 {
                0u32
            } else {
                !0u32 << (32 - prefix_len)
            };
            let ip_bits = u32::from(ip4);
            let net_bits = u32::from(net4);
            (ip_bits & mask) == (net_bits & mask)
        }
        (IpAddr::V6(ip6), IpAddr::V6(net6)) => {
            if prefix_len > 128 {
                return false;
            }
            let ip_bits = u128::from(ip6);
            let net_bits = u128::from(net6);
            let mask = if prefix_len == 0 {
                0u128
            } else {
                !0u128 << (128 - prefix_len)
            };
            (ip_bits & mask) == (net_bits & mask)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pjsip_conf() -> &'static str {
        r#"
[transport-udp]
type=transport
protocol=udp
bind=0.0.0.0:5060

[transport-tcp]
type=transport
protocol=tcp
bind=0.0.0.0:5060

[alice]
type=endpoint
context=default
disallow=all
allow=ulaw
allow=alaw
auth=alice-auth
aors=alice
direct_media=no
rtp_symmetric=yes
dtmf_mode=rfc4733

[alice-auth]
type=auth
auth_type=userpass
username=alice
password=alice123

[alice-aor]
type=aor
max_contacts=1
default_expiration=3600
qualify_frequency=60

[bob]
type=endpoint
context=internal
disallow=all
allow=ulaw,alaw,g722
auth=bob-auth
aors=bob
direct_media=yes

[bob-auth]
type=auth
auth_type=userpass
username=bob
password=bob456
realm=asterisk

[bob-aor]
type=aor
max_contacts=2
remove_existing=yes

[provider-id]
type=identify
endpoint=provider
match=203.0.113.0/24
match=198.51.100.1

[provider-reg]
type=registration
server_uri=sip:registrar.example.com
client_uri=sip:myaccount@example.com
outbound_auth=provider-auth
retry_interval=30
expiration=1800

[provider-auth]
type=auth
auth_type=userpass
username=myaccount
password=secret
"#
    }

    #[test]
    fn test_load_basic_config() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        assert_eq!(config.transports.len(), 2);
        assert_eq!(config.endpoints.len(), 2);
        assert_eq!(config.aors.len(), 2);
        assert_eq!(config.auths.len(), 3);
        assert_eq!(config.identifies.len(), 1);
        assert_eq!(config.registrations.len(), 1);
    }

    #[test]
    fn test_transport_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let udp = config.find_transport("transport-udp").unwrap();
        assert_eq!(udp.protocol, "udp");
        assert_eq!(udp.bind, "0.0.0.0:5060".parse::<SocketAddr>().unwrap());

        let tcp = config.find_transport("transport-tcp").unwrap();
        assert_eq!(tcp.protocol, "tcp");
    }

    #[test]
    fn test_endpoint_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();

        let alice = config.find_endpoint("alice").unwrap();
        assert_eq!(alice.context, "default");
        assert_eq!(alice.disallow, vec!["all"]);
        assert_eq!(alice.allow, vec!["ulaw", "alaw"]);
        assert_eq!(alice.auth.as_deref(), Some("alice-auth"));
        assert_eq!(alice.aors.as_deref(), Some("alice"));
        assert!(!alice.direct_media);
        assert!(alice.rtp_symmetric);
        assert_eq!(alice.dtmf_mode, "rfc4733");

        let bob = config.find_endpoint("bob").unwrap();
        assert_eq!(bob.context, "internal");
        assert_eq!(bob.allow, vec!["ulaw", "alaw", "g722"]);
        assert!(bob.direct_media);
    }

    #[test]
    fn test_auth_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let alice_auth = config.find_auth("alice-auth").unwrap();
        assert_eq!(alice_auth.auth_type, "userpass");
        assert_eq!(alice_auth.username, "alice");
        assert_eq!(alice_auth.password, "alice123");
        assert!(alice_auth.realm.is_none());

        let bob_auth = config.find_auth("bob-auth").unwrap();
        assert_eq!(bob_auth.realm.as_deref(), Some("asterisk"));
    }

    #[test]
    fn test_aor_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let alice_aor = config.find_aor("alice-aor").unwrap();
        assert_eq!(alice_aor.max_contacts, 1);
        assert_eq!(alice_aor.default_expiration, 3600);
        assert_eq!(alice_aor.qualify_frequency, 60);
        assert!(!alice_aor.remove_existing);

        let bob_aor = config.find_aor("bob-aor").unwrap();
        assert_eq!(bob_aor.max_contacts, 2);
        assert!(bob_aor.remove_existing);
    }

    #[test]
    fn test_identify_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let id = &config.identifies[0];
        assert_eq!(id.name, "provider-id");
        assert_eq!(id.endpoint, "provider");
        assert_eq!(id.matches.len(), 2);
        assert_eq!(id.matches[0], "203.0.113.0/24");
        assert_eq!(id.matches[1], "198.51.100.1");
    }

    #[test]
    fn test_registration_parsing() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let reg = &config.registrations[0];
        assert_eq!(reg.name, "provider-reg");
        assert_eq!(reg.server_uri, "sip:registrar.example.com");
        assert_eq!(reg.client_uri, "sip:myaccount@example.com");
        assert_eq!(reg.outbound_auth.as_deref(), Some("provider-auth"));
        assert_eq!(reg.retry_interval, 30);
        assert_eq!(reg.expiration, 1800);
    }

    #[test]
    fn test_verify_credentials() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        assert!(config.verify_credentials("alice-auth", "alice", "alice123"));
        assert!(!config.verify_credentials("alice-auth", "alice", "wrong"));
        assert!(!config.verify_credentials("alice-auth", "wrong", "alice123"));
        assert!(!config.verify_credentials("nonexistent", "alice", "alice123"));
    }

    #[test]
    fn test_identify_endpoint_by_ip() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();

        // Exact match
        assert_eq!(
            config.identify_endpoint_by_ip("198.51.100.1"),
            Some("provider")
        );

        // CIDR match
        assert_eq!(
            config.identify_endpoint_by_ip("203.0.113.42"),
            Some("provider")
        );

        // No match
        assert_eq!(config.identify_endpoint_by_ip("10.0.0.1"), None);
    }

    #[test]
    fn test_endpoint_context() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        assert_eq!(config.endpoint_context("alice"), "default");
        assert_eq!(config.endpoint_context("bob"), "internal");
        assert_eq!(config.endpoint_context("nonexistent"), "default");
    }

    #[test]
    fn test_endpoint_codecs() {
        let config = load_pjsip_config_from_str(sample_pjsip_conf()).unwrap();
        let (disallow, allow) = config.endpoint_codecs("alice");
        assert_eq!(disallow, vec!["all"]);
        assert_eq!(allow, vec!["ulaw", "alaw"]);
    }

    #[test]
    fn test_ip_matches_exact() {
        assert!(ip_matches("192.168.1.1", "192.168.1.1"));
        assert!(!ip_matches("192.168.1.1", "192.168.1.2"));
    }

    #[test]
    fn test_ip_matches_cidr() {
        assert!(ip_matches("192.168.1.1", "192.168.1.0/24"));
        assert!(ip_matches("192.168.1.254", "192.168.1.0/24"));
        assert!(!ip_matches("192.168.2.1", "192.168.1.0/24"));
        assert!(ip_matches("10.0.0.1", "10.0.0.0/8"));
    }

    #[test]
    fn test_parse_bool_values() {
        assert!(parse_bool(Some("yes"), false));
        assert!(parse_bool(Some("true"), false));
        assert!(parse_bool(Some("1"), false));
        assert!(parse_bool(Some("on"), false));
        assert!(!parse_bool(Some("no"), true));
        assert!(!parse_bool(Some("false"), true));
        assert!(!parse_bool(Some("0"), true));
        assert!(!parse_bool(Some("off"), true));
        assert!(parse_bool(None, true));
        assert!(!parse_bool(None, false));
    }

    #[test]
    fn test_default_endpoint_values() {
        let content = r#"
[minimal]
type=endpoint
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let ep = config.find_endpoint("minimal").unwrap();
        assert_eq!(ep.context, "default");
        assert!(ep.disallow.is_empty());
        assert!(ep.allow.is_empty());
        assert!(ep.auth.is_none());
        assert!(ep.aors.is_none());
        assert!(ep.direct_media); // default is true
        assert!(!ep.rtp_symmetric); // default is false
        assert_eq!(ep.dtmf_mode, "rfc4733");
        assert!(ep.force_rport);
    }

    #[test]
    fn test_transport_default_port() {
        let content = r#"
[simple-transport]
type=transport
protocol=udp
bind=0.0.0.0
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let t = config.find_transport("simple-transport").unwrap();
        assert_eq!(t.bind.port(), 5060);
    }

    #[test]
    fn test_transport_tls_default_port() {
        let content = r#"
[tls-transport]
type=transport
protocol=tls
bind=0.0.0.0
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let t = config.find_transport("tls-transport").unwrap();
        assert_eq!(t.bind.port(), 5061);
    }

    #[test]
    fn test_template_inheritance() {
        let content = r#"
[endpoint-template](!)
type=endpoint
context=from-internal
disallow=all
allow=ulaw
direct_media=no
rtp_symmetric=yes

[phone1](endpoint-template)
type=endpoint
auth=phone1-auth
aors=phone1

[phone2](endpoint-template)
type=endpoint
auth=phone2-auth
aors=phone2
context=from-external
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        assert_eq!(config.endpoints.len(), 2);

        let phone1 = config.find_endpoint("phone1").unwrap();
        assert_eq!(phone1.context, "from-internal"); // inherited
        assert_eq!(phone1.allow, vec!["ulaw"]); // inherited
        assert!(!phone1.direct_media); // inherited
        assert!(phone1.rtp_symmetric); // inherited
        assert_eq!(phone1.auth.as_deref(), Some("phone1-auth")); // own

        let phone2 = config.find_endpoint("phone2").unwrap();
        assert_eq!(phone2.context, "from-external"); // overridden
        assert_eq!(phone2.auth.as_deref(), Some("phone2-auth"));
    }

    #[test]
    fn test_empty_config() {
        let content = "";
        let config = load_pjsip_config_from_str(content).unwrap();
        assert!(config.transports.is_empty());
        assert!(config.endpoints.is_empty());
        assert!(config.aors.is_empty());
        assert!(config.auths.is_empty());
        assert!(config.identifies.is_empty());
        assert!(config.registrations.is_empty());
    }

    #[test]
    fn test_section_without_type() {
        let content = r#"
[general]
debug=yes
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        assert!(config.endpoints.is_empty());
    }

    #[test]
    fn test_multiple_contacts_in_aor() {
        let content = r#"
[trunk-aor]
type=aor
contact=sip:primary.example.com
contact=sip:backup.example.com
qualify_frequency=30
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let aor = config.find_aor("trunk-aor").unwrap();
        assert_eq!(aor.contact.len(), 2);
        assert_eq!(aor.contact[0], "sip:primary.example.com");
        assert_eq!(aor.contact[1], "sip:backup.example.com");
        assert_eq!(aor.qualify_frequency, 30);
    }

    #[test]
    fn test_md5_auth() {
        let content = r#"
[md5-auth]
type=auth
auth_type=md5
username=testuser
md5_cred=0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d
realm=asterisk
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let auth = config.find_auth("md5-auth").unwrap();
        assert_eq!(auth.auth_type, "md5");
        assert_eq!(auth.md5_cred.as_deref(), Some("0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d"));
        assert_eq!(auth.realm.as_deref(), Some("asterisk"));
    }

    #[test]
    fn test_identify_multiple_matches() {
        let content = r#"
[my-identify]
type=identify
endpoint=trunk
match=10.0.0.0/8
match=172.16.0.0/12
match=192.168.0.0/16
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let id = &config.identifies[0];
        assert_eq!(id.matches.len(), 3);

        // All RFC1918 ranges should match
        assert_eq!(config.identify_endpoint_by_ip("10.1.2.3"), Some("trunk"));
        assert_eq!(config.identify_endpoint_by_ip("172.16.5.5"), Some("trunk"));
        assert_eq!(config.identify_endpoint_by_ip("192.168.1.1"), Some("trunk"));
        // Public IP should not match
        assert_eq!(config.identify_endpoint_by_ip("8.8.8.8"), None);
    }

    #[test]
    fn test_registration_defaults() {
        let content = r#"
[minimal-reg]
type=registration
server_uri=sip:example.com
client_uri=sip:me@example.com
"#;
        let config = load_pjsip_config_from_str(content).unwrap();
        let reg = config.find_registration("minimal-reg").unwrap();
        assert_eq!(reg.retry_interval, 60);
        assert_eq!(reg.expiration, 3600);
        assert!(reg.transport.is_none());
        assert!(reg.outbound_auth.is_none());
    }
}
