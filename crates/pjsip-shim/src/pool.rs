//! Pool allocator -- drop-in replacement for pjlib's pool.
//!
//! pjproject uses pool-based allocation everywhere.  Our implementation
//! provides a C-compatible `pj_pool_t` struct that matches the real pjlib
//! layout, so C code can directly access fields like `pool->alignment` and
//! `pool->block_list.next->buf/cur/end`.

use std::alloc::{alloc_zeroed, dealloc, Layout};

use crate::types::*;

/// PJ_POOL_ALIGNMENT -- default alignment for pool allocations.
pub const PJ_POOL_ALIGNMENT: usize = 4;

/// Pool block -- matches `struct pj_pool_block` in pjlib.
///
/// ```c
/// struct pj_pool_block {
///     struct pj_pool_block *prev;
///     struct pj_pool_block *next;
///     unsigned char *buf;
///     unsigned char *cur;
///     unsigned char *end;
/// };
/// ```
#[repr(C)]
pub struct pj_pool_block {
    pub prev: *mut pj_pool_block,
    pub next: *mut pj_pool_block,
    pub buf: *mut u8,
    pub cur: *mut u8,
    pub end: *mut u8,
}

/// PJ_MAX_OBJ_NAME
const PJ_MAX_OBJ_NAME: usize = 32;

/// C-compatible pool struct matching `struct pj_pool_t` in pjlib.
///
/// ```c
/// struct pj_pool_t {
///     struct pj_pool_t *prev;          // PJ_DECL_LIST_MEMBER
///     struct pj_pool_t *next;
///     char obj_name[PJ_MAX_OBJ_NAME];
///     pj_pool_factory *factory;
///     void *factory_data;
///     pj_size_t capacity;
///     pj_size_t increment_size;
///     pj_pool_block block_list;        // sentinel node for block list
///     pj_pool_callback *callback;
///     pj_size_t alignment;
/// };
/// ```
#[repr(C)]
pub struct CPoolT {
    pub prev: *mut CPoolT,
    pub next: *mut CPoolT,
    pub obj_name: [u8; PJ_MAX_OBJ_NAME],
    pub factory: *mut libc::c_void,
    pub factory_data: *mut libc::c_void,
    pub capacity: usize,
    pub increment_size: usize,
    pub block_list: pj_pool_block, // sentinel node
    pub callback: *mut libc::c_void,
    pub alignment: usize,
    // --- Extra fields beyond the C struct (C code doesn't access these) ---
    /// Tracks all allocations (ptr, layout) for proper dealloc.
    allocations: Vec<(*mut u8, Layout)>,
    /// List of allocated blocks (the raw memory for each block).
    blocks: Vec<(*mut u8, Layout)>,
}

impl CPoolT {
    /// Align `ptr` up to the given alignment.
    fn align_up(ptr: *mut u8, alignment: usize) -> *mut u8 {
        let addr = ptr as usize;
        let aligned = (addr + alignment - 1) & !(alignment - 1);
        aligned as *mut u8
    }

    /// Allocate a new block and add it to the block list.
    unsafe fn add_block(&mut self, min_data_size: usize) {
        let block_struct_size = std::mem::size_of::<pj_pool_block>();
        // Allocate enough for block struct + alignment padding + data
        let total = block_struct_size + self.alignment + min_data_size + self.alignment;
        let layout = Layout::from_size_align(total, 16).unwrap();
        let raw = alloc_zeroed(layout);
        if raw.is_null() {
            return;
        }
        self.blocks.push((raw, layout));

        // The block struct sits at the start of the raw allocation
        let block = raw as *mut pj_pool_block;
        let data_start = raw.add(block_struct_size);
        let aligned_start = Self::align_up(data_start, self.alignment);

        (*block).buf = aligned_start;
        (*block).cur = aligned_start;
        (*block).end = raw.add(total);
        (*block).prev = std::ptr::null_mut();
        (*block).next = std::ptr::null_mut();

        // Insert into the block list (after sentinel)
        let sentinel = &mut self.block_list as *mut pj_pool_block;
        let old_first = (*sentinel).next;
        (*block).next = old_first;
        (*block).prev = sentinel;
        if !old_first.is_null() {
            (*old_first).prev = block;
        } else {
            (*sentinel).prev = block;
        }
        (*sentinel).next = block;

        self.capacity += (*block).end.offset_from((*block).buf) as usize;
    }
}

