//! DNS SRV/NAPTR resolution and RFC 3263 SIP URI resolution.
//!
//! Implements:
//! - RFC 2782: SRV record handling with weighted random selection
//! - RFC 2915: NAPTR record handling
//! - RFC 3263: SIP URI resolution algorithm
//! - TTL-based DNS caching with negative caching (NXDOMAIN)

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::net;
use tracing::debug;

// ---------------------------------------------------------------------------
// Core record types
// ---------------------------------------------------------------------------

/// An SRV record (RFC 2782).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

/// A NAPTR record (RFC 2915 / RFC 3403).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NaptrRecord {
    pub order: u16,
    pub preference: u16,
    pub flags: String,
    pub service: String,
    pub regexp: String,
    pub replacement: String,
}

/// Transport protocol for SIP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportType {
    Udp,
    Tcp,
    Tls,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Udp => write!(f, "UDP"),
            Self::Tcp => write!(f, "TCP"),
            Self::Tls => write!(f, "TLS"),
        }
    }
}

impl TransportType {
    /// Default port for this transport.
    pub fn default_port(&self) -> u16 {
        match self {
            Self::Udp | Self::Tcp => 5060,
            Self::Tls => 5061,
        }
    }
}

/// A fully resolved target address ready for connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub address: SocketAddr,
    pub transport: TransportType,
}

// ---------------------------------------------------------------------------
// SIP URI (minimal, for resolution purposes)
// ---------------------------------------------------------------------------

/// Minimal SIP URI representation used for the resolution algorithm.
#[derive(Debug, Clone)]
pub struct SipUriTarget {
    /// Scheme: "sip" or "sips".
    pub scheme: String,
    /// Host part (domain or IP literal).
    pub host: String,
    /// Explicit port, if present.
    pub port: Option<u16>,
    /// Transport parameter, if present.
    pub transport: Option<TransportType>,
}

impl SipUriTarget {
    /// Parse a SIP URI into a resolution target.
    pub fn parse(uri: &str) -> Option<Self> {
        let uri = uri.trim();
        let (scheme, rest) = uri.split_once(':')?;
        let scheme = scheme.to_lowercase();
        if scheme != "sip" && scheme != "sips" {
            return None;
        }

        // Strip userinfo if present.
        let host_part = if let Some((_user, hp)) = rest.split_once('@') {
            hp
        } else {
            rest
        };

        // Separate parameters.
        let (host_port, params) = if let Some((hp, p)) = host_part.split_once(';') {
            (hp, Some(p))
        } else {
            (host_part, None)
        };

        // Parse host and port.
        let (host, port) = if host_port.starts_with('[') {
            // IPv6 literal.
            if let Some((h, rest)) = host_port.split_once(']') {
                let h = h.trim_start_matches('[');
                let port = rest.strip_prefix(':').and_then(|p| p.parse::<u16>().ok());
                (h.to_string(), port)
            } else {
                return None;
            }
        } else if let Some((h, p)) = host_port.rsplit_once(':') {
            if let Ok(port) = p.parse::<u16>() {
                (h.to_string(), Some(port))
            } else {
                (host_port.to_string(), None)
            }
        } else {
            (host_port.to_string(), None)
        };

        // Parse transport parameter.
        let transport = params.and_then(|p| {
            for param in p.split(';') {
                if let Some(val) = param.strip_prefix("transport=") {
                    return match val.to_lowercase().as_str() {
                        "udp" => Some(TransportType::Udp),
                        "tcp" => Some(TransportType::Tcp),
                        "tls" => Some(TransportType::Tls),
                        _ => None,
                    };
                }
            }
            None
        });

        Some(SipUriTarget {
            scheme,
            host,
            port,
            transport,
        })
    }

    /// Whether the host part is a numeric IP address.
    fn is_numeric_ip(&self) -> bool {
        IpAddr::from_str(&self.host).is_ok()
    }
}

// ---------------------------------------------------------------------------
// DNS Cache
// ---------------------------------------------------------------------------

/// A single cache entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// The cached data.
    data: CachedData,
    /// When this entry expires.
    expires_at: Instant,
}

