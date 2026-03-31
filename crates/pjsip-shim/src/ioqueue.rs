//! pj_ioqueue -- async I/O queue using select().
//!
//! Implements the pjlib ioqueue reactor pattern: register sockets,
//! request async I/O operations, poll for completions, fire callbacks.
//!
//! Thread-safe: multiple threads may call poll concurrently.  Per-key
//! mutex prevents two threads from processing the same key at the same
//! time, preserving TCP byte-stream ordering.

use std::sync::Mutex;

use crate::misc::{pj_ioqueue_callback, pj_ioqueue_key_t, pj_ioqueue_op_key_t, pj_ioqueue_t};
use crate::socket::pj_sock_t;
use crate::types::*;

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
    flags: u32,
    to: Option<(*const pj_sockaddr, i32)>,
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
// IoKey / IoQueueInner
// ---------------------------------------------------------------------------

/// Per-key state.  The Mutex serialises access from concurrent poll
/// threads AND from the application thread that queues new operations.
struct IoKey {
    /// Guards all mutable fields below.
    lock: Mutex<IoKeyInner>,
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
    /// True while a poll thread is processing this key's completions.
    /// Other poll threads skip this key until the processing thread is done.
    processing: bool,
}
unsafe impl Send for IoKeyInner {}

struct IoQueueInner {
    data: Mutex<IoQueueData>,
}

struct IoQueueData {
    keys: Vec<*mut IoKey>,
    max_fd: usize,
}
unsafe impl Send for IoQueueData {}
unsafe impl Sync for IoQueueData {}

// IoKey contains a Mutex which is Send+Sync
unsafe impl Send for IoKey {}
unsafe impl Sync for IoKey {}

fn get_errno() -> i32 {
    unsafe { *libc::__error() }
}

fn is_wouldblock(err: i32) -> bool {
    err == libc::EAGAIN || err == libc::EWOULDBLOCK
}

// ---------------------------------------------------------------------------
// Create / Destroy
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_create_impl(max_fd: u32) -> *mut pj_ioqueue_t {
    let inner = Box::new(IoQueueInner {
        data: Mutex::new(IoQueueData {
            keys: Vec::new(),
            max_fd: max_fd as usize,
        }),
    });
    Box::into_raw(inner) as *mut pj_ioqueue_t
}

pub unsafe fn ioqueue_destroy_impl(ioqueue: *mut pj_ioqueue_t) {
    if ioqueue.is_null() {
        return;
    }
    let inner = Box::from_raw(ioqueue as *mut IoQueueInner);
    let mut data = inner.data.lock().unwrap();
    for &key_ptr in &data.keys {
        if !key_ptr.is_null() {
            let mut ki = (*key_ptr).lock.lock().unwrap();
            if !ki.closing {
                ki.closing = true;
                let fd = ki.fd;
                if fd >= 0 {
                    ki.fd = -1;
                    libc::close(fd);
                }
                ki.ioqueue = std::ptr::null_mut();
            }
        }
    }
    data.keys.clear();
    drop(data);
}

