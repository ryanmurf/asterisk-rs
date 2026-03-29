//! SIP endpoint identification.
//!
//! Port of the PJSIP endpoint identification modules. Provides a pluggable
//! chain of identifiers that map incoming SIP requests to configured
//! endpoints by source IP, username, or custom header values.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum EndpointIdError {
    #[error("identifier already registered: {0}")]
    AlreadyRegistered(String),
    #[error("identifier not found: {0}")]
    NotFound(String),
    #[error("identification failed: {0}")]
    IdentificationFailed(String),
}

pub type EndpointIdResult<T> = Result<T, EndpointIdError>;

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

/// Information extracted from an incoming SIP request for identification.
#[derive(Debug, Clone)]
pub struct IdentifyContext {
    /// Source IP address.
    pub source_ip: IpAddr,
    /// Source port.
    pub source_port: u16,
    /// SIP headers (name -> values).
    pub headers: HashMap<String, Vec<String>>,
}

impl IdentifyContext {
    pub fn new(source_ip: IpAddr, source_port: u16) -> Self {
        Self {
            source_ip,
            source_port,
            headers: HashMap::new(),
        }
    }

    /// Add a SIP header value.
    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers
            .entry(name.to_lowercase())
            .or_default()
            .push(value.to_string());
    }

    /// Get the first value of a header (case-insensitive name).
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_lowercase())
            .and_then(|vals| vals.first().map(|s| s.as_str()))
    }

    /// Extract the user part from a SIP From header value.
    pub fn from_user(&self) -> Option<&str> {
        self.get_header("from").and_then(|from| {
            // Simple extraction: look for sip:user@ pattern.
            let start = from.find("sip:")? + 4;
            let rest = &from[start..];
            let end = rest.find('@')?;
            Some(&rest[..end])
        })
    }
}

// ---------------------------------------------------------------------------
// Endpoint identifier trait
// ---------------------------------------------------------------------------

/// Trait for endpoint identification strategies.
pub trait EndpointIdentifier: Send + Sync + fmt::Debug {
    /// Name of this identifier.
    fn name(&self) -> &str;

    /// Attempt to identify the endpoint from the request context.
    ///
    /// Returns `Some(endpoint_name)` on success, `None` if this identifier
    /// cannot determine the endpoint.
    fn identify(&self, ctx: &IdentifyContext) -> Option<String>;
}

// ---------------------------------------------------------------------------
// IP-based identifier
// ---------------------------------------------------------------------------

/// Identifies endpoints by source IP address or subnet.
#[derive(Debug)]
pub struct IpIdentifier {
    /// Map of IP address (as string) to endpoint name.
    matches: RwLock<Vec<IpMatch>>,
}

/// A single IP-to-endpoint mapping.
#[derive(Debug, Clone)]
struct IpMatch {
    /// IP address to match.
    addr: IpAddr,
    /// Subnet prefix length (32 for exact match).
    prefix_len: u8,
    /// Endpoint name.
    endpoint: String,
}

impl IpIdentifier {
    pub fn new() -> Self {
        Self {
            matches: RwLock::new(Vec::new()),
        }
    }

    /// Add an IP match rule. `prefix_len` is the CIDR prefix (e.g., 32 for
    /// exact match, 24 for /24 subnet).
    pub fn add_match(&self, addr: IpAddr, prefix_len: u8, endpoint: &str) {
        self.matches.write().push(IpMatch {
            addr,
            prefix_len,
            endpoint: endpoint.to_string(),
        });
        debug!(addr = %addr, prefix = prefix_len, endpoint, "IP identifier rule added");
    }

    /// Check whether `candidate` is in the subnet defined by `network`/`prefix_len`.
    fn ip_in_subnet(candidate: IpAddr, network: IpAddr, prefix_len: u8) -> bool {
        match (candidate, network) {
            (IpAddr::V4(c), IpAddr::V4(n)) => {
                if prefix_len >= 32 {
                    return c == n;
                }
                let mask = u32::MAX << (32 - prefix_len);
                (u32::from(c) & mask) == (u32::from(n) & mask)
            }
            (IpAddr::V6(c), IpAddr::V6(n)) => {
                if prefix_len >= 128 {
                    return c == n;
                }
                let c_bits = u128::from(c);
                let n_bits = u128::from(n);
                let mask = u128::MAX << (128 - prefix_len);
                (c_bits & mask) == (n_bits & mask)
            }
            _ => false,
        }
    }
}

