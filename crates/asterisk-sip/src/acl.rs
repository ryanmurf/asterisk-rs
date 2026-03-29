//! SIP Access Control Lists (port of res_pjsip_acl.c).
//!
//! Provides IP-based and Contact-header-based access control for incoming
//! SIP traffic. ACL rules are evaluated in order; the first match wins.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tracing::warn;

use crate::parser::{extract_uri, header_names, SipMessage, SipUri};

// ---------------------------------------------------------------------------
// ACL rule
// ---------------------------------------------------------------------------

/// Whether a rule permits or denies access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclAction {
    Permit,
    Deny,
}

/// A single ACL rule matching an IP address/network.
#[derive(Debug, Clone)]
pub struct AclRule {
    /// Whether this rule permits or denies.
    pub action: AclAction,
    /// Network address (host part of the CIDR).
    pub address: IpAddr,
    /// CIDR prefix length (e.g. 24 for /24).
    pub prefix_len: u8,
}

impl AclRule {
    /// Create a permit rule.
    pub fn permit(cidr: &str) -> Option<Self> {
        Self::parse(AclAction::Permit, cidr)
    }

    /// Create a deny rule.
    pub fn deny(cidr: &str) -> Option<Self> {
        Self::parse(AclAction::Deny, cidr)
    }

    fn parse(action: AclAction, cidr: &str) -> Option<Self> {
        let cidr = cidr.trim();

        // Handle "address/mask" or "address/prefix" or bare "address".
        let (addr_str, mask_str) = match cidr.split_once('/') {
            Some((a, m)) => (a, Some(m)),
            None => (cidr, None),
        };

        let address: IpAddr = addr_str.parse().ok()?;

        let prefix_len = match mask_str {
            Some(m) => {
                // Try CIDR prefix length first.
                if let Ok(p) = m.parse::<u8>() {
                    p
                } else {
                    // Try dotted-decimal netmask (IPv4 only).
                    netmask_to_prefix(m)?
                }
            }
            None => match address {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            },
        };

        Some(AclRule {
            action,
            address,
            prefix_len,
        })
    }

    /// Check whether the given IP address matches this rule.
    pub fn matches(&self, addr: &IpAddr) -> bool {
        match (&self.address, addr) {
            (IpAddr::V4(net), IpAddr::V4(check)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                if self.prefix_len >= 32 {
                    return net == check;
                }
                let mask = u32::MAX << (32 - self.prefix_len);
                (u32::from(*net) & mask) == (u32::from(*check) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(check)) => {
                if self.prefix_len == 0 {
                    return true;
                }
                if self.prefix_len >= 128 {
                    return net == check;
                }
                let net_bits = u128::from(*net);
                let check_bits = u128::from(*check);
                let mask = u128::MAX << (128 - self.prefix_len);
                (net_bits & mask) == (check_bits & mask)
            }
            _ => false, // v4/v6 mismatch.
        }
    }
}

/// Convert a dotted-decimal netmask (e.g. "255.255.255.0") to a CIDR prefix.
///
/// Rejects non-contiguous masks (e.g. "255.0.255.0") that cannot be
/// represented as a CIDR prefix.
fn netmask_to_prefix(mask: &str) -> Option<u8> {
    let ip: Ipv4Addr = mask.parse().ok()?;
    let bits = u32::from(ip);
    let leading = bits.leading_ones() as u8;
    // Verify the mask is contiguous: after the leading 1s, all bits must be 0.
    let expected = if leading == 32 {
        u32::MAX
    } else if leading == 0 {
        0u32
    } else {
        u32::MAX << (32 - leading)
    };
    if bits != expected {
        return None; // Non-contiguous mask.
    }
    Some(leading)
}

// ---------------------------------------------------------------------------
// ACL (ordered list of rules)
// ---------------------------------------------------------------------------

/// An access control list composed of ordered rules.
#[derive(Debug, Clone, Default)]
pub struct Acl {
    /// Name of this ACL (for logging/identification).
    pub name: String,
    /// Rules evaluated in order.
    pub rules: Vec<AclRule>,
}

impl Acl {
    /// Create a new empty ACL.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            rules: Vec::new(),
        }
    }

    /// Add a permit rule from a CIDR string.
    pub fn permit(&mut self, cidr: &str) -> &mut Self {
        if let Some(rule) = AclRule::permit(cidr) {
            self.rules.push(rule);
        } else {
            warn!(acl = %self.name, cidr, "Invalid permit rule");
        }
        self
    }

    /// Add a deny rule from a CIDR string.
    pub fn deny(&mut self, cidr: &str) -> &mut Self {
        if let Some(rule) = AclRule::deny(cidr) {
            self.rules.push(rule);
        } else {
            warn!(acl = %self.name, cidr, "Invalid deny rule");
        }
        self
    }

    /// Check whether the given IP address is allowed by this ACL.
    ///
    /// Rules are evaluated in order. If no rule matches, access is allowed
    /// (default-permit behavior, same as Asterisk's `ast_apply_acl`).
    pub fn check(&self, addr: &IpAddr) -> bool {
        for rule in &self.rules {
            if rule.matches(addr) {
                return rule.action == AclAction::Permit;
            }
        }
        // Default: allow if no rules match.
        true
    }

    /// Check a socket address (ignoring the port).
    pub fn check_addr(&self, addr: &SocketAddr) -> bool {
        self.check(&addr.ip())
    }

    /// Return true if this ACL has no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