// ---------------------------------------------------------------------------
// Register / Unregister
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_register_impl(
    ioqueue: *mut pj_ioqueue_t,
    sock: pj_sock_t,
    user_data: *mut libc::c_void,
    cb: *const pj_ioqueue_callback,
) -> Result<*mut pj_ioqueue_key_t, pj_status_t> {
    if ioqueue.is_null() {
        return Err(PJ_EINVAL);
    }
    let inner = &*(ioqueue as *const IoQueueInner);
    let mut data = inner.data.lock().unwrap();

    if data.keys.len() >= data.max_fd {
        return Err(PJ_ETOOMANY);
    }

    // Set socket to non-blocking
    let flags = libc::fcntl(sock as i32, libc::F_GETFL, 0);
    if flags >= 0 {
        libc::fcntl(sock as i32, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let callback = if !cb.is_null() {
        *cb
    } else {
        pj_ioqueue_callback {
            on_read_complete: None,
            on_write_complete: None,
            on_accept_complete: None,
            on_connect_complete: None,
        }
    };

    let key = Box::new(IoKey {
        lock: Mutex::new(IoKeyInner {
            fd: sock as i32,
            user_data,
            cb: callback,
            pending_reads: Vec::new(),
            pending_writes: Vec::new(),
            pending_accept: None,
            connecting: false,
            ioqueue: ioqueue as *mut IoQueueInner,
            closing: false,
            processing: false,
        }),
    });
    let key_ptr = Box::into_raw(key);
    data.keys.push(key_ptr);

    Ok(key_ptr as *mut pj_ioqueue_key_t)
}

pub unsafe fn ioqueue_unregister_impl(key: *mut pj_ioqueue_key_t) {
    if key.is_null() {
        return;
    }
    let key_ptr = key as *mut IoKey;
    let fd;
    let ioqueue;

    {
        let mut ki = (*key_ptr).lock.lock().unwrap();
        if ki.closing {
            return;
        }
        ki.closing = true;
        fd = ki.fd;
        ioqueue = ki.ioqueue;

        ki.pending_reads.clear();
        ki.pending_writes.clear();
        ki.pending_accept = None;
        ki.connecting = false;
        ki.ioqueue = std::ptr::null_mut();
        ki.fd = -1;
    }

    if !ioqueue.is_null() {
        let inner = &*ioqueue;
        let mut data = inner.data.lock().unwrap();
        data.keys.retain(|&k| k != key_ptr);
    }

    if fd >= 0 {
        libc::close(fd);
    }
    // Key memory intentionally leaked -- pjlib allocates keys from pool.
}

// ---------------------------------------------------------------------------
// User data / OS handle
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_get_user_data_impl(key: *mut pj_ioqueue_key_t) -> *mut libc::c_void {
    if key.is_null() {
        return std::ptr::null_mut();
    }
    let key_ptr = key as *mut IoKey;
    let ki = (*key_ptr).lock.lock().unwrap();
    ki.user_data
}

pub unsafe fn ioqueue_set_user_data_impl(
    key: *mut pj_ioqueue_key_t,
    user_data: *mut libc::c_void,
    old_data: *mut *mut libc::c_void,
) {
    if key.is_null() {
        return;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();
    if !old_data.is_null() {
        *old_data = ki.user_data;
    }
    ki.user_data = user_data;
}

pub unsafe fn ioqueue_get_os_handle_impl(key: *mut libc::c_void) -> pj_sock_t {
    if key.is_null() {
        return -1;
    }
    let key_ptr = key as *mut IoKey;
    let ki = (*key_ptr).lock.lock().unwrap();
    ki.fd as pj_sock_t
}

// ---------------------------------------------------------------------------
// Async recv / recvfrom
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_recv_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void,
    length: *mut isize,
    flags: u32,
) -> pj_status_t {
    if key.is_null() || buf.is_null() || length.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let n = libc::recv(ki.fd, buf, *length as usize, flags as i32);
    if n >= 0 {
        *length = n as isize;
        return PJ_SUCCESS;
    }

    let err = get_errno();
    if is_wouldblock(err) {
        ki.pending_reads.push(PendingRead {
            op_key, buf, len: *length, flags,
            from: std::ptr::null_mut(), fromlen: std::ptr::null_mut(),
        });
        return PJ_EPENDING;
    }
    120000 + err
}

pub unsafe fn ioqueue_recvfrom_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void,
    length: *mut isize,
    flags: u32,
    addr: *mut pj_sockaddr,
    addrlen: *mut i32,
) -> pj_status_t {
    if key.is_null() || buf.is_null() || length.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let mut slen: libc::socklen_t = if !addrlen.is_null() {
        *addrlen as libc::socklen_t
    } else {
        std::mem::size_of::<pj_sockaddr>() as libc::socklen_t
    };
    let from_ptr = if addr.is_null() {
        std::ptr::null_mut()
    } else {
        addr as *mut libc::sockaddr
    };

    let n = libc::recvfrom(ki.fd, buf, *length as usize, flags as i32, from_ptr, &mut slen);
    if n >= 0 {
        *length = n as isize;
        if !addrlen.is_null() {
            *addrlen = slen as i32;
        }
        return PJ_SUCCESS;
    }

    let err = get_errno();
    if is_wouldblock(err) {
        ki.pending_reads.push(PendingRead {
            op_key, buf, len: *length, flags, from: addr, fromlen: addrlen,
        });
        return PJ_EPENDING;
    }
    120000 + err
}

// ---------------------------------------------------------------------------
// Async send / sendto
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_send_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void,
    length: *mut isize,
    flags: u32,
) -> pj_status_t {
    if key.is_null() || data.is_null() || length.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let n = libc::send(ki.fd, data, *length as usize, flags as i32);
    if n >= 0 {
        *length = n as isize;
        return PJ_SUCCESS;
    }

    let err = get_errno();
    if is_wouldblock(err) {
        ki.pending_writes.push(PendingWrite {
            op_key, buf: data, len: *length, flags, to: None,
        });
        return PJ_EPENDING;
    }
    120000 + err
}

