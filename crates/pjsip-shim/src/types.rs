//! C-compatible type definitions matching pjproject's layout.
//!
//! These `#[repr(C)]` structs allow C programs to use our Rust SIP stack
//! as a drop-in replacement for pjproject/pjsip.

use std::mem::ManuallyDrop;

// ---------------------------------------------------------------------------
// Status codes
// ---------------------------------------------------------------------------

/// pj_status_t -- the universal return type for pjproject APIs.
pub type pj_status_t = i32;
pub const PJ_SUCCESS: pj_status_t = 0;
pub const PJ_EINVAL: pj_status_t = 70014;
pub const PJ_ENOMEM: pj_status_t = 70015;
pub const PJ_ENOTFOUND: pj_status_t = 70018;
pub const PJ_ETOOMANY: pj_status_t = 70027;
pub const PJ_EEOF: pj_status_t = 70028;
pub const PJ_EBUSY: pj_status_t = 70029;
pub const PJ_EINVALIDOP: pj_status_t = 70030;

/// Boolean type used by pjproject.
pub type pj_bool_t = i32;
pub const PJ_TRUE: pj_bool_t = 1;
pub const PJ_FALSE: pj_bool_t = 0;

// ---------------------------------------------------------------------------
// pj_str_t -- length-delimited string (NOT null-terminated)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pj_str_t {
    pub ptr: *mut libc::c_char,
    pub slen: isize,
}

impl pj_str_t {
    pub const EMPTY: pj_str_t = pj_str_t {
        ptr: std::ptr::null_mut(),
        slen: 0,
    };

    /// Convert to a Rust `&str` (unsafe: caller must guarantee validity).
    pub unsafe fn as_str(&self) -> &str {
        if self.ptr.is_null() || self.slen <= 0 {
            ""
        } else {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                self.ptr as *const u8,
                self.slen as usize,
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// Opaque pool handle.  The real data is a `Box<PoolInner>` behind this pointer.
#[repr(C)]
pub struct pj_pool_t {
    _opaque: [u8; 0],
}

/// Opaque pool factory.
#[repr(C)]
pub struct pj_pool_factory {
    _opaque: [u8; 0],
}

/// Opaque caching pool (wraps a pool factory).
#[repr(C)]
pub struct pj_caching_pool {
    pub factory: pj_pool_factory,
    _pad: [u8; 256], // padding so C code that takes sizeof() doesn't crash
}

// ---------------------------------------------------------------------------
// SIP method
// ---------------------------------------------------------------------------

/// pjsip_method_e enum values.
pub const PJSIP_INVITE_METHOD: i32 = 0;
pub const PJSIP_CANCEL_METHOD: i32 = 1;
pub const PJSIP_ACK_METHOD: i32 = 2;
pub const PJSIP_BYE_METHOD: i32 = 3;
pub const PJSIP_REGISTER_METHOD: i32 = 4;
pub const PJSIP_OPTIONS_METHOD: i32 = 5;
pub const PJSIP_OTHER_METHOD: i32 = 6;

#[repr(C)]
pub struct pjsip_method {
    pub id: i32,
    pub name: pj_str_t,
}

// ---------------------------------------------------------------------------
// URI types
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct pjsip_uri {
    pub vptr: *const libc::c_void,
}

/// SIP URI fields matching pjproject's pjsip_sip_uri.
#[repr(C)]
pub struct pjsip_sip_uri {
    pub vptr: *const libc::c_void,
    pub scheme: pj_str_t,
    pub user: pj_str_t,
    pub passwd: pj_str_t,
    pub host: pj_str_t,
    pub port: i32,
    pub transport_param: pj_str_t,
    pub user_param: pj_str_t,
    pub method_param: pj_str_t,
    pub ttl_param: i32,
    pub lr_param: i32,
    pub maddr_param: pj_str_t,
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

/// Header type constants matching pjsip_hdr_e.
pub const PJSIP_H_VIA: i32 = 1;
pub const PJSIP_H_FROM: i32 = 2;
pub const PJSIP_H_TO: i32 = 3;
pub const PJSIP_H_CALL_ID: i32 = 4;
pub const PJSIP_H_CSEQ: i32 = 5;
pub const PJSIP_H_CONTACT: i32 = 6;
pub const PJSIP_H_CONTENT_TYPE: i32 = 7;
pub const PJSIP_H_CONTENT_LENGTH: i32 = 8;
pub const PJSIP_H_ROUTE: i32 = 9;
pub const PJSIP_H_RECORD_ROUTE: i32 = 10;
pub const PJSIP_H_MAX_FORWARDS: i32 = 11;
pub const PJSIP_H_EXPIRES: i32 = 12;
pub const PJSIP_H_REQUIRE: i32 = 13;
pub const PJSIP_H_SUPPORTED: i32 = 14;
pub const PJSIP_H_OTHER: i32 = 63;

#[repr(C)]
pub struct pjsip_hdr {
    pub prev: *mut pjsip_hdr,
    pub next: *mut pjsip_hdr,
    pub htype: i32,
    pub name: pj_str_t,
    pub sname: pj_str_t,
}

/// Generic string header (e.g. User-Agent, Server).
#[repr(C)]
pub struct pjsip_generic_string_hdr {
    pub prev: *mut pjsip_hdr,
    pub next: *mut pjsip_hdr,
    pub htype: i32,
    pub name: pj_str_t,
    pub sname: pj_str_t,
    pub hvalue: pj_str_t,
}

// ---------------------------------------------------------------------------
// SIP message
// ---------------------------------------------------------------------------

/// Message type.
pub const PJSIP_REQUEST_MSG: i32 = 0;
pub const PJSIP_RESPONSE_MSG: i32 = 1;

#[repr(C)]
pub struct pjsip_request_line {
    pub method: pjsip_method,
    pub uri: *mut pjsip_uri,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pjsip_status_line {
    pub code: i32,
    pub reason: pj_str_t,
}

#[repr(C)]
pub union pjsip_msg_line {
    pub req: ManuallyDrop<pjsip_request_line>,
    pub status: ManuallyDrop<pjsip_status_line>,
}

#[repr(C)]
pub struct pjsip_media_type {
    pub type_: pj_str_t,
    pub subtype: pj_str_t,
}

#[repr(C)]
pub struct pjsip_msg_body {
    pub content_type: pjsip_media_type,
    pub data: *mut libc::c_void,
    pub len: u32,
}

#[repr(C)]
pub struct pjsip_msg {
    pub msg_type: i32, // PJSIP_REQUEST_MSG or PJSIP_RESPONSE_MSG
    pub line: pjsip_msg_line,
    pub hdr: pjsip_hdr, // linked list sentinel
    pub body: *mut pjsip_msg_body,
}

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

/// Opaque endpoint handle.
#[repr(C)]
pub struct pjsip_endpoint {
    _opaque: [u8; 0],
}

// ---------------------------------------------------------------------------
// Sockaddr
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pj_sockaddr_in {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: pj_in_addr,
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pj_in_addr {
    pub s_addr: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pj_sockaddr_in6 {
    pub sin6_family: u16,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: pj_in6_addr,
    pub sin6_scope_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct pj_in6_addr {
    pub s6_addr: [u8; 16],
}

#[repr(C)]
pub union pj_sockaddr {
    pub addr: pj_sockaddr_in,
    pub ipv6: pj_sockaddr_in6,
}

/// Address family constants.
pub const PJ_AF_INET: u16 = 2;
#[cfg(target_os = "macos")]
pub const PJ_AF_INET6: u16 = 30;
#[cfg(target_os = "linux")]
pub const PJ_AF_INET6: u16 = 10;
