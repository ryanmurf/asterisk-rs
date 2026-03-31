//! pj_ioqueue -- async I/O queue using select().
//!
//! Implements the pjlib ioqueue reactor pattern matching pjproject's
//! ioqueue_select.c + ioqueue_common_abs.c semantics.
//!
//! Key design choices (matching pjproject):
//!   - Per-key REENTRANT mutex so callbacks can re-enter send/recv.
//!   - When allow_concurrent=false, the key lock is held through callbacks.
//!   - The immediate-send fast path does NOT hold the key lock (matching
//!     pjproject's speculative optimisation in pj_ioqueue_send).
//!   - Partial writes on stream sockets are tracked to completion.

use std::cell::UnsafeCell;

use crate::misc::{pj_ioqueue_callback, pj_ioqueue_key_t, pj_ioqueue_op_key_t, pj_ioqueue_t};
use crate::socket::pj_sock_t;
use crate::threading::PthreadMutex;
use crate::types::*;

// ---------------------------------------------------------------------------
// Op-key pending flag -- mirrors pjproject's write_op->op field.
//
// pjproject stores a 4-byte int at byte offset 16 inside the op_key.
// 0 = PJ_IOQUEUE_OP_NONE, non-zero = pending.
// ---------------------------------------------------------------------------

const OP_BYTE_OFFSET: usize = 2 * std::mem::size_of::<*mut libc::c_void>();

unsafe fn op_key_set_pending(op_key: *mut pj_ioqueue_op_key_t, op_type: i32) {
    if !op_key.is_null() {
        let p = (op_key as *mut u8).add(OP_BYTE_OFFSET) as *mut i32;
        *p = op_type;
    }
}

unsafe fn op_key_clear(op_key: *mut pj_ioqueue_op_key_t) {
    if !op_key.is_null() {
        let p = (op_key as *mut u8).add(OP_BYTE_OFFSET) as *mut i32;
        *p = 0;
    }
}

// ---------------------------------------------------------------------------
// Pending operation types
// ---------------------------------------------------------------------------

struct PendingRead {
    op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void,
    len: isize,
    flags: u32,
    from: *mut pj_sockaddr,
    fromlen: *mut i32,
}
unsafe impl Send for PendingRead {}

struct PendingWrite {
    op_key: *mut pj_ioqueue_op_key_t,
    buf: *const libc::c_void,
    len: isize,
    written: isize,
    flags: u32,
    to: Option<(*const pj_sockaddr, i32)>,
    is_dgram: bool,
}
unsafe impl Send for PendingWrite {}

struct PendingAccept {
    op_key: *mut pj_ioqueue_op_key_t,
    new_sock: *mut pj_sock_t,
    local: *mut pj_sockaddr,
    remote: *mut pj_sockaddr,
    addrlen: *mut i32,
}
unsafe impl Send for PendingAccept {}

// ---------------------------------------------------------------------------
// IoKey / IoQueueInner -- reentrant mutex approach
// ---------------------------------------------------------------------------

struct IoKey {
    lock: PthreadMutex,
    /// When a group lock is provided via register_sock2, this points to
    /// the group lock's internal PthreadMutex.  key_lock/key_try_lock
    /// use this instead of `self.lock` so that the ioqueue dispatch
    /// acquires the SAME lock the test callbacks use, matching pjproject.
    ext_lock: *const PthreadMutex,
    inner: UnsafeCell<IoKeyInner>,
    /// Mirrors pjproject's `processing` flag.  Set to `true` while a poll
    /// thread is dispatching an event for this key.  Lives outside the
    /// UnsafeCell so it can be accessed with proper atomic ordering even
    /// after the key lock is released (allow_concurrent=true).
    processing: std::sync::atomic::AtomicBool,
}

struct IoKeyInner {
    fd: i32,
    user_data: *mut libc::c_void,
    cb: pj_ioqueue_callback,
    pending_reads: Vec<PendingRead>,
    pending_writes: Vec<PendingWrite>,
    pending_accept: Option<PendingAccept>,
    connecting: bool,
    ioqueue: *mut IoQueueInner,
    closing: bool,
    allow_concurrent: bool,
    fd_type: i32,
}
unsafe impl Send for IoKeyInner {}

unsafe impl Send for IoKey {}
unsafe impl Sync for IoKey {}

struct IoQueueInner {
    data: std::sync::Mutex<IoQueueData>,
    default_concurrency: std::sync::atomic::AtomicBool,
}

