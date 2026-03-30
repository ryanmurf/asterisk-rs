//! Pool allocator -- drop-in replacement for pjlib's pool.
//!
//! pjproject uses pool-based allocation everywhere.  Our implementation wraps
//! the system allocator while tracking every allocation so `pj_pool_release`
//! frees them all in one shot, just like the original.

use std::alloc::{alloc_zeroed, dealloc, Layout};

use crate::types::*;

/// Internal pool bookkeeping.  The pointer we hand out as `*mut pj_pool_t`
/// is actually a `*mut PoolInner` in disguise.
pub(crate) struct PoolInner {
    #[allow(dead_code)]
    pub name: String,
    pub allocations: Vec<(*mut u8, Layout)>,
}

// ---------------------------------------------------------------------------
// pj_pool_create
// ---------------------------------------------------------------------------

/// Create a new memory pool.
///
/// The `factory`, `initial_size`, `increment_size`, and `callback` parameters
/// are accepted for API compatibility but ignored -- we just use the system
/// allocator.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_create(
    _factory: *mut libc::c_void,
    name: *const libc::c_char,
    _initial_size: usize,
    _increment_size: usize,
    _callback: *mut libc::c_void,
) -> *mut pj_pool_t {
    let name_str = if name.is_null() {
        "pool".to_string()
    } else {
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    };
    let inner = Box::new(PoolInner {
        name: name_str,
        allocations: Vec::new(),
    });
    Box::into_raw(inner) as *mut pj_pool_t
}

// ---------------------------------------------------------------------------
// pj_pool_alloc / pj_pool_zalloc / pj_pool_calloc
// ---------------------------------------------------------------------------

/// Allocate memory from a pool.  Returns zero-filled memory (matching
/// pjproject's behaviour for pool allocations from a freshly-reset pool).
#[no_mangle]
pub unsafe extern "C" fn pj_pool_alloc(
    pool: *mut pj_pool_t,
    size: usize,
) -> *mut libc::c_void {
    if pool.is_null() {
        return std::ptr::null_mut();
    }
    let inner = &mut *(pool as *mut PoolInner);
    let layout = Layout::from_size_align(size.max(1), 8).unwrap();
    let ptr = alloc_zeroed(layout);
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    inner.allocations.push((ptr, layout));
    ptr as *mut libc::c_void
}

/// Allocate zero-filled memory from a pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_zalloc(
    pool: *mut pj_pool_t,
    size: usize,
) -> *mut libc::c_void {
    pj_pool_alloc(pool, size) // already zero-fills
}

/// Allocate an array of `count` elements, each `size` bytes, from the pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_calloc(
    pool: *mut pj_pool_t,
    count: usize,
    size: usize,
) -> *mut libc::c_void {
    pj_pool_alloc(pool, count.saturating_mul(size))
}

// ---------------------------------------------------------------------------
// pj_pool_release / pj_pool_reset
// ---------------------------------------------------------------------------

/// Release a pool and all memory allocated from it.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_release(pool: *mut pj_pool_t) {
    if pool.is_null() {
        return;
    }
    let inner = Box::from_raw(pool as *mut PoolInner);
    for (ptr, layout) in inner.allocations {
        dealloc(ptr, layout);
    }
    // `inner` is dropped here, freeing the PoolInner itself.
}

/// Reset the pool -- free all allocations but keep the pool alive.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_reset(pool: *mut pj_pool_t) {
    if pool.is_null() {
        return;
    }
    let inner = &mut *(pool as *mut PoolInner);
    for (ptr, layout) in inner.allocations.drain(..) {
        dealloc(ptr, layout);
    }
}

// ---------------------------------------------------------------------------
// pj_pool_get_used_size / pj_pool_get_capacity
// ---------------------------------------------------------------------------

/// Return total bytes currently allocated from the pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_get_used_size(pool: *const pj_pool_t) -> usize {
    if pool.is_null() {
        return 0;
    }
    let inner = &*(pool as *const PoolInner);
    inner.allocations.iter().map(|(_, l)| l.size()).sum()
}

/// Return pool capacity.  We report used size since we don't pre-allocate.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_get_capacity(pool: *const pj_pool_t) -> usize {
    pj_pool_get_used_size(pool)
}

// ---------------------------------------------------------------------------
// Caching pool
// ---------------------------------------------------------------------------

/// Initialise a caching pool factory.  We just zero-fill it; the real
/// allocation happens in `pj_pool_create`.
#[no_mangle]
pub unsafe extern "C" fn pj_caching_pool_init(
    cp: *mut pj_caching_pool,
    _policy: *const libc::c_void,
    _max_capacity: usize,
) {
    if !cp.is_null() {
        std::ptr::write_bytes(cp as *mut u8, 0, std::mem::size_of::<pj_caching_pool>());
    }
}

/// Destroy a caching pool.
#[no_mangle]
pub unsafe extern "C" fn pj_caching_pool_destroy(_cp: *mut pj_caching_pool) {
    // Nothing to do -- individual pools are released by pj_pool_release.
}
