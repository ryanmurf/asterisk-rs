//! SIP URI / message parsing -- delegates to the Rust `asterisk_sip::parser`.

use crate::pool::pj_pool_alloc;
use crate::types::*;

// ---------------------------------------------------------------------------
// Internal helper: populate a C-layout pjsip_sip_uri from our Rust SipUri
// ---------------------------------------------------------------------------

unsafe fn str_to_pool(pool: *mut pj_pool_t, s: &str) -> pj_str_t {
    if s.is_empty() {
        return pj_str_t::EMPTY;
    }
    let len = s.len();
    let buf = pj_pool_alloc(pool, len + 1) as *mut libc::c_char;
    if buf.is_null() {
        return pj_str_t::EMPTY;
    }
    std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, len);
    *buf.add(len) = 0;
    pj_str_t {
        ptr: buf,
        slen: len as isize,
    }
}

unsafe fn populate_c_uri(
    pool: *mut pj_pool_t,
    c_uri: *mut pjsip_sip_uri,
    uri: &asterisk_sip::SipUri,
) {
    (*c_uri).vptr = std::ptr::null();
    (*c_uri).scheme = str_to_pool(pool, &uri.scheme);
    (*c_uri).user = match &uri.user {
        Some(u) => str_to_pool(pool, u),
        None => pj_str_t::EMPTY,
    };
    (*c_uri).passwd = match &uri.password {
        Some(p) => str_to_pool(pool, p),
        None => pj_str_t::EMPTY,
    };
    (*c_uri).host = str_to_pool(pool, &uri.host);
    (*c_uri).port = uri.port.unwrap_or(0) as i32;

    // Transport parameter
    (*c_uri).transport_param = match uri.transport() {
        Some(t) => str_to_pool(pool, t),
        None => pj_str_t::EMPTY,
    };

    // User parameter
    (*c_uri).user_param = match uri.get_param("user") {
        Some(u) => str_to_pool(pool, u),
        None => pj_str_t::EMPTY,
    };

    // Method parameter
    (*c_uri).method_param = match uri.get_param("method") {
        Some(m) => str_to_pool(pool, m),
        None => pj_str_t::EMPTY,
    };

    // TTL parameter
    (*c_uri).ttl_param = match uri.get_param("ttl") {
        Some(t) => t.parse().unwrap_or(0),
        None => 0,
    };

    // lr parameter (boolean -- present or not)
    (*c_uri).lr_param = if uri.parameters.contains_key("lr") {
        1
    } else {
        0
    };

    // maddr parameter
    (*c_uri).maddr_param = match uri.get_param("maddr") {
        Some(m) => str_to_pool(pool, m),
        None => pj_str_t::EMPTY,
    };
}

// ---------------------------------------------------------------------------
// pjsip_parse_uri
// ---------------------------------------------------------------------------

