//! pj_str_t string functions -- drop-in replacements for pjlib's string API.
//!
//! pjproject uses `pj_str_t` (ptr + slen) everywhere instead of C strings.
//! These functions provide the full set of operations that C callers expect.

use crate::pool::pj_pool_alloc;
use crate::types::*;

// ---------------------------------------------------------------------------
// pj_str -- wrap a C string in a pj_str_t
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_str(s: *mut libc::c_char) -> pj_str_t {
    if s.is_null() {
        return pj_str_t::EMPTY;
    }
    pj_str_t {
        ptr: s,
        slen: libc::strlen(s) as isize,
    }
}

// ---------------------------------------------------------------------------
// Length / buffer accessors
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_strlen(s: *const pj_str_t) -> isize {
    if s.is_null() {
        0
    } else {
        (*s).slen
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_strbuf(s: *const pj_str_t) -> *mut libc::c_char {
    if s.is_null() {
        std::ptr::null_mut()
    } else {
        (*s).ptr
    }
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Compare pj_str_t against pj_str_t.
#[no_mangle]
pub unsafe extern "C" fn pj_strcmp(s1: *const pj_str_t, s2: *const pj_str_t) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let len1 = (*s1).slen as usize;
    let len2 = (*s2).slen as usize;
    let cmp_len = len1.min(len2);
    if cmp_len > 0 {
        let result = libc::memcmp((*s1).ptr as *const _, (*s2).ptr as *const _, cmp_len);
        if result != 0 {
            return result;
        }
    }
    (len1 as i32) - (len2 as i32)
}

/// Compare pj_str_t against a C string.
#[no_mangle]
pub unsafe extern "C" fn pj_strcmp2(s1: *const pj_str_t, s2: *const libc::c_char) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let len1 = (*s1).slen as usize;
    let s2_len = libc::strlen(s2);
    let cmp_len = len1.min(s2_len);
    if cmp_len > 0 {
        let result = libc::memcmp((*s1).ptr as *const _, s2 as *const _, cmp_len);
        if result != 0 {
            return result;
        }
    }
    (len1 as i32) - (s2_len as i32)
}

/// Case-insensitive compare of pj_str_t against pj_str_t.
#[no_mangle]
pub unsafe extern "C" fn pj_stricmp(s1: *const pj_str_t, s2: *const pj_str_t) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let a = (*s1).as_str();
    let b = (*s2).as_str();
    // Compare character by character, case-insensitively
    for (ca, cb) in a.bytes().zip(b.bytes()) {
        let diff = ca.to_ascii_lowercase() as i32 - cb.to_ascii_lowercase() as i32;
        if diff != 0 {
            return diff;
        }
    }
    (a.len() as i32) - (b.len() as i32)
}

/// Case-insensitive compare of pj_str_t against a C string.
#[no_mangle]
pub unsafe extern "C" fn pj_stricmp2(s1: *const pj_str_t, s2: *const libc::c_char) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let a = (*s1).as_str();
    let b = std::ffi::CStr::from_ptr(s2).to_bytes();
    for (ca, cb) in a.bytes().zip(b.iter().copied()) {
        let diff = ca.to_ascii_lowercase() as i32 - cb.to_ascii_lowercase() as i32;
        if diff != 0 {
            return diff;
        }
    }
    (a.len() as i32) - (b.len() as i32)
}

// ---------------------------------------------------------------------------
// Copy / duplicate
// ---------------------------------------------------------------------------

/// Duplicate a pj_str_t into pool memory.
#[no_mangle]
pub unsafe extern "C" fn pj_strdup(
    pool: *mut pj_pool_t,
    dst: *mut pj_str_t,
    src: *const pj_str_t,
) {
    if dst.is_null() || src.is_null() {
        return;
    }
    let len = (*src).slen as usize;
    if len == 0 || (*src).ptr.is_null() {
        (*dst).ptr = std::ptr::null_mut();
        (*dst).slen = 0;
        return;
    }
    let buf = pj_pool_alloc(pool, len) as *mut libc::c_char;
    if buf.is_null() {
        return;
    }
    libc::memcpy(buf as *mut _, (*src).ptr as *const _, len);
    (*dst).ptr = buf;
    (*dst).slen = len as isize;
}