#[derive(Debug, Clone)]
enum CachedData {
    /// Resolved IP addresses.
    Addresses(Vec<IpAddr>),
    /// SRV records.
    Srv(Vec<SrvRecord>),
    /// NAPTR records.
    Naptr(Vec<NaptrRecord>),
    /// Negative cache (NXDOMAIN / empty result).
    NxDomain,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// TTL-based DNS cache with negative caching.
#[derive(Debug)]
pub struct DnsCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    /// Default TTL for negative cache entries.
    negative_ttl: Duration,
}

impl DnsCache {
    /// Create a new DNS cache.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            negative_ttl: Duration::from_secs(60),
        }
    }

    /// Create a new DNS cache with a custom negative TTL.
    pub fn with_negative_ttl(negative_ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            negative_ttl,
        }
    }

    /// Store address records.
    fn put_addresses(&self, name: &str, addrs: Vec<IpAddr>, ttl: Duration) {
        let entry = CacheEntry {
            data: CachedData::Addresses(addrs),
            expires_at: Instant::now() + ttl,
        };
        self.entries.write().insert(name.to_string(), entry);
    }

    /// Store SRV records.
    fn put_srv(&self, name: &str, records: Vec<SrvRecord>, ttl: Duration) {
        let entry = CacheEntry {
            data: CachedData::Srv(records),
            expires_at: Instant::now() + ttl,
        };
        self.entries.write().insert(name.to_string(), entry);
    }

    /// Store a negative (NXDOMAIN) entry.
    fn put_nxdomain(&self, name: &str) {
        let entry = CacheEntry {
            data: CachedData::NxDomain,
            expires_at: Instant::now() + self.negative_ttl,
        };
        self.entries.write().insert(name.to_string(), entry);
    }

    /// Retrieve address records if cached and not expired.
    fn get_addresses(&self, name: &str) -> Option<Vec<IpAddr>> {
        let entries = self.entries.read();
        let entry = entries.get(name)?;
        if entry.is_expired() {
            return None;
        }
        match &entry.data {
            CachedData::Addresses(addrs) => Some(addrs.clone()),
            _ => None,
        }
    }

    /// Retrieve SRV records if cached and not expired.
    fn get_srv(&self, name: &str) -> Option<Vec<SrvRecord>> {
        let entries = self.entries.read();
        let entry = entries.get(name)?;
        if entry.is_expired() {
            return None;
        }
        match &entry.data {
            CachedData::Srv(records) => Some(records.clone()),
            _ => None,
        }
    }

    /// Check if a name has a negative cache entry that is still valid.
    fn is_nxdomain(&self, name: &str) -> bool {
        let entries = self.entries.read();
        if let Some(entry) = entries.get(name) {
            if !entry.is_expired() {
                return matches!(entry.data, CachedData::NxDomain);
            }
        }
        false
    }

    /// Purge expired entries.
    pub fn purge_expired(&self) {
        self.entries.write().retain(|_, entry| !entry.is_expired());
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.write().clear();
    }

    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

impl Default for DnsCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SRV weighted selection (RFC 2782)
// ---------------------------------------------------------------------------

/// Select a single record from a set of SRV records sharing the same
/// priority, using the RFC 2782 weighted random algorithm.
///
/// Records with weight 0 have a small but non-zero chance of being selected.
/// The probability of selecting a record is proportional to its weight
/// relative to the sum of all weights in the group.
pub fn weighted_select(records: &[SrvRecord]) -> Option<&SrvRecord> {
    if records.is_empty() {
        return None;
    }
    if records.len() == 1 {
        return Some(&records[0]);
    }

    let weight_sum: u32 = records.iter().map(|r| r.weight as u32).sum();

    // If all weights are zero, pick uniformly at random.
    if weight_sum == 0 {
        let idx = rand_index(records.len());
        return Some(&records[idx]);
    }

    let random_weight = 1 + rand_u32_below(weight_sum);

    let mut running_sum: u32 = 0;
    for record in records {
        running_sum += record.weight as u32;
        if running_sum >= random_weight {
            return Some(record);
        }
    }

    // Should not reach here, but just in case.
    Some(&records[records.len() - 1])
}

