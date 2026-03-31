//! pj_sock -- socket operations.
//!
//! Wraps libc socket functions to provide the pjlib socket API.

use crate::types::*;

// ---------------------------------------------------------------------------
// Constants (exported as C symbols)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static PJ_AF_INET_VAL: i32 = PJ_AF_INET as i32;

#[cfg(target_os = "macos")]
#[no_mangle]
pub static PJ_AF_INET6_VAL: i32 = 30;
#[cfg(target_os = "linux")]
#[no_mangle]
pub static PJ_AF_INET6_VAL: i32 = 10;

#[no_mangle]
pub static PJ_SOCK_STREAM_VAL: i32 = libc::SOCK_STREAM;
#[no_mangle]
pub static PJ_SOCK_DGRAM_VAL: i32 = libc::SOCK_DGRAM;
#[no_mangle]
pub static PJ_SOL_SOCKET_VAL: i32 = libc::SOL_SOCKET;
#[no_mangle]
pub static PJ_SO_REUSEADDR_VAL: i32 = libc::SO_REUSEADDR;
#[no_mangle]
pub static PJ_SO_RCVBUF_VAL: i32 = libc::SO_RCVBUF;
#[no_mangle]
pub static PJ_SO_SNDBUF_VAL: i32 = libc::SO_SNDBUF;
#[no_mangle]
pub static PJ_IPPROTO_TCP_VAL: i32 = libc::IPPROTO_TCP;
#[no_mangle]
pub static PJ_IPPROTO_UDP_VAL: i32 = libc::IPPROTO_UDP;
#[no_mangle]
pub static PJ_INVALID_SOCKET: i64 = -1;

/// pj_sock_t is just an integer file descriptor.
pub type pj_sock_t = i64;