struct IoQueueData {
    keys: Vec<*mut IoKey>,
    max_fd: usize,
}
unsafe impl Send for IoQueueData {}
unsafe impl Sync for IoQueueData {}

fn get_errno() -> i32 { unsafe { *libc::__error() } }

fn is_wouldblock(err: i32) -> bool {
    err == libc::EAGAIN || err == libc::EWOULDBLOCK
}

/// RAII guard for PthreadMutex -- calls unlock on drop.
struct KeyLockGuard {
    mutex: *const PthreadMutex,
}

impl Drop for KeyLockGuard {
    fn drop(&mut self) {
        unsafe { (*self.mutex).unlock(); }
    }
}

/// Lock the key's reentrant mutex.  When an external group lock is
/// set, we lock that instead (matching pjproject's behaviour where
/// register_sock2's grp_lock replaces the per-key lock).
unsafe fn key_lock(key: &IoKey) -> KeyLockGuard {
    let mutex = if !key.ext_lock.is_null() {
        key.ext_lock
    } else {
        &key.lock as *const PthreadMutex
    };
    (*mutex).lock();
    KeyLockGuard { mutex }
}

unsafe fn key_try_lock(key: &IoKey) -> Option<KeyLockGuard> {
    let mutex = if !key.ext_lock.is_null() {
        key.ext_lock
    } else {
        &key.lock as *const PthreadMutex
    };
    if (*mutex).try_lock() {
        Some(KeyLockGuard { mutex })
    } else {
        None
    }
}

#[inline]
unsafe fn ki(key: &IoKey) -> &mut IoKeyInner {
    &mut *key.inner.get()
}

// ---------------------------------------------------------------------------
// Create / Destroy
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_create_impl(max_fd: u32) -> *mut pj_ioqueue_t {
    let inner = Box::new(IoQueueInner {
        data: std::sync::Mutex::new(IoQueueData {
            keys: Vec::new(),
            max_fd: max_fd as usize,
        }),
        default_concurrency: std::sync::atomic::AtomicBool::new(true),
    });
    Box::into_raw(inner) as *mut pj_ioqueue_t
}

pub unsafe fn ioqueue_destroy_impl(ioqueue: *mut pj_ioqueue_t) {
    if ioqueue.is_null() { return; }
    let inner = Box::from_raw(ioqueue as *mut IoQueueInner);
    let mut data = inner.data.lock().unwrap();
    for &key_ptr in &data.keys {
        if !key_ptr.is_null() {
            let _g = key_lock(&*key_ptr);
            let k = ki(&*key_ptr);
            if !k.closing {
                k.closing = true;
                let fd = k.fd;
                if fd >= 0 { k.fd = -1; libc::close(fd); }
                k.ioqueue = std::ptr::null_mut();
            }
        }
    }
    data.keys.clear();
    drop(data);
}

pub unsafe fn ioqueue_set_default_concurrency_impl(
    ioqueue: *mut pj_ioqueue_t, allow: pj_bool_t,
) -> pj_status_t {
    if ioqueue.is_null() { return PJ_EINVAL; }
    let inner = &*(ioqueue as *const IoQueueInner);
    inner.default_concurrency.store(allow != 0, std::sync::atomic::Ordering::Relaxed);
    PJ_SUCCESS
}