impl Default for IpIdentifier {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointIdentifier for IpIdentifier {
    fn name(&self) -> &str {
        "ip"
    }

    fn identify(&self, ctx: &IdentifyContext) -> Option<String> {
        let matches = self.matches.read();
        // Try most specific (longest prefix) first.
        let mut best: Option<&IpMatch> = None;
        for m in matches.iter() {
            if Self::ip_in_subnet(ctx.source_ip, m.addr, m.prefix_len) {
                if best.is_none() || m.prefix_len > best.unwrap().prefix_len {
                    best = Some(m);
                }
            }
        }
        best.map(|m| m.endpoint.clone())
    }
}

// ---------------------------------------------------------------------------
// Username-based identifier
// ---------------------------------------------------------------------------

/// Identifies endpoints by the user part of the SIP From header.
#[derive(Debug)]
pub struct UsernameIdentifier {
    /// Map of username to endpoint name.
    usernames: RwLock<HashMap<String, String>>,
}

impl UsernameIdentifier {
    pub fn new() -> Self {
        Self {
            usernames: RwLock::new(HashMap::new()),
        }
    }

    /// Map a username to an endpoint.
    pub fn add_match(&self, username: &str, endpoint: &str) {
        self.usernames
            .write()
            .insert(username.to_string(), endpoint.to_string());
    }
}

impl Default for UsernameIdentifier {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointIdentifier for UsernameIdentifier {
    fn name(&self) -> &str {
        "username"
    }

    fn identify(&self, ctx: &IdentifyContext) -> Option<String> {
        let user = ctx.from_user()?;
        let usernames = self.usernames.read();
        usernames.get(user).cloned()
    }
}

// ---------------------------------------------------------------------------
// Header-based identifier
// ---------------------------------------------------------------------------

/// Identifies endpoints by the value of a custom SIP header.
#[derive(Debug)]
pub struct HeaderIdentifier {
    /// Header name to check.
    header_name: String,
    /// Map of header value to endpoint name.
    values: RwLock<HashMap<String, String>>,
}

impl HeaderIdentifier {
    pub fn new(header_name: &str) -> Self {
        Self {
            header_name: header_name.to_lowercase(),
            values: RwLock::new(HashMap::new()),
        }
    }

    /// Map a header value to an endpoint.
    pub fn add_match(&self, value: &str, endpoint: &str) {
        self.values
            .write()
            .insert(value.to_string(), endpoint.to_string());
    }
}

impl EndpointIdentifier for HeaderIdentifier {
    fn name(&self) -> &str {
        "header"
    }

    fn identify(&self, ctx: &IdentifyContext) -> Option<String> {
        let header_val = ctx.get_header(&self.header_name)?;
        let values = self.values.read();
        values.get(header_val).cloned()
    }
}

// ---------------------------------------------------------------------------
// Identifier chain
// ---------------------------------------------------------------------------

/// Ordered chain of endpoint identifiers, tried in priority order.
///
/// When an incoming request arrives, each identifier is tried in sequence.
/// The first one to return an endpoint name wins.
pub struct IdentifierChain {
    /// (priority, identifier) pairs, sorted by priority (lower = tried first).
    identifiers: RwLock<Vec<(i32, Arc<dyn EndpointIdentifier>)>>,
}

impl IdentifierChain {
    pub fn new() -> Self {
        Self {
            identifiers: RwLock::new(Vec::new()),
        }
    }

    /// Register an identifier with the given priority (lower = higher priority).
    pub fn register(
        &self,
        priority: i32,
        identifier: Arc<dyn EndpointIdentifier>,
    ) -> EndpointIdResult<()> {
        let name = identifier.name().to_string();
        let mut chain = self.identifiers.write();
        if chain.iter().any(|(_, id)| id.name() == name) {
            return Err(EndpointIdError::AlreadyRegistered(name));
        }
        chain.push((priority, identifier));
        chain.sort_by_key(|(p, _)| *p);
        debug!(name = %name, priority, "Endpoint identifier registered");
        Ok(())
    }

    /// Attempt to identify the endpoint from the given context.
    ///
    /// Tries each identifier in priority order and returns the first match.
    pub fn identify(&self, ctx: &IdentifyContext) -> Option<String> {
        let chain = self.identifiers.read();
        for (_, identifier) in chain.iter() {
            if let Some(endpoint) = identifier.identify(ctx) {
                debug!(
                    identifier = %identifier.name(),
                    endpoint = %endpoint,
                    "Endpoint identified"
                );
                return Some(endpoint);
            }
        }
        None
    }