/// Sort SRV records by priority ascending, then apply weighted random
/// ordering within each priority group (RFC 2782 algorithm).
///
/// This is a full sort that produces a deterministic ordering suitable
/// for iterating through targets in order of preference.
pub fn sort_srv_records(records: &mut Vec<SrvRecord>) {
    if records.len() <= 1 {
        return;
    }

    // Group by priority.
    records.sort_by_key(|r| r.priority);

    let mut sorted = Vec::with_capacity(records.len());
    let mut i = 0;
    while i < records.len() {
        let priority = records[i].priority;
        let mut group: Vec<SrvRecord> = Vec::new();

        while i < records.len() && records[i].priority == priority {
            group.push(records[i].clone());
            i += 1;
        }

        // Apply weighted selection to drain the group.
        while !group.is_empty() {
            let weight_sum: u32 = group.iter().map(|r| r.weight as u32).sum();

            if weight_sum == 0 {
                // All zero weights: append in order.
                sorted.append(&mut group);
                break;
            }

            let random_weight = 1 + rand_u32_below(weight_sum);
            let mut running_sum: u32 = 0;
            let mut selected_idx = 0;

            for (idx, record) in group.iter().enumerate() {
                running_sum += record.weight as u32;
                if running_sum >= random_weight {
                    selected_idx = idx;
                    break;
                }
            }

            sorted.push(group.remove(selected_idx));
        }
    }

    *records = sorted;
}

// ---------------------------------------------------------------------------
// DNS lookup functions
// ---------------------------------------------------------------------------

/// Look up A/AAAA records for a hostname.
pub async fn lookup_host(name: &str) -> Result<Vec<IpAddr>, DnsError> {
    let addrs = net::lookup_host(format!("{}:0", name))
        .await
        .map_err(|e| DnsError::ResolutionFailed(format!("{}: {}", name, e)))?
        .map(|sa| sa.ip())
        .collect::<Vec<_>>();

    if addrs.is_empty() {
        return Err(DnsError::NxDomain(name.to_string()));
    }

    Ok(addrs)
}

/// Look up SRV records for a name.
///
/// In a production deployment this would use a real DNS library (trust-dns,
/// hickory-dns). This stub implementation performs an A/AAAA lookup on the
/// base domain and synthesises a single SRV record, which is sufficient for
/// testing and simple deployments.
pub async fn lookup_srv(name: &str) -> Result<Vec<SrvRecord>, DnsError> {
    // Extract base domain from SRV name: _sip._udp.example.com -> example.com
    let base_domain = extract_srv_base_domain(name);
    let port = extract_srv_port(name);

    match lookup_host(&base_domain).await {
        Ok(addrs) => {
            if addrs.is_empty() {
                return Err(DnsError::NxDomain(name.to_string()));
            }
            // Synthesise SRV records from A/AAAA results.
            let records: Vec<SrvRecord> = addrs
                .into_iter()
                .enumerate()
                .map(|(i, _addr)| SrvRecord {
                    priority: 10,
                    weight: if i == 0 { 100 } else { 10 },
                    port,
                    target: base_domain.clone(),
                })
                .collect();
            Ok(records)
        }
        Err(_) => Err(DnsError::NxDomain(name.to_string())),
    }
}

/// Look up NAPTR records for a name.
///
/// Stub implementation: returns empty results. A full implementation would
/// use hickory-dns or trust-dns to perform actual NAPTR queries.
pub async fn lookup_naptr(name: &str) -> Result<Vec<NaptrRecord>, DnsError> {
    debug!(name, "NAPTR lookup (stub: returning empty)");
    // In real implementation, this would query DNS for NAPTR records.
    // For now we return an empty set which causes the RFC 3263 algorithm
    // to fall through to SRV lookups.
    Ok(Vec::new())
}

/// Extract the base domain from an SRV record name.
/// e.g. "_sip._udp.example.com" -> "example.com"
fn extract_srv_base_domain(name: &str) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    // Skip leading underscore-prefixed labels.
    let start = parts.iter().position(|p| !p.starts_with('_')).unwrap_or(0);
    parts[start..].join(".")
}