pub unsafe fn ioqueue_set_concurrency_impl(
    key: *mut pj_ioqueue_key_t, allow: pj_bool_t,
) -> pj_status_t {
    if key.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    ki(&*key_ptr).allow_concurrent = allow != 0;
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Register / Unregister
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_register_impl(
    ioqueue: *mut pj_ioqueue_t, sock: pj_sock_t,
    user_data: *mut libc::c_void, cb: *const pj_ioqueue_callback,
    grp_lock: *mut crate::atomic::pj_grp_lock_t,
) -> Result<*mut pj_ioqueue_key_t, pj_status_t> {
    if ioqueue.is_null() { return Err(PJ_EINVAL); }
    let inner = &*(ioqueue as *const IoQueueInner);
    let mut data = inner.data.lock().unwrap();
    if data.keys.len() >= data.max_fd { return Err(PJ_ETOOMANY); }

    let flags = libc::fcntl(sock as i32, libc::F_GETFL, 0);
    if flags >= 0 { libc::fcntl(sock as i32, libc::F_SETFL, flags | libc::O_NONBLOCK); }

    let callback = if !cb.is_null() { *cb } else {
        pj_ioqueue_callback {
            on_read_complete: None, on_write_complete: None,
            on_accept_complete: None, on_connect_complete: None,
        }
    };

    let mut fd_type: i32 = libc::SOCK_STREAM;
    let mut optlen: libc::socklen_t = std::mem::size_of::<i32>() as _;
    libc::getsockopt(sock as i32, libc::SOL_SOCKET, libc::SO_TYPE,
        &mut fd_type as *mut _ as *mut libc::c_void, &mut optlen);

    let default_c = inner.default_concurrency.load(std::sync::atomic::Ordering::Relaxed);

    // If a group lock is provided, use its internal mutex as the key lock.
    // This matches pjproject where register_sock2's grp_lock replaces the
    // per-key lock, ensuring the ioqueue dispatch and the application
    // callbacks acquire the SAME lock.
    let ext = crate::atomic::grp_lock_inner_mutex(grp_lock);

    let key = Box::new(IoKey {
        lock: PthreadMutex::new(),
        ext_lock: ext,
        inner: UnsafeCell::new(IoKeyInner {
            fd: sock as i32, user_data, cb: callback,
            pending_reads: Vec::new(), pending_writes: Vec::new(),
            pending_accept: None, connecting: false,
            ioqueue: ioqueue as *mut IoQueueInner,
            closing: false, allow_concurrent: default_c, fd_type,
        }),
        processing: std::sync::atomic::AtomicBool::new(false),
    });
    let key_ptr = Box::into_raw(key);
    data.keys.push(key_ptr);
    Ok(key_ptr as *mut pj_ioqueue_key_t)
}

pub unsafe fn ioqueue_unregister_impl(key: *mut pj_ioqueue_key_t) {
    if key.is_null() { return; }
    let key_ptr = key as *mut IoKey;
    let fd; let ioqueue;
    {
        let _g = key_lock(&*key_ptr);
        let k = ki(&*key_ptr);
        if k.closing { return; }
        k.closing = true; fd = k.fd; ioqueue = k.ioqueue;
        for pr in &k.pending_reads { op_key_clear(pr.op_key); }
        for pw in &k.pending_writes { op_key_clear(pw.op_key); }
        if let Some(ref pa) = k.pending_accept { op_key_clear(pa.op_key); }
        k.pending_reads.clear(); k.pending_writes.clear();
        k.pending_accept = None; k.connecting = false;
        k.ioqueue = std::ptr::null_mut(); k.fd = -1;
    }
    if !ioqueue.is_null() {
        let inner = &*ioqueue;
        let mut data = inner.data.lock().unwrap();
        data.keys.retain(|&kp| kp != key_ptr);
    }
    if fd >= 0 { libc::close(fd); }
}

// ---------------------------------------------------------------------------
// User data / OS handle
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_get_user_data_impl(key: *mut pj_ioqueue_key_t) -> *mut libc::c_void {
    if key.is_null() { return std::ptr::null_mut(); }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    ki(&*key_ptr).user_data
}

pub unsafe fn ioqueue_set_user_data_impl(
    key: *mut pj_ioqueue_key_t, user_data: *mut libc::c_void,
    old_data: *mut *mut libc::c_void,
) {
    if key.is_null() { return; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);
    if !old_data.is_null() { *old_data = k.user_data; }
    k.user_data = user_data;
}

pub unsafe fn ioqueue_get_os_handle_impl(key: *mut libc::c_void) -> pj_sock_t {
    if key.is_null() { return -1; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    ki(&*key_ptr).fd as pj_sock_t
}

// ---------------------------------------------------------------------------
// Async recv / recvfrom
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_recv_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void, length: *mut isize, flags: u32,
) -> pj_status_t {
    if key.is_null() || buf.is_null() || length.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);

    // Only attempt immediate recv if no reads are already pending.
    // This matches pjproject's behaviour: if the read list is non-empty,
    // we must queue behind existing reads to preserve ordering.
    if k.pending_reads.is_empty() {
        let n = libc::recv(k.fd, buf, *length as usize, flags as i32);
        if n >= 0 {
            *length = n as isize;
            return PJ_SUCCESS;
        }

        let err = get_errno();
        if !is_wouldblock(err) {
            return 120000 + err;
        }
    }

    op_key_set_pending(op_key, 2); // OP_RECV
    k.pending_reads.push(PendingRead {
        op_key, buf, len: *length, flags,
        from: std::ptr::null_mut(), fromlen: std::ptr::null_mut(),
    });
    PJ_EPENDING
}