    /// Number of registered identifiers.
    pub fn len(&self) -> usize {
        self.identifiers.read().len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.identifiers.read().is_empty()
    }

    /// List identifier names in priority order.
    pub fn identifier_names(&self) -> Vec<String> {
        self.identifiers
            .read()
            .iter()
            .map(|(_, id)| id.name().to_string())
            .collect()
    }
}

impl Default for IdentifierChain {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for IdentifierChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentifierChain")
            .field("identifiers", &self.identifier_names())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn make_ctx(ip: &str) -> IdentifyContext {
        let addr: IpAddr = ip.parse().unwrap();
        let mut ctx = IdentifyContext::new(addr, 5060);
        ctx.add_header("From", "\"Alice\" <sip:alice@example.com>");
        ctx
    }

    #[test]
    fn test_ip_identifier_exact() {
        let id = IpIdentifier::new();
        id.add_match("192.168.1.100".parse().unwrap(), 32, "phone1");

        let ctx = make_ctx("192.168.1.100");
        assert_eq!(id.identify(&ctx), Some("phone1".to_string()));

        let ctx2 = make_ctx("192.168.1.101");
        assert_eq!(id.identify(&ctx2), None);
    }

    #[test]
    fn test_ip_identifier_subnet() {
        let id = IpIdentifier::new();
        id.add_match("10.0.0.0".parse().unwrap(), 24, "office_net");

        let ctx = make_ctx("10.0.0.42");
        assert_eq!(id.identify(&ctx), Some("office_net".to_string()));

        let ctx2 = make_ctx("10.0.1.42");
        assert_eq!(id.identify(&ctx2), None);
    }

    #[test]
    fn test_ip_identifier_most_specific() {
        let id = IpIdentifier::new();
        id.add_match("10.0.0.0".parse().unwrap(), 24, "office_net");
        id.add_match("10.0.0.5".parse().unwrap(), 32, "ceo_phone");

        let ctx = make_ctx("10.0.0.5");
        assert_eq!(id.identify(&ctx), Some("ceo_phone".to_string()));
    }

    #[test]
    fn test_username_identifier() {
        let id = UsernameIdentifier::new();
        id.add_match("alice", "alice_endpoint");
        id.add_match("bob", "bob_endpoint");

        let ctx = make_ctx("192.168.1.1");
        assert_eq!(id.identify(&ctx), Some("alice_endpoint".to_string()));
    }

    #[test]
    fn test_header_identifier() {
        let id = HeaderIdentifier::new("X-Tenant");
        id.add_match("acme", "acme_trunk");

        let mut ctx = IdentifyContext::new("1.2.3.4".parse().unwrap(), 5060);
        ctx.add_header("X-Tenant", "acme");
        assert_eq!(id.identify(&ctx), Some("acme_trunk".to_string()));
    }

    #[test]
    fn test_from_user_extraction() {
        let ctx = make_ctx("1.2.3.4");
        assert_eq!(ctx.from_user(), Some("alice"));
    }

    #[test]
    fn test_identifier_chain() {
        let chain = IdentifierChain::new();

        let ip_id = Arc::new(IpIdentifier::new());
        ip_id.add_match("10.0.0.1".parse().unwrap(), 32, "from_ip");

        let user_id = Arc::new(UsernameIdentifier::new());
        user_id.add_match("alice", "from_username");

        // IP identifier has higher priority (lower number).
        chain.register(10, ip_id).unwrap();
        chain.register(20, user_id).unwrap();

        assert_eq!(chain.len(), 2);

        // When IP matches, it should win.
        let ctx = make_ctx("10.0.0.1");
        assert_eq!(chain.identify(&ctx), Some("from_ip".to_string()));

        // When IP doesn't match, fall through to username.
        let ctx2 = make_ctx("192.168.1.1");
        assert_eq!(chain.identify(&ctx2), Some("from_username".to_string()));
    }

    #[test]
    fn test_chain_no_match() {
        let chain = IdentifierChain::new();
        let ip_id = Arc::new(IpIdentifier::new());
        chain.register(10, ip_id).unwrap();

        let ctx = make_ctx("1.2.3.4");
        assert_eq!(chain.identify(&ctx), None);
    }

    #[test]
    fn test_chain_duplicate_registration() {
        let chain = IdentifierChain::new();
        let id1 = Arc::new(IpIdentifier::new());
        let id2 = Arc::new(IpIdentifier::new());
        chain.register(10, id1).unwrap();
        assert!(chain.register(20, id2).is_err());
    }
}
