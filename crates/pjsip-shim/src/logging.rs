//! pj_log -- logging functions.
//!
//! pjproject's logging system routes through pj_log_1..5 functions.
//! We route to eprintln for now; the level/decoration state is tracked
//! in module-level statics.
//!
//! Note: The actual C signatures are variadic (pj_log_N(sender, fmt, ...))
//! but since Rust stable doesn't support C-variadic functions, we declare
//! them with fixed args.  The symbol names are all that matter for linking.

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

/// Generic pj_perror (non-level-specific).  C signature is variadic.
#[no_mangle]
pub unsafe extern "C" fn pj_perror(
    level: i32,
    sender: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(level, sender, fmt);
}

// ---------------------------------------------------------------------------
// pj_log_1 .. pj_log_5
//
// C signature: void pj_log_N(const char *sender, const char *fmt, ...);
// We declare with fixed args -- the symbol name matches for the linker.
// The C calling convention passes all extra args in registers/stack which
// we simply ignore. This is safe on x86_64 and aarch64.
// ---------------------------------------------------------------------------

unsafe fn do_log(level: i32, sender: *const libc::c_char, fmt: *const libc::c_char) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let sender_s = if sender.is_null() {
        "?"
    } else {
        std::ffi::CStr::from_ptr(sender).to_str().unwrap_or("?")
    };
    let fmt_s = if fmt.is_null() {
        ""
    } else {
        std::ffi::CStr::from_ptr(fmt).to_str().unwrap_or("")
    };
    eprintln!("[pj_log:{}] {}: {}", level, sender_s, fmt_s);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_1(sender: *const libc::c_char, fmt: *const libc::c_char) {
    do_log(1, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_2(sender: *const libc::c_char, fmt: *const libc::c_char) {
    do_log(2, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_3(sender: *const libc::c_char, fmt: *const libc::c_char) {
    do_log(3, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_4(sender: *const libc::c_char, fmt: *const libc::c_char) {
    do_log(4, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_log_5(sender: *const libc::c_char, fmt: *const libc::c_char) {
    do_log(5, sender, fmt);
}

// ---------------------------------------------------------------------------
// pj_perror_1 .. pj_perror_5
//
// C signature: void pj_perror_N(const char *sender, const char *title,
//                               pj_status_t status, const char *fmt, ...);
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_perror_1(
    sender: *const libc::c_char,
    _title: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(1, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_perror_2(
    sender: *const libc::c_char,
    _title: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(2, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_perror_3(
    sender: *const libc::c_char,
    _title: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(3, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_perror_4(
    sender: *const libc::c_char,
    _title: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(4, sender, fmt);
}

#[no_mangle]
pub unsafe extern "C" fn pj_perror_5(
    sender: *const libc::c_char,
    _title: *const libc::c_char,
    _status: i32,
    fmt: *const libc::c_char,
) {
    do_log(5, sender, fmt);
}
