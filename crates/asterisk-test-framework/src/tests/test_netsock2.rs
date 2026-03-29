//! Port of asterisk/tests/test_netsock2.c
//!
//! Tests network socket address operations:
//! - IPv4 address parsing
//! - IPv6 address parsing
//! - Address with port parsing
//! - Invalid address rejection
//! - Address stringification and round-trip
//! - Host:port splitting
//! - Loopback detection
//! - Any-address detection
//! - Address comparison

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

// ---------------------------------------------------------------------------
// Address parsing helper (mirrors ast_sockaddr_parse)
// ---------------------------------------------------------------------------

/// Parse an address string into an IpAddr or SocketAddr.
/// Handles IPv4, IPv6, [IPv6], and port suffixes.
/// Returns None if invalid.
fn parse_addr(s: &str) -> Option<ParsedAddr> {
    let s = s.trim();

    // Try [IPv6]:port
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            let ip_str = &s[1..bracket_end];
            let ip: Ipv6Addr = ip_str.parse().ok()?;
            let rest = &s[bracket_end + 1..];
            if rest.is_empty() {
                return Some(ParsedAddr {
                    ip: IpAddr::V6(ip),
                    port: None,
                });
            }
            if rest.starts_with(':') {
                let port: u16 = rest[1..].parse().ok()?;
                return Some(ParsedAddr {
                    ip: IpAddr::V6(ip),
                    port: Some(port),
                });
            }
            return None; // invalid format after ']'
        }
        return None;
    }

    // Try IPv4 with optional port
    if let Ok(ip4) = s.parse::<Ipv4Addr>() {
        return Some(ParsedAddr {
            ip: IpAddr::V4(ip4),
            port: None,
        });
    }

    // Try IPv4:port
    if let Some(colon) = s.rfind(':') {
        let before = &s[..colon];
        let after = &s[colon + 1..];
        if let Ok(ip4) = before.parse::<Ipv4Addr>() {
            if let Ok(port) = after.parse::<u16>() {
                return Some(ParsedAddr {
                    ip: IpAddr::V4(ip4),
                    port: Some(port),
                });
            }
        }
    }

    // Try IPv6 (no brackets, no port)
    if let Ok(ip6) = s.parse::<Ipv6Addr>() {
        return Some(ParsedAddr {
            ip: IpAddr::V6(ip6),
            port: None,
        });
    }

    // Try IPv4-mapped IPv6
    if s.starts_with("::ffff:") {
        let rest = &s[7..];
        if let Ok(ip4) = rest.parse::<Ipv4Addr>() {
            let ip6 = ip4.to_ipv6_mapped();
            return Some(ParsedAddr {
                ip: IpAddr::V6(ip6),
                port: None,
            });
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedAddr {
    ip: IpAddr,
    port: Option<u16>,
}

// ---------------------------------------------------------------------------
// Host:port splitting (mirrors ast_sockaddr_split_hostport)
// ---------------------------------------------------------------------------

/// Split flags mirroring PARSE_PORT_*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortFlag {
    Default,
    Ignore,
    Require,
    Forbid,
}

/// Split a host:port string.
fn split_hostport(s: &str, flag: PortFlag) -> Option<(String, Option<String>)> {
    let s = s.trim();

    // [IPv6]:port format
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            let host = s[1..bracket_end].to_string();
            let rest = &s[bracket_end + 1..];
            if rest.is_empty() {
                match flag {
                    PortFlag::Require => return None,
                    _ => return Some((host, None)),
                }
            }
            if rest.starts_with(':') {
                let port = rest[1..].to_string();
                match flag {
                    PortFlag::Forbid => return None,
                    PortFlag::Ignore => return Some((host, None)),
                    _ => return Some((host, Some(port))),
                }
            }
            return None;
        }
        return None;
    }

    // Check if it looks like IPv6 (has colons but no brackets)
    let colon_count = s.matches(':').count();
    if colon_count > 1 {
        // IPv6 without brackets -- port not possible
        match flag {
            PortFlag::Require => return None,
            _ => return Some((s.to_string(), None)),
        }
    }

    // IPv4 or hostname with optional :port
    if let Some(colon) = s.rfind(':') {
        let host = s[..colon].to_string();
        let port = s[colon + 1..].to_string();
        match flag {
            PortFlag::Forbid => return None,
            PortFlag::Ignore => return Some((host, None)),
            _ => return Some((host, Some(port))),
        }
    }

    // No port
    match flag {
        PortFlag::Require => None,
        _ => Some((s.to_string(), None)),
    }
}