/// Duplicate a C string into pool memory as a pj_str_t.
#[no_mangle]
pub unsafe extern "C" fn pj_strdup2(
    pool: *mut pj_pool_t,
    dst: *mut pj_str_t,
    src: *const libc::c_char,
) {
    if dst.is_null() || src.is_null() {
        if !dst.is_null() {
            (*dst).ptr = std::ptr::null_mut();
            (*dst).slen = 0;
        }
        return;
    }
    let len = libc::strlen(src);
    let buf = pj_pool_alloc(pool, len + 1) as *mut libc::c_char;
    if buf.is_null() {
        return;
    }
    libc::memcpy(buf as *mut _, src as *const _, len);
    *buf.add(len) = 0;
    (*dst).ptr = buf;
    (*dst).slen = len as isize;
}

/// Duplicate a pj_str_t into pool memory with a trailing null byte.
#[no_mangle]
pub unsafe extern "C" fn pj_strdup_with_null(
    pool: *mut pj_pool_t,
    dst: *mut pj_str_t,
    src: *const pj_str_t,
) {
    if dst.is_null() || src.is_null() {
        return;
    }
    let len = (*src).slen as usize;
    if len == 0 || (*src).ptr.is_null() {
        let buf = pj_pool_alloc(pool, 1) as *mut libc::c_char;
        if !buf.is_null() {
            *buf = 0;
        }
        (*dst).ptr = buf;
        (*dst).slen = 0;
        return;
    }
    let buf = pj_pool_alloc(pool, len + 1) as *mut libc::c_char;
    if buf.is_null() {
        return;
    }
    libc::memcpy(buf as *mut _, (*src).ptr as *const _, len);
    *buf.add(len) = 0;
    (*dst).ptr = buf;
    (*dst).slen = len as isize;
}

/// Assign one pj_str_t to another (shallow copy -- shares pointer).
#[no_mangle]
pub unsafe extern "C" fn pj_strassign(dst: *mut pj_str_t, src: *const pj_str_t) {
    if dst.is_null() || src.is_null() {
        return;
    }
    (*dst).ptr = (*src).ptr;
    (*dst).slen = (*src).slen;
}

/// Copy from a pj_str_t to a fixed buffer. Returns the destination.
#[no_mangle]
pub unsafe extern "C" fn pj_strcpy(
    dst: *mut pj_str_t,
    src: *const pj_str_t,
) -> *mut pj_str_t {
    if dst.is_null() || src.is_null() {
        return dst;
    }
    if (*src).slen > 0 && !(*src).ptr.is_null() && !(*dst).ptr.is_null() {
        libc::memcpy(
            (*dst).ptr as *mut _,
            (*src).ptr as *const _,
            (*src).slen as usize,
        );
    }
    (*dst).slen = (*src).slen;
    dst
}

/// Copy from a C string to a pj_str_t (the dst buffer must be pre-allocated).
#[no_mangle]
pub unsafe extern "C" fn pj_strcpy2(
    dst: *mut pj_str_t,
    src: *const libc::c_char,
) -> *mut pj_str_t {
    if dst.is_null() {
        return dst;
    }
    if src.is_null() {
        (*dst).slen = 0;
        return dst;
    }
    let len = libc::strlen(src);
    if !(*dst).ptr.is_null() {
        libc::memcpy((*dst).ptr as *mut _, src as *const _, len);
    }
    (*dst).slen = len as isize;
    dst
}

// ---------------------------------------------------------------------------
// Search / find
// ---------------------------------------------------------------------------

