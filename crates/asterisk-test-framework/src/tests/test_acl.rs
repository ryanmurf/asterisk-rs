//! Port of asterisk/tests/test_acl.c
//!
//! Tests Access Control List (ACL) behavior:
//! - IPv4 permit/deny rules
//! - CIDR notation matching
//! - Dotted-decimal netmask matching
//! - ACL ordering (last match wins in Asterisk)
//! - IPv6 ACL rules
//! - Invalid ACL rejection
//! - Combined/comma-separated rules
//!
//! Since the Rust codebase uses std::net for networking, we implement
//! a minimal ACL engine here that mirrors the C ast_ha behavior.

use std::net::{IpAddr, Ipv4Addr};

// ---------------------------------------------------------------------------
// ACL implementation mirroring Asterisk's ast_ha
// ---------------------------------------------------------------------------

/// Access sense -- mirrors AST_SENSE_ALLOW / AST_SENSE_DENY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sense {
    Allow,
    Deny,
}

/// A single ACL rule (host access entry).
#[derive(Debug, Clone)]
struct HaRule {
    sense: Sense,
    addr: IpAddr,
    prefix_len: u8,
}

/// An access control list -- a list of rules evaluated in order.
/// The last matching rule wins (Asterisk semantics).
#[derive(Debug, Clone, Default)]
struct Acl {
    rules: Vec<HaRule>,
}

/// Parse an address/mask string like "10.0.0.0/8" or "10.0.0.0/255.0.0.0"
/// or "fe80::/64". Returns None if the string is invalid.
fn parse_host_mask(s: &str) -> Option<(IpAddr, u8)> {
    let s = s.trim();
    let (addr_str, mask_str) = if let Some(slash) = s.find('/') {
        (&s[..slash], Some(&s[slash + 1..]))
    } else {
        (s, None)
    };

    let addr: IpAddr = addr_str.parse().ok()?;

    let prefix_len = match mask_str {
        None => match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        },
        Some(m) => {
            // Try CIDR notation first.
            if let Ok(p) = m.parse::<u8>() {
                match addr {
                    IpAddr::V4(_) if p > 32 => return None,
                    IpAddr::V6(_) if p > 128 => return None,
                    _ => p,
                }
            } else {
                // Try dotted-decimal netmask (IPv4 only).
                let mask: Ipv4Addr = m.parse().ok()?;
                let bits = u32::from(mask);
                // Validate it's a contiguous mask.
                if bits != 0 && bits.leading_ones() + bits.trailing_zeros() != 32 {
                    return None;
                }
                bits.leading_ones() as u8
            }
        }
    };

    Some((addr, prefix_len))
}

/// Check if `addr` matches the network `network/prefix_len`.
fn addr_matches(addr: &IpAddr, network: &IpAddr, prefix_len: u8) -> bool {
    match (addr, network) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            if prefix_len == 0 {
                return true;
            }
            let a_bits = u32::from(*a);
            let n_bits = u32::from(*n);
            let mask = u32::MAX.checked_shl(32 - prefix_len as u32).unwrap_or(0);
            (a_bits & mask) == (n_bits & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            if prefix_len == 0 {
                return true;
            }
            let a_bits = u128::from(*a);
            let n_bits = u128::from(*n);
            let mask = u128::MAX.checked_shl(128 - prefix_len as u32).unwrap_or(0);
            (a_bits & mask) == (n_bits & mask)
        }
        _ => false, // IPv4 addr never matches IPv6 rule and vice versa
    }
}

impl Acl {
    fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Append a rule. Returns Err if the host/mask is invalid.
    fn append(&mut self, sense_str: &str, host: &str) -> Result<(), String> {
        let sense = match sense_str {
            "permit" => Sense::Allow,
            "deny" => Sense::Deny,
            _ => return Err(format!("Unknown sense: {}", sense_str)),
        };

        let (addr, prefix_len) =
            parse_host_mask(host).ok_or_else(|| format!("Invalid host/mask: {}", host))?;

        self.rules.push(HaRule {
            sense,
            addr,
            prefix_len,
        });
        Ok(())
    }