pub unsafe fn ioqueue_sendto_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void,
    length: *mut isize,
    flags: u32,
    addr: *const pj_sockaddr,
    addrlen: i32,
) -> pj_status_t {
    if key.is_null() || data.is_null() || length.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let n = libc::sendto(
        ki.fd, data, *length as usize, flags as i32,
        addr as *const libc::sockaddr, addrlen as libc::socklen_t,
    );
    if n >= 0 {
        *length = n as isize;
        return PJ_SUCCESS;
    }

    let err = get_errno();
    if is_wouldblock(err) {
        ki.pending_writes.push(PendingWrite {
            op_key, buf: data, len: *length, flags,
            to: Some((addr, addrlen)),
        });
        return PJ_EPENDING;
    }
    120000 + err
}

// ---------------------------------------------------------------------------
// Async read / write (same as recv/send with flags=0)
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_read_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    buf: *mut libc::c_void,
    length: *mut isize,
) -> pj_status_t {
    ioqueue_recv_impl(key, op_key, buf, length, 0)
}

pub unsafe fn ioqueue_write_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    data: *const libc::c_void,
    length: *mut isize,
) -> pj_status_t {
    ioqueue_send_impl(key, op_key, data, length, 0)
}

// ---------------------------------------------------------------------------
// Async accept
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_accept_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    sock: *mut pj_sock_t,
    local: *mut pj_sockaddr,
    remote: *mut pj_sockaddr,
    addrlen: *mut i32,
) -> pj_status_t {
    if key.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let mut slen: libc::socklen_t = if !addrlen.is_null() {
        *addrlen as libc::socklen_t
    } else {
        std::mem::size_of::<pj_sockaddr>() as libc::socklen_t
    };
    let remote_ptr = if remote.is_null() {
        std::ptr::null_mut()
    } else {
        remote as *mut libc::sockaddr
    };

    let fd = libc::accept(ki.fd, remote_ptr, &mut slen);
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
        ki.pending_accept = Some(PendingAccept {
            op_key, new_sock: sock, local, remote, addrlen,
        });
        return PJ_EPENDING;
    }
    120000 + err
}

// ---------------------------------------------------------------------------
// Async connect
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_connect_impl(
    key: *mut pj_ioqueue_key_t,
    addr: *const pj_sockaddr,
    addrlen: i32,
) -> pj_status_t {
    if key.is_null() || addr.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let mut ki = (*key_ptr).lock.lock().unwrap();

    let rc = libc::connect(
        ki.fd, addr as *const libc::sockaddr, addrlen as libc::socklen_t,
    );
    if rc == 0 {
        return PJ_SUCCESS;
    }

    let err = get_errno();
    if err == libc::EINPROGRESS {
        ki.connecting = true;
        return PJ_EPENDING;
    }
    120000 + err
}