/// Find a character in a pj_str_t.  Returns the index or -1.
#[no_mangle]
pub unsafe extern "C" fn pj_strfind(s: *const pj_str_t, sub: *const pj_str_t) -> isize {
    if s.is_null() || sub.is_null() {
        return -1;
    }
    let haystack = (*s).as_str();
    let needle = (*sub).as_str();
    match haystack.find(needle) {
        Some(pos) => pos as isize,
        None => -1,
    }
}

/// Find a character in a pj_str_t.  Returns a pointer to the char or null.
#[no_mangle]
pub unsafe extern "C" fn pj_strchr(s: *const pj_str_t, c: i32) -> *const libc::c_char {
    if s.is_null() || (*s).ptr.is_null() || (*s).slen <= 0 {
        return std::ptr::null();
    }
    let ch = c as u8;
    let slice = std::slice::from_raw_parts((*s).ptr as *const u8, (*s).slen as usize);
    match slice.iter().position(|&b| b == ch) {
        Some(pos) => (*s).ptr.add(pos),
        None => std::ptr::null(),
    }
}

// ---------------------------------------------------------------------------
// Trim
// ---------------------------------------------------------------------------

/// Trim leading and trailing whitespace from a pj_str_t (in-place).
#[no_mangle]
pub unsafe extern "C" fn pj_strtrim(s: *mut pj_str_t) {
    if s.is_null() || (*s).ptr.is_null() || (*s).slen <= 0 {
        return;
    }
    let slice = std::slice::from_raw_parts((*s).ptr as *const u8, (*s).slen as usize);

    // Trim leading
    let start = slice.iter().position(|&b| !b.is_ascii_whitespace()).unwrap_or(slice.len());
    // Trim trailing
    let end = slice.iter().rposition(|&b| !b.is_ascii_whitespace()).map(|p| p + 1).unwrap_or(start);

    (*s).ptr = (*s).ptr.add(start);
    (*s).slen = (end - start) as isize;
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Convert a pj_str_t to a long integer.
#[no_mangle]
pub unsafe extern "C" fn pj_strtol(s: *const pj_str_t) -> libc::c_long {
    if s.is_null() || (*s).ptr.is_null() || (*s).slen <= 0 {
        return 0;
    }
    let text = (*s).as_str().trim();
    text.parse::<libc::c_long>().unwrap_or(0)
}

/// Convert a pj_str_t to an unsigned long integer.
#[no_mangle]
pub unsafe extern "C" fn pj_strtoul(s: *const pj_str_t) -> libc::c_ulong {
    if s.is_null() || (*s).ptr.is_null() || (*s).slen <= 0 {
        return 0;
    }
    let text = (*s).as_str().trim();
    text.parse::<libc::c_ulong>().unwrap_or(0)
}

/// Set a pj_str_t to an empty state.
#[no_mangle]
pub unsafe extern "C" fn pj_strset(
    s: *mut pj_str_t,
    ptr: *mut libc::c_char,
    len: isize,
) -> *mut pj_str_t {
    if !s.is_null() {
        (*s).ptr = ptr;
        (*s).slen = len;
    }
    s
}

/// Set a pj_str_t from a buffer and compute length from strlen.
#[no_mangle]
pub unsafe extern "C" fn pj_strset2(
    s: *mut pj_str_t,
    src: *mut libc::c_char,
) -> *mut pj_str_t {
    if s.is_null() {
        return s;
    }
    if src.is_null() {
        (*s).ptr = std::ptr::null_mut();
        (*s).slen = 0;
    } else {
        (*s).ptr = src;
        (*s).slen = libc::strlen(src) as isize;
    }
    s
}

/// Set a pj_str_t from a buffer with explicit length (alias for pj_strset).
#[no_mangle]
pub unsafe extern "C" fn pj_strset3(
    s: *mut pj_str_t,
    begin: *mut libc::c_char,
    end: *mut libc::c_char,
) -> *mut pj_str_t {
    if !s.is_null() {
        (*s).ptr = begin;
        (*s).slen = end.offset_from(begin) as isize;
    }
    s
}