// ---------------------------------------------------------------------------
// Tests: Address parsing (port of AST_TEST_DEFINE(parsing))
// ---------------------------------------------------------------------------

#[test]
fn test_parse_ipv4_basic() {
    assert!(parse_addr("192.168.1.0").is_some());
    assert!(parse_addr("10.255.255.254").is_some());
    assert!(parse_addr("172.18.5.4").is_some());
    assert!(parse_addr("8.8.4.4").is_some());
    assert!(parse_addr("0.0.0.0").is_some());
    assert!(parse_addr("127.0.0.1").is_some());
}

#[test]
fn test_parse_ipv4_invalid() {
    assert!(parse_addr("1.256.3.4").is_none());
    assert!(parse_addr("256.0.0.1").is_none());
}

#[test]
fn test_parse_ipv4_with_port() {
    let parsed = parse_addr("1.2.3.4:5060").unwrap();
    assert_eq!(parsed.ip, IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
    assert_eq!(parsed.port, Some(5060));
}

#[test]
fn test_parse_ipv6_basic() {
    assert!(parse_addr("fdf8:f53b:82e4::53").is_some());
    assert!(parse_addr("fe80::200:5aee:feaa:20a2").is_some());
    assert!(parse_addr("2001::1").is_some());
    assert!(parse_addr("2001:0000:4136:e378:8000:63bf:3fff:fdd2").is_some());
    assert!(parse_addr("2001:0002:6c::430").is_some());
    assert!(parse_addr("ff01:0:0:0:0:0:0:2").is_some());
}

#[test]
fn test_parse_ipv6_bracketed() {
    assert!(parse_addr("[fdf8:f53b:82e4::53]").is_some());
    assert!(parse_addr("[fe80::200:5aee:feaa:20a2]").is_some());
    assert!(parse_addr("[2001::1]").is_some());
}

#[test]
fn test_parse_ipv6_with_port() {
    let parsed = parse_addr("[2001:0000:4136:e378:8000:63bf:3fff:fdd2]:5060").unwrap();
    assert_eq!(parsed.port, Some(5060));
}

#[test]
fn test_parse_ipv6_invalid() {
    // Port without brackets.
    assert!(parse_addr("2001:0000:4136:e378:8000:63bf:3fff:fdd2:5060").is_none());
    // Multiple zero expansions.
    assert!(parse_addr("fe80::200::abcd").is_none());
}

#[test]
fn test_parse_ipv4_mapped_ipv6() {
    let parsed = parse_addr("::ffff:5.6.7.8").unwrap();
    match parsed.ip {
        IpAddr::V6(v6) => {
            // Should be an IPv4-mapped IPv6 address.
            assert!(v6.to_ipv4_mapped().is_some() || v6.to_ipv4().is_some());
        }
        _ => panic!("Expected IPv6"),
    }
}

// ---------------------------------------------------------------------------
// Tests: Round-trip stringification
// ---------------------------------------------------------------------------

#[test]
fn test_addr_roundtrip_ipv4() {
    let addr: IpAddr = "192.168.1.1".parse().unwrap();
    let s = addr.to_string();
    let reparsed: IpAddr = s.parse().unwrap();
    assert_eq!(addr, reparsed);
}

#[test]
fn test_addr_roundtrip_ipv6() {
    let addr: Ipv6Addr = "2001:db8:8:4::2".parse().unwrap();
    let s = addr.to_string();
    let reparsed: Ipv6Addr = s.parse().unwrap();
    assert_eq!(addr, reparsed);
}

// ---------------------------------------------------------------------------
// Tests: Host:port splitting (port of AST_TEST_DEFINE(split_hostport))
// ---------------------------------------------------------------------------

#[test]
fn test_split_hostport_ipv4_no_port() {
    let (host, port) = split_hostport("192.168.1.1", PortFlag::Default).unwrap();
    assert_eq!(host, "192.168.1.1");
    assert!(port.is_none());
}

#[test]
fn test_split_hostport_ipv4_with_port() {
    let (host, port) = split_hostport("192.168.1.1:5060", PortFlag::Default).unwrap();
    assert_eq!(host, "192.168.1.1");
    assert_eq!(port, Some("5060".to_string()));
}

#[test]
fn test_split_hostport_ipv6_no_port() {
    let (host, port) = split_hostport("::ffff:5.6.7.8", PortFlag::Default).unwrap();
    assert_eq!(host, "::ffff:5.6.7.8");
    assert!(port.is_none());
}

#[test]
fn test_split_hostport_ipv6_bracketed_with_port() {
    let (host, port) =
        split_hostport("[::ffff:5.6.7.8]:5060", PortFlag::Default).unwrap();
    assert_eq!(host, "::ffff:5.6.7.8");
    assert_eq!(port, Some("5060".to_string()));
}

#[test]
fn test_split_hostport_ipv6_bracketed_no_port() {
    let (host, port) =
        split_hostport("[fdf8:f53b:82e4::53]", PortFlag::Default).unwrap();
    assert_eq!(host, "fdf8:f53b:82e4::53");
    assert!(port.is_none());
}

#[test]
fn test_split_hostport_hostname() {
    let (host, port) = split_hostport("host:port", PortFlag::Default).unwrap();
    assert_eq!(host, "host");
    assert_eq!(port, Some("port".to_string()));
}

#[test]
fn test_split_hostport_hostname_no_port() {
    let (host, port) = split_hostport("host", PortFlag::Default).unwrap();
    assert_eq!(host, "host");
    assert!(port.is_none());
}

// Port flag tests.

#[test]
fn test_split_hostport_ignore_port() {
    let (host, port) =
        split_hostport("192.168.1.1:5060", PortFlag::Ignore).unwrap();
    assert_eq!(host, "192.168.1.1");
    assert!(port.is_none()); // port should be ignored
}

#[test]
fn test_split_hostport_require_port_present() {
    let (host, port) =
        split_hostport("192.168.1.1:5060", PortFlag::Require).unwrap();
    assert_eq!(host, "192.168.1.1");
    assert_eq!(port, Some("5060".to_string()));
}

#[test]
fn test_split_hostport_require_port_missing() {
    assert!(split_hostport("192.168.1.1", PortFlag::Require).is_none());
}

#[test]
fn test_split_hostport_forbid_port_absent() {
    let (host, port) =
        split_hostport("192.168.1.1", PortFlag::Forbid).unwrap();
    assert_eq!(host, "192.168.1.1");
    assert!(port.is_none());
}

#[test]
fn test_split_hostport_forbid_port_present() {
    assert!(split_hostport("192.168.1.1:5060", PortFlag::Forbid).is_none());
}

#[test]
fn test_split_hostport_ipv6_require_missing() {
    assert!(split_hostport("::ffff:5.6.7.8", PortFlag::Require).is_none());
}

#[test]
fn test_split_hostport_ipv6_forbid_present() {
    assert!(
        split_hostport("[::ffff:5.6.7.8]:5060", PortFlag::Forbid).is_none()
    );
}

// ---------------------------------------------------------------------------
// Tests: Loopback detection
// ---------------------------------------------------------------------------

#[test]
fn test_is_loopback_ipv4() {
    let addr: IpAddr = "127.0.0.1".parse().unwrap();
    assert!(addr.is_loopback());
}

#[test]
fn test_is_loopback_ipv6() {
    let addr: IpAddr = "::1".parse().unwrap();
    assert!(addr.is_loopback());
}

#[test]
fn test_is_not_loopback() {
    let addr: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(!addr.is_loopback());
}

// ---------------------------------------------------------------------------
// Tests: Any-address detection
// ---------------------------------------------------------------------------

#[test]
fn test_is_any_ipv4() {
    let addr = Ipv4Addr::UNSPECIFIED;
    assert!(addr.is_unspecified());
}

#[test]
fn test_is_any_ipv6() {
    let addr = Ipv6Addr::UNSPECIFIED;
    assert!(addr.is_unspecified());
}

#[test]
fn test_is_not_any() {
    let addr: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(!addr.is_unspecified());
}

// ---------------------------------------------------------------------------
// Tests: Address comparison
// ---------------------------------------------------------------------------

#[test]
fn test_addr_equal() {
    let a: IpAddr = "192.168.1.1".parse().unwrap();
    let b: IpAddr = "192.168.1.1".parse().unwrap();
    assert_eq!(a, b);
}

#[test]
fn test_addr_not_equal() {
    let a: IpAddr = "192.168.1.1".parse().unwrap();
    let b: IpAddr = "192.168.1.2".parse().unwrap();
    assert_ne!(a, b);
}

#[test]
fn test_socket_addr_with_port() {
    let a: SocketAddr = "192.168.1.1:5060".parse().unwrap();
    let b: SocketAddr = "192.168.1.1:5061".parse().unwrap();
    // Same IP but different ports.
    assert_eq!(a.ip(), b.ip());
    assert_ne!(a.port(), b.port());
}