    /// Apply the ACL to an address. Returns Allow or Deny.
    /// Default (no rules match) is Allow -- matching Asterisk behavior where
    /// an empty ACL permits everything.
    fn apply(&self, addr: &IpAddr) -> Sense {
        let mut result = Sense::Allow;
        for rule in &self.rules {
            if addr_matches(addr, &rule.addr, rule.prefix_len) {
                result = rule.sense;
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Tests: invalid ACL values should be rejected
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(invalid_acl) from test_acl.c.
///
/// Ensures that garbage ACL values are not accepted.
#[test]
fn test_invalid_acl_negative_netmask() {
    // "1.3.3.7/-1" -- negative netmask
    let mut acl = Acl::new();
    assert!(acl.append("permit", "1.3.3.7/-1").is_err());
}

#[test]
fn test_invalid_acl_netmask_too_large() {
    // "1.3.3.7/33" -- netmask > 32 for IPv4
    let mut acl = Acl::new();
    assert!(acl.append("permit", "1.3.3.7/33").is_err());
}

#[test]
fn test_invalid_acl_netmask_way_too_large() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "1.3.3.7/92342348927389492307420").is_err());
}

#[test]
fn test_invalid_acl_netmask_non_numeric() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "1.3.3.7/California").is_err());
}

#[test]
fn test_invalid_acl_octets_exceed_255() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "57.60.278.900/31").is_err());
}

#[test]
fn test_invalid_acl_bad_ip_format() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "EGGSOFDEATH/4000").is_err());
}

#[test]
fn test_invalid_acl_too_many_ip_octets() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "3.1.4.1.5.9/3").is_err());
}

#[test]
fn test_invalid_acl_ipv6_multiple_double_colons() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "ff::ff::ff/3").is_err());
}

#[test]
fn test_invalid_acl_ipv6_too_long() {
    let mut acl = Acl::new();
    assert!(
        acl.append("permit", "1234:5678:90ab:cdef:1234:5678:90ab:cdef:1234/56")
            .is_err()
    );
}

#[test]
fn test_invalid_acl_ipv6_netmask_too_large() {
    let mut acl = Acl::new();
    assert!(acl.append("permit", "::ffff/129").is_err());
}

// ---------------------------------------------------------------------------
// Tests: ACL permit/deny rules
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(acl) from test_acl.c.
///
/// Tests that hosts are properly permitted or denied with various ACL
/// configurations.

const ALLOW: Sense = Sense::Allow;
const DENY: Sense = Sense::Deny;

fn make_acl(rules: &[(&str, &str)]) -> Acl {
    let mut acl = Acl::new();
    for &(host, access) in rules {
        acl.append(access, host).unwrap();
    }
    acl
}

#[test]
fn test_acl_permit_all_v4() {
    let acl = make_acl(&[("0.0.0.0/0", "permit")]);
    let addr: IpAddr = "10.1.1.5".parse().unwrap();
    assert_eq!(acl.apply(&addr), ALLOW);
    let addr2: IpAddr = "172.16.0.1".parse().unwrap();
    assert_eq!(acl.apply(&addr2), ALLOW);
}

#[test]
fn test_acl_deny_all_v4() {
    let acl = make_acl(&[("0.0.0.0/0", "deny")]);
    let addr: IpAddr = "10.1.1.5".parse().unwrap();
    assert_eq!(acl.apply(&addr), DENY);
    let addr2: IpAddr = "192.168.0.5".parse().unwrap();
    assert_eq!(acl.apply(&addr2), DENY);
}

#[test]
fn test_acl_permit_all_v6() {
    let acl = make_acl(&[("::/0", "permit")]);
    let addr: IpAddr = "fe80::1234".parse().unwrap();
    assert_eq!(acl.apply(&addr), ALLOW);
}

#[test]
fn test_acl_deny_all_v6() {
    let acl = make_acl(&[("::/0", "deny")]);
    let addr: IpAddr = "fe80::1234".parse().unwrap();
    assert_eq!(acl.apply(&addr), DENY);
}

/// ACL1: deny all, then permit 10.0.0.0/8 and 192.168.0.0/24.
#[test]
fn test_acl1_permit_private() {
    let acl = make_acl(&[
        ("0.0.0.0/0", "deny"),
        ("10.0.0.0/8", "permit"),
        ("192.168.0.0/24", "permit"),
    ]);

    assert_eq!(acl.apply(&"10.1.1.5".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"192.168.0.5".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"192.168.1.5".parse().unwrap()), DENY);
    assert_eq!(acl.apply(&"10.0.0.1".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"172.16.0.1".parse().unwrap()), DENY);
}

