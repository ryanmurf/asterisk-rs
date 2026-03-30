//! C-exported socket constants.
//!
//! These are `#[no_mangle] pub static` values that pjlib-test references as
//! global symbols (e.g. `PJ_AF_INET`, `PJ_SOCK_STREAM`).
//!
//! They live in their own module so they don't shadow the `pub const` Rust
//! names from `types.rs` that the rest of the crate uses.

// Address families
#[no_mangle] pub static PJ_AF_INET:   i32 = 2;
#[no_mangle] pub static PJ_AF_UNIX:   i32 = 1;
#[no_mangle] pub static PJ_AF_UNSPEC: i32 = 0;

#[cfg(target_os = "macos")]
#[no_mangle] pub static PJ_AF_INET6:  i32 = 30;
#[cfg(target_os = "linux")]
#[no_mangle] pub static PJ_AF_INET6:  i32 = 10;

// Socket types
#[no_mangle] pub static PJ_SOCK_STREAM: i32 = 1;
#[no_mangle] pub static PJ_SOCK_DGRAM:  i32 = 2;

// Socket option levels
#[no_mangle] pub static PJ_SOL_SOCKET: i32 = libc::SOL_SOCKET;
#[no_mangle] pub static PJ_SOL_IP:     i32 = 0;   // IPPROTO_IP
#[no_mangle] pub static PJ_SOL_IPV6:   i32 = 41;  // IPPROTO_IPV6
#[no_mangle] pub static PJ_SOL_TCP:    i32 = 6;   // IPPROTO_TCP
#[no_mangle] pub static PJ_SOL_UDP:    i32 = 17;  // IPPROTO_UDP

// Socket options
#[no_mangle] pub static PJ_SO_REUSEADDR: i32 = libc::SO_REUSEADDR;
#[no_mangle] pub static PJ_SO_RCVBUF:    i32 = libc::SO_RCVBUF;
#[no_mangle] pub static PJ_SO_SNDBUF:    i32 = libc::SO_SNDBUF;
#[no_mangle] pub static PJ_SO_TYPE:      i32 = libc::SO_TYPE;
#[no_mangle] pub static PJ_TCP_NODELAY:  i32 = 1;  // TCP_NODELAY

// Message flags
#[no_mangle] pub static PJ_MSG_OOB:  i32 = libc::MSG_OOB;
#[no_mangle] pub static PJ_MSG_PEEK: i32 = libc::MSG_PEEK;

// ---------------------------------------------------------------------------
// FD_SET macro wrappers (uppercase PJ_ prefix, as referenced by pjlib-test)
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn PJ_FD_ZERO(fdsetp: *mut libc::c_void) {
    if fdsetp.is_null() {
        return;
    }
    libc::FD_ZERO(fdsetp as *mut libc::fd_set);
}

#[no_mangle]
pub unsafe extern "C" fn PJ_FD_SET(fd: i64, fdsetp: *mut libc::c_void) {
    if fdsetp.is_null() || fd < 0 {
        return;
    }
    libc::FD_SET(fd as i32, fdsetp as *mut libc::fd_set);
}

#[no_mangle]
pub unsafe extern "C" fn PJ_FD_ISSET(fd: i64, fdsetp: *const libc::c_void) -> i32 {
    if fdsetp.is_null() || fd < 0 {
        return 0;
    }
    if libc::FD_ISSET(fd as i32, fdsetp as *const libc::fd_set) {
        1
    } else {
        0
    }
}
