#![allow(non_camel_case_types)]

//! pjsip-shim: C-compatible shared library exposing the pjsip API.
//!
//! This crate produces a `.dylib` (macOS) or `.so` (Linux) that can be
//! linked by any C program as a drop-in replacement for pjproject's
//! libpjsip / libpj / libpjsip-ua libraries.
//!
//! The shim delegates to the Rust SIP stack in `asterisk-sip` while
//! presenting the exact C function signatures that pjproject consumers
//! expect (`pj_str`, `pj_pool_create`, `pjsip_parse_uri`, etc.).
//!
//! ## Modules
//!
//! - `types` -- `#[repr(C)]` struct definitions matching pjproject layout
//! - `pool`  -- pool-based memory allocator
//! - `string` -- `pj_str_t` string operations
//! - `sip_parser` -- SIP URI / message parsing (delegates to `asterisk_sip::parser`)
//! - `sip_message` -- header linked-list manipulation
//! - `sockaddr` -- socket address parsing/formatting
//! - `init` -- pj_init / pjsip_endpt_create / logging stubs

pub mod types;
pub mod pool;
pub mod string;
pub mod sip_parser;
pub mod sip_message;
pub mod sockaddr;
pub mod init;
pub mod test_framework;
pub mod list;
pub mod logging;
pub mod timer;
pub mod time;
pub mod threading;
pub mod socket;
pub mod atomic;
pub mod misc;

// Re-export everything so symbols appear in the shared library.
// The `#[no_mangle] pub unsafe extern "C"` functions in each module
// are automatically exported by the cdylib crate type.

#[cfg(test)]
mod tests {
    use super::*;
    use types::*;

    // -------------------------------------------------------------------
    // String tests
    // -------------------------------------------------------------------

    #[test]
    fn test_pj_str_roundtrip() {
        unsafe {
            let s = std::ffi::CString::new("hello").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            assert_eq!(string::pj_strlen(&pj), 5);
            assert_eq!(string::pj_strbuf(&pj), s.as_ptr() as *mut _);
        }
    }

    #[test]
    fn test_pj_str_null() {
        unsafe {
            let pj = string::pj_str(std::ptr::null_mut());
            assert_eq!(pj.slen, 0);
            assert!(pj.ptr.is_null());
        }
    }

    #[test]
    fn test_pj_strcmp2_equal() {
        unsafe {
            let s = std::ffi::CString::new("INVITE").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            let cmp = std::ffi::CString::new("INVITE").unwrap();
            assert_eq!(string::pj_strcmp2(&pj, cmp.as_ptr()), 0);
        }
    }

    #[test]
    fn test_pj_strcmp2_not_equal() {
        unsafe {
            let s = std::ffi::CString::new("INVITE").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            let cmp = std::ffi::CString::new("BYE").unwrap();
            assert_ne!(string::pj_strcmp2(&pj, cmp.as_ptr()), 0);
        }
    }

    #[test]
    fn test_pj_stricmp2_case_insensitive() {
        unsafe {
            let s = std::ffi::CString::new("invite").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            let cmp = std::ffi::CString::new("INVITE").unwrap();
            assert_eq!(string::pj_stricmp2(&pj, cmp.as_ptr()), 0);
        }
    }

    #[test]
    fn test_pj_strtol() {
        unsafe {
            let s = std::ffi::CString::new("12345").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            assert_eq!(string::pj_strtol(&pj), 12345);
        }
    }

    #[test]
    fn test_pj_strtol_negative() {
        unsafe {
            let s = std::ffi::CString::new("-42").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            assert_eq!(string::pj_strtol(&pj), -42);
        }
    }

    #[test]
    fn test_pj_strset() {
        unsafe {
            let mut pj = pj_str_t::EMPTY;
            let s = std::ffi::CString::new("test").unwrap();
            string::pj_strset(&mut pj, s.as_ptr() as *mut _, 4);
            assert_eq!(pj.slen, 4);
        }
    }

    #[test]
    fn test_pj_strcmp_equal() {
        unsafe {
            let a = std::ffi::CString::new("abc").unwrap();
            let b = std::ffi::CString::new("abc").unwrap();
            let pa = string::pj_str(a.as_ptr() as *mut _);
            let pb = string::pj_str(b.as_ptr() as *mut _);
            assert_eq!(string::pj_strcmp(&pa, &pb), 0);
        }
    }