pub unsafe fn ioqueue_recvfrom_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void, length: *mut isize, flags: u32,
    addr: *mut pj_sockaddr, addrlen: *mut i32,
) -> pj_status_t {
    if key.is_null() || buf.is_null() || length.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);

    // Only attempt immediate recv if no reads are already pending.
    if k.pending_reads.is_empty() {
        let mut slen: libc::socklen_t = if !addrlen.is_null() {
            *addrlen as libc::socklen_t
        } else { std::mem::size_of::<pj_sockaddr>() as _ };
        let from_ptr = if addr.is_null() { std::ptr::null_mut() }
                       else { addr as *mut libc::sockaddr };

        let n = libc::recvfrom(k.fd, buf, *length as usize, flags as i32, from_ptr, &mut slen);
        if n >= 0 {
            *length = n as isize;
            if !addrlen.is_null() { *addrlen = slen as i32; }
            return PJ_SUCCESS;
        }

        let err = get_errno();
        if !is_wouldblock(err) {
            return 120000 + err;
        }
    }

    op_key_set_pending(op_key, 3); // OP_RECV_FROM
    k.pending_reads.push(PendingRead {
        op_key, buf, len: *length, flags, from: addr, fromlen: addrlen,
    });
    PJ_EPENDING
}

// ---------------------------------------------------------------------------
// Async send / sendto
//
// CRITICAL: the immediate-send fast path does NOT hold the key lock.
// This matches pjproject's behaviour and avoids deadlock when the
// callback (which holds the key lock) calls pj_ioqueue_send and
// another thread holds grp_lock while trying to acquire the key lock.
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_send_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void, length: *mut isize, flags: u32,
) -> pj_status_t {
    if key.is_null() || data.is_null() || length.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;

    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);
    if k.closing { return 70015; }

    if k.pending_writes.is_empty() {
        let n = libc::send(k.fd, data, *length as usize, flags as i32);
        if n >= 0 {
            if n as isize >= *length || k.fd_type == libc::SOCK_DGRAM {
                *length = n as isize;
                return PJ_SUCCESS;
            }
            // Partial write on stream -- queue remainder
            op_key_set_pending(op_key, 1); // OP_SEND
            k.pending_writes.push(PendingWrite {
                op_key, buf: data, len: *length, written: n as isize,
                flags, to: None, is_dgram: false,
            });
            return PJ_EPENDING;
        }
        let err = get_errno();
        if !is_wouldblock(err) { return 120000 + err; }
    }

    // Write list non-empty or EWOULDBLOCK -- queue
    let is_dgram = k.fd_type == libc::SOCK_DGRAM;
    op_key_set_pending(op_key, 1); // OP_SEND
    k.pending_writes.push(PendingWrite {
        op_key, buf: data, len: *length, written: 0,
        flags, to: None, is_dgram,
    });
    PJ_EPENDING
}

pub unsafe fn ioqueue_sendto_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void, length: *mut isize, flags: u32,
    addr: *const pj_sockaddr, addrlen: i32,
) -> pj_status_t {
    if key.is_null() || data.is_null() || length.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;

    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);
    if k.closing { return 70015; }

    if k.pending_writes.is_empty() {
        let n = libc::sendto(k.fd, data, *length as usize, flags as i32,
            addr as *const libc::sockaddr, addrlen as libc::socklen_t);
        if n >= 0 {
            if n as isize >= *length || k.fd_type == libc::SOCK_DGRAM {
                *length = n as isize;
                return PJ_SUCCESS;
            }
            op_key_set_pending(op_key, 4); // OP_SEND_TO
            k.pending_writes.push(PendingWrite {
                op_key, buf: data, len: *length, written: n as isize,
                flags, to: Some((addr, addrlen)), is_dgram: false,
            });
            return PJ_EPENDING;
        }
        let err = get_errno();
        if !is_wouldblock(err) { return 120000 + err; }
    }

    let is_dgram = k.fd_type == libc::SOCK_DGRAM;
    op_key_set_pending(op_key, 4);
    k.pending_writes.push(PendingWrite {
        op_key, buf: data, len: *length, written: 0,
        flags, to: Some((addr, addrlen)), is_dgram,
    });
    PJ_EPENDING
}