/// Parse a SIP URI from a buffer.  Returns a pool-allocated `pjsip_sip_uri`
/// cast to `*mut pjsip_uri`, or null on failure.
#[no_mangle]
pub unsafe extern "C" fn pjsip_parse_uri(
    pool: *mut pj_pool_t,
    buf: *mut libc::c_char,
    size: usize,
    _options: u32,
) -> *mut pjsip_uri {
    if pool.is_null() || buf.is_null() || size == 0 {
        return std::ptr::null_mut();
    }

    // Convert C buffer to Rust &str
    let input = std::str::from_utf8_unchecked(std::slice::from_raw_parts(
        buf as *const u8,
        size,
    ));

    // Strip angle brackets if present (e.g. "<sip:alice@example.com>")
    let trimmed = input.trim();
    let uri_str = if trimmed.starts_with('<') && trimmed.ends_with('>') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    // Parse with our Rust parser
    match asterisk_sip::SipUri::parse(uri_str) {
        Ok(uri) => {
            let c_uri = pj_pool_alloc(pool, std::mem::size_of::<pjsip_sip_uri>())
                as *mut pjsip_sip_uri;
            if c_uri.is_null() {
                return std::ptr::null_mut();
            }
            populate_c_uri(pool, c_uri, &uri);
            c_uri as *mut pjsip_uri
        }
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// pjsip_parse_msg
// ---------------------------------------------------------------------------

/// Parse a complete SIP message from a buffer.
///
/// This is a basic implementation that creates the C-layout message structure.
/// Full header-level parsing is a TODO -- for now we populate the start line
/// and create an empty header list.
#[no_mangle]
pub unsafe extern "C" fn pjsip_parse_msg(
    pool: *mut pj_pool_t,
    buf: *mut libc::c_char,
    size: usize,
    _options: u32,
) -> *mut pjsip_msg {
    if pool.is_null() || buf.is_null() || size == 0 {
        return std::ptr::null_mut();
    }

    let data = std::slice::from_raw_parts(buf as *const u8, size);
    let parsed = match asterisk_sip::SipMessage::parse(data) {
        Ok(msg) => msg,
        Err(_) => return std::ptr::null_mut(),
    };

    // Allocate the message
    let msg = pj_pool_alloc(pool, std::mem::size_of::<pjsip_msg>()) as *mut pjsip_msg;
    if msg.is_null() {
        return std::ptr::null_mut();
    }

    // Init sentinel header (circular linked list)
    (*msg).hdr.prev = &mut (*msg).hdr;
    (*msg).hdr.next = &mut (*msg).hdr;
    (*msg).hdr.htype = 0;
    (*msg).hdr.name = pj_str_t::EMPTY;
    (*msg).hdr.sname = pj_str_t::EMPTY;
    (*msg).body = std::ptr::null_mut();

    match &parsed.start_line {
        asterisk_sip::StartLine::Request(req) => {
            (*msg).msg_type = PJSIP_REQUEST_MSG;
            let method_id = match req.method {
                asterisk_sip::SipMethod::Invite => PJSIP_INVITE_METHOD,
                asterisk_sip::SipMethod::Cancel => PJSIP_CANCEL_METHOD,
                asterisk_sip::SipMethod::Ack => PJSIP_ACK_METHOD,
                asterisk_sip::SipMethod::Bye => PJSIP_BYE_METHOD,
                asterisk_sip::SipMethod::Register => PJSIP_REGISTER_METHOD,
                asterisk_sip::SipMethod::Options => PJSIP_OPTIONS_METHOD,
                _ => PJSIP_OTHER_METHOD,
            };
            let method_name = str_to_pool(pool, req.method.as_str());

            // Parse the request URI
            let uri_str = req.uri.to_string();
            let uri_c = pjsip_parse_uri(
                pool,
                str_to_pool(pool, &uri_str).ptr,
                uri_str.len(),
                0,
            );

            (*msg).line.req = std::mem::ManuallyDrop::new(pjsip_request_line {
                method: pjsip_method {
                    id: method_id,
                    name: method_name,
                },
                uri: uri_c,
            });
        }
        asterisk_sip::StartLine::Response(resp) => {
            (*msg).msg_type = PJSIP_RESPONSE_MSG;
            (*msg).line.status = std::mem::ManuallyDrop::new(pjsip_status_line {
                code: resp.status_code as i32,
                reason: str_to_pool(pool, &resp.reason_phrase),
            });
        }
    }

    // Populate headers
    for hdr in &parsed.headers {
        let h = pj_pool_alloc(pool, std::mem::size_of::<pjsip_generic_string_hdr>())
            as *mut pjsip_generic_string_hdr;
        if h.is_null() {
            continue;
        }
        (*h).htype = header_name_to_type(&hdr.name);
        (*h).name = str_to_pool(pool, &hdr.name);
        (*h).sname = pj_str_t::EMPTY;
        (*h).hvalue = str_to_pool(pool, &hdr.value);

        // Insert before the sentinel (at the tail of the list)
        let base = h as *mut pjsip_hdr;
        let sentinel = &mut (*msg).hdr as *mut pjsip_hdr;
        let prev = (*sentinel).prev;
        (*base).prev = prev;
        (*base).next = sentinel;
        (*prev).next = base;
        (*sentinel).prev = base;
    }

    msg
}

/// Map common header names to pjsip_hdr_e type constants.
fn header_name_to_type(name: &str) -> i32 {
    match name.to_ascii_lowercase().as_str() {
        "via" | "v" => PJSIP_H_VIA,
        "from" | "f" => PJSIP_H_FROM,
        "to" | "t" => PJSIP_H_TO,
        "call-id" | "i" => PJSIP_H_CALL_ID,
        "cseq" => PJSIP_H_CSEQ,
        "contact" | "m" => PJSIP_H_CONTACT,
        "content-type" | "c" => PJSIP_H_CONTENT_TYPE,
        "content-length" | "l" => PJSIP_H_CONTENT_LENGTH,
        "route" => PJSIP_H_ROUTE,
        "record-route" => PJSIP_H_RECORD_ROUTE,
        "max-forwards" => PJSIP_H_MAX_FORWARDS,
        "expires" => PJSIP_H_EXPIRES,
        "require" => PJSIP_H_REQUIRE,
        "supported" | "k" => PJSIP_H_SUPPORTED,
        _ => PJSIP_H_OTHER,
    }
}