/// ACL2: deny 10/8, permit 10/8, deny 10/16, permit 10/24.
/// Last match wins, so it's a complex layered ACL.
#[test]
fn test_acl2_layered_rules() {
    let acl = make_acl(&[
        ("10.0.0.0/8", "deny"),
        ("10.0.0.0/8", "permit"),
        ("10.0.0.0/16", "deny"),
        ("10.0.0.0/24", "permit"),
    ]);

    // 10.1.1.5: matches /8 deny, /8 permit, not /16, not /24 -> last match is /8 permit = ALLOW
    assert_eq!(acl.apply(&"10.1.1.5".parse().unwrap()), ALLOW);
    // 10.0.0.1: matches /8 deny, /8 permit, /16 deny, /24 permit -> last = /24 permit = ALLOW
    assert_eq!(acl.apply(&"10.0.0.1".parse().unwrap()), ALLOW);
    // 10.0.10.10: matches /8 deny, /8 permit, /16 deny, not /24 -> last = /16 deny = DENY
    assert_eq!(acl.apply(&"10.0.10.10".parse().unwrap()), DENY);
}

/// ACL3: deny all IPv6, permit fe80::/64.
#[test]
fn test_acl3_ipv6_basic() {
    let acl = make_acl(&[("::/0", "deny"), ("fe80::/64", "permit")]);

    assert_eq!(acl.apply(&"fe80::1234".parse().unwrap()), ALLOW);
    // fe80::ffff:1213:dead:beef is in fe80::/64
    assert_eq!(
        acl.apply(&"fe80::ffff:1213:dead:beef".parse().unwrap()),
        ALLOW
    );
}

/// ACL4: deny all IPv6, permit fe80::/64, deny fe80::ffff:0:0:0/80, permit fe80::ffff:0:ffff:0/112.
#[test]
fn test_acl4_ipv6_layered() {
    let acl = make_acl(&[
        ("::/0", "deny"),
        ("fe80::/64", "permit"),
        ("fe80::ffff:0:0:0/80", "deny"),
        ("fe80::ffff:0:ffff:0/112", "permit"),
    ]);

    // fe80::1234 is in fe80::/64 -> permit (not in /80 deny range)
    assert_eq!(acl.apply(&"fe80::1234".parse().unwrap()), ALLOW);
    // fe80::ffff:1213:dead:beef is in fe80::/64 and fe80::ffff:0:0:0/80 -> deny
    assert_eq!(
        acl.apply(&"fe80::ffff:1213:dead:beef".parse().unwrap()),
        DENY
    );
    // fe80::ffff:0:ffff:ABCD is in all three: /64 permit, /80 deny, /112 permit -> permit
    assert_eq!(
        acl.apply(&"fe80::ffff:0:ffff:ABCD".parse().unwrap()),
        ALLOW
    );
}

/// Test that IPv4 addresses are not affected by IPv6-only ACLs.
#[test]
fn test_acl_v4_unaffected_by_v6_rules() {
    let acl = make_acl(&[("::/0", "deny")]);
    // IPv4 address should not match an IPv6 rule -> default ALLOW
    assert_eq!(acl.apply(&"10.1.1.5".parse().unwrap()), ALLOW);
}

/// Test that IPv6 addresses are not affected by IPv4-only ACLs.
#[test]
fn test_acl_v6_unaffected_by_v4_rules() {
    let acl = make_acl(&[("0.0.0.0/0", "deny")]);
    // IPv6 address should not match an IPv4 rule -> default ALLOW
    assert_eq!(acl.apply(&"fe80::1234".parse().unwrap()), ALLOW);
}

/// Test empty ACL permits everything.
#[test]
fn test_acl_empty_permits_all() {
    let acl = Acl::new();
    assert_eq!(acl.apply(&"10.0.0.1".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"fe80::1".parse().unwrap()), ALLOW);
}

/// Test single host match (exact /32).
#[test]
fn test_acl_single_host() {
    let acl = make_acl(&[("0.0.0.0/0", "deny"), ("10.0.0.1/32", "permit")]);
    assert_eq!(acl.apply(&"10.0.0.1".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"10.0.0.2".parse().unwrap()), DENY);
}

/// Test dotted-decimal netmask format (IPv4 only).
#[test]
fn test_acl_dotted_decimal_netmask() {
    let acl = make_acl(&[
        ("0.0.0.0/0", "deny"),
        ("10.0.0.0/255.0.0.0", "permit"),
    ]);
    assert_eq!(acl.apply(&"10.1.2.3".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"192.168.1.1".parse().unwrap()), DENY);
}

/// Test loopback address matching.
#[test]
fn test_acl_loopback() {
    let acl = make_acl(&[("0.0.0.0/0", "deny"), ("127.0.0.0/8", "permit")]);
    assert_eq!(acl.apply(&"127.0.0.1".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"127.255.255.255".parse().unwrap()), ALLOW);
    assert_eq!(acl.apply(&"128.0.0.1".parse().unwrap()), DENY);
}