// ---------------------------------------------------------------------------
// pj_pool_create
// ---------------------------------------------------------------------------

/// Create a new memory pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_create(
    _factory: *mut libc::c_void,
    name: *const libc::c_char,
    initial_size: usize,
    increment_size: usize,
    callback: *mut libc::c_void,
) -> *mut pj_pool_t {
    pj_pool_create_internal(name, initial_size, increment_size, PJ_POOL_ALIGNMENT, callback)
}

pub(crate) unsafe fn pj_pool_create_internal(
    name: *const libc::c_char,
    initial_size: usize,
    increment_size: usize,
    alignment: usize,
    callback: *mut libc::c_void,
) -> *mut pj_pool_t {
    let alignment = if alignment == 0 { PJ_POOL_ALIGNMENT } else { alignment };

    // Allocate the pool struct itself
    let layout = Layout::new::<CPoolT>();
    let raw = alloc_zeroed(layout);
    if raw.is_null() {
        return std::ptr::null_mut();
    }
    let pool = raw as *mut CPoolT;

    // Initialize name
    if !name.is_null() {
        let name_len = libc::strlen(name).min(PJ_MAX_OBJ_NAME - 1);
        std::ptr::copy_nonoverlapping(name as *const u8, (*pool).obj_name.as_mut_ptr(), name_len);
    } else {
        let default_name = b"pool\0";
        let copy_len = default_name.len().min(PJ_MAX_OBJ_NAME);
        std::ptr::copy_nonoverlapping(default_name.as_ptr(), (*pool).obj_name.as_mut_ptr(), copy_len);
    }

    (*pool).increment_size = increment_size;
    (*pool).callback = callback;
    (*pool).alignment = alignment;
    (*pool).capacity = 0;
    (*pool).allocations = Vec::new();
    (*pool).blocks = Vec::new();

    // Initialize block_list sentinel (points to itself when empty)
    let sentinel = &mut (*pool).block_list as *mut pj_pool_block;
    (*sentinel).prev = sentinel;
    (*sentinel).next = sentinel;
    (*sentinel).buf = std::ptr::null_mut();
    (*sentinel).cur = std::ptr::null_mut();
    (*sentinel).end = std::ptr::null_mut();

    // Allocate the initial block
    // Subtract pool struct overhead from initial_size
    let pool_struct_size = std::mem::size_of::<CPoolT>();
    let block_struct_size = std::mem::size_of::<pj_pool_block>();
    let data_size = if initial_size > pool_struct_size + block_struct_size {
        initial_size - pool_struct_size - block_struct_size
    } else {
        64 // minimum
    };
    (*pool).add_block(data_size);

    pool as *mut pj_pool_t
}

// ---------------------------------------------------------------------------
// pj_pool_alloc / pj_pool_zalloc / pj_pool_calloc
// ---------------------------------------------------------------------------

/// Allocate memory from a pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_alloc(
    pool: *mut pj_pool_t,
    size: usize,
) -> *mut libc::c_void {
    pj_pool_aligned_alloc_internal(pool, 0, size)
}

/// Allocate aligned memory from a pool.
pub(crate) unsafe fn pj_pool_aligned_alloc_internal(
    pool: *mut pj_pool_t,
    alignment: usize,
    size: usize,
) -> *mut libc::c_void {
    if pool.is_null() {
        return std::ptr::null_mut();
    }
    let cpool = pool as *mut CPoolT;
    let alloc_alignment = if alignment == 0 { (*cpool).alignment } else { alignment };
    let size = if size == 0 { 1 } else { size };

    // Try to allocate from existing blocks
    let sentinel = &mut (*cpool).block_list as *mut pj_pool_block;
    let mut block = (*sentinel).next;
    while block != sentinel {
        let cur_aligned = CPoolT::align_up((*block).cur, alloc_alignment);
        if cur_aligned.add(size) <= (*block).end {
            (*block).cur = cur_aligned.add(size);
            return cur_aligned as *mut libc::c_void;
        }
        block = (*block).next;
    }

    // No existing block had space -- allocate a new block
    let increment = (*cpool).increment_size;
    if increment == 0 {
        // Pool is not allowed to grow -- call the callback or return null
        if !(*cpool).callback.is_null() {
            let cb: unsafe extern "C" fn(*mut pj_pool_t, usize) =
                std::mem::transmute((*cpool).callback);
            cb(pool, size);
        }
        return std::ptr::null_mut();
    }

    let needed = size + alloc_alignment; // data + alignment padding
    let block_data_size = needed.max(increment);
    (*cpool).add_block(block_data_size);

    // Try again on the newly added block (it's first in the list)
    let block = (*sentinel).next;
    if block != sentinel {
        let cur_aligned = CPoolT::align_up((*block).cur, alloc_alignment);
        if cur_aligned.add(size) <= (*block).end {
            (*block).cur = cur_aligned.add(size);
            return cur_aligned as *mut libc::c_void;
        }
    }

    std::ptr::null_mut()
}