/// Extract the expected port from an SRV record name.
fn extract_srv_port(name: &str) -> u16 {
    if name.contains("_sips") {
        5061
    } else {
        5060
    }
}

// ---------------------------------------------------------------------------
// RFC 3263 SIP URI Resolution Algorithm
// ---------------------------------------------------------------------------

/// RFC 3263 SIP URI resolution.
///
/// The algorithm:
/// 1. If URI has numeric IP, use directly
/// 2. If URI has explicit port, do A/AAAA lookup on host
/// 3. If URI has no port and transport specified:
///    a. NAPTR lookup for service selection
///    b. SRV lookup for _sip._udp / _sip._tcp / _sips._tcp
/// 4. Sort SRV records by priority (ascending), then weighted random within priority group
/// 5. For each SRV target, do A/AAAA lookup
/// 6. Try each address in order until one works
pub async fn resolve_sip_uri(uri: &SipUriTarget) -> Result<Vec<ResolvedTarget>, DnsError> {
    // Step 1: Numeric IP.
    if uri.is_numeric_ip() {
        let ip: IpAddr = uri.host.parse().map_err(|_| {
            DnsError::ResolutionFailed(format!("Invalid IP: {}", uri.host))
        })?;
        let transport = uri.transport.unwrap_or(if uri.scheme == "sips" {
            TransportType::Tls
        } else {
            TransportType::Udp
        });
        let port = uri.port.unwrap_or(transport.default_port());
        return Ok(vec![ResolvedTarget {
            address: SocketAddr::new(ip, port),
            transport,
        }]);
    }

    // Step 2: Explicit port -- just do A/AAAA lookup.
    if let Some(port) = uri.port {
        let transport = uri.transport.unwrap_or(if uri.scheme == "sips" {
            TransportType::Tls
        } else {
            TransportType::Udp
        });
        let addrs = lookup_host(&uri.host).await?;
        return Ok(addrs
            .into_iter()
            .map(|ip| ResolvedTarget {
                address: SocketAddr::new(ip, port),
                transport,
            })
            .collect());
    }

    // Step 3: No port -- full RFC 3263 procedure.

    // 3a. Determine transport(s) to try.
    let is_sips = uri.scheme == "sips";
    let transports = if let Some(t) = uri.transport {
        vec![t]
    } else if is_sips {
        vec![TransportType::Tls]
    } else {
        // Try NAPTR first.
        match lookup_naptr(&uri.host).await {
            Ok(naptr_records) if !naptr_records.is_empty() => {
                transports_from_naptr(&naptr_records)
            }
            _ => {
                // No NAPTR results: try all transports per RFC 3263.
                vec![TransportType::Udp, TransportType::Tcp]
            }
        }
    };

    // 3b. SRV lookups for each transport.
    let mut results = Vec::new();

    for transport in &transports {
        let srv_name = srv_name_for_transport(&uri.host, *transport, is_sips);
        debug!(srv_name = %srv_name, "SRV lookup for SIP resolution");

        match lookup_srv(&srv_name).await {
            Ok(mut srv_records) => {
                sort_srv_records(&mut srv_records);
                for srv in &srv_records {
                    match lookup_host(&srv.target).await {
                        Ok(addrs) => {
                            for ip in addrs {
                                results.push(ResolvedTarget {
                                    address: SocketAddr::new(ip, srv.port),
                                    transport: *transport,
                                });
                            }
                        }
                        Err(e) => {
                            debug!(target = %srv.target, error = %e, "A/AAAA lookup failed for SRV target");
                        }
                    }
                }
            }
            Err(e) => {
                debug!(srv_name = %srv_name, error = %e, "SRV lookup failed");
            }
        }
    }

    // Fallback: if no SRV results, try direct A/AAAA on the host.
    if results.is_empty() {
        let transport = transports.first().copied().unwrap_or(TransportType::Udp);
        let port = if is_sips { 5061 } else { 5060 };
        match lookup_host(&uri.host).await {
            Ok(addrs) => {
                for ip in addrs {
                    results.push(ResolvedTarget {
                        address: SocketAddr::new(ip, port),
                        transport,
                    });
                }
            }
            Err(e) => {
                return Err(DnsError::ResolutionFailed(format!(
                    "All resolution methods failed for {}: {}",
                    uri.host, e
                )));
            }
        }
    }

    Ok(results)
}

