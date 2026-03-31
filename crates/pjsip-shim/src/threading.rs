//! pj_thread / pj_mutex / pj_sem / pj_rwmutex / pj_lock -- threading primitives.
//!
//! All threading, mutex, semaphore, rwmutex, and lock functions are now
//! provided by pjlib's real C sources (os_core_unix.c and lock.c) compiled
//! into the library via build.rs. This module provides opaque type
//! declarations and extern "C" bindings.

use crate::types::*;

/// Opaque mutex (defined in C).
#[repr(C)]
pub struct pj_mutex_t {
    _opaque: [u8; 0],
}

/// Opaque semaphore (defined in C).
#[repr(C)]
pub struct pj_sem_t {
    _opaque: [u8; 0],
}

extern "C" {
    pub fn pj_mutex_create_simple(
        pool: *mut pj_pool_t,
        name: *const libc::c_char,
        p_mutex: *mut *mut pj_mutex_t,
    ) -> pj_status_t;
    pub fn pj_mutex_lock(mutex: *mut pj_mutex_t) -> pj_status_t;
    pub fn pj_mutex_unlock(mutex: *mut pj_mutex_t) -> pj_status_t;
    pub fn pj_mutex_destroy(mutex: *mut pj_mutex_t) -> pj_status_t;

    pub fn pj_sem_create(
        pool: *mut pj_pool_t,
        name: *const libc::c_char,
        initial: u32,
        max: u32,
        p_sem: *mut *mut pj_sem_t,
    ) -> pj_status_t;
    pub fn pj_sem_wait(sem: *mut pj_sem_t) -> pj_status_t;
    pub fn pj_sem_trywait(sem: *mut pj_sem_t) -> pj_status_t;
    pub fn pj_sem_post(sem: *mut pj_sem_t) -> pj_status_t;
    pub fn pj_sem_destroy(sem: *mut pj_sem_t) -> pj_status_t;
}