    #[test]
    fn test_pj_stricmp_different_case() {
        unsafe {
            let a = std::ffi::CString::new("Hello").unwrap();
            let b = std::ffi::CString::new("hELLO").unwrap();
            let pa = string::pj_str(a.as_ptr() as *mut _);
            let pb = string::pj_str(b.as_ptr() as *mut _);
            assert_eq!(string::pj_stricmp(&pa, &pb), 0);
        }
    }

    #[test]
    fn test_pj_strchr_found() {
        unsafe {
            let s = std::ffi::CString::new("hello@world").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            let result = string::pj_strchr(&pj, b'@' as i32);
            assert!(!result.is_null());
            assert_eq!(*result as u8, b'@');
        }
    }

    #[test]
    fn test_pj_strchr_not_found() {
        unsafe {
            let s = std::ffi::CString::new("hello").unwrap();
            let pj = string::pj_str(s.as_ptr() as *mut _);
            let result = string::pj_strchr(&pj, b'@' as i32);
            assert!(result.is_null());
        }
    }

    // -------------------------------------------------------------------
    // Pool tests
    // -------------------------------------------------------------------

    #[test]
    fn test_pool_alloc_and_release() {
        unsafe {
            let name = b"test\0".as_ptr() as *const libc::c_char;
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                name,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            assert!(!p.is_null());

            let mem = pool::pj_pool_alloc(p, 100);
            assert!(!mem.is_null());

            // Verify zero-fill
            let bytes = std::slice::from_raw_parts(mem as *const u8, 100);
            assert!(bytes.iter().all(|&b| b == 0));

            let mem2 = pool::pj_pool_alloc(p, 200);
            assert!(!mem2.is_null());
            assert_ne!(mem, mem2);

            // Check used size
            let used = pool::pj_pool_get_used_size(p);
            assert!(used >= 300);

            pool::pj_pool_release(p); // should not crash
        }
    }

