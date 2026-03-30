//! SIP message manipulation functions -- header list operations.

use crate::pool::pj_pool_alloc;
use crate::types::*;

// ---------------------------------------------------------------------------
// pjsip_msg_find_hdr
// ---------------------------------------------------------------------------

/// Walk the header linked list and find the first header matching `htype`.
/// If `start` is non-null, search starts from the element *after* `start`.
/// Returns null if not found.
#[no_mangle]
pub unsafe extern "C" fn pjsip_msg_find_hdr(
    msg: *const pjsip_msg,
    htype: i32,
    start: *const pjsip_hdr,
) -> *mut pjsip_hdr {
    if msg.is_null() {
        return std::ptr::null_mut();
    }

    let sentinel = &(*msg).hdr as *const pjsip_hdr;

    // Determine where to begin searching
    let mut cur = if start.is_null() {
        (*sentinel).next
    } else {
        // Start from the element after `start`
        (*start).next
    };

    while cur != sentinel as *mut pjsip_hdr {
        if (*cur).htype == htype {
            return cur;
        }
        cur = (*cur).next;
    }

    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// pjsip_msg_find_hdr_by_name
// ---------------------------------------------------------------------------

/// Find a header by name (case-insensitive).
#[no_mangle]
pub unsafe extern "C" fn pjsip_msg_find_hdr_by_name(
    msg: *const pjsip_msg,
    name: *const pj_str_t,
    start: *const pjsip_hdr,
) -> *mut pjsip_hdr {
    if msg.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }

    let sentinel = &(*msg).hdr as *const pjsip_hdr;
    let search_name = (*name).as_str();

    let mut cur = if start.is_null() {
        (*sentinel).next
    } else {
        (*start).next
    };

    while cur != sentinel as *mut pjsip_hdr {
        let cur_name = (*cur).name.as_str();
        if cur_name.eq_ignore_ascii_case(search_name) {
            return cur;
        }
        cur = (*cur).next;
    }

    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// pjsip_msg_add_hdr
// ---------------------------------------------------------------------------

/// Insert a header at the end of the message's header list (before sentinel).
#[no_mangle]
pub unsafe extern "C" fn pjsip_msg_add_hdr(
    msg: *mut pjsip_msg,
    hdr: *mut pjsip_hdr,
) {
    if msg.is_null() || hdr.is_null() {
        return;
    }

    let sentinel = &mut (*msg).hdr as *mut pjsip_hdr;
    let prev = (*sentinel).prev;

    (*hdr).prev = prev;
    (*hdr).next = sentinel;
    (*prev).next = hdr;
    (*sentinel).prev = hdr;
}

// ---------------------------------------------------------------------------
// pjsip_msg_create
// ---------------------------------------------------------------------------

/// Create a new empty SIP message in pool memory.
#[no_mangle]
pub unsafe extern "C" fn pjsip_msg_create(
    pool: *mut pj_pool_t,
    msg_type: i32,
) -> *mut pjsip_msg {
    let msg = pj_pool_alloc(pool, std::mem::size_of::<pjsip_msg>()) as *mut pjsip_msg;
    if msg.is_null() {
        return std::ptr::null_mut();
    }

    (*msg).msg_type = msg_type;
    (*msg).body = std::ptr::null_mut();

    // Init the sentinel as a circular linked list
    (*msg).hdr.prev = &mut (*msg).hdr;
    (*msg).hdr.next = &mut (*msg).hdr;
    (*msg).hdr.htype = 0;
    (*msg).hdr.name = pj_str_t::EMPTY;
    (*msg).hdr.sname = pj_str_t::EMPTY;

    // Zero out the line union
    if msg_type == PJSIP_RESPONSE_MSG {
        (*msg).line.status = std::mem::ManuallyDrop::new(pjsip_status_line {
            code: 0,
            reason: pj_str_t::EMPTY,
        });
    }

    msg
}

// ---------------------------------------------------------------------------
// pjsip_hdr_clone
// ---------------------------------------------------------------------------

/// Clone a generic string header into pool memory.
#[no_mangle]
pub unsafe extern "C" fn pjsip_hdr_clone(
    pool: *mut pj_pool_t,
    hdr: *const pjsip_hdr,
) -> *mut pjsip_hdr {
    if pool.is_null() || hdr.is_null() {
        return std::ptr::null_mut();
    }

    // We treat every header as a generic_string_hdr for simplicity
    let src = hdr as *const pjsip_generic_string_hdr;
    let dst = pj_pool_alloc(pool, std::mem::size_of::<pjsip_generic_string_hdr>())
        as *mut pjsip_generic_string_hdr;
    if dst.is_null() {
        return std::ptr::null_mut();
    }

    (*dst).htype = (*src).htype;

    // Duplicate name
    crate::string::pj_strdup(pool, &mut (*dst).name, &(*src).name);
    crate::string::pj_strdup(pool, &mut (*dst).sname, &(*src).sname);
    crate::string::pj_strdup(pool, &mut (*dst).hvalue, &(*src).hvalue);

    // Init linked list pointers to self
    let base = dst as *mut pjsip_hdr;
    (*base).prev = base;
    (*base).next = base;

    base
}
