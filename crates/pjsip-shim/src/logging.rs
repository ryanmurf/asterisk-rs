//! pj_log -- logging functions.
//!
//! pjproject's logging system routes through pj_log_1..5 functions.
//! Those are variadic C functions, so they are implemented in
//! `log_wrapper.c` (compiled via build.rs).  The C wrappers do
//! printf formatting and then call `pj_log_write()` which is
//! the Rust entry-point defined here.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

static LOG_LEVEL: AtomicI32 = AtomicI32::new(3);
static LOG_DECOR: AtomicU32 = AtomicU32::new(0);
static LOG_INDENT: AtomicI32 = AtomicI32::new(0);

/// Log writer callback type.
pub type pj_log_func = unsafe extern "C" fn(level: i32, data: *const libc::c_char, len: i32);

static mut LOG_WRITER: Option<pj_log_func> = None;

// ---------------------------------------------------------------------------
// Level / decoration
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_log_set_level(level: i32) {
    LOG_LEVEL.store(level, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_get_level() -> i32 {
    LOG_LEVEL.load(Ordering::Relaxed)
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_set_decor(decor: u32) {
    LOG_DECOR.store(decor, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_get_decor() -> u32 {
    LOG_DECOR.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Log writer
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_log_set_log_func(func: Option<pj_log_func>) {
    LOG_WRITER = func;
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_get_log_func() -> Option<pj_log_func> {
    LOG_WRITER
}

// ---------------------------------------------------------------------------
// Indent
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_log_push_indent() {
    LOG_INDENT.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_pop_indent() {
    let _ = LOG_INDENT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(if v > 0 { v - 1 } else { 0 })
    });
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_add_indent(indent: i32) {
    LOG_INDENT.fetch_add(indent, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_get_indent() -> i32 {
    LOG_INDENT.load(Ordering::Relaxed)
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_set_indent(indent: i32) {
    LOG_INDENT.store(indent, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// pj_log_write -- called from the C variadic wrappers in log_wrapper.c
//
// The C wrappers (pj_log_1..5, pj_perror, pj_perror_1..5) do the
// printf-style formatting and then call this function with the
// already-formatted message.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_log_write(
    level: i32,
    sender: *const libc::c_char,
    msg: *const libc::c_char,
) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) {
        return;
    }

    // If a custom log writer is registered, dispatch to it.
    if let Some(writer) = LOG_WRITER {
        if !msg.is_null() {
            let len = libc::strlen(msg) as i32;
            writer(level, msg, len);
        }
        return;
    }

    let sender_str = if sender.is_null() {
        "?"
    } else {
        std::ffi::CStr::from_ptr(sender).to_str().unwrap_or("?")
    };
    let msg_str = if msg.is_null() {
        ""
    } else {
        std::ffi::CStr::from_ptr(msg).to_str().unwrap_or("")
    };
    eprintln!("[pj_log:{}] {}: {}", level, sender_str, msg_str);
}
