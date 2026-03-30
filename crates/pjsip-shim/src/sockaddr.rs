//! Socket address functions -- pj_sockaddr_parse / pj_sockaddr_print.
//!
//! These let C callers convert between text representations and binary
//! socket addresses, matching pjlib's API.

use crate::types::*;

// ---------------------------------------------------------------------------
// pj_sockaddr_parse
// ---------------------------------------------------------------------------

/// Parse a text address (IPv4 or IPv6) into a pj_sockaddr.
///
/// `af` is the address family (PJ_AF_INET, PJ_AF_INET6, or 0 for auto-detect).
/// `options` is currently unused.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_parse(
    af: i32,
    _options: u32,
    addr_str: *const pj_str_t,
    sockaddr: *mut pj_sockaddr,
) -> pj_status_t {
    if addr_str.is_null() || sockaddr.is_null() {
        return PJ_EINVAL;
    }

    let text = (*addr_str).as_str().trim();
    if text.is_empty() {
        return PJ_EINVAL;
    }

    // Split host:port
    let (host, port) = parse_host_port_text(text);

    let af = af as u16;

    // Try IPv4
    if af == 0 || af == PJ_AF_INET as u16 {
        // Empty host => 0.0.0.0
        let ipv4 = if host.is_empty() {
            Some(0u32)
        } else {
            parse_ipv4(host)
        };
        if let Some(ipv4) = ipv4 {
            std::ptr::write_bytes(sockaddr as *mut u8, 0, std::mem::size_of::<pj_sockaddr_in>());
            (*sockaddr).addr.sin_family = PJ_AF_INET;
            (*sockaddr).addr.sin_port = (port as u16).to_be();
            (*sockaddr).addr.sin_addr.s_addr = ipv4;
            return PJ_SUCCESS;
        }
    }

    // Try IPv6
    if af == 0 || af == PJ_AF_INET6 as u16 {
        if let Some(ipv6_bytes) = parse_ipv6(host) {
            std::ptr::write_bytes(sockaddr as *mut u8, 0, std::mem::size_of::<pj_sockaddr_in6>());
            (*sockaddr).ipv6.sin6_family = PJ_AF_INET6;
            (*sockaddr).ipv6.sin6_port = (port as u16).to_be();
            (*sockaddr).ipv6.sin6_addr.s6_addr = ipv6_bytes;
            return PJ_SUCCESS;
        }
    }

    // Try hostname resolution (localhost => 127.0.0.1)
    if af == 0 || af == PJ_AF_INET as u16 {
        if host.eq_ignore_ascii_case("localhost") {
            std::ptr::write_bytes(sockaddr as *mut u8, 0, std::mem::size_of::<pj_sockaddr_in>());
            (*sockaddr).addr.sin_family = PJ_AF_INET;
            (*sockaddr).addr.sin_port = (port as u16).to_be();
            (*sockaddr).addr.sin_addr.s_addr = 0x0100007fu32; // 127.0.0.1
            return PJ_SUCCESS;
        }
    }

    PJ_EINVAL
}

// ---------------------------------------------------------------------------
// pj_sockaddr_print
// ---------------------------------------------------------------------------

/// Print a socket address to a text buffer.
///
/// `with_port` controls whether `:port` is appended.
/// Returns the buffer pointer on success, null on failure.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_print(
    addr: *const pj_sockaddr,
    buf: *mut libc::c_char,
    size: i32,
    with_port: u32,
) -> *mut libc::c_char {
    if addr.is_null() || buf.is_null() || size <= 0 {
        return std::ptr::null_mut();
    }

    let family = (*addr).addr.sin_family;

    let output = if family == PJ_AF_INET {
        let ip = u32::from_be((*addr).addr.sin_addr.s_addr);
        let a = (ip >> 24) & 0xFF;
        let b = (ip >> 16) & 0xFF;
        let c = (ip >> 8) & 0xFF;
        let d = ip & 0xFF;
        let host = format!("{}.{}.{}.{}", a, b, c, d);
        if with_port != 0 {
            let port = u16::from_be((*addr).addr.sin_port);
            format!("{}:{}", host, port)
        } else {
            host
        }
    } else if family == PJ_AF_INET6 {
        let bytes = (*addr).ipv6.sin6_addr.s6_addr;
        // Simple IPv6 formatting (full form)
        let mut parts = Vec::new();
        for i in 0..8 {
            let word = ((bytes[i * 2] as u16) << 8) | (bytes[i * 2 + 1] as u16);
            parts.push(format!("{:x}", word));
        }
        let ip_str = parts.join(":");
        if with_port != 0 {
            let port = u16::from_be((*addr).ipv6.sin6_port);
            format!("[{}]:{}", ip_str, port)
        } else {
            ip_str
        }
    } else {
        return std::ptr::null_mut();
    };

    let out_bytes = output.as_bytes();
    let copy_len = out_bytes.len().min((size as usize) - 1);
    std::ptr::copy_nonoverlapping(out_bytes.as_ptr(), buf as *mut u8, copy_len);
    *buf.add(copy_len) = 0;

    buf
}

// ---------------------------------------------------------------------------
// pj_sockaddr_get_port / pj_sockaddr_set_port
// ---------------------------------------------------------------------------

