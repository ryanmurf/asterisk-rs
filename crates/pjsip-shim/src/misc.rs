//! Miscellaneous pjlib symbols -- file I/O, hash table, random, exception,
//! fifobuf, activesock, ioqueue, pool extensions, error strings, etc.

use crate::types::*;

// ============================================================================
// File I/O
// ============================================================================

/// Opaque file descriptor.
pub type pj_oshandle_t = *mut libc::c_void;
pub type pj_off_t = i64;

#[no_mangle]
pub unsafe extern "C" fn pj_file_open(
    _pool: *mut pj_pool_t,
    path: *const libc::c_char,
    flags: u32,
    handle: *mut pj_oshandle_t,
) -> pj_status_t {
    if path.is_null() || handle.is_null() {
        return PJ_EINVAL;
    }
    // PJ_O_RDONLY=1, PJ_O_WRONLY=2, PJ_O_RDWR=4, PJ_O_APPEND=8
    let mut oflags = if flags & 2 != 0 || flags & 4 != 0 {
        let base = if flags & 1 != 0 || flags & 4 != 0 {
            libc::O_RDWR
        } else {
            libc::O_WRONLY
        };
        base | libc::O_CREAT
    } else {
        libc::O_RDONLY
    };
    if flags & 8 != 0 {
        oflags |= libc::O_APPEND;
    }
    let fd = libc::open(path, oflags, 0o644);
    if fd < 0 {
        *handle = std::ptr::null_mut();
        return PJ_EINVAL;
    }
    *handle = fd as isize as *mut libc::c_void;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_close(handle: pj_oshandle_t) -> pj_status_t {
    let fd = handle as isize as i32;
    if libc::close(fd) == 0 { PJ_SUCCESS } else { PJ_EINVAL }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_read(
    handle: pj_oshandle_t,
    buf: *mut libc::c_void,
    size: *mut isize,
) -> pj_status_t {
    if buf.is_null() || size.is_null() {
        return PJ_EINVAL;
    }
    let fd = handle as isize as i32;
    let n = libc::read(fd, buf, *size as usize);
    if n < 0 {
        return PJ_EINVAL;
    }
    *size = n as isize;
    if n == 0 { PJ_EEOF } else { PJ_SUCCESS }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_write(
    handle: pj_oshandle_t,
    buf: *const libc::c_void,
    size: *mut isize,
) -> pj_status_t {
    if buf.is_null() || size.is_null() {
        return PJ_EINVAL;
    }
    let fd = handle as isize as i32;
    let n = libc::write(fd, buf, *size as usize);
    if n < 0 {
        return PJ_EINVAL;
    }
    *size = n as isize;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_setpos(
    handle: pj_oshandle_t,
    offset: pj_off_t,
    whence: i32,
) -> pj_status_t {
    let fd = handle as isize as i32;
    let w = match whence {
        0 => libc::SEEK_SET,
        1 => libc::SEEK_CUR,
        2 => libc::SEEK_END,
        _ => libc::SEEK_SET,
    };
    if libc::lseek(fd, offset as libc::off_t, w) < 0 {
        PJ_EINVAL
    } else {
        PJ_SUCCESS
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_getpos(
    handle: pj_oshandle_t,
    pos: *mut pj_off_t,
) -> pj_status_t {
    if pos.is_null() {
        return PJ_EINVAL;
    }
    let fd = handle as isize as i32;
    let p = libc::lseek(fd, 0, libc::SEEK_CUR);
    if p < 0 {
        return PJ_EINVAL;
    }
    *pos = p as pj_off_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_flush(handle: pj_oshandle_t) -> pj_status_t {
    let fd = handle as isize as i32;
    if libc::fsync(fd) == 0 { PJ_SUCCESS } else { PJ_EINVAL }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_exists(path: *const libc::c_char) -> pj_bool_t {
    if path.is_null() {
        return PJ_FALSE;
    }
    if libc::access(path, libc::F_OK) == 0 {
        PJ_TRUE
    } else {
        PJ_FALSE
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_size(path: *const libc::c_char) -> pj_off_t {
    if path.is_null() {
        return -1;
    }
    let mut st: libc::stat = std::mem::zeroed();
    if libc::stat(path, &mut st) == 0 {
        st.st_size as pj_off_t
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_delete(path: *const libc::c_char) -> pj_status_t {
    if path.is_null() {
        return PJ_EINVAL;
    }
    if libc::unlink(path) == 0 { PJ_SUCCESS } else { PJ_EINVAL }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_move(
    oldpath: *const libc::c_char,
    newpath: *const libc::c_char,
) -> pj_status_t {
    if oldpath.is_null() || newpath.is_null() {
        return PJ_EINVAL;
    }
    if libc::rename(oldpath, newpath) == 0 { PJ_SUCCESS } else { PJ_EINVAL }
}

#[no_mangle]
pub unsafe extern "C" fn pj_file_getstat(
    path: *const libc::c_char,
    stat: *mut libc::c_void,
) -> pj_status_t {
    if path.is_null() || stat.is_null() {
        return PJ_EINVAL;
    }
    if libc::stat(path, stat as *mut libc::stat) == 0 {
        PJ_SUCCESS
    } else {
        PJ_EINVAL
    }
}

// ============================================================================
// Hash table
// ============================================================================

/// Opaque hash table.
#[repr(C)]
pub struct pj_hash_table_t {
    _opaque: [u8; 0],
}

/// Hash iteration (opaque).
#[repr(C)]
pub struct pj_hash_iterator_t {
    _index: u32,
    _entry: *mut libc::c_void,
}

use std::collections::HashMap;

struct HashInner {
    map: HashMap<Vec<u8>, (*mut libc::c_void, u32)>,
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_create(
    _pool: *mut pj_pool_t,
    _size: u32,
) -> *mut pj_hash_table_t {
    let inner = Box::new(HashInner {
        map: HashMap::new(),
    });
    Box::into_raw(inner) as *mut pj_hash_table_t
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_get(
    ht: *mut pj_hash_table_t,
    key: *const libc::c_void,
    keylen: i32,
    _hval: *mut u32,
) -> *mut libc::c_void {
    if ht.is_null() || key.is_null() {
        return std::ptr::null_mut();
    }
    let inner = &*(ht as *const HashInner);
    let keylen = if keylen < 0 {
        libc::strlen(key as *const _)
    } else {
        keylen as usize
    };
    let key_bytes = std::slice::from_raw_parts(key as *const u8, keylen).to_vec();
    match inner.map.get(&key_bytes) {
        Some(&(val, _)) => val,
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_set(
    _pool: *mut pj_pool_t,
    ht: *mut pj_hash_table_t,
    key: *const libc::c_void,
    keylen: i32,
    hval: u32,
    value: *mut libc::c_void,
) {
    if ht.is_null() || key.is_null() {
        return;
    }
    let inner = &mut *(ht as *mut HashInner);
    let keylen = if keylen < 0 {
        libc::strlen(key as *const _)
    } else {
        keylen as usize
    };
    let key_bytes = std::slice::from_raw_parts(key as *const u8, keylen).to_vec();
    if value.is_null() {
        inner.map.remove(&key_bytes);
    } else {
        inner.map.insert(key_bytes, (value, hval));
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_set_np(
    ht: *mut pj_hash_table_t,
    key: *const libc::c_void,
    keylen: i32,
    hval: u32,
    value: *mut libc::c_void,
) {
    pj_hash_set(std::ptr::null_mut(), ht, key, keylen, hval, value);
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_count(ht: *mut pj_hash_table_t) -> u32 {
    if ht.is_null() {
        return 0;
    }
    let inner = &*(ht as *const HashInner);
    inner.map.len() as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_first(
    ht: *mut pj_hash_table_t,
    it: *mut pj_hash_iterator_t,
) -> *mut pj_hash_iterator_t {
    if ht.is_null() || it.is_null() {
        return std::ptr::null_mut();
    }
    let inner = &*(ht as *const HashInner);
    if inner.map.is_empty() {
        return std::ptr::null_mut();
    }
    (*it)._index = 0;
    it
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_next(
    ht: *mut pj_hash_table_t,
    it: *mut pj_hash_iterator_t,
) -> *mut pj_hash_iterator_t {
    if ht.is_null() || it.is_null() {
        return std::ptr::null_mut();
    }
    let inner = &*(ht as *const HashInner);
    (*it)._index += 1;
    if ((*it)._index as usize) < inner.map.len() {
        it
    } else {
        std::ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_this(
    _ht: *mut pj_hash_table_t,
    it: *mut pj_hash_iterator_t,
    key: *mut *const libc::c_void,
    keylen: *mut i32,
    value: *mut *mut libc::c_void,
) {
    // Stub: set all to null
    if !key.is_null() {
        *key = std::ptr::null();
    }
    if !keylen.is_null() {
        *keylen = 0;
    }
    if !value.is_null() {
        *value = std::ptr::null_mut();
    }
    let _ = it;
}

#[no_mangle]
pub unsafe extern "C" fn pj_hash_calc(
    _hval: u32,
    key: *const libc::c_void,
    keylen: i32,
) -> u32 {
    if key.is_null() {
        return 0;
    }
    let len = if keylen < 0 {
        libc::strlen(key as *const _)
    } else {
        keylen as usize
    };
    let bytes = std::slice::from_raw_parts(key as *const u8, len);
    // Simple hash (DJB2)
    let mut hash = 5381u32;
    for &b in bytes {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    hash
}

// ============================================================================
// Random
// ============================================================================

static mut RANDOM_SEED: u32 = 12345;

#[no_mangle]
pub unsafe extern "C" fn pj_srand(seed: u32) {
    RANDOM_SEED = seed;
}

#[no_mangle]
pub unsafe extern "C" fn pj_rand() -> i32 {
    // Simple LCG
    RANDOM_SEED = RANDOM_SEED.wrapping_mul(1103515245).wrapping_add(12345);
    ((RANDOM_SEED >> 16) & 0x7FFF) as i32
}

// ============================================================================
// Exception handling
// ============================================================================

/// Exception handler stack (simplified with setjmp/longjmp stubs).
#[repr(C)]
pub struct pj_exception_state_t {
    pub prev: *mut pj_exception_state_t,
    _pad: [u8; 256], // space for jmp_buf
}

static mut EXCEPTION_STACK: *mut pj_exception_state_t = std::ptr::null_mut();
static mut EXCEPTION_ID_COUNTER: i32 = 1;

#[no_mangle]
pub unsafe extern "C" fn pj_throw_exception(id: i32) {
    // In a real impl this would longjmp. We just log and abort.
    eprintln!("pj_throw_exception({}): not supported in shim, aborting", id);
    std::process::abort();
}

#[no_mangle]
pub unsafe extern "C" fn pj_push_exception_handler(rec: *mut pj_exception_state_t) {
    if !rec.is_null() {
        (*rec).prev = EXCEPTION_STACK;
        EXCEPTION_STACK = rec;
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_pop_exception_handler(rec: *mut pj_exception_state_t) {
    if !rec.is_null() && EXCEPTION_STACK == rec {
        EXCEPTION_STACK = (*rec).prev;
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_exception_id_alloc(
    _name: *const libc::c_char,
    id: *mut i32,
) -> pj_status_t {
    if id.is_null() {
        return PJ_EINVAL;
    }
    *id = EXCEPTION_ID_COUNTER;
    EXCEPTION_ID_COUNTER += 1;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_exception_id_free(id: i32) {
    let _ = id;
}

#[no_mangle]
pub unsafe extern "C" fn pj_exception_id_name(id: i32) -> *const libc::c_char {
    let _ = id;
    b"exception\0".as_ptr() as *const _
}

// ============================================================================
// FIFO buffer
// ============================================================================

#[repr(C)]
pub struct pj_fifobuf_t {
    pub first: *mut libc::c_char,
    pub last: *mut libc::c_char,
    pub ubegin: *mut libc::c_char,
    pub uend: *mut libc::c_char,
    pub full: i32,
}

#[no_mangle]
pub unsafe extern "C" fn pj_fifobuf_init(
    fb: *mut pj_fifobuf_t,
    buffer: *mut libc::c_void,
    size: u32,
) {
    if fb.is_null() {
        return;
    }
    (*fb).first = buffer as *mut _;
    (*fb).last = (buffer as *mut u8).add(size as usize) as *mut _;
    (*fb).ubegin = (*fb).first;
    (*fb).uend = (*fb).first;
    (*fb).full = 0;
}

#[no_mangle]
pub unsafe extern "C" fn pj_fifobuf_max_size(fb: *mut pj_fifobuf_t) -> u32 {
    if fb.is_null() {
        return 0;
    }
    (*fb).last.offset_from((*fb).first) as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_fifobuf_alloc(
    fb: *mut pj_fifobuf_t,
    size: u32,
) -> *mut libc::c_void {
    if fb.is_null() || size == 0 {
        return std::ptr::null_mut();
    }
    // Simplified: allocate from the end
    let available = (*fb).last.offset_from((*fb).uend) as u32;
    if available >= size + 4 {
        let ptr = (*fb).uend;
        // Store size in first 4 bytes
        *(ptr as *mut u32) = size;
        let result = ptr.add(4);
        (*fb).uend = result.add(size as usize);
        return result as *mut _;
    }
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn pj_fifobuf_unalloc(
    fb: *mut pj_fifobuf_t,
    buf: *mut libc::c_void,
) -> pj_status_t {
    if fb.is_null() || buf.is_null() {
        return PJ_EINVAL;
    }
    let ptr = (buf as *mut u8).sub(4);
    let size = *(ptr as *const u32);
    // Only free if this was the last allocation
    let expected_end = (buf as *mut u8).add(size as usize);
    if expected_end == (*fb).uend as *mut u8 {
        (*fb).uend = ptr as *mut _;
        return PJ_SUCCESS;
    }
    PJ_EINVAL
}

#[no_mangle]
pub unsafe extern "C" fn pj_fifobuf_free(
    fb: *mut pj_fifobuf_t,
    buf: *mut libc::c_void,
) -> pj_status_t {
    pj_fifobuf_unalloc(fb, buf)
}

// ============================================================================
// Active socket (stubs)
// ============================================================================

/// Opaque active socket.
#[repr(C)]
pub struct pj_activesock_t {
    _opaque: [u8; 0],
}

/// Active socket callbacks.
#[repr(C)]
pub struct pj_activesock_cb {
    pub on_data_read: Option<unsafe extern "C" fn(*mut pj_activesock_t, *mut libc::c_void, isize, pj_status_t, *mut usize) -> pj_bool_t>,
    pub on_data_recvfrom: Option<unsafe extern "C" fn(*mut pj_activesock_t, *mut libc::c_void, isize, *const pj_sockaddr, i32, pj_status_t) -> pj_bool_t>,
    pub on_data_sent: Option<unsafe extern "C" fn(*mut pj_activesock_t, *mut libc::c_void, isize) -> pj_bool_t>,
    pub on_accept_complete: Option<unsafe extern "C" fn(*mut pj_activesock_t, crate::socket::pj_sock_t, *const pj_sockaddr, i32) -> pj_bool_t>,
    pub on_connect_complete: Option<unsafe extern "C" fn(*mut pj_activesock_t, pj_status_t) -> pj_bool_t>,
}

/// Active socket configuration.
#[repr(C)]
pub struct pj_activesock_cfg {
    pub grp_lock: *mut crate::atomic::pj_grp_lock_t,
    pub async_cnt: u32,
    pub concurrency: i32,
    pub whole_data: pj_bool_t,
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_cfg_default(cfg: *mut pj_activesock_cfg) {
    if cfg.is_null() {
        return;
    }
    std::ptr::write_bytes(cfg as *mut u8, 0, std::mem::size_of::<pj_activesock_cfg>());
    (*cfg).async_cnt = 1;
    (*cfg).concurrency = -1;
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_create(
    _pool: *mut pj_pool_t,
    _sock: crate::socket::pj_sock_t,
    _sock_type: i32,
    _opt: *const pj_activesock_cfg,
    _ioqueue: *mut libc::c_void,
    _cb: *const pj_activesock_cb,
    _user_data: *mut libc::c_void,
    p_asock: *mut *mut pj_activesock_t,
) -> pj_status_t {
    if p_asock.is_null() {
        return PJ_EINVAL;
    }
    *p_asock = Box::into_raw(Box::new(0u64)) as *mut pj_activesock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_create_udp(
    _pool: *mut pj_pool_t,
    _addr: *const pj_sockaddr,
    _opt: *const pj_activesock_cfg,
    _ioqueue: *mut libc::c_void,
    _cb: *const pj_activesock_cb,
    _user_data: *mut libc::c_void,
    p_asock: *mut *mut pj_activesock_t,
    _bound_addr: *mut pj_sockaddr,
) -> pj_status_t {
    if p_asock.is_null() {
        return PJ_EINVAL;
    }
    *p_asock = Box::into_raw(Box::new(0u64)) as *mut pj_activesock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_close(asock: *mut pj_activesock_t) -> pj_status_t {
    if !asock.is_null() {
        let _ = Box::from_raw(asock as *mut u64);
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_set_user_data(
    _asock: *mut pj_activesock_t,
    _user_data: *mut libc::c_void,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_get_user_data(
    _asock: *mut pj_activesock_t,
) -> *mut libc::c_void {
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_read(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
    _buff_size: u32,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_read2(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
    _buff_size: u32,
    _readbuf: *mut *mut libc::c_void,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_recvfrom(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
    _buff_size: u32,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_recvfrom2(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
    _buff_size: u32,
    _readbuf: *mut libc::c_void,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_send(
    _asock: *mut pj_activesock_t,
    _send_key: *mut libc::c_void,
    _data: *const libc::c_void,
    _size: *mut isize,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_sendto(
    _asock: *mut pj_activesock_t,
    _send_key: *mut libc::c_void,
    _data: *const libc::c_void,
    _size: *mut isize,
    _flags: u32,
    _addr: *const pj_sockaddr,
    _addr_len: i32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_accept(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_activesock_start_connect(
    _asock: *mut pj_activesock_t,
    _pool: *mut pj_pool_t,
    _remaddr: *const pj_sockaddr,
    _addr_len: i32,
) -> pj_status_t {
    PJ_SUCCESS
}

// ============================================================================
// I/O Queue
// ============================================================================

/// Opaque ioqueue.
#[repr(C)]
pub struct pj_ioqueue_t {
    _opaque: [u8; 0],
}

/// Opaque ioqueue key.
#[repr(C)]
pub struct pj_ioqueue_key_t {
    _opaque: [u8; 0],
}

/// I/O queue operation key.
#[repr(C)]
pub struct pj_ioqueue_op_key_t {
    pub internal_: [*mut libc::c_void; 32],
    pub activesock_data: *mut libc::c_void,
    pub user_data: *mut libc::c_void,
}

/// I/O queue callbacks.
#[repr(C)]
pub struct pj_ioqueue_callback {
    pub on_read_complete: Option<unsafe extern "C" fn(*mut pj_ioqueue_key_t, *mut pj_ioqueue_op_key_t, isize)>,
    pub on_write_complete: Option<unsafe extern "C" fn(*mut pj_ioqueue_key_t, *mut pj_ioqueue_op_key_t, isize)>,
    pub on_accept_complete: Option<unsafe extern "C" fn(*mut pj_ioqueue_key_t, *mut pj_ioqueue_op_key_t, crate::socket::pj_sock_t, pj_status_t)>,
    pub on_connect_complete: Option<unsafe extern "C" fn(*mut pj_ioqueue_key_t, pj_status_t)>,
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_create(
    _pool: *mut pj_pool_t,
    _max_fd: u32,
    p_ioqueue: *mut *mut pj_ioqueue_t,
) -> pj_status_t {
    if p_ioqueue.is_null() {
        return PJ_EINVAL;
    }
    *p_ioqueue = Box::into_raw(Box::new(0u64)) as *mut pj_ioqueue_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_destroy(ioqueue: *mut pj_ioqueue_t) -> pj_status_t {
    if !ioqueue.is_null() {
        let _ = Box::from_raw(ioqueue as *mut u64);
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_poll(
    _ioqueue: *mut pj_ioqueue_t,
    _timeout: *const crate::timer::pj_time_val,
) -> i32 {
    0 // no events
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_register_sock(
    _pool: *mut pj_pool_t,
    _ioqueue: *mut pj_ioqueue_t,
    _sock: crate::socket::pj_sock_t,
    _user_data: *mut libc::c_void,
    _cb: *const pj_ioqueue_callback,
    p_key: *mut *mut pj_ioqueue_key_t,
) -> pj_status_t {
    if p_key.is_null() {
        return PJ_EINVAL;
    }
    *p_key = Box::into_raw(Box::new(0u64)) as *mut pj_ioqueue_key_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_register_sock2(
    _pool: *mut pj_pool_t,
    _ioqueue: *mut pj_ioqueue_t,
    _sock: crate::socket::pj_sock_t,
    _grp_lock: *mut crate::atomic::pj_grp_lock_t,
    _user_data: *mut libc::c_void,
    _cb: *const pj_ioqueue_callback,
    p_key: *mut *mut pj_ioqueue_key_t,
) -> pj_status_t {
    if p_key.is_null() {
        return PJ_EINVAL;
    }
    *p_key = Box::into_raw(Box::new(0u64)) as *mut pj_ioqueue_key_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_unregister(key: *mut pj_ioqueue_key_t) -> pj_status_t {
    if !key.is_null() {
        let _ = Box::from_raw(key as *mut u64);
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_get_user_data(
    _key: *mut pj_ioqueue_key_t,
) -> *mut libc::c_void {
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_set_user_data(
    _key: *mut pj_ioqueue_key_t,
    _user_data: *mut libc::c_void,
    _old_data: *mut *mut libc::c_void,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_recv(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _buf: *mut libc::c_void,
    _length: *mut isize,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_recvfrom(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _buf: *mut libc::c_void,
    _length: *mut isize,
    _flags: u32,
    _addr: *mut pj_sockaddr,
    _addrlen: *mut i32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_send(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _data: *const libc::c_void,
    _length: *mut isize,
    _flags: u32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_sendto(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _data: *const libc::c_void,
    _length: *mut isize,
    _flags: u32,
    _addr: *const pj_sockaddr,
    _addrlen: i32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_accept(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _sock: *mut crate::socket::pj_sock_t,
    _local: *mut pj_sockaddr,
    _remote: *mut pj_sockaddr,
    _addrlen: *mut i32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_connect(
    _key: *mut pj_ioqueue_key_t,
    _addr: *const pj_sockaddr,
    _addrlen: i32,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_op_key_init(
    op_key: *mut pj_ioqueue_op_key_t,
    _size: usize,
) {
    if !op_key.is_null() {
        std::ptr::write_bytes(op_key as *mut u8, 0, std::mem::size_of::<pj_ioqueue_op_key_t>());
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_is_pending(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
) -> pj_bool_t {
    PJ_FALSE
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_post_completion(
    _key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
    _bytes_status: isize,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_set_default_concurrency(
    _ioqueue: *mut pj_ioqueue_t,
    _allow: pj_bool_t,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_set_concurrency(
    _key: *mut pj_ioqueue_key_t,
    _allow: pj_bool_t,
) -> pj_status_t {
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_ioqueue_name() -> *const libc::c_char {
    b"select\0".as_ptr() as *const _
}

// ============================================================================
// Pool extensions
// ============================================================================

#[no_mangle]
pub unsafe extern "C" fn pj_pool_safe_release(ppool: *mut *mut pj_pool_t) {
    if ppool.is_null() || (*ppool).is_null() {
        return;
    }
    crate::pool::pj_pool_release(*ppool);
    *ppool = std::ptr::null_mut();
}

#[no_mangle]
pub unsafe extern "C" fn pj_pool_secure_release(ppool: *mut *mut pj_pool_t) {
    pj_pool_safe_release(ppool);
}

#[no_mangle]
pub unsafe extern "C" fn pj_pool_aligned_alloc(
    pool: *mut pj_pool_t,
    size: usize,
    _alignment: usize,
) -> *mut libc::c_void {
    // Our alloc already aligns to 8 bytes
    crate::pool::pj_pool_alloc(pool, size)
}

// ============================================================================
// Error string
// ============================================================================

#[no_mangle]
pub unsafe extern "C" fn pj_strerror(
    status: pj_status_t,
    buf: *mut libc::c_char,
    bufsize: usize,
) -> *mut pj_str_t {
    if buf.is_null() || bufsize == 0 {
        return std::ptr::null_mut();
    }
    let msg = match status {
        0 => "Success",
        70014 => "Invalid value or argument",
        70015 => "Not enough memory",
        70018 => "Not found",
        70027 => "Too many objects",
        70028 => "End of file",
        70029 => "Resource busy",
        70030 => "Invalid operation",
        _ => "Unknown error",
    };
    let bytes = msg.as_bytes();
    let copy_len = bytes.len().min(bufsize - 1);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
    *buf.add(copy_len) = 0;

    // Return a static pj_str_t pointing to the buffer
    // (The caller typically ignores the return value)
    static mut RET_STR: pj_str_t = pj_str_t {
        ptr: std::ptr::null_mut(),
        slen: 0,
    };
    RET_STR.ptr = buf;
    RET_STR.slen = copy_len as isize;
    std::ptr::addr_of_mut!(RET_STR)
}

#[no_mangle]
pub unsafe extern "C" fn pj_register_strerror(
    _start: pj_status_t,
    _count: i32,
    _f: Option<unsafe extern "C" fn(pj_status_t, *mut libc::c_char, usize) -> *mut pj_str_t>,
) -> pj_status_t {
    PJ_SUCCESS
}

// ============================================================================
// Miscellaneous system functions
// ============================================================================

#[no_mangle]
pub unsafe extern "C" fn pj_getpid() -> u32 {
    libc::getpid() as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_local_alloc(index: *mut i32) -> pj_status_t {
    if index.is_null() {
        return PJ_EINVAL;
    }
    static mut TLS_COUNTER: i32 = 0;
    *index = TLS_COUNTER;
    TLS_COUNTER += 1;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_local_free(index: i32) {
    let _ = index;
}

static mut TLS_VALUES: [*mut libc::c_void; 64] = [std::ptr::null_mut(); 64];

#[no_mangle]
pub unsafe extern "C" fn pj_thread_local_set(
    index: i32,
    value: *mut libc::c_void,
) -> pj_status_t {
    if index < 0 || index >= 64 {
        return PJ_EINVAL;
    }
    TLS_VALUES[index as usize] = value;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_local_get(index: i32) -> *mut libc::c_void {
    if index < 0 || index >= 64 {
        return std::ptr::null_mut();
    }
    TLS_VALUES[index as usize]
}

// ============================================================================
// Sorting and utilities
// ============================================================================

/// qsort wrapper
#[no_mangle]
pub unsafe extern "C" fn pj_sort(
    base: *mut libc::c_void,
    count: usize,
    size: usize,
    comp: Option<unsafe extern "C" fn(*const libc::c_void, *const libc::c_void) -> i32>,
) {
    if let Some(comp) = comp {
        libc::qsort(base, count, size, Some(std::mem::transmute(comp)));
    }
}

/// pj_strerror2 -- just calls pj_strerror.
#[no_mangle]
pub unsafe extern "C" fn pj_strerror2(
    status: pj_status_t,
    buf: *mut libc::c_char,
    bufsize: usize,
) -> *const libc::c_char {
    pj_strerror(status, buf, bufsize);
    buf as *const _
}

/// IP helper count
#[no_mangle]
pub unsafe extern "C" fn pj_enum_ip_interface(
    af: i32,
    count: *mut u32,
    ifs: *mut pj_sockaddr,
) -> pj_status_t {
    if count.is_null() {
        return PJ_EINVAL;
    }
    if *count == 0 || ifs.is_null() {
        *count = 0;
        return PJ_SUCCESS;
    }
    // Return loopback
    std::ptr::write_bytes(ifs as *mut u8, 0, std::mem::size_of::<pj_sockaddr>());
    if af == PJ_AF_INET as i32 || af == 0 {
        (*ifs).addr.sin_family = PJ_AF_INET;
        (*ifs).addr.sin_addr.s_addr = 0x0100007f; // 127.0.0.1
    } else {
        (*ifs).ipv6.sin6_family = PJ_AF_INET6;
        (*ifs).ipv6.sin6_addr.s6_addr[15] = 1; // ::1
    }
    *count = 1;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_enum_ip_interface2(
    _param: *const libc::c_void,
    count: *mut u32,
    ifs: *mut pj_sockaddr,
) -> pj_status_t {
    if count.is_null() {
        return PJ_EINVAL;
    }
    pj_enum_ip_interface(PJ_AF_INET as i32, count, ifs)
}

/// System info
#[repr(C)]
pub struct pj_sys_info {
    pub machine: pj_str_t,
    pub os_name: pj_str_t,
    pub os_ver: u32,
    pub sdk_name: pj_str_t,
    pub sdk_ver: u32,
    pub info: pj_str_t,
    pub flags: u32,
}

#[no_mangle]
pub unsafe extern "C" fn pj_get_sys_info() -> *const pj_sys_info {
    static mut SYS_INFO: pj_sys_info = pj_sys_info {
        machine: pj_str_t { ptr: std::ptr::null_mut(), slen: 0 },
        os_name: pj_str_t { ptr: std::ptr::null_mut(), slen: 0 },
        os_ver: 0,
        sdk_name: pj_str_t { ptr: std::ptr::null_mut(), slen: 0 },
        sdk_ver: 0,
        info: pj_str_t { ptr: std::ptr::null_mut(), slen: 0 },
        flags: 0,
    };
    std::ptr::addr_of!(SYS_INFO)
}

/// Math / UUID / misc stubs
#[no_mangle]
pub unsafe extern "C" fn pj_generate_unique_string(
    _pool: *mut pj_pool_t,
    s: *mut pj_str_t,
) {
    if s.is_null() {
        return;
    }
    // Return the hex of a random value
    let val = pj_rand() as u32;
    let hex = format!("{:08x}", val);
    // Write into existing buffer if available
    if !(*s).ptr.is_null() && (*s).slen >= 8 {
        std::ptr::copy_nonoverlapping(hex.as_bytes().as_ptr(), (*s).ptr as *mut u8, 8);
        (*s).slen = 8;
    }
}

/// Sleep in seconds
#[no_mangle]
pub unsafe extern "C" fn pj_thread_sleep_ms(msec: u32) -> pj_status_t {
    std::thread::sleep(std::time::Duration::from_millis(msec as u64));
    PJ_SUCCESS
}

/// pj_assert - no-op at runtime
#[no_mangle]
pub unsafe extern "C" fn pj_assert_on_fail(
    _expr: *const libc::c_char,
    _file: *const libc::c_char,
    _line: i32,
) {
    // In debug mode we could panic, but for linking purposes this is a no-op
}

/// Math helpers
#[no_mangle]
pub unsafe extern "C" fn pj_math_stat_init(stat: *mut libc::c_void) {
    if !stat.is_null() {
        std::ptr::write_bytes(stat as *mut u8, 0, 64);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_math_stat_update(stat: *mut libc::c_void, _val: i32) {
    let _ = stat;
}

/// DNS resolver stubs (needed by some tests)
#[no_mangle]
pub unsafe extern "C" fn pj_dns_resolver_create(
    _pf: *mut libc::c_void,
    _name: *const libc::c_char,
    _flags: u32,
    _timer_heap: *mut libc::c_void,
    _ioqueue: *mut libc::c_void,
    p_resolver: *mut *mut libc::c_void,
) -> pj_status_t {
    if !p_resolver.is_null() {
        *p_resolver = Box::into_raw(Box::new(0u64)) as *mut _;
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_dns_resolver_destroy(
    resolver: *mut libc::c_void,
    _notify: pj_bool_t,
) -> pj_status_t {
    if !resolver.is_null() {
        let _ = Box::from_raw(resolver as *mut u64);
    }
    PJ_SUCCESS
}

/// IP version preference
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_addr_len(addr: *const pj_sockaddr) -> u32 {
    if addr.is_null() {
        return 0;
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        4
    } else if family == PJ_AF_INET6 {
        16
    } else {
        0
    }
}

/// Pool factory policy (stub)
#[repr(C)]
pub struct pj_pool_factory_policy {
    pub block_alloc: Option<unsafe extern "C" fn(*mut libc::c_void, usize) -> *mut libc::c_void>,
    pub block_free: Option<unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void, usize)>,
    pub callback: Option<unsafe extern "C" fn(*mut pj_pool_t, usize)>,
    pub flags: u32,
}

#[no_mangle]
pub static mut pj_pool_factory_default_policy: pj_pool_factory_policy = pj_pool_factory_policy {
    block_alloc: None,
    block_free: None,
    callback: None,
    flags: 0,
};

/// Unicode stubs
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_to_unicode(
    _src: *const libc::c_char,
    _src_len: i32,
    _dst: *mut u16,
    _dst_len: i32,
) -> *mut u16 {
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn pj_unicode_to_ansi(
    _src: *const u16,
    _src_len: i32,
    _dst: *mut libc::c_char,
    _dst_len: i32,
) -> *mut libc::c_char {
    std::ptr::null_mut()
}

/// Hostname for IP
#[no_mangle]
pub unsafe extern "C" fn pj_gethostbyname(
    _name: *const pj_str_t,
    he: *mut libc::c_void,
) -> pj_status_t {
    let _ = he;
    PJ_SUCCESS
}

/// High resolution sleep
#[no_mangle]
pub unsafe extern "C" fn pj_highprec_mod(_a: *mut libc::c_void, _b: *mut libc::c_void) {
    // stub
}

/// crc32
#[no_mangle]
pub unsafe extern "C" fn pj_crc32_init(_ctx: *mut u32) {
    // stub
}

#[no_mangle]
pub unsafe extern "C" fn pj_crc32_update(_ctx: *mut u32, _data: *const u8, _len: u32) {
    // stub
}

#[no_mangle]
pub unsafe extern "C" fn pj_crc32_final(_ctx: *mut u32) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pj_crc32_calc(data: *const u8, len: u32) -> u32 {
    if data.is_null() {
        return 0;
    }
    // Simple checksum, not real CRC32
    let bytes = std::slice::from_raw_parts(data, len as usize);
    let mut hash = 0u32;
    for &b in bytes {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    hash
}

/// SSL/TLS stubs
#[no_mangle]
pub unsafe extern "C" fn pj_ssl_sock_param_default(param: *mut libc::c_void) {
    if !param.is_null() {
        std::ptr::write_bytes(param as *mut u8, 0, 256);
    }
}

/// IP helper
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_has_addr(addr: *const pj_sockaddr) -> pj_bool_t {
    if addr.is_null() {
        return PJ_FALSE;
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        if (*addr).addr.sin_addr.s_addr != 0 { PJ_TRUE } else { PJ_FALSE }
    } else if family == PJ_AF_INET6 {
        let zeros = [0u8; 16];
        if (*addr).ipv6.sin6_addr.s6_addr != zeros { PJ_TRUE } else { PJ_FALSE }
    } else {
        PJ_FALSE
    }
}

/// IP text address
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_str_addr(
    addr: *const pj_sockaddr,
    buf: *mut libc::c_char,
    size: usize,
) -> *const libc::c_char {
    crate::sockaddr::pj_sockaddr_print(addr, buf, size as i32, 0)
}

/// Sockaddr in set str
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_in_set_str_addr(
    addr: *mut pj_sockaddr_in,
    str_addr: *const pj_str_t,
) -> pj_status_t {
    if addr.is_null() || str_addr.is_null() {
        return PJ_EINVAL;
    }
    let text = (*str_addr).as_str();
    if let Some(ipv4) = parse_ipv4_for_misc(text) {
        (*addr).sin_addr.s_addr = ipv4;
        PJ_SUCCESS
    } else {
        PJ_EINVAL
    }
}

fn parse_ipv4_for_misc(s: &str) -> Option<u32> {
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

/// String buffer utilities
#[no_mangle]
pub unsafe extern "C" fn pj_create_unique_string(
    _pool: *mut pj_pool_t,
    str_: *mut pj_str_t,
) -> *mut pj_str_t {
    pj_generate_unique_string(_pool, str_);
    str_
}

/// Get sockaddr family
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_family(addr: *const pj_sockaddr) -> u16 {
    if addr.is_null() {
        return 0;
    }
    (*addr).addr.sin_family
}

/// Set sockaddr length (no-op for us)
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_set_len(_addr: *mut pj_sockaddr, _len: i32) {
    // no-op
}