/// Allocate zero-filled memory from a pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_zalloc(
    pool: *mut pj_pool_t,
    size: usize,
) -> *mut libc::c_void {
    let ptr = pj_pool_alloc(pool, size);
    if !ptr.is_null() && size > 0 {
        libc::memset(ptr, 0, size);
    }
    ptr
}

/// Allocate an array of `count` elements, each `size` bytes, from the pool.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_calloc(
    pool: *mut pj_pool_t,
    count: usize,
    size: usize,
) -> *mut libc::c_void {
    let total = count.saturating_mul(size);
    let ptr = pj_pool_alloc(pool, total);
    if !ptr.is_null() && total > 0 {
        libc::memset(ptr, 0, total);
    }
    ptr
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
    let cpool = pool as *mut CPoolT;

    // Free all blocks
    let blocks = std::ptr::read(&(*cpool).blocks);
    for (ptr, layout) in blocks {
        dealloc(ptr, layout);
    }

    // Drop the allocations vec
    let _ = std::ptr::read(&(*cpool).allocations);

    // Deallocate the pool struct itself
    let layout = Layout::new::<CPoolT>();
    dealloc(pool as *mut u8, layout);
}

/// Reset the pool -- free all allocations but keep the pool alive.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_reset(pool: *mut pj_pool_t) {
    if pool.is_null() {
        return;
    }
    let cpool = pool as *mut CPoolT;

    // Free all blocks except the first one
    let blocks = &mut (*cpool).blocks;
    let alignment = (*cpool).alignment;

    // Reset capacity
    (*cpool).capacity = 0;

    // Free all blocks
    for (ptr, layout) in blocks.drain(..) {
        dealloc(ptr, layout);
    }

    // Reset block list sentinel
    let sentinel = &mut (*cpool).block_list as *mut pj_pool_block;
    (*sentinel).next = sentinel;
    (*sentinel).prev = sentinel;

    // Allocate a fresh initial block
    let block_struct_size = std::mem::size_of::<pj_pool_block>();
    let data_size = if (*cpool).increment_size > 0 {
        (*cpool).increment_size
    } else {
        64
    };
    (*cpool).add_block(data_size + block_struct_size + alignment);
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
    let cpool = pool as *const CPoolT;
    let mut used = std::mem::size_of::<CPoolT>(); // pool struct overhead

    let sentinel = &(*cpool).block_list as *const pj_pool_block;
    let mut block = (*sentinel).next;
    while block != sentinel as *mut pj_pool_block {
        // Add block struct overhead + used data
        used += std::mem::size_of::<pj_pool_block>();
        used += (*block).cur.offset_from((*block).buf) as usize;
        block = (*block).next;
    }
    used
}

/// Return pool capacity.
#[no_mangle]
pub unsafe extern "C" fn pj_pool_get_capacity(pool: *const pj_pool_t) -> usize {
    if pool.is_null() {
        return 0;
    }
    let cpool = pool as *const CPoolT;
    let mut cap = std::mem::size_of::<CPoolT>(); // pool struct overhead

    let sentinel = &(*cpool).block_list as *const pj_pool_block;
    let mut block = (*sentinel).next;
    while block != sentinel as *mut pj_pool_block {
        cap += std::mem::size_of::<pj_pool_block>();
        cap += (*block).end.offset_from((*block).buf) as usize;
        block = (*block).next;
    }
    cap
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
