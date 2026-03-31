//! pj_ioqueue -- thin Rust bindings to the C ioqueue implementation.
//!
//! The actual ioqueue is compiled from pjproject's C source files
//! (ioqueue_select.c + ioqueue_common_abs.c) and linked into the cdylib.
//! This module provides `extern "C"` declarations so that other Rust code
//! (e.g. the activesock layer in misc.rs) can call the C functions.

use crate::misc::{pj_ioqueue_callback, pj_ioqueue_key_t, pj_ioqueue_op_key_t, pj_ioqueue_t};
use crate::socket::pj_sock_t;
use crate::types::*;

// pj_pool_t is defined in crate::types
use crate::types::pj_pool_t;

extern "C" {
    pub fn pj_ioqueue_create(
        pool: *mut pj_pool_t,
        max_fd: usize,
        p_ioqueue: *mut *mut pj_ioqueue_t,
    ) -> pj_status_t;

    pub fn pj_ioqueue_destroy(ioqueue: *mut pj_ioqueue_t) -> pj_status_t;

    pub fn pj_ioqueue_poll(
        ioqueue: *mut pj_ioqueue_t,
        timeout: *const crate::timer::pj_time_val,
    ) -> i32;

    pub fn pj_ioqueue_register_sock(
        pool: *mut pj_pool_t,
        ioqueue: *mut pj_ioqueue_t,
        sock: pj_sock_t,
        user_data: *mut libc::c_void,
        cb: *const pj_ioqueue_callback,
        p_key: *mut *mut pj_ioqueue_key_t,
    ) -> pj_status_t;

    pub fn pj_ioqueue_register_sock2(
        pool: *mut pj_pool_t,
        ioqueue: *mut pj_ioqueue_t,
        sock: pj_sock_t,
        grp_lock: *mut libc::c_void,
        user_data: *mut libc::c_void,
        cb: *const pj_ioqueue_callback,
        p_key: *mut *mut pj_ioqueue_key_t,
    ) -> pj_status_t;

    pub fn pj_ioqueue_unregister(key: *mut pj_ioqueue_key_t) -> pj_status_t;

    pub fn pj_ioqueue_get_user_data(key: *mut pj_ioqueue_key_t) -> *mut libc::c_void;

    pub fn pj_ioqueue_set_user_data(
        key: *mut pj_ioqueue_key_t,
        user_data: *mut libc::c_void,
        old_data: *mut *mut libc::c_void,
    ) -> pj_status_t;

    pub fn pj_ioqueue_recv(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        buf: *mut libc::c_void,
        length: *mut isize,
        flags: u32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_recvfrom(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        buf: *mut libc::c_void,
        length: *mut isize,
        flags: u32,
        addr: *mut libc::c_void,
        addrlen: *mut i32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_send(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        data: *const libc::c_void,
        length: *mut isize,
        flags: u32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_sendto(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        data: *const libc::c_void,
        length: *mut isize,
        flags: u32,
        addr: *const libc::c_void,
        addrlen: i32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_accept(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        sock: *mut pj_sock_t,
        local: *mut libc::c_void,
        remote: *mut libc::c_void,
        addrlen: *mut i32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_connect(
        key: *mut pj_ioqueue_key_t,
        addr: *const libc::c_void,
        addrlen: i32,
    ) -> pj_status_t;

    pub fn pj_ioqueue_op_key_init(
        op_key: *mut pj_ioqueue_op_key_t,
        size: usize,
    );

    pub fn pj_ioqueue_is_pending(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
    ) -> pj_bool_t;

    pub fn pj_ioqueue_post_completion(
        key: *mut pj_ioqueue_key_t,
        op_key: *mut pj_ioqueue_op_key_t,
        bytes_status: isize,
    ) -> pj_status_t;

    pub fn pj_ioqueue_set_default_concurrency(
        ioqueue: *mut pj_ioqueue_t,
        allow: pj_bool_t,
    ) -> pj_status_t;

    pub fn pj_ioqueue_set_concurrency(
        key: *mut pj_ioqueue_key_t,
        allow: pj_bool_t,
    ) -> pj_status_t;

    pub fn pj_ioqueue_name() -> *const libc::c_char;

    pub fn pj_ioqueue_get_os_handle(
        ioqueue: *mut pj_ioqueue_t,
    ) -> *mut libc::c_void;
}