/// Get the port from a socket address (host byte order).
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_port(addr: *const pj_sockaddr) -> u16 {
    if addr.is_null() {
        return 0;
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        u16::from_be((*addr).addr.sin_port)
    } else if family == PJ_AF_INET6 {
        u16::from_be((*addr).ipv6.sin6_port)
    } else {
        0
    }
}

/// Set the port on a socket address (host byte order).
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_set_port(addr: *mut pj_sockaddr, port: u16) {
    if addr.is_null() {
        return;
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        (*addr).addr.sin_port = port.to_be();
    } else if family == PJ_AF_INET6 {
        (*addr).ipv6.sin6_port = port.to_be();
    }
}

// ---------------------------------------------------------------------------
// pj_sockaddr_init
// ---------------------------------------------------------------------------

/// Initialize a socket address with an address family.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_init(
    af: i32,
    addr: *mut pj_sockaddr,
    host: *const pj_str_t,
    port: u16,
) -> pj_status_t {
    if addr.is_null() {
        return PJ_EINVAL;
    }

    std::ptr::write_bytes(addr as *mut u8, 0, std::mem::size_of::<pj_sockaddr>());

    let af = af as u16;
    if af == PJ_AF_INET as u16 {
        (*addr).addr.sin_family = PJ_AF_INET;
        (*addr).addr.sin_port = port.to_be();
        if !host.is_null() {
            let text = (*host).as_str();
            if let Some(ipv4) = parse_ipv4(text) {
                (*addr).addr.sin_addr.s_addr = ipv4;
            }
        }
    } else if af == PJ_AF_INET6 as u16 {
        (*addr).ipv6.sin6_family = PJ_AF_INET6;
        (*addr).ipv6.sin6_port = port.to_be();
        if !host.is_null() {
            let text = (*host).as_str();
            if let Some(bytes) = parse_ipv6(text) {
                (*addr).ipv6.sin6_addr.s6_addr = bytes;
            }
        }
    } else {
        return PJ_EINVAL;
    }

    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn parse_host_port_text(text: &str) -> (&str, u16) {
    // [ipv6]:port
    if text.starts_with('[') {
        if let Some(bracket_end) = text.find(']') {
            let host = &text[1..bracket_end];
            let rest = &text[bracket_end + 1..];
            let port = rest
                .strip_prefix(':')
                .and_then(|p| if p.is_empty() { Some(0) } else { p.parse().ok() })
                .unwrap_or(0u16);
            return (host, port);
        }
    }
    // ipv4:port or hostname:port
    if let Some(colon) = text.rfind(':') {
        // Make sure it's not an IPv6 address (multiple colons)
        if text[..colon].contains(':') {
            // IPv6 without brackets -- check for trailing colon (e.g. ":::")
            let stripped = text.trim_end_matches(':');
            if stripped.is_empty() {
                return ("::", 0);
            }
            // Check if last colon is a port separator (e.g. ":::80")
            if let Some(last_colon) = stripped.rfind(':') {
                let after = &text[last_colon + 1..].trim_end_matches(':');
                if !after.is_empty() {
                    if let Ok(port) = after.parse::<u16>() {
                        return (&text[..last_colon], port);
                    }
                }
            }
            return (text, 0);
        }
        let port_str = &text[colon + 1..];
        let port = if port_str.is_empty() {
            0
        } else if let Ok(p) = port_str.parse::<u16>() {
            p
        } else {
            // Port is not a valid number -- return full text as host
            return (text, 0);
        };
        return (&text[..colon], port);
    }
    (text, 0)
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let a = parts[0].parse::<u8>().ok()? as u32;
    let b = parts[1].parse::<u8>().ok()? as u32;
    let c = parts[2].parse::<u8>().ok()? as u32;
    let d = parts[3].parse::<u8>().ok()? as u32;
    Some(((a << 24) | (b << 16) | (c << 8) | d).to_be())
}

fn parse_ipv6(s: &str) -> Option<[u8; 16]> {
    // Handle :: expansion
    let s = s.trim_matches(|c| c == '[' || c == ']');

    // Simple zero address
    if s == "::" {
        return Some([0u8; 16]);
    }

    let mut result = [0u8; 16];
    let parts: Vec<&str> = if s.contains("::") {
        // Split on :: and expand
        let halves: Vec<&str> = s.splitn(2, "::").collect();
        let left: Vec<&str> = if halves[0].is_empty() {
            vec![]
        } else {
            halves[0].split(':').collect()
        };
        let right: Vec<&str> = if halves.len() > 1 && !halves[1].is_empty() {
            halves[1].split(':').collect()
        } else {
            vec![]
        };
        let zeroes_needed = 8 - left.len() - right.len();
        let mut expanded = left;
        for _ in 0..zeroes_needed {
            expanded.push("0");
        }
        expanded.extend(right);
        expanded
    } else {
        s.split(':').collect()
    };

    if parts.len() != 8 {
        return None;
    }

    for (i, part) in parts.iter().enumerate() {
        let word = u16::from_str_radix(part, 16).ok()?;
        result[i * 2] = (word >> 8) as u8;
        result[i * 2 + 1] = (word & 0xff) as u8;
    }

    Some(result)
}
