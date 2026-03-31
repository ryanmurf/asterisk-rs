//! Initialization and shutdown -- pjsip_endpt_create and friends.
//!
//! pj_init and pj_shutdown are now provided by pjlib's real C source
//! (os_core_unix.c) compiled into the library via build.rs.

use crate::types::*;

extern "C" {
    pub fn pj_init() -> pj_status_t;
    pub fn pj_shutdown() -> pj_status_t;
}

// ---------------------------------------------------------------------------
// pjlib-util init
// ---------------------------------------------------------------------------

/// Initialize pjlib-util.  No-op for our implementation.
#[no_mangle]
pub unsafe extern "C" fn pjlib_util_init() -> pj_status_t {
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// pjsip_endpt_create
// ---------------------------------------------------------------------------

/// Create a SIP endpoint.  The endpoint handle is an opaque pointer;
/// callers just pass it around.  We allocate a small sentinel so the
/// pointer is non-null.
#[no_mangle]
pub unsafe extern "C" fn pjsip_endpt_create(
    _pf: *mut libc::c_void,
    _name: *const libc::c_char,
    endpt: *mut *mut pjsip_endpoint,
) -> pj_status_t {
    if endpt.is_null() {
        return PJ_EINVAL;
    }

    // Allocate a small block so the handle is non-null
    let ptr = Box::into_raw(Box::new(0u64)) as *mut pjsip_endpoint;
    *endpt = ptr;
    PJ_SUCCESS
}

/// Destroy a SIP endpoint.
#[no_mangle]
pub unsafe extern "C" fn pjsip_endpt_destroy(endpt: *mut pjsip_endpoint) {
    if !endpt.is_null() {
        let _ = Box::from_raw(endpt as *mut u64);
    }
}

/// Get the pool factory from an endpoint.  Returns a non-null sentinel
/// so callers that pass it to pj_pool_create don't fail null checks.
#[no_mangle]
pub unsafe extern "C" fn pjsip_endpt_get_pool_factory(
    _endpt: *mut pjsip_endpoint,
) -> *mut libc::c_void {
    // Return a non-null sentinel.  Our pj_pool_create ignores the factory
    // parameter anyway, so any non-null value works.
    static mut FACTORY_SENTINEL: u64 = 0;
    std::ptr::addr_of_mut!(FACTORY_SENTINEL) as *mut libc::c_void
}

/// Create a pool from the endpoint's factory.
#[no_mangle]
pub unsafe extern "C" fn pjsip_endpt_create_pool(
    _endpt: *mut pjsip_endpoint,
    name: *const libc::c_char,
    initial: usize,
    increment: usize,
) -> *mut pj_pool_t {
    crate::pool::pj_pool_create(std::ptr::null_mut(), name, initial, increment, std::ptr::null_mut())
}

/// Release a pool obtained from the endpoint.
#[no_mangle]
pub unsafe extern "C" fn pjsip_endpt_release_pool(
    _endpt: *mut pjsip_endpoint,
    pool: *mut pj_pool_t,
) {
    crate::pool::pj_pool_release(pool);
}

// Logging functions have been moved to logging.rs