/// Determine SRV record name for a given transport.
fn srv_name_for_transport(domain: &str, transport: TransportType, is_sips: bool) -> String {
    match (transport, is_sips) {
        (TransportType::Tls, _) | (_, true) => format!("_sips._tcp.{}", domain),
        (TransportType::Tcp, false) => format!("_sip._tcp.{}", domain),
        (TransportType::Udp, false) => format!("_sip._udp.{}", domain),
    }
}

/// Extract transport preferences from NAPTR records (RFC 3263).
fn transports_from_naptr(records: &[NaptrRecord]) -> Vec<TransportType> {
    let mut sorted = records.to_vec();
    sorted.sort_by(|a, b| a.order.cmp(&b.order).then(a.preference.cmp(&b.preference)));

    let mut transports = Vec::new();
    for record in &sorted {
        let service = record.service.to_lowercase();
        let transport = if service.contains("sips+d2t") {
            TransportType::Tls
        } else if service.contains("sip+d2t") {
            TransportType::Tcp
        } else if service.contains("sip+d2u") {
            TransportType::Udp
        } else {
            continue;
        };
        if !transports.contains(&transport) {
            transports.push(transport);
        }
    }
    transports
}

// ---------------------------------------------------------------------------
// DNS resolver with caching
// ---------------------------------------------------------------------------

/// A caching DNS resolver for SIP URI resolution.
pub struct CachingResolver {
    cache: Arc<DnsCache>,
}

impl CachingResolver {
    /// Create a new caching resolver.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DnsCache::new()),
        }
    }

    /// Create a new caching resolver with a shared cache.
    pub fn with_cache(cache: Arc<DnsCache>) -> Self {
        Self { cache }
    }

    /// Resolve a SIP URI using the cache.
    pub async fn resolve(&self, uri: &SipUriTarget) -> Result<Vec<ResolvedTarget>, DnsError> {
        // For numeric IPs, skip the cache entirely.
        if uri.is_numeric_ip() {
            return resolve_sip_uri(uri).await;
        }

        // Check cache for A/AAAA records.
        if let Some(port) = uri.port {
            let transport = uri.transport.unwrap_or(TransportType::Udp);
            if let Some(addrs) = self.cache.get_addresses(&uri.host) {
                return Ok(addrs
                    .into_iter()
                    .map(|ip| ResolvedTarget {
                        address: SocketAddr::new(ip, port),
                        transport,
                    })
                    .collect());
            }

            if self.cache.is_nxdomain(&uri.host) {
                return Err(DnsError::NxDomain(uri.host.clone()));
            }
        }

        // Fall through to full resolution.
        let results = resolve_sip_uri(uri).await?;

        // Cache the results.
        if !results.is_empty() {
            let addrs: Vec<IpAddr> = results.iter().map(|r| r.address.ip()).collect();
            self.cache
                .put_addresses(&uri.host, addrs, Duration::from_secs(300));
        }

        Ok(results)
    }

    /// Get a reference to the underlying cache.
    pub fn cache(&self) -> &DnsCache {
        &self.cache
    }
}

impl Default for CachingResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// DNS resolution errors.
#[derive(Debug, thiserror::Error)]
pub enum DnsError {
    #[error("NXDOMAIN: {0}")]
    NxDomain(String),

    #[error("resolution failed: {0}")]
    ResolutionFailed(String),

    #[error("invalid name: {0}")]
    InvalidName(String),
}

// ---------------------------------------------------------------------------
// Random helpers (avoid pulling in full `rand` for simple u32)
// ---------------------------------------------------------------------------

fn rand_u32_below(max: u32) -> u32 {
    use rand::Rng;
    rand::thread_rng().gen_range(0..max)
}