// ---------------------------------------------------------------------------
// SIP ACL checker
// ---------------------------------------------------------------------------

/// A SIP-level ACL checker that combines IP-based and Contact-header-based
/// access control.
#[derive(Debug, Clone, Default)]
pub struct SipAcl {
    /// IP-based ACL applied to the source address of the SIP message.
    pub ip_acl: Acl,
    /// Contact-header-based ACL applied to addresses in Contact headers
    /// of incoming REGISTER requests.
    pub contact_acl: Acl,
}

impl SipAcl {
    pub fn new(name: &str) -> Self {
        Self {
            ip_acl: Acl::new(&format!("{}-ip", name)),
            contact_acl: Acl::new(&format!("{}-contact", name)),
        }
    }

    /// Check whether a SIP message from the given source address is allowed.
    pub fn check_message(&self, source: &SocketAddr, msg: &SipMessage) -> bool {
        // Check source IP ACL.
        if !self.ip_acl.is_empty() && !self.ip_acl.check_addr(source) {
            warn!(
                source = %source,
                acl = %self.ip_acl.name,
                "SIP message blocked by IP ACL"
            );
            return false;
        }

        // Check Contact header ACL (for REGISTER requests).
        if !self.contact_acl.is_empty() {
            for contact_hdr in msg.get_headers(header_names::CONTACT) {
                if contact_hdr.trim() == "*" {
                    continue;
                }
                if let Some(uri_str) = extract_uri(contact_hdr) {
                    if let Some(ip) = extract_ip_from_uri(&uri_str) {
                        if !self.contact_acl.check(&ip) {
                            warn!(
                                contact = %uri_str,
                                acl = %self.contact_acl.name,
                                "SIP message blocked by Contact ACL"
                            );
                            return false;
                        }
                    }
                }
            }
        }

        true
    }
}

/// Extract the IP address from a SIP URI string.
fn extract_ip_from_uri(uri_str: &str) -> Option<IpAddr> {
    SipUri::parse(uri_str)
        .ok()
        .and_then(|u| u.host.parse::<IpAddr>().ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acl_permit_deny() {
        let mut acl = Acl::new("test");
        acl.deny("10.0.0.0/8");
        acl.permit("10.0.1.0/24");

        // 10.0.1.5 matches the deny first (10.0.0.0/8), so denied.
        assert!(!acl.check(&"10.0.1.5".parse().unwrap()));
        // 192.168.1.1 doesn't match any rule -> default allow.
        assert!(acl.check(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_acl_permit_first() {
        let mut acl = Acl::new("test");
        acl.permit("10.0.1.0/24");
        acl.deny("10.0.0.0/8");

        // 10.0.1.5 matches the permit first.
        assert!(acl.check(&"10.0.1.5".parse().unwrap()));
        // 10.0.2.5 matches the deny.
        assert!(!acl.check(&"10.0.2.5".parse().unwrap()));
    }

    #[test]
    fn test_acl_exact_host() {
        let mut acl = Acl::new("test");
        acl.deny("192.168.1.100");

        assert!(!acl.check(&"192.168.1.100".parse().unwrap()));
        assert!(acl.check(&"192.168.1.101".parse().unwrap()));
    }

    #[test]
    fn test_acl_deny_all() {
        let mut acl = Acl::new("test");
        acl.deny("0.0.0.0/0");

        assert!(!acl.check(&"10.0.0.1".parse().unwrap()));
        assert!(!acl.check(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_netmask_to_prefix() {
        assert_eq!(netmask_to_prefix("255.255.255.0"), Some(24));
        assert_eq!(netmask_to_prefix("255.255.0.0"), Some(16));
        assert_eq!(netmask_to_prefix("255.255.255.255"), Some(32));
        assert_eq!(netmask_to_prefix("0.0.0.0"), Some(0));
    }

    #[test]
    fn test_netmask_cidr_in_rule() {
        let rule = AclRule::deny("10.0.0.0/255.255.0.0").unwrap();
        assert_eq!(rule.prefix_len, 16);
        assert!(rule.matches(&"10.0.5.5".parse().unwrap()));
        assert!(!rule.matches(&"10.1.0.1".parse().unwrap()));
    }

    #[test]
    fn test_ipv6_acl() {
        let mut acl = Acl::new("v6test");
        acl.permit("::1/128");
        acl.deny("::/0");

        assert!(acl.check(&"::1".parse().unwrap()));
        assert!(!acl.check(&"::2".parse().unwrap()));
    }
}
