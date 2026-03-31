//! pj_atomic / pj_grp_lock -- atomic operations and group locks.
//!
//! Wraps std::sync::atomic to provide the C-callable atomic API.

use crate::types::*;
use crate::threading::PthreadMutex;
use std::sync::atomic::{AtomicIsize, Ordering};

/// Opaque atomic variable.
#[repr(C)]
pub struct pj_atomic_t {
    _opaque: [u8; 0],
}

struct AtomicInner {
    value: AtomicIsize,
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_create(
    _pool: *mut pj_pool_t,
    initial_value: isize,
    p_atomic: *mut *mut pj_atomic_t,
) -> pj_status_t {
    if p_atomic.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(AtomicInner {
        value: AtomicIsize::new(initial_value),
    });
    *p_atomic = Box::into_raw(inner) as *mut pj_atomic_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_destroy(atomic: *mut pj_atomic_t) -> pj_status_t {
    if !atomic.is_null() {
        let _ = Box::from_raw(atomic as *mut AtomicInner);
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_get(atomic: *mut pj_atomic_t) -> isize {
    if atomic.is_null() {
        return 0;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.load(Ordering::SeqCst)
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_set(atomic: *mut pj_atomic_t, val: isize) {
    if atomic.is_null() {
        return;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.store(val, Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_inc(atomic: *mut pj_atomic_t) {
    if atomic.is_null() {
        return;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_add(1, Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_inc_and_get(atomic: *mut pj_atomic_t) -> isize {
    if atomic.is_null() {
        return 0;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_add(1, Ordering::SeqCst) + 1
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_dec(atomic: *mut pj_atomic_t) {
    if atomic.is_null() {
        return;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_sub(1, Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_dec_and_get(atomic: *mut pj_atomic_t) -> isize {
    if atomic.is_null() {
        return 0;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_sub(1, Ordering::SeqCst) - 1
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_add(atomic: *mut pj_atomic_t, val: isize) {
    if atomic.is_null() {
        return;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_add(val, Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn pj_atomic_add_and_get(atomic: *mut pj_atomic_t, val: isize) -> isize {
    if atomic.is_null() {
        return 0;
    }
    let inner = &*(atomic as *const AtomicInner);
    inner.value.fetch_add(val, Ordering::SeqCst) + val
}

/// Alias for pj_atomic_get (pjproject has both names).
#[no_mangle]
pub unsafe extern "C" fn pj_atomic_value(atomic: *mut pj_atomic_t) -> isize {
    pj_atomic_get(atomic)
}

// ============================================================================
// Group lock
// ============================================================================

/// Opaque group lock.
#[repr(C)]
pub struct pj_grp_lock_t {
    _opaque: [u8; 0],
}

struct GrpLockInner {
    /// Must be first field -- pj_lock_acquire reads it to dispatch.
    tag: u32,
    lock: PthreadMutex,
    ref_count: AtomicIsize,
    destroy_handlers: Vec<(unsafe extern "C" fn(*mut libc::c_void), *mut libc::c_void)>,
}

// The raw pointer in destroy_handlers is managed by the caller.
unsafe impl Send for GrpLockInner {}
unsafe impl Sync for GrpLockInner {}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_create(
    _pool: *mut pj_pool_t,
    _cfg: *const libc::c_void,
    p_lock: *mut *mut pj_grp_lock_t,
) -> pj_status_t {
    if p_lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(GrpLockInner {
        tag: crate::threading::LOCK_TAG_GRP,
        lock: PthreadMutex::new(),
        ref_count: AtomicIsize::new(0),
        destroy_handlers: Vec::new(),
    });
    *p_lock = Box::into_raw(inner) as *mut pj_grp_lock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_create_w_handler(
    pool: *mut pj_pool_t,
    cfg: *const libc::c_void,
    member: *mut libc::c_void,
    handler: Option<unsafe extern "C" fn(*mut libc::c_void)>,
    p_lock: *mut *mut pj_grp_lock_t,
) -> pj_status_t {
    let status = pj_grp_lock_create(pool, cfg, p_lock);
    if status == PJ_SUCCESS && !p_lock.is_null() {
        if let Some(h) = handler {
            let inner = &mut *(*p_lock as *mut GrpLockInner);
            inner.destroy_handlers.push((h, member));
        }
    }
    status
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_acquire(lock: *mut pj_grp_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const GrpLockInner);
    inner.lock.lock();
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_tryacquire(lock: *mut pj_grp_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const GrpLockInner);
    if inner.lock.try_lock() {
        PJ_SUCCESS
    } else {
        PJ_EBUSY
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_release(lock: *mut pj_grp_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const GrpLockInner);
    inner.lock.unlock();
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_add_ref(lock: *mut pj_grp_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const GrpLockInner);
    inner.ref_count.fetch_add(1, Ordering::SeqCst);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_dec_ref(lock: *mut pj_grp_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &mut *(lock as *mut GrpLockInner);
    let prev = inner.ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev <= 1 {
        // Call destroy handlers
        for (handler, data) in inner.destroy_handlers.drain(..) {
            handler(data);
        }
        let _ = Box::from_raw(lock as *mut GrpLockInner);
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_add_handler(
    lock: *mut pj_grp_lock_t,
    _pool: *mut pj_pool_t,
    member: *mut libc::c_void,
    handler: Option<unsafe extern "C" fn(*mut libc::c_void)>,
) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    if let Some(h) = handler {
        let inner = &mut *(lock as *mut GrpLockInner);
        inner.destroy_handlers.push((h, member));
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_del_handler(
    lock: *mut pj_grp_lock_t,
    member: *mut libc::c_void,
    handler: Option<unsafe extern "C" fn(*mut libc::c_void)>,
) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    if let Some(h) = handler {
        let inner = &mut *(lock as *mut GrpLockInner);
        inner.destroy_handlers.retain(|&(f, d)| !(f == h && d == member));
    }
    PJ_SUCCESS
}

/// Dump group lock info (no-op).
#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_dump(lock: *mut pj_grp_lock_t) {
    let _ = lock;
}

/// Replace group lock in a chain (stub).
#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_replace(
    _old_lock: *mut pj_grp_lock_t,
    _new_lock: *mut pj_grp_lock_t,
) -> pj_status_t {
    PJ_SUCCESS
}

/// Get the group lock's ref count.
#[no_mangle]
pub unsafe extern "C" fn pj_grp_lock_get_ref(lock: *mut pj_grp_lock_t) -> i32 {
    if lock.is_null() {
        return 0;
    }
    let inner = &*(lock as *const GrpLockInner);
    inner.ref_count.load(Ordering::SeqCst) as i32
}

/// Return a raw pointer to the underlying PthreadMutex of a group lock.
/// Used by the ioqueue to use the group lock as the per-key lock.
pub(crate) unsafe fn grp_lock_inner_mutex(
    lock: *mut pj_grp_lock_t,
) -> *const PthreadMutex {
    if lock.is_null() {
        return std::ptr::null();
    }
    let inner = &*(lock as *const GrpLockInner);
    &inner.lock as *const PthreadMutex
}