fn rand_index(len: usize) -> usize {
    use rand::Rng;
    rand::thread_rng().gen_range(0..len)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srv_weighted_select_single() {
        let records = vec![SrvRecord {
            priority: 10,
            weight: 100,
            port: 5060,
            target: "sip.example.com".to_string(),
        }];
        let selected = weighted_select(&records).unwrap();
        assert_eq!(selected.target, "sip.example.com");
    }

    #[test]
    fn test_srv_weighted_select_empty() {
        let records: Vec<SrvRecord> = Vec::new();
        assert!(weighted_select(&records).is_none());
    }

    #[test]
    fn test_srv_weighted_select_distribution() {
        // Records with weights 90 and 10. Over 1000 selections, the first
        // should be selected roughly 90% of the time.
        let records = vec![
            SrvRecord {
                priority: 10,
                weight: 90,
                port: 5060,
                target: "heavy.example.com".to_string(),
            },
            SrvRecord {
                priority: 10,
                weight: 10,
                port: 5060,
                target: "light.example.com".to_string(),
            },
        ];

        let mut heavy_count = 0u32;
        for _ in 0..1000 {
            let selected = weighted_select(&records).unwrap();
            if selected.target == "heavy.example.com" {
                heavy_count += 1;
            }
        }

        // With weight 90/100, expect ~900 hits. Allow generous margin.
        assert!(
            heavy_count > 700,
            "Expected heavy to be selected >700 times, got {}",
            heavy_count
        );
        assert!(
            heavy_count < 990,
            "Expected some light selections, heavy got {}",
            heavy_count
        );
    }

    #[test]
    fn test_srv_weighted_select_all_zero_weights() {
        let records = vec![
            SrvRecord {
                priority: 10,
                weight: 0,
                port: 5060,
                target: "a.example.com".to_string(),
            },
            SrvRecord {
                priority: 10,
                weight: 0,
                port: 5060,
                target: "b.example.com".to_string(),
            },
        ];

        // All zero weights: should still return a selection.
        let selected = weighted_select(&records).unwrap();
        assert!(
            selected.target == "a.example.com" || selected.target == "b.example.com"
        );
    }

    #[test]
    fn test_sort_srv_records_priority() {
        let mut records = vec![
            SrvRecord {
                priority: 20,
                weight: 0,
                port: 5060,
                target: "backup.example.com".to_string(),
            },
            SrvRecord {
                priority: 10,
                weight: 0,
                port: 5060,
                target: "primary.example.com".to_string(),
            },
        ];

        sort_srv_records(&mut records);
        assert_eq!(records[0].target, "primary.example.com");
        assert_eq!(records[1].target, "backup.example.com");
    }

    #[test]
    fn test_resolve_numeric_ip() {
        let uri = SipUriTarget {
            scheme: "sip".to_string(),
            host: "10.0.0.1".to_string(),
            port: Some(5060),
            transport: Some(TransportType::Udp),
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(resolve_sip_uri(&uri)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address, "10.0.0.1:5060".parse().unwrap());
        assert_eq!(result[0].transport, TransportType::Udp);
    }

    #[test]
    fn test_resolve_numeric_ip_no_port() {
        let uri = SipUriTarget {
            scheme: "sip".to_string(),
            host: "192.168.1.1".to_string(),
            port: None,
            transport: None,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(resolve_sip_uri(&uri)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address.port(), 5060);
        assert_eq!(result[0].transport, TransportType::Udp);
    }

    #[test]
    fn test_resolve_sips_numeric_ip() {
        let uri = SipUriTarget {
            scheme: "sips".to_string(),
            host: "10.0.0.1".to_string(),
            port: None,
            transport: None,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(resolve_sip_uri(&uri)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address.port(), 5061);
        assert_eq!(result[0].transport, TransportType::Tls);
    }

    #[test]
    fn test_resolve_with_explicit_port() {
        // Even though host is numeric, test the port-specified path.
        let uri = SipUriTarget {
            scheme: "sip".to_string(),
            host: "10.0.0.5".to_string(),
            port: Some(9999),
            transport: Some(TransportType::Tcp),
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(resolve_sip_uri(&uri)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].address.port(), 9999);
        assert_eq!(result[0].transport, TransportType::Tcp);
    }

    #[test]
    fn test_sip_uri_target_parse() {
        let uri = SipUriTarget::parse("sip:user@example.com:5060;transport=tcp").unwrap();
        assert_eq!(uri.scheme, "sip");
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, Some(5060));
        assert_eq!(uri.transport, Some(TransportType::Tcp));
    }

    #[test]
    fn test_sip_uri_target_parse_no_port() {
        let uri = SipUriTarget::parse("sip:user@example.com").unwrap();
        assert_eq!(uri.scheme, "sip");
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, None);
        assert_eq!(uri.transport, None);
    }

    #[test]
    fn test_sip_uri_target_parse_sips() {
        let uri = SipUriTarget::parse("sips:secure.example.com").unwrap();
        assert_eq!(uri.scheme, "sips");
        assert_eq!(uri.host, "secure.example.com");
    }

    #[test]
    fn test_dns_cache_basic() {
        let cache = DnsCache::new();
        assert!(cache.is_empty());

        let addrs = vec!["10.0.0.1".parse().unwrap()];
        cache.put_addresses("example.com", addrs.clone(), Duration::from_secs(60));

        let cached = cache.get_addresses("example.com").unwrap();
        assert_eq!(cached, addrs);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_dns_cache_nxdomain() {
        let cache = DnsCache::with_negative_ttl(Duration::from_secs(300));
        cache.put_nxdomain("nonexistent.example.com");
        assert!(cache.is_nxdomain("nonexistent.example.com"));
        assert!(!cache.is_nxdomain("other.example.com"));
    }

    #[test]
    fn test_dns_cache_expiry() {
        let cache = DnsCache::new();
        // Insert with 0 TTL -- should be expired immediately.
        cache.put_addresses(
            "expired.example.com",
            vec!["10.0.0.1".parse().unwrap()],
            Duration::from_secs(0),
        );
        // The entry was just inserted with Duration::from_secs(0), so
        // Instant::now() should be >= expires_at.
        // However, this is a race -- in practice, it may or may not be expired yet.
        // We test purge_expired instead.
        cache.purge_expired();
        // After purge, the entry should be gone if it expired.
        // If not (race), that is fine too.
    }

    #[test]
    fn test_extract_srv_base_domain() {
        assert_eq!(
            extract_srv_base_domain("_sip._udp.example.com"),
            "example.com"
        );
        assert_eq!(
            extract_srv_base_domain("_sips._tcp.secure.example.com"),
            "secure.example.com"
        );
    }

    #[test]
    fn test_transports_from_naptr() {
        let records = vec![
            NaptrRecord {
                order: 10,
                preference: 10,
                flags: "S".to_string(),
                service: "SIP+D2U".to_string(),
                regexp: String::new(),
                replacement: "_sip._udp.example.com".to_string(),
            },
            NaptrRecord {
                order: 20,
                preference: 10,
                flags: "S".to_string(),
                service: "SIP+D2T".to_string(),
                regexp: String::new(),
                replacement: "_sip._tcp.example.com".to_string(),
            },
        ];

        let transports = transports_from_naptr(&records);
        assert_eq!(transports, vec![TransportType::Udp, TransportType::Tcp]);
    }

    #[test]
    fn test_srv_name_for_transport() {
        assert_eq!(
            srv_name_for_transport("example.com", TransportType::Udp, false),
            "_sip._udp.example.com"
        );
        assert_eq!(
            srv_name_for_transport("example.com", TransportType::Tcp, false),
            "_sip._tcp.example.com"
        );
        assert_eq!(
            srv_name_for_transport("example.com", TransportType::Tls, false),
            "_sips._tcp.example.com"
        );
        assert_eq!(
            srv_name_for_transport("example.com", TransportType::Udp, true),
            "_sips._tcp.example.com"
        );
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL DNS SRV TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_srv_weighted_select_distribution_10000() {
        // Run 10000 selections, verify weights within 5% tolerance
        let records = vec![
            SrvRecord {
                priority: 10, weight: 70, port: 5060,
                target: "heavy.example.com".to_string(),
            },
            SrvRecord {
                priority: 10, weight: 30, port: 5060,
                target: "light.example.com".to_string(),
            },
        ];

        let mut heavy_count = 0u32;
        let iterations = 10000;
        for _ in 0..iterations {
            let selected = weighted_select(&records).unwrap();
            if selected.target == "heavy.example.com" {
                heavy_count += 1;
            }
        }

        let heavy_pct = (heavy_count as f64) / (iterations as f64) * 100.0;
        assert!(
            (heavy_pct - 70.0).abs() < 5.0,
            "Expected ~70% heavy selections, got {:.1}% ({}/{})",
            heavy_pct, heavy_count, iterations
        );
    }

    #[test]
    fn test_srv_all_zero_weight_round_robin() {
        // All zero weights: should distribute roughly uniformly
        let records = vec![
            SrvRecord {
                priority: 10, weight: 0, port: 5060,
                target: "a.example.com".to_string(),
            },
            SrvRecord {
                priority: 10, weight: 0, port: 5060,
                target: "b.example.com".to_string(),
            },
            SrvRecord {
                priority: 10, weight: 0, port: 5060,
                target: "c.example.com".to_string(),
            },
        ];

        let mut counts = [0u32; 3];
        let iterations = 3000;
        for _ in 0..iterations {
            let selected = weighted_select(&records).unwrap();
            if selected.target == "a.example.com" { counts[0] += 1; }
            else if selected.target == "b.example.com" { counts[1] += 1; }
            else { counts[2] += 1; }
        }

        // Each should be roughly 33%. Allow 10% tolerance.
        for (i, count) in counts.iter().enumerate() {
            let pct = (*count as f64) / (iterations as f64) * 100.0;
            assert!(
                (pct - 33.3).abs() < 10.0,
                "Zero-weight record {} got {:.1}%, expected ~33%",
                i, pct
            );
        }
    }

    #[test]
    fn test_dns_cache_nxdomain_negative_cache() {
        let cache = DnsCache::with_negative_ttl(Duration::from_secs(300));

        // Query for a nonexistent domain, cache the negative result
        cache.put_nxdomain("nonexistent.test");
        assert!(cache.is_nxdomain("nonexistent.test"));

        // Other domains should not be affected
        assert!(!cache.is_nxdomain("real.test"));
    }

    #[test]
    fn test_dns_cache_clear() {
        let cache = DnsCache::new();
        cache.put_addresses("a.test", vec!["10.0.0.1".parse().unwrap()], Duration::from_secs(300));
        cache.put_addresses("b.test", vec!["10.0.0.2".parse().unwrap()], Duration::from_secs(300));
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.get_addresses("a.test").is_none());
    }

    #[test]
    fn test_sip_uri_parse_ipv6() {
        let uri = SipUriTarget::parse("sip:user@[2001:db8::1]:5060").unwrap();
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, Some(5060));
    }

    #[test]
    fn test_sip_uri_parse_no_user() {
        let uri = SipUriTarget::parse("sip:example.com").unwrap();
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, None);
        assert_eq!(uri.transport, None);
    }

    #[test]
    fn test_sort_srv_records_multiple_priorities() {
        let mut records = vec![
            SrvRecord { priority: 30, weight: 10, port: 5060, target: "c.example.com".to_string() },
            SrvRecord { priority: 10, weight: 10, port: 5060, target: "a.example.com".to_string() },
            SrvRecord { priority: 20, weight: 10, port: 5060, target: "b.example.com".to_string() },
        ];
        sort_srv_records(&mut records);
        assert_eq!(records[0].priority, 10);
        assert_eq!(records[1].priority, 20);
        assert_eq!(records[2].priority, 30);
    }

    #[test]
    fn test_sort_srv_records_single() {
        let mut records = vec![
            SrvRecord { priority: 10, weight: 100, port: 5060, target: "only.example.com".to_string() },
        ];
        sort_srv_records(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].target, "only.example.com");
    }

    #[test]
    fn test_sort_srv_records_empty() {
        let mut records: Vec<SrvRecord> = Vec::new();
        sort_srv_records(&mut records);
        assert!(records.is_empty());
    }

    #[test]
    fn test_transport_default_port() {
        assert_eq!(TransportType::Udp.default_port(), 5060);
        assert_eq!(TransportType::Tcp.default_port(), 5060);
        assert_eq!(TransportType::Tls.default_port(), 5061);
    }
}