    #[test]
    fn test_pool_zalloc() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"ztest\0".as_ptr() as *const _,
                1024,
                1024,
                std::ptr::null_mut(),
            );
            let mem = pool::pj_pool_zalloc(p, 64);
            let bytes = std::slice::from_raw_parts(mem as *const u8, 64);
            assert!(bytes.iter().all(|&b| b == 0));
            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_pool_calloc() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"ctest\0".as_ptr() as *const _,
                1024,
                1024,
                std::ptr::null_mut(),
            );
            let mem = pool::pj_pool_calloc(p, 10, 16);
            assert!(!mem.is_null());
            let bytes = std::slice::from_raw_parts(mem as *const u8, 160);
            assert!(bytes.iter().all(|&b| b == 0));
            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_pool_reset() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"rtest\0".as_ptr() as *const _,
                1024,
                1024,
                std::ptr::null_mut(),
            );
            pool::pj_pool_alloc(p, 512);
            assert!(pool::pj_pool_get_used_size(p) >= 512);

            pool::pj_pool_reset(p);
            assert_eq!(pool::pj_pool_get_used_size(p), 0);

            // Pool is still usable after reset
            let mem = pool::pj_pool_alloc(p, 64);
            assert!(!mem.is_null());

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_pool_strdup2() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"stest\0".as_ptr() as *const _,
                1024,
                1024,
                std::ptr::null_mut(),
            );
            let mut dst = pj_str_t::EMPTY;
            let src = std::ffi::CString::new("hello world").unwrap();
            string::pj_strdup2(p, &mut dst, src.as_ptr());
            assert_eq!(dst.slen, 11);
            assert_eq!(dst.as_str(), "hello world");
            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_pool_strdup_with_null() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"sntest\0".as_ptr() as *const _,
                1024,
                1024,
                std::ptr::null_mut(),
            );
            let src_c = std::ffi::CString::new("test").unwrap();
            let src = string::pj_str(src_c.as_ptr() as *mut _);
            let mut dst = pj_str_t::EMPTY;
            string::pj_strdup_with_null(p, &mut dst, &src);
            assert_eq!(dst.slen, 4);
            // Verify null termination
            assert_eq!(*dst.ptr.add(4), 0);
            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // Init tests
    // -------------------------------------------------------------------

    #[test]
    fn test_pj_init_idempotent() {
        unsafe {
            assert_eq!(init::pj_init(), PJ_SUCCESS);
            assert_eq!(init::pj_init(), PJ_SUCCESS); // second call is fine
        }
    }

    #[test]
    fn test_pj_shutdown() {
        unsafe {
            assert_eq!(init::pj_shutdown(), PJ_SUCCESS);
        }
    }

    #[test]
    fn test_endpt_create_and_destroy() {
        unsafe {
            let mut endpt: *mut pjsip_endpoint = std::ptr::null_mut();
            let status = init::pjsip_endpt_create(
                std::ptr::null_mut(),
                b"test\0".as_ptr() as *const _,
                &mut endpt,
            );
            assert_eq!(status, PJ_SUCCESS);
            assert!(!endpt.is_null());

            // Can get pool factory
            let factory = init::pjsip_endpt_get_pool_factory(endpt);
            assert!(!factory.is_null());

            // Can create/release pool from endpoint
            let p = init::pjsip_endpt_create_pool(
                endpt,
                b"ep\0".as_ptr() as *const _,
                4096,
                4096,
            );
            assert!(!p.is_null());
            init::pjsip_endpt_release_pool(endpt, p);

            init::pjsip_endpt_destroy(endpt);
        }
    }

    // -------------------------------------------------------------------
    // URI parsing tests
    // -------------------------------------------------------------------

    #[test]
    fn test_parse_sip_uri_simple() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"sip:alice@example.com\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1, // exclude null
                0,
            );
            assert!(!result.is_null());

            // Cast to sip_uri and check fields
            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).scheme.as_str(), "sip");
            assert_eq!((*sip_uri).user.as_str(), "alice");
            assert_eq!((*sip_uri).host.as_str(), "example.com");
            assert_eq!((*sip_uri).port, 0);

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_sip_uri_with_port() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri2\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"sip:bob@192.168.1.1:5060\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(!result.is_null());

            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).user.as_str(), "bob");
            assert_eq!((*sip_uri).host.as_str(), "192.168.1.1");
            assert_eq!((*sip_uri).port, 5060);

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_sips_uri() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri3\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"sips:secure@example.org\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(!result.is_null());

            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).scheme.as_str(), "sips");

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_uri_with_transport_param() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri4\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"sip:alice@example.com;transport=tcp\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(!result.is_null());

            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).transport_param.as_str(), "tcp");

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_uri_with_lr() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri5\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"sip:proxy.example.com;lr\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(!result.is_null());

            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).lr_param, 1);

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_uri_invalid() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri6\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"not-a-uri\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(result.is_null());

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_uri_angle_brackets() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"uri7\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            let uri_str = b"<sip:alice@example.com>\0";
            let result = sip_parser::pjsip_parse_uri(
                p,
                uri_str.as_ptr() as *mut _,
                uri_str.len() - 1,
                0,
            );
            assert!(!result.is_null());

            let sip_uri = result as *const pjsip_sip_uri;
            assert_eq!((*sip_uri).user.as_str(), "alice");

            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // SIP message tests
    // -------------------------------------------------------------------

    #[test]
    fn test_msg_create_and_add_hdr() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"msg\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );

            let msg = sip_message::pjsip_msg_create(p, PJSIP_REQUEST_MSG);
            assert!(!msg.is_null());

            // Create a header
            let hdr = pool::pj_pool_alloc(p, std::mem::size_of::<pjsip_generic_string_hdr>())
                as *mut pjsip_generic_string_hdr;
            (*hdr).htype = PJSIP_H_VIA;
            let via_name = std::ffi::CString::new("Via").unwrap();
            (*hdr).name = string::pj_str(via_name.as_ptr() as *mut _);
            (*hdr).sname = pj_str_t::EMPTY;

            // Add it
            sip_message::pjsip_msg_add_hdr(msg, hdr as *mut pjsip_hdr);

            // Find it
            let found = sip_message::pjsip_msg_find_hdr(msg, PJSIP_H_VIA, std::ptr::null());
            assert!(!found.is_null());
            assert_eq!(found, hdr as *mut pjsip_hdr);

            // Should not find a different type
            let not_found =
                sip_message::pjsip_msg_find_hdr(msg, PJSIP_H_FROM, std::ptr::null());
            assert!(not_found.is_null());

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_msg_find_hdr_by_name() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"msgh\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );

            let msg = sip_message::pjsip_msg_create(p, PJSIP_REQUEST_MSG);

            let hdr = pool::pj_pool_alloc(p, std::mem::size_of::<pjsip_generic_string_hdr>())
                as *mut pjsip_generic_string_hdr;
            (*hdr).htype = PJSIP_H_OTHER;
            let name = std::ffi::CString::new("X-Custom").unwrap();
            (*hdr).name = string::pj_str(name.as_ptr() as *mut _);
            (*hdr).sname = pj_str_t::EMPTY;

            sip_message::pjsip_msg_add_hdr(msg, hdr as *mut pjsip_hdr);

            let search = std::ffi::CString::new("x-custom").unwrap();
            let search_pj = string::pj_str(search.as_ptr() as *mut _);
            let found = sip_message::pjsip_msg_find_hdr_by_name(msg, &search_pj, std::ptr::null());
            assert!(!found.is_null());

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_sip_message() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"pmsg\0".as_ptr() as *const _,
                8192,
                4096,
                std::ptr::null_mut(),
            );

            let raw = b"INVITE sip:bob@example.com SIP/2.0\r\nVia: SIP/2.0/UDP 192.168.1.1:5060\r\nFrom: <sip:alice@example.com>;tag=1234\r\nTo: <sip:bob@example.com>\r\nCall-ID: abcd1234@192.168.1.1\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n";

            let msg = sip_parser::pjsip_parse_msg(
                p,
                raw.as_ptr() as *mut _,
                raw.len(),
                0,
            );
            assert!(!msg.is_null());
            assert_eq!((*msg).msg_type, PJSIP_REQUEST_MSG);

            // Check method
            let method_id = (&(*msg).line.req).method.id;
            assert_eq!(method_id, PJSIP_INVITE_METHOD);

            // Find Via header
            let via = sip_message::pjsip_msg_find_hdr(msg, PJSIP_H_VIA, std::ptr::null());
            assert!(!via.is_null());

            // Find Call-ID header
            let callid = sip_message::pjsip_msg_find_hdr(msg, PJSIP_H_CALL_ID, std::ptr::null());
            assert!(!callid.is_null());

            pool::pj_pool_release(p);
        }
    }

    #[test]
    fn test_parse_sip_response() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"resp\0".as_ptr() as *const _,
                8192,
                4096,
                std::ptr::null_mut(),
            );

            let raw = b"SIP/2.0 200 OK\r\nVia: SIP/2.0/UDP 192.168.1.1:5060\r\nFrom: <sip:alice@example.com>;tag=1234\r\nTo: <sip:bob@example.com>;tag=5678\r\nCall-ID: abcd1234@192.168.1.1\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n";

            let msg = sip_parser::pjsip_parse_msg(
                p,
                raw.as_ptr() as *mut _,
                raw.len(),
                0,
            );
            assert!(!msg.is_null());
            assert_eq!((*msg).msg_type, PJSIP_RESPONSE_MSG);

            let status = (*msg).line.status;
            assert_eq!(status.code, 200);
            assert_eq!(status.reason.as_str(), "OK");

            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // Sockaddr tests
    // -------------------------------------------------------------------

    #[test]
    fn test_sockaddr_parse_ipv4() {
        unsafe {
            let addr_str_c = std::ffi::CString::new("192.168.1.100:5060").unwrap();
            let addr_str = string::pj_str(addr_str_c.as_ptr() as *mut _);

            let mut addr: pj_sockaddr = std::mem::zeroed();
            let status =
                sockaddr::pj_sockaddr_parse(0, 0, &addr_str, &mut addr);
            assert_eq!(status, PJ_SUCCESS);
            assert_eq!(addr.addr.sin_family, PJ_AF_INET);
            assert_eq!(sockaddr::pj_sockaddr_get_port(&addr), 5060);

            // Print it back
            let mut buf = [0u8; 64];
            let result = sockaddr::pj_sockaddr_print(
                &addr,
                buf.as_mut_ptr() as *mut _,
                64,
                1, // with port
            );
            assert!(!result.is_null());
            let printed = std::ffi::CStr::from_ptr(result).to_str().unwrap();
            assert_eq!(printed, "192.168.1.100:5060");
        }
    }

    #[test]
    fn test_sockaddr_parse_ipv4_no_port() {
        unsafe {
            let addr_str_c = std::ffi::CString::new("10.0.0.1").unwrap();
            let addr_str = string::pj_str(addr_str_c.as_ptr() as *mut _);
            let mut addr: pj_sockaddr = std::mem::zeroed();
            let status =
                sockaddr::pj_sockaddr_parse(PJ_AF_INET as i32, 0, &addr_str, &mut addr);
            assert_eq!(status, PJ_SUCCESS);
            assert_eq!(sockaddr::pj_sockaddr_get_port(&addr), 0);
        }
    }

    #[test]
    fn test_sockaddr_set_port() {
        unsafe {
            let addr_str_c = std::ffi::CString::new("10.0.0.1").unwrap();
            let addr_str = string::pj_str(addr_str_c.as_ptr() as *mut _);
            let mut addr: pj_sockaddr = std::mem::zeroed();
            sockaddr::pj_sockaddr_parse(PJ_AF_INET as i32, 0, &addr_str, &mut addr);

            sockaddr::pj_sockaddr_set_port(&mut addr, 8080);
            assert_eq!(sockaddr::pj_sockaddr_get_port(&addr), 8080);
        }
    }

    #[test]
    fn test_sockaddr_init_ipv4() {
        unsafe {
            let host_c = std::ffi::CString::new("127.0.0.1").unwrap();
            let host = string::pj_str(host_c.as_ptr() as *mut _);
            let mut addr: pj_sockaddr = std::mem::zeroed();
            let status = sockaddr::pj_sockaddr_init(PJ_AF_INET as i32, &mut addr, &host, 5060);
            assert_eq!(status, PJ_SUCCESS);
            assert_eq!(addr.addr.sin_family, PJ_AF_INET);
            assert_eq!(sockaddr::pj_sockaddr_get_port(&addr), 5060);

            let mut buf = [0u8; 64];
            sockaddr::pj_sockaddr_print(&addr, buf.as_mut_ptr() as *mut _, 64, 0);
            let printed = std::ffi::CStr::from_ptr(buf.as_ptr() as *const _).to_str().unwrap();
            assert_eq!(printed, "127.0.0.1");
        }
    }

    #[test]
    fn test_sockaddr_parse_ipv6() {
        unsafe {
            let addr_str_c = std::ffi::CString::new("::1").unwrap();
            let addr_str = string::pj_str(addr_str_c.as_ptr() as *mut _);
            let mut addr: pj_sockaddr = std::mem::zeroed();
            let status =
                sockaddr::pj_sockaddr_parse(PJ_AF_INET6 as i32, 0, &addr_str, &mut addr);
            assert_eq!(status, PJ_SUCCESS);
            assert_eq!(addr.ipv6.sin6_family, PJ_AF_INET6);
        }
    }

    // -------------------------------------------------------------------
    // Caching pool tests
    // -------------------------------------------------------------------

    #[test]
    fn test_caching_pool() {
        unsafe {
            let mut cp: pj_caching_pool = std::mem::zeroed();
            pool::pj_caching_pool_init(&mut cp, std::ptr::null(), 0);

            // Create a pool from the caching pool's factory
            let p = pool::pj_pool_create(
                &mut cp.factory as *mut _ as *mut _,
                b"cptest\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );
            assert!(!p.is_null());
            pool::pj_pool_release(p);

            pool::pj_caching_pool_destroy(&mut cp);
        }
    }

    // -------------------------------------------------------------------
    // Logging tests
    // -------------------------------------------------------------------

    #[test]
    fn test_logging_stubs() {
        unsafe {
            logging::pj_log_set_level(5);
            assert_eq!(logging::pj_log_get_level(), 5);
            logging::pj_log_set_decor(0);
            logging::pj_log_set_log_func(None);
        }
    }

    // -------------------------------------------------------------------
    // Header clone test
    // -------------------------------------------------------------------

    #[test]
    fn test_hdr_clone() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"hclone\0".as_ptr() as *const _,
                4096,
                4096,
                std::ptr::null_mut(),
            );

            let hdr = pool::pj_pool_alloc(p, std::mem::size_of::<pjsip_generic_string_hdr>())
                as *mut pjsip_generic_string_hdr;
            (*hdr).htype = PJSIP_H_VIA;
            let name_c = std::ffi::CString::new("Via").unwrap();
            (*hdr).name = string::pj_str(name_c.as_ptr() as *mut _);
            (*hdr).sname = pj_str_t::EMPTY;
            let val_c = std::ffi::CString::new("SIP/2.0/UDP 10.0.0.1:5060").unwrap();
            (*hdr).hvalue = string::pj_str(val_c.as_ptr() as *mut _);

            let cloned = sip_message::pjsip_hdr_clone(p, hdr as *const pjsip_hdr);
            assert!(!cloned.is_null());
            assert_eq!((*cloned).htype, PJSIP_H_VIA);

            let cloned_str = cloned as *const pjsip_generic_string_hdr;
            assert_eq!((*cloned_str).name.as_str(), "Via");
            assert_eq!((*cloned_str).hvalue.as_str(), "SIP/2.0/UDP 10.0.0.1:5060");

            // Verify it's a deep copy (different pointer)
            assert_ne!((*cloned_str).name.ptr, (*hdr).name.ptr);

            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // String extension tests
    // -------------------------------------------------------------------

    #[test]
    fn test_pj_strcat() {
        unsafe {
            let mut buf = [0u8; 64];
            let mut dst = pj_str_t {
                ptr: buf.as_mut_ptr() as *mut _,
                slen: 0,
            };
            let hello = std::ffi::CString::new("hello").unwrap();
            let src = string::pj_str(hello.as_ptr() as *mut _);
            string::pj_strcat(&mut dst, &src);
            assert_eq!(dst.slen, 5);

            let world = std::ffi::CString::new(" world").unwrap();
            let src2 = string::pj_str(world.as_ptr() as *mut _);
            string::pj_strcat(&mut dst, &src2);
            assert_eq!(dst.slen, 11);
            assert_eq!(dst.as_str(), "hello world");
        }
    }

    #[test]
    fn test_pj_strncmp() {
        unsafe {
            let a_c = std::ffi::CString::new("abc123").unwrap();
            let b_c = std::ffi::CString::new("abc456").unwrap();
            let a = string::pj_str(a_c.as_ptr() as *mut _);
            let b = string::pj_str(b_c.as_ptr() as *mut _);
            // First 3 chars are equal
            assert_eq!(string::pj_strncmp(&a, &b, 3), 0);
            // First 4 differ
            assert_ne!(string::pj_strncmp(&a, &b, 4), 0);
        }
    }

    #[test]
    fn test_pj_utoa() {
        unsafe {
            let mut buf = [0i8; 32];
            let len = string::pj_utoa(42, buf.as_mut_ptr());
            assert_eq!(len, 2);
            assert_eq!(
                std::ffi::CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
                "42"
            );
        }
    }

    #[test]
    fn test_pj_ansi_strxcpy() {
        unsafe {
            let mut buf = [0i8; 10];
            let src = std::ffi::CString::new("hello").unwrap();
            let status = string::pj_ansi_strxcpy(buf.as_mut_ptr(), src.as_ptr(), 10);
            assert_eq!(status, PJ_SUCCESS);
            assert_eq!(
                std::ffi::CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
                "hello"
            );
        }
    }

    // -------------------------------------------------------------------
    // List tests
    // -------------------------------------------------------------------

    #[test]
    fn test_list_operations() {
        unsafe {
            use list::*;

            // Create a sentinel
            let mut sentinel: pj_list_node = std::mem::zeroed();
            pj_list_init(&mut sentinel);
            assert_eq!(pj_list_size(&sentinel), 0);

            // Create 3 nodes
            let mut n1: pj_list_node = std::mem::zeroed();
            let mut n2: pj_list_node = std::mem::zeroed();
            let mut n3: pj_list_node = std::mem::zeroed();

            pj_list_insert_after(&mut sentinel, &mut n1);
            assert_eq!(pj_list_size(&sentinel), 1);

            pj_list_insert_after(&mut n1, &mut n2);
            assert_eq!(pj_list_size(&sentinel), 2);

            pj_list_insert_before(&mut sentinel, &mut n3);
            assert_eq!(pj_list_size(&sentinel), 3);

            // Find
            let found = pj_list_find_node(&mut sentinel, &mut n2);
            assert_eq!(found, &mut n2 as *mut _);

            // Erase
            pj_list_erase(&mut n2);
            assert_eq!(pj_list_size(&sentinel), 2);
            let not_found = pj_list_find_node(&mut sentinel, &mut n2);
            assert!(not_found.is_null());
        }
    }

    // -------------------------------------------------------------------
    // Timer tests
    // -------------------------------------------------------------------

    #[test]
    fn test_timer_create_destroy() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"timer\0".as_ptr() as *const _,
                4096, 4096, std::ptr::null_mut(),
            );
            let mut heap: *mut timer::pj_timer_heap_t = std::ptr::null_mut();
            let status = timer::pj_timer_heap_create(p, 64, &mut heap);
            assert_eq!(status, PJ_SUCCESS);
            assert!(!heap.is_null());
            assert_eq!(timer::pj_timer_heap_count(heap), 0);

            // Schedule a timer
            let mut entry: timer::pj_timer_entry = std::mem::zeroed();
            timer::pj_timer_entry_init(&mut entry, 1, std::ptr::null_mut(), None);
            let delay = timer::pj_time_val { sec: 999, msec: 0 };
            let s = timer::pj_timer_heap_schedule(heap, &mut entry, &delay);
            assert_eq!(s, PJ_SUCCESS);
            assert_eq!(timer::pj_timer_heap_count(heap), 1);

            // Cancel
            let cancelled = timer::pj_timer_heap_cancel(heap, &mut entry);
            assert_eq!(cancelled, 1);
            assert_eq!(timer::pj_timer_heap_count(heap), 0);

            timer::pj_timer_heap_destroy(heap);
            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // Time tests
    // -------------------------------------------------------------------

    #[test]
    fn test_gettimeofday() {
        unsafe {
            let mut tv = timer::pj_time_val { sec: 0, msec: 0 };
            let status = time::pj_gettimeofday(&mut tv);
            assert_eq!(status, PJ_SUCCESS);
            assert!(tv.sec > 0); // Should be a reasonable unix timestamp
        }
    }

    #[test]
    fn test_time_val_normalize() {
        unsafe {
            let mut tv = timer::pj_time_val { sec: 1, msec: 2500 };
            time::pj_time_val_normalize(&mut tv);
            assert_eq!(tv.sec, 3);
            assert_eq!(tv.msec, 500);
        }
    }

    // -------------------------------------------------------------------
    // Threading tests
    // -------------------------------------------------------------------

    #[test]
    fn test_mutex_create_lock_unlock() {
        unsafe {
            let mut mutex: *mut threading::pj_mutex_t = std::ptr::null_mut();
            let status = threading::pj_mutex_create_simple(
                std::ptr::null_mut(),
                b"test\0".as_ptr() as *const _,
                &mut mutex,
            );
            assert_eq!(status, PJ_SUCCESS);
            assert!(!mutex.is_null());

            assert_eq!(threading::pj_mutex_lock(mutex), PJ_SUCCESS);
            assert_eq!(threading::pj_mutex_unlock(mutex), PJ_SUCCESS);
            assert_eq!(threading::pj_mutex_destroy(mutex), PJ_SUCCESS);
        }
    }

    #[test]
    fn test_semaphore() {
        unsafe {
            let mut sem: *mut threading::pj_sem_t = std::ptr::null_mut();
            let status = threading::pj_sem_create(
                std::ptr::null_mut(),
                b"sem\0".as_ptr() as *const _,
                1, 10, &mut sem,
            );
            assert_eq!(status, PJ_SUCCESS);

            // Should succeed (count=1)
            assert_eq!(threading::pj_sem_trywait(sem), PJ_SUCCESS);
            // Should fail now (count=0)
            assert_ne!(threading::pj_sem_trywait(sem), PJ_SUCCESS);
            // Post
            assert_eq!(threading::pj_sem_post(sem), PJ_SUCCESS);
            // Should succeed again
            assert_eq!(threading::pj_sem_trywait(sem), PJ_SUCCESS);

            assert_eq!(threading::pj_sem_destroy(sem), PJ_SUCCESS);
        }
    }

    // -------------------------------------------------------------------
    // Atomic tests
    // -------------------------------------------------------------------

    #[test]
    fn test_atomic_ops() {
        unsafe {
            let mut a: *mut atomic::pj_atomic_t = std::ptr::null_mut();
            assert_eq!(atomic::pj_atomic_create(std::ptr::null_mut(), 0, &mut a), PJ_SUCCESS);
            assert_eq!(atomic::pj_atomic_get(a), 0);

            atomic::pj_atomic_inc(a);
            assert_eq!(atomic::pj_atomic_get(a), 1);

            atomic::pj_atomic_add(a, 5);
            assert_eq!(atomic::pj_atomic_get(a), 6);

            let val = atomic::pj_atomic_dec_and_get(a);
            assert_eq!(val, 5);

            atomic::pj_atomic_set(a, 42);
            assert_eq!(atomic::pj_atomic_value(a), 42);

            assert_eq!(atomic::pj_atomic_destroy(a), PJ_SUCCESS);
        }
    }

    // -------------------------------------------------------------------
    // Hash table tests
    // -------------------------------------------------------------------

    #[test]
    fn test_hash_table() {
        unsafe {
            let ht = misc::pj_hash_create(std::ptr::null_mut(), 31);
            assert!(!ht.is_null());
            assert_eq!(misc::pj_hash_count(ht), 0);

            let key = b"test_key\0";
            let val = 42usize as *mut libc::c_void;
            misc::pj_hash_set(
                std::ptr::null_mut(),
                ht, key.as_ptr() as *const _, -1, 0, val,
            );
            assert_eq!(misc::pj_hash_count(ht), 1);

            let found = misc::pj_hash_get(ht, key.as_ptr() as *const _, -1, std::ptr::null_mut());
            assert_eq!(found as usize, 42);

            // Remove
            misc::pj_hash_set(
                std::ptr::null_mut(),
                ht, key.as_ptr() as *const _, -1, 0, std::ptr::null_mut(),
            );
            assert_eq!(misc::pj_hash_count(ht), 0);
        }
    }

    // -------------------------------------------------------------------
    // Test framework tests
    // -------------------------------------------------------------------

    #[test]
    fn test_test_framework() {
        unsafe {
            let p = pool::pj_pool_create(
                std::ptr::null_mut(),
                b"tf\0".as_ptr() as *const _,
                4096, 4096, std::ptr::null_mut(),
            );
            let suite = test_framework::pj_test_suite_create(p);
            assert!(!suite.is_null());

            // Create a test case with a function that returns 0 (success)
            unsafe extern "C" fn my_test(_tc: *mut test_framework::pj_test_case) -> i32 {
                0 // success
            }

            let mut tc: test_framework::pj_test_case = std::mem::zeroed();
            test_framework::pj_test_case_init(
                &mut tc,
                b"my_test\0".as_ptr() as *const _,
                0,
                Some(my_test),
            );

            test_framework::pj_test_suite_add_case(suite, &mut tc);

            // Create runner
            let runner = test_framework::pj_test_create_basic_runner(p);
            assert!(!runner.is_null());

            // Run
            test_framework::pj_test_run(runner, suite);

            // Check stat
            let mut stat = test_framework::pj_test_stat::default();
            test_framework::pj_test_get_stat(suite, &mut stat);
            assert_eq!(stat.ntests, 1);
            assert_eq!(stat.nfailed, 0);

            test_framework::pj_test_destroy_runner(runner);
            pool::pj_pool_release(p);
        }
    }

    // -------------------------------------------------------------------
    // File I/O tests
    // -------------------------------------------------------------------

    #[test]
    fn test_file_exists() {
        unsafe {
            let path = b"/tmp\0".as_ptr() as *const libc::c_char;
            assert_eq!(misc::pj_file_exists(path), PJ_TRUE);

            let nopath = b"/nonexistent_test_path_xyz\0".as_ptr() as *const libc::c_char;
            assert_eq!(misc::pj_file_exists(nopath), PJ_FALSE);
        }
    }

    // -------------------------------------------------------------------
    // Group lock tests
    // -------------------------------------------------------------------

    #[test]
    fn test_grp_lock() {
        unsafe {
            let mut lock: *mut atomic::pj_grp_lock_t = std::ptr::null_mut();
            let status = atomic::pj_grp_lock_create(
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut lock,
            );
            assert_eq!(status, PJ_SUCCESS);
            assert!(!lock.is_null());

            assert_eq!(atomic::pj_grp_lock_acquire(lock), PJ_SUCCESS);
            assert_eq!(atomic::pj_grp_lock_release(lock), PJ_SUCCESS);

            atomic::pj_grp_lock_add_ref(lock);
            assert_eq!(atomic::pj_grp_lock_dec_ref(lock), PJ_SUCCESS);
            // Still alive (ref_count was 2, now 1)
            assert_eq!(atomic::pj_grp_lock_dec_ref(lock), PJ_SUCCESS);
            // Now destroyed (ref_count was 1, now 0)
        }
    }

    // -------------------------------------------------------------------
    // I/O Queue tests
    // -------------------------------------------------------------------

    #[test]
    fn test_ioqueue_create_destroy() {
        unsafe {
            let mut ioq: *mut misc::pj_ioqueue_t = std::ptr::null_mut();
            let status = misc::pj_ioqueue_create(
                std::ptr::null_mut(), 64, &mut ioq,
            );
            assert_eq!(status, PJ_SUCCESS);
            assert!(!ioq.is_null());

            let count = misc::pj_ioqueue_poll(ioq, std::ptr::null());
            assert_eq!(count, 0);

            assert_eq!(misc::pj_ioqueue_destroy(ioq), PJ_SUCCESS);
        }
    }

    // -------------------------------------------------------------------
    // Random tests
    // -------------------------------------------------------------------

    #[test]
    fn test_random() {
        unsafe {
            misc::pj_srand(12345);
            let r1 = misc::pj_rand();
            let r2 = misc::pj_rand();
            assert_ne!(r1, r2);
        }
    }
}