pub unsafe fn ioqueue_read_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void, length: *mut isize,
) -> pj_status_t { ioqueue_recv_impl(key, op_key, buf, length, 0) }

pub unsafe fn ioqueue_write_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void, length: *mut isize,
) -> pj_status_t { ioqueue_send_impl(key, op_key, data, length, 0) }

// ---------------------------------------------------------------------------
// Async accept / connect
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_accept_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    sock: *mut pj_sock_t, local: *mut pj_sockaddr,
    remote: *mut pj_sockaddr, addrlen: *mut i32,
) -> pj_status_t {
    if key.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);

    let mut slen: libc::socklen_t = if !addrlen.is_null() {
        *addrlen as libc::socklen_t } else { std::mem::size_of::<pj_sockaddr>() as _ };
    let rp = if remote.is_null() { std::ptr::null_mut() }
             else { remote as *mut libc::sockaddr };

    let fd = libc::accept(k.fd, rp, &mut slen);
    if fd >= 0 {
        if !sock.is_null() { *sock = fd as pj_sock_t; }
        if !addrlen.is_null() { *addrlen = slen as i32; }
        if !local.is_null() {
            let mut ll: libc::socklen_t = std::mem::size_of::<pj_sockaddr>() as _;
            libc::getsockname(fd, local as *mut libc::sockaddr, &mut ll);
        }
        return PJ_SUCCESS;
    }
    let err = get_errno();
    if is_wouldblock(err) {
        op_key_set_pending(op_key, 5);
        k.pending_accept = Some(PendingAccept { op_key, new_sock: sock, local, remote, addrlen });
        return PJ_EPENDING;
    }
    120000 + err
}

pub unsafe fn ioqueue_connect_impl(
    key: *mut pj_ioqueue_key_t, addr: *const pj_sockaddr, addrlen: i32,
) -> pj_status_t {
    if key.is_null() || addr.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let _g = key_lock(&*key_ptr);
    let k = ki(&*key_ptr);
    let rc = libc::connect(k.fd, addr as *const libc::sockaddr, addrlen as libc::socklen_t);
    if rc == 0 { return PJ_SUCCESS; }
    let err = get_errno();
    if err == libc::EINPROGRESS { k.connecting = true; return PJ_EPENDING; }
    120000 + err
}

