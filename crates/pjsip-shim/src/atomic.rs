//! pj_atomic / pj_grp_lock -- atomic operations and group locks.
//!
//! All atomic and group lock functions are now provided by pjlib's real C
//! sources (os_core_unix.c and lock.c) compiled into the library via build.rs.
//! This module provides opaque type declarations and extern "C" bindings.

use crate::types::*;

/// Opaque atomic variable (defined in C).
#[repr(C)]
pub struct pj_atomic_t {
    _opaque: [u8; 0],
}

/// Opaque group lock type (defined in C).
#[repr(C)]
pub struct pj_grp_lock_t {
    _opaque: [u8; 0],
}

/// Opaque group lock config (defined in C).
#[repr(C)]
pub struct pj_grp_lock_config {
    _opaque: [u8; 0],
}

extern "C" {
    pub fn pj_atomic_create(
        pool: *mut pj_pool_t,
        initial: isize,
        p_atomic: *mut *mut pj_atomic_t,
    ) -> pj_status_t;
    pub fn pj_atomic_destroy(atomic: *mut pj_atomic_t) -> pj_status_t;
    pub fn pj_atomic_get(atomic: *mut pj_atomic_t) -> isize;
    pub fn pj_atomic_set(atomic: *mut pj_atomic_t, val: isize);
    pub fn pj_atomic_inc(atomic: *mut pj_atomic_t);
    pub fn pj_atomic_inc_and_get(atomic: *mut pj_atomic_t) -> isize;
    pub fn pj_atomic_dec(atomic: *mut pj_atomic_t);
    pub fn pj_atomic_dec_and_get(atomic: *mut pj_atomic_t) -> isize;
    pub fn pj_atomic_add(atomic: *mut pj_atomic_t, val: isize);
    pub fn pj_atomic_add_and_get(atomic: *mut pj_atomic_t, val: isize) -> isize;

    pub fn pj_grp_lock_create(
        pool: *mut pj_pool_t,
        cfg: *const pj_grp_lock_config,
        p_lock: *mut *mut pj_grp_lock_t,
    ) -> pj_status_t;
    pub fn pj_grp_lock_acquire(lock: *mut pj_grp_lock_t) -> pj_status_t;
    pub fn pj_grp_lock_tryacquire(lock: *mut pj_grp_lock_t) -> pj_status_t;
    pub fn pj_grp_lock_release(lock: *mut pj_grp_lock_t) -> pj_status_t;
    pub fn pj_grp_lock_add_ref(lock: *mut pj_grp_lock_t) -> pj_status_t;
    pub fn pj_grp_lock_dec_ref(lock: *mut pj_grp_lock_t) -> pj_status_t;
    pub fn pj_grp_lock_get_ref(lock: *mut pj_grp_lock_t) -> i32;
}

/// Alias for pj_atomic_get (pjproject has both names in some contexts).
pub unsafe fn pj_atomic_value(atomic: *mut pj_atomic_t) -> isize {
    pj_atomic_get(atomic)
}