// ---------------------------------------------------------------------------
// Poll  -- thread-safe: multiple threads may call concurrently.
//
// Strategy:
//   1.  Lock ioqueue, snapshot which keys have pending ops, unlock.
//   2.  Build fd_sets and call select() (no lock held).
//   3.  For each ready fd, lock the *per-key* mutex, set `processing`,
//       claim the pending op, do the syscall, release lock, fire callback,
//       then clear `processing`.
//
// The per-key `processing` flag stays set DURING the callback so that no
// other poll thread can race on the same key.  This preserves TCP byte
// ordering across concurrent poll threads.
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_poll_impl(
    ioqueue: *mut pj_ioqueue_t,
    timeout: *const crate::timer::pj_time_val,
) -> i32 {
    if ioqueue.is_null() {
        return -1;
    }
    let inner = &*(ioqueue as *const IoQueueInner);

    // ------------------------------------------------------------------
    // Step 1: snapshot which fds to watch.
    // ------------------------------------------------------------------

    struct PollEntry {
        key_ptr: *mut IoKey,
        fd: i32,
        want_read: bool,
        want_write: bool,
    }

    let entries: Vec<PollEntry>;
    {
        let data = inner.data.lock().unwrap();
        entries = data
            .keys
            .iter()
            .filter_map(|&key_ptr| {
                if key_ptr.is_null() {
                    return None;
                }
                // Try to lock key; if another poll thread has it, skip.
                let ki = match (*key_ptr).lock.try_lock() {
                    Ok(g) => g,
                    Err(_) => return None,
                };
                if ki.closing || ki.fd < 0 || ki.processing {
                    return None;
                }
                let want_read = !ki.pending_reads.is_empty() || ki.pending_accept.is_some();
                let want_write = !ki.pending_writes.is_empty() || ki.connecting;
                if !want_read && !want_write {
                    return None;
                }
                Some(PollEntry {
                    key_ptr,
                    fd: ki.fd,
                    want_read,
                    want_write,
                })
            })
            .collect();
    }

    if entries.is_empty() {
        if !timeout.is_null() {
            let total_us = (*timeout).sec as u64 * 1_000_000 + (*timeout).msec as u64 * 1000;
            if total_us > 0 {
                std::thread::sleep(std::time::Duration::from_micros(total_us));
            }
        }
        return 0;
    }

    // ------------------------------------------------------------------
    // Step 2: build fd_sets and call select()
    // ------------------------------------------------------------------

    let mut read_fds: libc::fd_set = std::mem::zeroed();
    let mut write_fds: libc::fd_set = std::mem::zeroed();
    libc::FD_ZERO(&mut read_fds);
    libc::FD_ZERO(&mut write_fds);

    let mut max_fd: i32 = 0;
    for e in &entries {
        if e.want_read {
            libc::FD_SET(e.fd, &mut read_fds);
        }
        if e.want_write {
            libc::FD_SET(e.fd, &mut write_fds);
        }
        if e.fd >= max_fd {
            max_fd = e.fd + 1;
        }
    }

    let mut tv = if !timeout.is_null() {
        libc::timeval {
            tv_sec: (*timeout).sec as libc::time_t,
            tv_usec: ((*timeout).msec * 1000) as libc::suseconds_t,
        }
    } else {
        libc::timeval { tv_sec: 60, tv_usec: 0 }
    };

    let nready = libc::select(
        max_fd, &mut read_fds, &mut write_fds, std::ptr::null_mut(), &mut tv,
    );
    if nready <= 0 {
        return nready;
    }

    // ------------------------------------------------------------------
    // Step 3: for each ready fd, claim pending op under per-key lock,
    //         do I/O, release lock, fire callback, then clear processing.
    // ------------------------------------------------------------------

    let mut events_fired = 0i32;

    for e in &entries {
        let readable = e.want_read && libc::FD_ISSET(e.fd, &read_fds);
        let writable = e.want_write && libc::FD_ISSET(e.fd, &write_fds);
        if !readable && !writable {
            continue;
        }

        // Lock the key.  Use try_lock to avoid blocking if another
        // poll thread got there first.
        let mut ki = match (*e.key_ptr).lock.try_lock() {
            Ok(g) => g,
            Err(_) => continue,
        };
        if ki.closing || ki.fd < 0 || ki.processing {
            continue;
        }
        ki.processing = true;

        // --- Readable: accept ---
        if readable {
            if let Some(pa) = ki.pending_accept.take() {
                let mut slen: libc::socklen_t = if !pa.addrlen.is_null() {
                    *pa.addrlen as libc::socklen_t
                } else {
                    std::mem::size_of::<pj_sockaddr>() as _
                };
                let rp = if pa.remote.is_null() {
                    std::ptr::null_mut()
                } else {
                    pa.remote as *mut libc::sockaddr
                };
                let new_fd = libc::accept(ki.fd, rp, &mut slen);
                if new_fd >= 0 {
                    if !pa.new_sock.is_null() { *pa.new_sock = new_fd as pj_sock_t; }
                    if !pa.addrlen.is_null() { *pa.addrlen = slen as i32; }
                    if !pa.local.is_null() {
                        let mut ll: libc::socklen_t = std::mem::size_of::<pj_sockaddr>() as _;
                        libc::getsockname(new_fd, pa.local as *mut libc::sockaddr, &mut ll);
                    }
                    let cb = ki.cb.on_accept_complete;
                    drop(ki); // release lock before callback
                    events_fired += 1;
                    if let Some(f) = cb {
                        f(e.key_ptr as *mut pj_ioqueue_key_t, pa.op_key,
                          new_fd as pj_sock_t, PJ_SUCCESS);
                    }
                    // Clear processing flag
                    let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                    ki2.processing = false;
                    continue; // done with this entry
                } else {
                    let err = get_errno();
                    if is_wouldblock(err) {
                        ki.pending_accept = Some(pa);
                    } else {
                        let cb = ki.cb.on_accept_complete;
                        drop(ki);
                        events_fired += 1;
                        if let Some(f) = cb {
                            f(e.key_ptr as *mut pj_ioqueue_key_t, pa.op_key,
                              -1 as pj_sock_t, 120000 + err);
                        }
                        let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                        ki2.processing = false;
                        continue;
                    }
                }
            }

            // --- Readable: recv ---
            if !ki.pending_reads.is_empty() {
                let pr = ki.pending_reads.remove(0);
                let mut slen: libc::socklen_t = if !pr.fromlen.is_null() {
                    *pr.fromlen as libc::socklen_t
                } else {
                    std::mem::size_of::<pj_sockaddr>() as _
                };

                let n = if !pr.from.is_null() {
                    libc::recvfrom(
                        ki.fd, pr.buf, pr.len as usize, pr.flags as i32,
                        pr.from as *mut libc::sockaddr, &mut slen,
                    )
                } else {
                    libc::recv(ki.fd, pr.buf, pr.len as usize, pr.flags as i32)
                };

                if n >= 0 {
                    if !pr.fromlen.is_null() { *pr.fromlen = slen as i32; }
                    let cb = ki.cb.on_read_complete;
                    drop(ki);
                    events_fired += 1;
                    if let Some(f) = cb {
                        f(e.key_ptr as *mut pj_ioqueue_key_t, pr.op_key, n as isize);
                    }
                    let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                    ki2.processing = false;
                    continue;
                } else {
                    let err = get_errno();
                    if is_wouldblock(err) {
                        ki.pending_reads.insert(0, pr);
                    } else {
                        let cb = ki.cb.on_read_complete;
                        drop(ki);
                        events_fired += 1;
                        if let Some(f) = cb {
                            f(e.key_ptr as *mut pj_ioqueue_key_t, pr.op_key,
                              -(err as isize));
                        }
                        let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                        ki2.processing = false;
                        continue;
                    }
                }
            }
        }

        // --- Writable: connect ---
        if writable {
            if ki.connecting {
                ki.connecting = false;
                let mut err: i32 = 0;
                let mut errlen: libc::socklen_t = std::mem::size_of::<i32>() as _;
                libc::getsockopt(
                    ki.fd, libc::SOL_SOCKET, libc::SO_ERROR,
                    &mut err as *mut _ as *mut libc::c_void, &mut errlen,
                );
                let status = if err == 0 { PJ_SUCCESS } else { 120000 + err };
                let cb = ki.cb.on_connect_complete;
                drop(ki);
                events_fired += 1;
                if let Some(f) = cb {
                    f(e.key_ptr as *mut pj_ioqueue_key_t, status);
                }
                let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                ki2.processing = false;
                continue;
            } else if !ki.pending_writes.is_empty() {
                // --- Writable: send ---
                let pw = ki.pending_writes.remove(0);
                let n = if let Some((to, tolen)) = pw.to {
                    libc::sendto(
                        ki.fd, pw.buf, pw.len as usize, pw.flags as i32,
                        to as *const libc::sockaddr, tolen as libc::socklen_t,
                    )
                } else {
                    libc::send(ki.fd, pw.buf, pw.len as usize, pw.flags as i32)
                };

                if n >= 0 {
                    let cb = ki.cb.on_write_complete;
                    drop(ki);
                    events_fired += 1;
                    if let Some(f) = cb {
                        f(e.key_ptr as *mut pj_ioqueue_key_t, pw.op_key, n as isize);
                    }
                    let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                    ki2.processing = false;
                    continue;
                } else {
                    let err = get_errno();
                    if is_wouldblock(err) {
                        ki.pending_writes.insert(0, pw);
                    } else {
                        let cb = ki.cb.on_write_complete;
                        drop(ki);
                        events_fired += 1;
                        if let Some(f) = cb {
                            f(e.key_ptr as *mut pj_ioqueue_key_t, pw.op_key,
                              -(err as isize));
                        }
                        let mut ki2 = (*e.key_ptr).lock.lock().unwrap();
                        ki2.processing = false;
                        continue;
                    }
                }
            }
        }

        // No completion was produced (e.g. wouldblock on both read/write).
        ki.processing = false;
        drop(ki);
    }

    events_fired
}