// ---------------------------------------------------------------------------
// Socket operations
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_sock_socket(
    family: i32,
    sock_type: i32,
    protocol: i32,
    sock: *mut pj_sock_t,
) -> pj_status_t {
    if sock.is_null() {
        return PJ_EINVAL;
    }
    let fd = libc::socket(family, sock_type, protocol);
    if fd < 0 {
        *sock = -1;
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *sock = fd as i64;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_close(sock: pj_sock_t) -> pj_status_t {
    if sock < 0 {
        return PJ_EINVAL;
    }
    if libc::close(sock as i32) == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_bind(
    sock: pj_sock_t,
    addr: *const pj_sockaddr,
    addrlen: i32,
) -> pj_status_t {
    if addr.is_null() || sock < 0 {
        return PJ_EINVAL;
    }
    let rc = libc::bind(
        sock as i32,
        addr as *const libc::sockaddr,
        addrlen as libc::socklen_t,
    );
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err // PJ_STATUS_FROM_OS(errno)
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_listen(sock: pj_sock_t, backlog: i32) -> pj_status_t {
    if sock < 0 {
        return PJ_EINVAL;
    }
    if libc::listen(sock as i32, backlog) == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err // PJ_STATUS_FROM_OS(errno)
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_accept(
    sock: pj_sock_t,
    new_sock: *mut pj_sock_t,
    addr: *mut pj_sockaddr,
    addrlen: *mut i32,
) -> pj_status_t {
    if sock < 0 || new_sock.is_null() {
        return PJ_EINVAL;
    }
    let mut len: libc::socklen_t = if !addrlen.is_null() {
        *addrlen as libc::socklen_t
    } else {
        std::mem::size_of::<pj_sockaddr>() as libc::socklen_t
    };
    let fd = libc::accept(
        sock as i32,
        if addr.is_null() {
            std::ptr::null_mut()
        } else {
            addr as *mut libc::sockaddr
        },
        &mut len,
    );
    if fd < 0 {
        *new_sock = -1;
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *new_sock = fd as i64;
    if !addrlen.is_null() {
        *addrlen = len as i32;
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_connect(
    sock: pj_sock_t,
    addr: *const pj_sockaddr,
    addrlen: i32,
) -> pj_status_t {
    if sock < 0 || addr.is_null() {
        return PJ_EINVAL;
    }
    let rc = libc::connect(
        sock as i32,
        addr as *const libc::sockaddr,
        addrlen as libc::socklen_t,
    );
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err // PJ_STATUS_FROM_OS(errno)
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_send(
    sock: pj_sock_t,
    buf: *const libc::c_void,
    len: *mut isize,
    flags: i32,
) -> pj_status_t {
    if sock < 0 || buf.is_null() || len.is_null() {
        return PJ_EINVAL;
    }
    let sent = libc::send(sock as i32, buf, *len as usize, flags);
    if sent < 0 {
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *len = sent as isize;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_recv(
    sock: pj_sock_t,
    buf: *mut libc::c_void,
    len: *mut isize,
    flags: i32,
) -> pj_status_t {
    if sock < 0 || buf.is_null() || len.is_null() {
        return PJ_EINVAL;
    }
    let recvd = libc::recv(sock as i32, buf, *len as usize, flags);
    if recvd < 0 {
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *len = recvd as isize;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_sendto(
    sock: pj_sock_t,
    buf: *const libc::c_void,
    len: *mut isize,
    flags: i32,
    to: *const pj_sockaddr,
    tolen: i32,
) -> pj_status_t {
    if sock < 0 || buf.is_null() || len.is_null() || to.is_null() {
        return PJ_EINVAL;
    }
    let sent = libc::sendto(
        sock as i32,
        buf,
        *len as usize,
        flags,
        to as *const libc::sockaddr,
        tolen as libc::socklen_t,
    );
    if sent < 0 {
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *len = sent as isize;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_recvfrom(
    sock: pj_sock_t,
    buf: *mut libc::c_void,
    len: *mut isize,
    flags: i32,
    from: *mut pj_sockaddr,
    fromlen: *mut i32,
) -> pj_status_t {
    if sock < 0 || buf.is_null() || len.is_null() {
        return PJ_EINVAL;
    }
    let mut slen: libc::socklen_t = if !fromlen.is_null() {
        *fromlen as libc::socklen_t
    } else {
        std::mem::size_of::<pj_sockaddr>() as libc::socklen_t
    };
    let recvd = libc::recvfrom(
        sock as i32,
        buf,
        *len as usize,
        flags,
        if from.is_null() {
            std::ptr::null_mut()
        } else {
            from as *mut libc::sockaddr
        },
        &mut slen,
    );
    if recvd < 0 {
        let err = *libc::__error();
        return 120000 + err; // PJ_STATUS_FROM_OS(errno)
    }
    *len = recvd as isize;
    if !fromlen.is_null() {
        *fromlen = slen as i32;
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_shutdown(sock: pj_sock_t, how: i32) -> pj_status_t {
    if sock < 0 {
        return PJ_EINVAL;
    }
    if libc::shutdown(sock as i32, how) == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_setsockopt(
    sock: pj_sock_t,
    level: i32,
    optname: i32,
    optval: *const libc::c_void,
    optlen: i32,
) -> pj_status_t {
    if sock < 0 {
        return PJ_EINVAL;
    }
    let rc = libc::setsockopt(
        sock as i32,
        level,
        optname,
        optval,
        optlen as libc::socklen_t,
    );
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_getsockopt(
    sock: pj_sock_t,
    level: i32,
    optname: i32,
    optval: *mut libc::c_void,
    optlen: *mut i32,
) -> pj_status_t {
    if sock < 0 || optval.is_null() || optlen.is_null() {
        return PJ_EINVAL;
    }
    let mut len = *optlen as libc::socklen_t;
    let rc = libc::getsockopt(sock as i32, level, optname, optval, &mut len);
    *optlen = len as i32;
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_getsockname(
    sock: pj_sock_t,
    addr: *mut pj_sockaddr,
    addrlen: *mut i32,
) -> pj_status_t {
    if sock < 0 || addr.is_null() || addrlen.is_null() {
        return PJ_EINVAL;
    }
    let mut len = *addrlen as libc::socklen_t;
    let rc = libc::getsockname(sock as i32, addr as *mut libc::sockaddr, &mut len);
    *addrlen = len as i32;
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_getpeername(
    sock: pj_sock_t,
    addr: *mut pj_sockaddr,
    addrlen: *mut i32,
) -> pj_status_t {
    if sock < 0 || addr.is_null() || addrlen.is_null() {
        return PJ_EINVAL;
    }
    let mut len = *addrlen as libc::socklen_t;
    let rc = libc::getpeername(sock as i32, addr as *mut libc::sockaddr, &mut len);
    *addrlen = len as i32;
    if rc == 0 {
        PJ_SUCCESS
    } else {
        let err = *libc::__error();
        120000 + err
    }
}

// ---------------------------------------------------------------------------
// Sockaddr init helpers
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_in_init(
    addr: *mut pj_sockaddr_in,
    host: *const pj_str_t,
    port: u16,
) -> pj_status_t {
    if addr.is_null() {
        return PJ_EINVAL;
    }
    std::ptr::write_bytes(addr as *mut u8, 0, std::mem::size_of::<pj_sockaddr_in>());
    (*addr).sin_family = PJ_AF_INET;
    (*addr).sin_port = port.to_be();
    if !host.is_null() && (*host).slen > 0 {
        let text = (*host).as_str();
        if let Some(ipv4) = parse_ipv4_simple(text) {
            (*addr).sin_addr.s_addr = ipv4;
        }
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_in6_init(
    addr: *mut pj_sockaddr_in6,
    host: *const pj_str_t,
    port: u16,
) -> pj_status_t {
    if addr.is_null() {
        return PJ_EINVAL;
    }
    std::ptr::write_bytes(addr as *mut u8, 0, std::mem::size_of::<pj_sockaddr_in6>());
    (*addr).sin6_family = PJ_AF_INET6;
    (*addr).sin6_port = port.to_be();
    // IPv6 address parsing is handled by the sockaddr module
    let _ = host;
    PJ_SUCCESS
}

fn parse_ipv4_simple(s: &str) -> Option<u32> {
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

// ---------------------------------------------------------------------------
// Address conversion
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_inet_aton(
    cp: *const pj_str_t,
    inp: *mut pj_in_addr,
) -> i32 {
    if cp.is_null() || inp.is_null() {
        return 0;
    }
    let text = (*cp).as_str();
    match parse_ipv4_simple(text) {
        Some(addr) => {
            (*inp).s_addr = addr;
            1 // nonzero = success
        }
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_inet_ntoa(addr: pj_in_addr) -> *const libc::c_char {
    // Use a thread-local buffer. The UnsafeCell avoids RefCell overhead and
    // allows returning a pointer directly.
    use std::cell::UnsafeCell;
    thread_local! {
        static BUF: UnsafeCell<[u8; 20]> = const { UnsafeCell::new([0u8; 20]) };
    }
    let ip = u32::from_be(addr.s_addr);
    let a = (ip >> 24) & 0xFF;
    let b = (ip >> 16) & 0xFF;
    let c = (ip >> 8) & 0xFF;
    let d = ip & 0xFF;
    let s = format!("{}.{}.{}.{}", a, b, c, d);

    BUF.with(|cell| {
        let buf = &mut *cell.get();
        let bytes = s.as_bytes();
        let len = bytes.len().min(buf.len() - 1);
        buf[..len].copy_from_slice(&bytes[..len]);
        buf[len] = 0; // null terminator
        buf.as_ptr() as *const libc::c_char
    })
}

// inet_pton/inet_ntop are not always in the `libc` crate, declare them via FFI.
extern "C" {
    fn inet_pton(af: i32, src: *const libc::c_char, dst: *mut libc::c_void) -> i32;
    fn inet_ntop(af: i32, src: *const libc::c_void, dst: *mut libc::c_char, size: libc::socklen_t) -> *const libc::c_char;
}

#[no_mangle]
pub unsafe extern "C" fn pj_inet_pton(
    af: i32,
    src: *const pj_str_t,
    dst: *mut libc::c_void,
) -> pj_status_t {
    if src.is_null() || dst.is_null() {
        return PJ_EINVAL;
    }
    let text = (*src).as_str();
    // Need null-terminated string
    let mut buf = text.as_bytes().to_vec();
    buf.push(0);
    let rc = inet_pton(af, buf.as_ptr() as *const _, dst);
    if rc == 1 { PJ_SUCCESS } else { PJ_EINVAL }
}

#[no_mangle]
pub unsafe extern "C" fn pj_inet_ntop(
    af: i32,
    src: *const libc::c_void,
    dst: *mut libc::c_char,
    size: i32,
) -> pj_status_t {
    if src.is_null() || dst.is_null() || size <= 0 {
        return PJ_EINVAL;
    }
    let result = inet_ntop(af, src, dst, size as libc::socklen_t);
    if result.is_null() { PJ_EINVAL } else { PJ_SUCCESS }
}

// ---------------------------------------------------------------------------
// Hostname / address info
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_gethostname() -> *const pj_str_t {
    static mut HOST_STR: pj_str_t = pj_str_t {
        ptr: std::ptr::null_mut(),
        slen: 0,
    };
    static mut HOST_BUF: [u8; 256] = [0u8; 256];

    if HOST_STR.ptr.is_null() {
        let rc = libc::gethostname(HOST_BUF.as_mut_ptr() as *mut _, 255);
        if rc == 0 {
            let len = libc::strlen(HOST_BUF.as_ptr() as *const _);
            HOST_STR.ptr = HOST_BUF.as_mut_ptr() as *mut _;
            HOST_STR.slen = len as isize;
        }
    }
    std::ptr::addr_of!(HOST_STR)
}

#[no_mangle]
pub unsafe extern "C" fn pj_gethostip(
    af: i32,
    addr: *mut pj_sockaddr,
) -> pj_status_t {
    if addr.is_null() {
        return PJ_EINVAL;
    }
    // Return 127.0.0.1 as a fallback
    std::ptr::write_bytes(addr as *mut u8, 0, std::mem::size_of::<pj_sockaddr>());
    if af == PJ_AF_INET as i32 || af == 0 {
        (*addr).addr.sin_family = PJ_AF_INET;
        (*addr).addr.sin_addr.s_addr = 0x0100007f; // 127.0.0.1 in network byte order
    } else {
        (*addr).ipv6.sin6_family = PJ_AF_INET6;
        (*addr).ipv6.sin6_addr.s6_addr[15] = 1; // ::1
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_getdefaultipinterface(
    af: i32,
    addr: *mut pj_sockaddr,
) -> pj_status_t {
    pj_gethostip(af, addr)
}

/// pj_addrinfo -- matches the C layout: char ai_canonname[PJ_MAX_HOSTNAME]
/// followed by pj_sockaddr ai_addr.  PJ_MAX_HOSTNAME is 254.
const PJ_MAX_HOSTNAME: usize = 254;

#[repr(C)]
pub struct pj_addrinfo {
    pub ai_canonname: [u8; PJ_MAX_HOSTNAME],
    pub ai_addr: pj_sockaddr,
}

#[no_mangle]
pub unsafe extern "C" fn pj_getaddrinfo(
    af: i32,
    name: *const pj_str_t,
    count: *mut u32,
    ai: *mut pj_addrinfo,
) -> pj_status_t {
    if name.is_null() || count.is_null() || ai.is_null() || *count == 0 {
        return PJ_EINVAL;
    }
    // Simplified: just return one address
    let text = (*name).as_str();
    // Try parsing as IP first
    if let Some(ipv4) = parse_ipv4_simple(text) {
        if af == PJ_AF_INET as i32 || af == 0 {
            std::ptr::write_bytes(ai as *mut u8, 0, std::mem::size_of::<pj_addrinfo>());
            (*ai).ai_addr.addr.sin_family = PJ_AF_INET;
            (*ai).ai_addr.addr.sin_addr.s_addr = ipv4;
            // Copy name into ai_canonname buffer
            let name_bytes = (*name).as_str().as_bytes();
            let copy_len = name_bytes.len().min(PJ_MAX_HOSTNAME - 1);
            std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), (*ai).ai_canonname.as_mut_ptr(), copy_len);
            *count = 1;
            return PJ_SUCCESS;
        }
    }
    // Fallback to 127.0.0.1
    std::ptr::write_bytes(ai as *mut u8, 0, std::mem::size_of::<pj_addrinfo>());
    (*ai).ai_addr.addr.sin_family = PJ_AF_INET;
    (*ai).ai_addr.addr.sin_addr.s_addr = 0x0100007f; // 127.0.0.1
    // Copy name into ai_canonname buffer
    let name_bytes = (*name).as_str().as_bytes();
    let copy_len = name_bytes.len().min(PJ_MAX_HOSTNAME - 1);
    std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), (*ai).ai_canonname.as_mut_ptr(), copy_len);
    *count = 1;
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Select (fd_set operations)
// ---------------------------------------------------------------------------
//
// We compile the C ioqueue with FD_SETSIZE=2048 so that select() can handle
// fd values >= 1024 (the default macOS limit). The Rust `libc` crate's fd_set
// is fixed at 1024, so we implement our own raw bitmap operations here.
//
// On macOS, fd_set is an array of i32 where bit N of the bitmap represents
// fd N. With FD_SETSIZE=2048, fd_set is 256 bytes (2048 bits).
// Our PJ_FD_SET_SIZE must match what we pass to cc::Build.

/// Our custom FD_SETSIZE, matching the -DFD_SETSIZE=2048 in build.rs.
const PJ_FD_SET_SIZE: usize = 2048;

/// Number of bits per i32 word.
const NFDBITS: usize = 32;

/// Number of i32 words in our fd_set bitmap.
const FD_SET_WORDS: usize = PJ_FD_SET_SIZE / NFDBITS; // 64

/// Raw fd_set bitmap matching the macOS `struct fd_set { __int32_t fds_bits[N]; }`
/// layout when compiled with FD_SETSIZE=2048.
#[repr(C)]
pub struct RawFdSet {
    fds_bits: [i32; FD_SET_WORDS],
}

/// pj_fd_set_t -- the C pjlib type is `pj_sock_t data[PJ_IOQUEUE_MAX_HANDLES+4]`
/// (= 8224 bytes). We don't need to match that layout exactly because pj_fd_set_t
/// is always accessed through our PJ_FD_* functions and pj_sock_select; the C
/// ioqueue code only stores it inline in pj_ioqueue_t which is allocated by
/// the compiled C code. Our Rust-side pj_fd_set_t is used only for pointer
/// casting, never for direct allocation of ioqueue structures.
#[repr(C)]
pub struct pj_fd_set_t {
    _data: [u8; std::mem::size_of::<RawFdSet>()],
}

/// Reinterpret a pj_fd_set_t pointer as our raw bitmap.
#[inline]
unsafe fn as_raw_mut(fdsetp: *mut pj_fd_set_t) -> *mut RawFdSet {
    fdsetp as *mut RawFdSet
}

#[inline]
unsafe fn as_raw(fdsetp: *const pj_fd_set_t) -> *const RawFdSet {
    fdsetp as *const RawFdSet
}

#[no_mangle]
pub unsafe extern "C" fn pj_FD_ZERO(fdsetp: *mut pj_fd_set_t) {
    if fdsetp.is_null() {
        return;
    }
    let raw = as_raw_mut(fdsetp);
    std::ptr::write_bytes(raw, 0, 1);
}

#[no_mangle]
pub unsafe extern "C" fn pj_FD_SET(fd: pj_sock_t, fdsetp: *mut pj_fd_set_t) {
    if fdsetp.is_null() || fd < 0 || (fd as usize) >= PJ_FD_SET_SIZE {
        return;
    }
    let raw = &mut *as_raw_mut(fdsetp);
    let fd = fd as usize;
    raw.fds_bits[fd / NFDBITS] |= 1i32 << (fd % NFDBITS);
}

#[no_mangle]
pub unsafe extern "C" fn pj_FD_CLR(fd: pj_sock_t, fdsetp: *mut pj_fd_set_t) {
    if fdsetp.is_null() || fd < 0 || (fd as usize) >= PJ_FD_SET_SIZE {
        return;
    }
    let raw = &mut *as_raw_mut(fdsetp);
    let fd = fd as usize;
    raw.fds_bits[fd / NFDBITS] &= !(1i32 << (fd % NFDBITS));
}

#[no_mangle]
pub unsafe extern "C" fn pj_FD_ISSET(fd: pj_sock_t, fdsetp: *const pj_fd_set_t) -> pj_bool_t {
    if fdsetp.is_null() || fd < 0 || (fd as usize) >= PJ_FD_SET_SIZE {
        return PJ_FALSE;
    }
    let raw = &*as_raw(fdsetp);
    let fd = fd as usize;
    if (raw.fds_bits[fd / NFDBITS] & (1i32 << (fd % NFDBITS))) != 0 {
        PJ_TRUE
    } else {
        PJ_FALSE
    }
}

// On macOS, the select() syscall is not limited to FD_SETSIZE=1024 --
// it processes fds 0..nfds-1 in the bitmap. The system FD_SET macros
// and fd_set type enforce the limit, but the syscall itself works with
// larger bitmaps as long as the buffer is big enough. We bypass libc's
// wrapper and call the syscall directly through libc::select, casting
// our RawFdSet to libc::fd_set. This is safe because:
// - Our RawFdSet (256 bytes) is larger than libc::fd_set (128 bytes)
// - select() only reads/writes the first ceil(nfds/8) bytes of the bitmap
// - nfds is always <= PJ_FD_SET_SIZE (2048)

extern "C" {
    /// On macOS, the default `select()` symbol enforces nfds <= FD_SETSIZE
    /// (1024) and returns EINVAL otherwise (UNIX conformance mode).
    /// We need the `select$DARWIN_EXTSN` variant which has no such limit.
    /// This is the symbol you get when compiling C with _DARWIN_UNLIMITED_SELECT
    /// or _DARWIN_C_SOURCE.
    #[cfg(target_os = "macos")]
    #[link_name = "select$DARWIN_EXTSN"]
    fn select_unlimited(
        nfds: i32,
        readfds: *mut RawFdSet,
        writefds: *mut RawFdSet,
        exceptfds: *mut RawFdSet,
        timeout: *mut libc::timeval,
    ) -> i32;

    #[cfg(not(target_os = "macos"))]
    #[link_name = "select"]
    fn select_unlimited(
        nfds: i32,
        readfds: *mut RawFdSet,
        writefds: *mut RawFdSet,
        exceptfds: *mut RawFdSet,
        timeout: *mut libc::timeval,
    ) -> i32;
}

#[no_mangle]
pub unsafe extern "C" fn pj_sock_select(
    nfds: i32,
    readfds: *mut pj_fd_set_t,
    writefds: *mut pj_fd_set_t,
    exceptfds: *mut pj_fd_set_t,
    timeout: *const crate::timer::pj_time_val,
) -> i32 {
    let mut tv = if !timeout.is_null() {
        libc::timeval {
            tv_sec: (*timeout).sec as libc::time_t,
            tv_usec: ((*timeout).msec * 1000) as libc::suseconds_t,
        }
    } else {
        libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        }
    };

    select_unlimited(
        nfds,
        if readfds.is_null() {
            std::ptr::null_mut()
        } else {
            readfds as *mut RawFdSet
        },
        if writefds.is_null() {
            std::ptr::null_mut()
        } else {
            writefds as *mut RawFdSet
        },
        if exceptfds.is_null() {
            std::ptr::null_mut()
        } else {
            exceptfds as *mut RawFdSet
        },
        if timeout.is_null() {
            std::ptr::null_mut()
        } else {
            &mut tv
        },
    )
}

// ---------------------------------------------------------------------------
// Socket address size
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_len(addr: *const pj_sockaddr) -> i32 {
    if addr.is_null() {
        return 0;
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        std::mem::size_of::<pj_sockaddr_in>() as i32
    } else if family == PJ_AF_INET6 {
        std::mem::size_of::<pj_sockaddr_in6>() as i32
    } else {
        std::mem::size_of::<pj_sockaddr>() as i32
    }
}

/// Set the address part of a sockaddr from a string.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_set_str_addr(
    af: i32,
    addr: *mut pj_sockaddr,
    str_addr: *const pj_str_t,
) -> pj_status_t {
    if addr.is_null() || str_addr.is_null() {
        return PJ_EINVAL;
    }
    let text = (*str_addr).as_str();
    let af = af as u16;
    if af == PJ_AF_INET as u16 {
        if let Some(ipv4) = parse_ipv4_simple(text) {
            (*addr).addr.sin_addr.s_addr = ipv4;
            return PJ_SUCCESS;
        }
    }
    PJ_EINVAL
}

/// Get the address part as a string.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_get_addr(addr: *const pj_sockaddr) -> *mut libc::c_void {
    if addr.is_null() {
        return std::ptr::null_mut();
    }
    let family = (*addr).addr.sin_family;
    if family == PJ_AF_INET {
        &(*addr).addr.sin_addr as *const _ as *mut _
    } else if family == PJ_AF_INET6 {
        &(*addr).ipv6.sin6_addr as *const _ as *mut _
    } else {
        std::ptr::null_mut()
    }
}

/// Compare two sockaddrs (family, address, port -- ignoring padding/sin_len).
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_cmp(
    addr1: *const pj_sockaddr,
    addr2: *const pj_sockaddr,
) -> i32 {
    if addr1.is_null() || addr2.is_null() {
        return if addr1.is_null() && addr2.is_null() { 0 } else { -1 };
    }
    let f1 = (*addr1).addr.sin_family;
    let f2 = (*addr2).addr.sin_family;

    // Compare address family
    if f1 < f2 {
        return -1;
    } else if f1 > f2 {
        return 1;
    }

    // Compare the address part only (sin_addr or sin6_addr)
    let a1 = pj_sockaddr_get_addr(addr1);
    let a2 = pj_sockaddr_get_addr(addr2);
    let addr_len = if f1 == PJ_AF_INET {
        std::mem::size_of::<pj_in_addr>()  // 4 bytes
    } else {
        std::mem::size_of::<pj_in6_addr>() // 16 bytes
    };
    let result = libc::memcmp(a1, a2, addr_len);
    if result != 0 {
        return result;
    }

    // Compare port
    let p1 = crate::sockaddr::pj_sockaddr_get_port(addr1) as i32;
    let p2 = crate::sockaddr::pj_sockaddr_get_port(addr2) as i32;
    if p1 < p2 {
        return -1;
    } else if p1 > p2 {
        return 1;
    }

    0
}

/// Copy a sockaddr.
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_cp(
    dst: *mut pj_sockaddr,
    src: *const pj_sockaddr,
) {
    if dst.is_null() || src.is_null() {
        return;
    }
    std::ptr::copy_nonoverlapping(
        src as *const u8,
        dst as *mut u8,
        std::mem::size_of::<pj_sockaddr>(),
    );
}

/// Synthesize IPv6 from IPv4 (stub -- just copies).
#[no_mangle]
pub unsafe extern "C" fn pj_sockaddr_synthesize(
    dst_af: i32,
    dst: *mut pj_sockaddr,
    src: *const pj_sockaddr,
) -> pj_status_t {
    if dst.is_null() || src.is_null() {
        return PJ_EINVAL;
    }
    let _ = dst_af;
    pj_sockaddr_cp(dst, src);
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// pj_sock_bind_in -- convenience bind with addr/port as u32/u16
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_sock_bind_in(
    sock: pj_sock_t,
    addr: u32,
    port: u16,
) -> pj_status_t {
    if sock < 0 {
        return PJ_EINVAL;
    }
    let mut sa: libc::sockaddr_in = std::mem::zeroed();
    sa.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
    sa.sin_family = libc::AF_INET as u8;
    sa.sin_port = port.to_be();
    sa.sin_addr.s_addr = addr;
    let rc = libc::bind(
        sock as i32,
        &sa as *const libc::sockaddr_in as *const libc::sockaddr,
        std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
    );
    if rc == 0 { PJ_SUCCESS } else { PJ_EINVAL }
}

// ---------------------------------------------------------------------------
// pj_sock_socketpair
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_sock_socketpair(
    family: i32,
    type_: i32,
    protocol: i32,
    sv: *mut [pj_sock_t; 2],
) -> pj_status_t {
    if sv.is_null() {
        return PJ_EINVAL;
    }

    // First try the OS socketpair
    let mut raw_sv: [i32; 2] = [0; 2];
    let rc = libc::socketpair(family, type_, protocol, raw_sv.as_mut_ptr());
    if rc == 0 {
        (*sv)[0] = raw_sv[0] as pj_sock_t;
        (*sv)[1] = raw_sv[1] as pj_sock_t;
        return PJ_SUCCESS;
    }

    // Fallback: manually create a connected socket pair (needed for AF_INET)
    socketpair_imp(family, type_, protocol, sv)
}

/// Manual socketpair implementation: create two sockets, bind/connect them.
/// This handles AF_INET (and AF_INET6) which libc::socketpair doesn't support.
unsafe fn socketpair_imp(
    family: i32,
    type_: i32,
    protocol: i32,
    sv: *mut [pj_sock_t; 2],
) -> pj_status_t {
    use crate::sockaddr::*;

    let mut lfd: pj_sock_t = -1;
    let mut cfd: pj_sock_t = -1;
    let mut sa: pj_sockaddr = std::mem::zeroed();
    let mut status: pj_status_t;

    // Create listen/server socket
    status = pj_sock_socket(family, type_, protocol, &mut lfd);
    if status != PJ_SUCCESS {
        return status;
    }

    // Init loopback address on port 0 (OS picks port)
    let loopback_str = b"127.0.0.1\0";
    let loopback = pj_str_t {
        ptr: loopback_str.as_ptr() as *mut _,
        slen: 9,
    };
    status = pj_sockaddr_init(family, &mut sa, &loopback, 0);
    if status != PJ_SUCCESS {
        pj_sock_close(lfd);
        return status;
    }

    let salen = pj_sockaddr_get_len(&sa);
    status = pj_sock_bind(lfd, &sa, salen);
    if status != PJ_SUCCESS {
        pj_sock_close(lfd);
        return status;
    }

    // Get the actual bound address (to learn the port)
    let mut salen_mut = salen;
    status = pj_sock_getsockname(lfd, &mut sa, &mut salen_mut);
    if status != PJ_SUCCESS {
        pj_sock_close(lfd);
        return status;
    }

    let sock_type_masked = type_ & 0xF;

    if sock_type_masked == libc::SOCK_STREAM {
        // TCP: listen, connect, accept
        status = pj_sock_listen(lfd, 1);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            return status;
        }

        // Create client socket and connect
        status = pj_sock_socket(family, type_, protocol, &mut cfd);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            return status;
        }

        status = pj_sock_connect(cfd, &sa, salen_mut);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            pj_sock_close(cfd);
            return status;
        }

        // Accept the connection
        let mut newfd: pj_sock_t = -1;
        status = pj_sock_accept(lfd, &mut newfd, std::ptr::null_mut(), std::ptr::null_mut());
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            pj_sock_close(cfd);
            return status;
        }
        pj_sock_close(lfd);
        (*sv)[0] = newfd;
        (*sv)[1] = cfd;
    } else {
        // UDP: connect both ends to each other
        status = pj_sock_socket(family, type_, protocol, &mut cfd);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            return status;
        }

        status = pj_sock_connect(cfd, &sa, salen_mut);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            pj_sock_close(cfd);
            return status;
        }

        // Get client's bound address
        let mut client_sa: pj_sockaddr = std::mem::zeroed();
        let mut client_salen = std::mem::size_of::<pj_sockaddr>() as i32;
        status = pj_sock_getsockname(cfd, &mut client_sa, &mut client_salen);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            pj_sock_close(cfd);
            return status;
        }

        // Connect server socket back to client
        status = pj_sock_connect(lfd, &client_sa, client_salen);
        if status != PJ_SUCCESS {
            pj_sock_close(lfd);
            pj_sock_close(cfd);
            return status;
        }

        (*sv)[0] = lfd;
        (*sv)[1] = cfd;
    }

    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Byte-order helpers
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_ntohs(v: u16) -> u16 {
    u16::from_be(v)
}

#[no_mangle]
pub unsafe extern "C" fn pj_htons(v: u16) -> u16 {
    v.to_be()
}

// ---------------------------------------------------------------------------
// pj_inet_addr / pj_inet_addr2 -- shorthand for parsing dotted-quad
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_inet_addr(cp: *const pj_str_t) -> pj_in_addr {
    let mut result = pj_in_addr { s_addr: 0 };
    if !cp.is_null() {
        let _ = pj_inet_aton(cp, &mut result);
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn pj_inet_addr2(cp: *const libc::c_char) -> pj_in_addr {
    let mut result = pj_in_addr { s_addr: 0 };
    if !cp.is_null() {
        let len = libc::strlen(cp);
        let s = pj_str_t { ptr: cp as *mut _, slen: len as isize };
        let _ = pj_inet_aton(&s, &mut result);
    }
    result
}

// ---------------------------------------------------------------------------
// OS error helpers
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_get_os_error() -> pj_status_t {
    let e = *libc::__error(); // macOS errno
    if e == 0 { PJ_SUCCESS } else { 120000 + e }
}

#[no_mangle]
pub unsafe extern "C" fn pj_get_netos_error() -> pj_status_t {
    pj_get_os_error()
}

#[no_mangle]
pub unsafe extern "C" fn pj_set_os_error(code: pj_status_t) {
    if code == PJ_SUCCESS {
        *libc::__error() = 0;
    } else if code >= 120000 {
        *libc::__error() = code - 120000;
    }
}

// NOTE: pj_ioqueue_get_os_handle is now provided by the compiled C ioqueue source.