// ---------------------------------------------------------------------------
// Poll
//
// When allow_concurrent=false, the key lock is held through the callback.
// The reentrant mutex allows the callback to call send/recv on the same key.
// The immediate-send fast path in ioqueue_send_impl does NOT acquire the
// key lock, so there is no deadlock.
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_poll_impl(
    ioqueue: *mut pj_ioqueue_t,
    timeout: *const crate::timer::pj_time_val,
) -> i32 {
    if ioqueue.is_null() { return -1; }
    let inner = &*(ioqueue as *const IoQueueInner);

    struct PollEntry { key_ptr: *mut IoKey, fd: i32, want_read: bool, want_write: bool }

    let entries: Vec<PollEntry>;
    {
        let data = inner.data.lock().unwrap();
        entries = data.keys.iter().filter_map(|&key_ptr| {
            if key_ptr.is_null() { return None; }
            let _g = key_try_lock(&*key_ptr)?;
            let k = ki(&*key_ptr);
            if k.closing || k.fd < 0 { return None; }
            // Skip keys that another poll thread is already dispatching.
            if (*key_ptr).processing.load(std::sync::atomic::Ordering::Acquire) { return None; }
            let want_read = !k.pending_reads.is_empty() || k.pending_accept.is_some();
            let want_write = !k.pending_writes.is_empty() || k.connecting;
            if !want_read && !want_write { return None; }
            Some(PollEntry { key_ptr, fd: k.fd, want_read, want_write })
        }).collect();
    }

    if entries.is_empty() {
        if !timeout.is_null() {
            let total_us = (*timeout).sec as u64 * 1_000_000 + (*timeout).msec as u64 * 1000;
            if total_us > 0 { std::thread::sleep(std::time::Duration::from_micros(total_us)); }
        }
        return 0;
    }

    let mut read_fds: libc::fd_set = std::mem::zeroed();
    let mut write_fds: libc::fd_set = std::mem::zeroed();
    libc::FD_ZERO(&mut read_fds); libc::FD_ZERO(&mut write_fds);

    let mut max_fd: i32 = 0;
    for e in &entries {
        if e.want_read { libc::FD_SET(e.fd, &mut read_fds); }
        if e.want_write { libc::FD_SET(e.fd, &mut write_fds); }
        if e.fd >= max_fd { max_fd = e.fd + 1; }
    }

    let mut tv = if !timeout.is_null() {
        libc::timeval { tv_sec: (*timeout).sec as libc::time_t,
                        tv_usec: ((*timeout).msec * 1000) as libc::suseconds_t }
    } else { libc::timeval { tv_sec: 60, tv_usec: 0 } };

    let nready = libc::select(max_fd, &mut read_fds, &mut write_fds, std::ptr::null_mut(), &mut tv);
    if nready <= 0 { return nready; }

    let mut events_fired = 0i32;

    for e in &entries {
        let readable = e.want_read && libc::FD_ISSET(e.fd, &read_fds);
        let writable = e.want_write && libc::FD_ISSET(e.fd, &write_fds);
        if !readable && !writable { continue; }

        let guard = match key_try_lock(&*e.key_ptr) {
            Some(g) => g,
            None => continue,
        };
        let k = ki(&*e.key_ptr);
        if k.closing || k.fd < 0 { drop(guard); continue; }

        // With allow_concurrent=false, the key lock is held through the
        // callback, preventing concurrent dispatch. With allow_concurrent=true,
        // the lock is dropped before the callback, so we must NOT let another
        // thread dispatch for the same key concurrently. Use the processing
        // flag for this.
        if (*e.key_ptr).processing.swap(true, std::sync::atomic::Ordering::AcqRel) {
            // Another thread is already processing this key.
            drop(guard);
            continue;
        }


        // --- Readable: accept ---
        if readable {
            if let Some(pa) = k.pending_accept.take() {
                op_key_clear(pa.op_key);
                let mut slen: libc::socklen_t = if !pa.addrlen.is_null() {
                    *pa.addrlen as _ } else { std::mem::size_of::<pj_sockaddr>() as _ };
                let rp = if pa.remote.is_null() { std::ptr::null_mut() }
                         else { pa.remote as *mut libc::sockaddr };
                let new_fd = libc::accept(k.fd, rp, &mut slen);
                if new_fd >= 0 {
                    if !pa.new_sock.is_null() { *pa.new_sock = new_fd as pj_sock_t; }
                    if !pa.addrlen.is_null() { *pa.addrlen = slen as i32; }
                    if !pa.local.is_null() {
                        let mut ll: libc::socklen_t = std::mem::size_of::<pj_sockaddr>() as _;
                        libc::getsockname(new_fd, pa.local as *mut libc::sockaddr, &mut ll);
                    }
                    let cb = k.cb.on_accept_complete;
                    let ac = k.allow_concurrent;
                    if ac { drop(guard); }
                    events_fired += 1;
                    if let Some(f) = cb {
                        f(e.key_ptr as *mut pj_ioqueue_key_t, pa.op_key, new_fd as pj_sock_t, PJ_SUCCESS);
                    }
                    (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                    continue;
                } else {
                    let err = get_errno();
                    if is_wouldblock(err) {
                        op_key_set_pending(pa.op_key, 5);
                        k.pending_accept = Some(pa);
                    } else {
                        let cb = k.cb.on_accept_complete; let ac = k.allow_concurrent;
                        if ac { drop(guard); }
                        events_fired += 1;
                        if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, pa.op_key, -1 as pj_sock_t, 120000+err); }
                        (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                        continue;
                    }
                }
            }

            // --- Readable: recv ---
            if !k.pending_reads.is_empty() {
                let pr = k.pending_reads.remove(0);
                op_key_clear(pr.op_key);
                let mut slen: libc::socklen_t = if !pr.fromlen.is_null() {
                    *pr.fromlen as _ } else { std::mem::size_of::<pj_sockaddr>() as _ };
                let n = if !pr.from.is_null() {
                    libc::recvfrom(k.fd, pr.buf, pr.len as usize, pr.flags as i32,
                        pr.from as *mut libc::sockaddr, &mut slen)
                } else {
                    libc::recv(k.fd, pr.buf, pr.len as usize, pr.flags as i32)
                };
                if n >= 0 {
                    if !pr.fromlen.is_null() { *pr.fromlen = slen as i32; }
                    let cb = k.cb.on_read_complete; let ac = k.allow_concurrent;
                    if ac { drop(guard); }
                    events_fired += 1;
                    if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, pr.op_key, n as isize); }
                    (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                    continue;
                } else {
                    let err = get_errno();
                    if is_wouldblock(err) {
                        op_key_set_pending(pr.op_key, 2);
                        k.pending_reads.insert(0, pr);
                    } else {
                        let cb = k.cb.on_read_complete; let ac = k.allow_concurrent;
                        if ac { drop(guard); }
                        events_fired += 1;
                        if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, pr.op_key, -(err as isize)); }
                        (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                        continue;
                    }
                }
            }
        }

        // --- Writable: connect ---
        if writable {
            if k.connecting {
                k.connecting = false;
                let mut err: i32 = 0;
                let mut errlen: libc::socklen_t = std::mem::size_of::<i32>() as _;
                libc::getsockopt(k.fd, libc::SOL_SOCKET, libc::SO_ERROR,
                    &mut err as *mut _ as *mut libc::c_void, &mut errlen);
                let status = if err == 0 { PJ_SUCCESS } else { 120000 + err };
                let cb = k.cb.on_connect_complete; let ac = k.allow_concurrent;
                if ac { drop(guard); }
                events_fired += 1;
                if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, status); }
                (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                continue;
            } else if !k.pending_writes.is_empty() {
                // --- Writable: send (with partial-write tracking) ---
                let fd = k.fd;
                let pw_written = k.pending_writes[0].written;
                let pw_len = k.pending_writes[0].len;
                let pw_flags = k.pending_writes[0].flags;
                let pw_buf = k.pending_writes[0].buf;
                let pw_to = k.pending_writes[0].to;
                let pw_is_dgram = k.pending_writes[0].is_dgram;

                let remaining = (pw_len - pw_written) as usize;
                let buf_offset = (pw_buf as *const u8).add(pw_written as usize) as *const libc::c_void;

                let n = if let Some((to, tolen)) = pw_to {
                    libc::sendto(fd, buf_offset, remaining, pw_flags as i32,
                        to as *const libc::sockaddr, tolen as libc::socklen_t)
                } else {
                    libc::send(fd, buf_offset, remaining, pw_flags as i32)
                };

                if n >= 0 {
                    k.pending_writes[0].written += n as isize;
                    let new_written = k.pending_writes[0].written;
                    if new_written >= pw_len || pw_is_dgram {
                        let pw = k.pending_writes.remove(0);
                        op_key_clear(pw.op_key);
                        let cb = k.cb.on_write_complete; let ac = k.allow_concurrent;
                        if ac { drop(guard); }
                        events_fired += 1;
                        if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, pw.op_key, new_written); }
                        (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                        continue;
                    }
                    // partial write -- stay in queue, clear processing
                } else {
                    let err = get_errno();
                    if !is_wouldblock(err) {
                        let pw = k.pending_writes.remove(0);
                        op_key_clear(pw.op_key);
                        let cb = k.cb.on_write_complete; let ac = k.allow_concurrent;
                        if ac { drop(guard); }
                        events_fired += 1;
                        if let Some(f) = cb { f(e.key_ptr as *mut pj_ioqueue_key_t, pw.op_key, -(err as isize)); }
                        (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
                        continue;
                    }
                }
            }
        }

        // No event dispatched or only partial write -- clear processing.
        (*e.key_ptr).processing.store(false, std::sync::atomic::Ordering::Release);
        drop(guard);
    }

    events_fired
}

// ---------------------------------------------------------------------------
// Is pending / Post completion
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_is_pending_impl(
    _key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
) -> pj_bool_t {
    if op_key.is_null() { return PJ_FALSE; }
    let p = (op_key as *const u8).add(OP_BYTE_OFFSET) as *const i32;
    if *p != 0 { PJ_TRUE } else { PJ_FALSE }
}

pub unsafe fn ioqueue_post_completion_impl(
    key: *mut pj_ioqueue_key_t, op_key: *mut pj_ioqueue_op_key_t,
    bytes_status: isize,
) -> pj_status_t {
    if key.is_null() { return PJ_EINVAL; }
    let key_ptr = key as *mut IoKey;
    let cb = { let _g = key_lock(&*key_ptr); ki(&*key_ptr).cb.on_read_complete };
    if let Some(f) = cb { f(key, op_key, bytes_status); }
    PJ_SUCCESS
}