// ---------------------------------------------------------------------------
// Is pending / Post completion
// ---------------------------------------------------------------------------

pub unsafe fn ioqueue_is_pending_impl(
    key: *mut pj_ioqueue_key_t,
    _op_key: *mut pj_ioqueue_op_key_t,
) -> pj_bool_t {
    if key.is_null() {
        return PJ_FALSE;
    }
    let key_ptr = key as *mut IoKey;
    let ki = (*key_ptr).lock.lock().unwrap();
    if !ki.pending_reads.is_empty()
        || !ki.pending_writes.is_empty()
        || ki.pending_accept.is_some()
        || ki.connecting
    {
        PJ_TRUE
    } else {
        PJ_FALSE
    }
}

pub unsafe fn ioqueue_post_completion_impl(
    key: *mut pj_ioqueue_key_t,
    op_key: *mut pj_ioqueue_op_key_t,
    bytes_status: isize,
) -> pj_status_t {
    if key.is_null() {
        return PJ_EINVAL;
    }
    let key_ptr = key as *mut IoKey;
    let cb = {
        let ki = (*key_ptr).lock.lock().unwrap();
        ki.cb.on_read_complete
    };
    if let Some(f) = cb {
        f(key, op_key, bytes_status);
    }
    PJ_SUCCESS
}
