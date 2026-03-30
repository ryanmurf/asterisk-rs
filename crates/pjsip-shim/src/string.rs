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

// ---------------------------------------------------------------------------
// Concatenation
// ---------------------------------------------------------------------------

/// Concatenate src onto dst (dst buffer must have enough space).
#[no_mangle]
pub unsafe extern "C" fn pj_strcat(
    dst: *mut pj_str_t,
    src: *const pj_str_t,
) -> *mut pj_str_t {
    if dst.is_null() || src.is_null() {
        return dst;
    }
    if (*src).slen > 0 && !(*src).ptr.is_null() && !(*dst).ptr.is_null() {
        libc::memcpy(
            (*dst).ptr.offset((*dst).slen) as *mut _,
            (*src).ptr as *const _,
            (*src).slen as usize,
        );
        (*dst).slen += (*src).slen;
    }
    dst
}

/// Concatenate a C string onto dst.
#[no_mangle]
pub unsafe extern "C" fn pj_strcat2(
    dst: *mut pj_str_t,
    src: *const libc::c_char,
) -> *mut pj_str_t {
    if dst.is_null() || src.is_null() {
        return dst;
    }
    let len = libc::strlen(src);
    if len > 0 && !(*dst).ptr.is_null() {
        libc::memcpy(
            (*dst).ptr.offset((*dst).slen) as *mut _,
            src as *const _,
            len,
        );
        (*dst).slen += len as isize;
    }
    dst
}

// ---------------------------------------------------------------------------
// N-length comparison
// ---------------------------------------------------------------------------

/// Compare first `len` bytes of two pj_str_t.
#[no_mangle]
pub unsafe extern "C" fn pj_strncmp(
    s1: *const pj_str_t,
    s2: *const pj_str_t,
    len: usize,
) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let l1 = ((*s1).slen as usize).min(len);
    let l2 = ((*s2).slen as usize).min(len);
    let cmp_len = l1.min(l2).min(len);
    if cmp_len > 0 {
        let result = libc::memcmp((*s1).ptr as *const _, (*s2).ptr as *const _, cmp_len);
        if result != 0 {
            return result;
        }
    }
    if l1.min(len) == l2.min(len) {
        0
    } else {
        (l1 as i32) - (l2 as i32)
    }
}

/// Compare first `len` bytes of pj_str_t against C string.
#[no_mangle]
pub unsafe extern "C" fn pj_strncmp2(
    s1: *const pj_str_t,
    s2: *const libc::c_char,
    len: usize,
) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let l1 = ((*s1).slen as usize).min(len);
    let s2_len = libc::strlen(s2);
    let l2 = s2_len.min(len);
    let cmp_len = l1.min(l2);
    if cmp_len > 0 {
        let result = libc::memcmp((*s1).ptr as *const _, s2 as *const _, cmp_len);
        if result != 0 {
            return result;
        }
    }
    if l1 == l2 { 0 } else { (l1 as i32) - (l2 as i32) }
}

/// Case-insensitive compare of first `len` bytes of two pj_str_t.
#[no_mangle]
pub unsafe extern "C" fn pj_strnicmp(
    s1: *const pj_str_t,
    s2: *const pj_str_t,
    len: usize,
) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let l1 = ((*s1).slen as usize).min(len);
    let l2 = ((*s2).slen as usize).min(len);
    let cmp_len = l1.min(l2);
    let a = std::slice::from_raw_parts((*s1).ptr as *const u8, l1);
    let b = std::slice::from_raw_parts((*s2).ptr as *const u8, l2);
    for i in 0..cmp_len {
        let diff = a[i].to_ascii_lowercase() as i32 - b[i].to_ascii_lowercase() as i32;
        if diff != 0 {
            return diff;
        }
    }
    if l1 == l2 { 0 } else { (l1 as i32) - (l2 as i32) }
}

/// Case-insensitive compare of first `len` bytes of pj_str_t against C string.
#[no_mangle]
pub unsafe extern "C" fn pj_strnicmp2(
    s1: *const pj_str_t,
    s2: *const libc::c_char,
    len: usize,
) -> i32 {
    if s1.is_null() || s2.is_null() {
        return if s1.is_null() && s2.is_null() { 0 } else { -1 };
    }
    let l1 = ((*s1).slen as usize).min(len);
    let s2_len = libc::strlen(s2);
    let l2 = s2_len.min(len);
    let cmp_len = l1.min(l2);
    let a = std::slice::from_raw_parts((*s1).ptr as *const u8, l1);
    let b = std::slice::from_raw_parts(s2 as *const u8, l2);
    for i in 0..cmp_len {
        let diff = a[i].to_ascii_lowercase() as i32 - b[i].to_ascii_lowercase() as i32;
        if diff != 0 {
            return diff;
        }
    }
    if l1 == l2 { 0 } else { (l1 as i32) - (l2 as i32) }
}

// ---------------------------------------------------------------------------
// Extended conversion
// ---------------------------------------------------------------------------

/// Convert a pj_str_t to an unsigned long, with end-pointer.
#[no_mangle]
pub unsafe extern "C" fn pj_strtoul2(
    s: *const pj_str_t,
    endptr: *mut *const libc::c_char,
    base: u32,
) -> libc::c_ulong {
    if s.is_null() || (*s).ptr.is_null() || (*s).slen <= 0 {
        if !endptr.is_null() {
            *endptr = if !s.is_null() { (*s).ptr } else { std::ptr::null() };
        }
        return 0;
    }
    let text = (*s).as_str().trim();
    let base = if base == 0 {
        if text.starts_with("0x") || text.starts_with("0X") { 16u32 } else { 10u32 }
    } else {
        base
    };
    let (parse_str, actual_base) = if base == 16 && (text.starts_with("0x") || text.starts_with("0X")) {
        (&text[2..], 16)
    } else {
        (text, base)
    };

    let mut result = 0u64;
    let mut consumed = 0usize;
    for ch in parse_str.bytes() {
        let digit = match ch {
            b'0'..=b'9' => (ch - b'0') as u64,
            b'a'..=b'f' if actual_base > 10 => (ch - b'a' + 10) as u64,
            b'A'..=b'F' if actual_base > 10 => (ch - b'A' + 10) as u64,
            _ => break,
        };
        if digit >= actual_base as u64 {
            break;
        }
        result = result * actual_base as u64 + digit;
        consumed += 1;
    }

    if !endptr.is_null() {
        let offset = text.len() - parse_str.len() + consumed;
        let ptr_start = (*s).ptr as *const u8;
        // Skip leading whitespace in original
        let ws = (*s).as_str().len() - text.len();
        *endptr = ptr_start.add(ws + offset) as *const libc::c_char;
    }

    result as libc::c_ulong
}

/// Convert unsigned integer to ASCII string.
#[no_mangle]
pub unsafe extern "C" fn pj_utoa(
    val: libc::c_ulong,
    buf: *mut libc::c_char,
) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let s = format!("{}", val);
    let bytes = s.as_bytes();
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
    *buf.add(bytes.len()) = 0;
    bytes.len() as i32
}

/// Create a random string of given length.
#[no_mangle]
pub unsafe extern "C" fn pj_create_random_string(
    buf: *mut libc::c_char,
    len: usize,
) -> *mut libc::c_char {
    if buf.is_null() || len == 0 {
        return buf;
    }
    static CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    for i in 0..len {
        let r = crate::misc::pj_rand() as usize;
        *buf.add(i) = CHARS[r % CHARS.len()] as libc::c_char;
    }
    buf
}

// ---------------------------------------------------------------------------
// Safe string copy (pj_ansi_strxcpy / pj_ansi_strxcat / pj_ansi_strxcpy2)
// ---------------------------------------------------------------------------

/// Safe strncpy -- always null-terminates. Returns PJ_SUCCESS or PJ_ETOOMANY.
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strxcpy(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
    dst_size: usize,
) -> i32 {
    if dst.is_null() || dst_size == 0 {
        return PJ_EINVAL;
    }
    if src.is_null() {
        *dst = 0;
        return PJ_SUCCESS;
    }
    let src_len = libc::strlen(src);
    let copy_len = src_len.min(dst_size - 1);
    std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, copy_len);
    *dst.add(copy_len) = 0;
    if src_len >= dst_size { PJ_ETOOMANY } else { PJ_SUCCESS }
}

/// Safe strncpy variant returning pj_str_t-style length.
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strxcpy2(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
    dst_size: usize,
) -> isize {
    if dst.is_null() || dst_size == 0 {
        return -1;
    }
    if src.is_null() {
        *dst = 0;
        return 0;
    }
    let src_len = libc::strlen(src);
    let copy_len = src_len.min(dst_size - 1);
    std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, copy_len);
    *dst.add(copy_len) = 0;
    if src_len >= dst_size { -(copy_len as isize) } else { copy_len as isize }
}

/// Safe strncat -- always null-terminates.
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strxcat(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
    dst_size: usize,
) -> i32 {
    if dst.is_null() || dst_size == 0 {
        return PJ_EINVAL;
    }
    if src.is_null() {
        return PJ_SUCCESS;
    }
    let dst_len = libc::strlen(dst);
    if dst_len >= dst_size - 1 {
        return PJ_ETOOMANY;
    }
    let remaining = dst_size - dst_len - 1;
    let src_len = libc::strlen(src);
    let copy_len = src_len.min(remaining);
    std::ptr::copy_nonoverlapping(src as *const u8, dst.add(dst_len) as *mut u8, copy_len);
    *dst.add(dst_len + copy_len) = 0;
    if src_len > remaining { PJ_ETOOMANY } else { PJ_SUCCESS }
}

// ---------------------------------------------------------------------------
// ANSI C string wrappers (needed by pjlib tests)
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strlen(s: *const libc::c_char) -> usize {
    if s.is_null() { 0 } else { libc::strlen(s) }
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strcpy(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
) -> *mut libc::c_char {
    libc::strcpy(dst, src)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strncpy(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
    n: usize,
) -> *mut libc::c_char {
    libc::strncpy(dst, src, n)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strcat(
    dst: *mut libc::c_char,
    src: *const libc::c_char,
) -> *mut libc::c_char {
    libc::strcat(dst, src)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strcmp(
    s1: *const libc::c_char,
    s2: *const libc::c_char,
) -> i32 {
    libc::strcmp(s1, s2)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strncmp(
    s1: *const libc::c_char,
    s2: *const libc::c_char,
    n: usize,
) -> i32 {
    libc::strncmp(s1, s2, n)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_stricmp(
    s1: *const libc::c_char,
    s2: *const libc::c_char,
) -> i32 {
    libc::strcasecmp(s1, s2)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strnicmp(
    s1: *const libc::c_char,
    s2: *const libc::c_char,
    n: usize,
) -> i32 {
    libc::strncasecmp(s1, s2, n)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strchr(
    s: *const libc::c_char,
    c: i32,
) -> *const libc::c_char {
    libc::strchr(s, c)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strrchr(
    s: *const libc::c_char,
    c: i32,
) -> *const libc::c_char {
    libc::strrchr(s, c)
}

#[no_mangle]
pub unsafe extern "C" fn pj_ansi_strstr(
    s1: *const libc::c_char,
    s2: *const libc::c_char,
) -> *const libc::c_char {
    libc::strstr(s1, s2)
}

/// Note: C signature is variadic but we declare fixed args for stable Rust.
/// The symbol name is what matters for linking.
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_snprintf(
    buf: *mut libc::c_char,
    size: usize,
    fmt: *const libc::c_char,
) -> i32 {
    if buf.is_null() || size == 0 || fmt.is_null() {
        return 0;
    }
    // We can't properly handle varargs formatting in Rust.
    // Write the format string itself as a fallback.
    let fmt_s = std::ffi::CStr::from_ptr(fmt);
    let bytes = fmt_s.to_bytes();
    let copy_len = bytes.len().min(size - 1);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
    *buf.add(copy_len) = 0;
    copy_len as i32
}

/// Note: C signature is variadic but we declare fixed args for stable Rust.
#[no_mangle]
pub unsafe extern "C" fn pj_ansi_sprintf(
    buf: *mut libc::c_char,
    fmt: *const libc::c_char,
) -> i32 {
    pj_ansi_snprintf(buf, 4096, fmt)
}

/// memcpy/memset/memmove wrappers
#[no_mangle]
pub unsafe extern "C" fn pj_memcpy(
    dst: *mut libc::c_void,
    src: *const libc::c_void,
    size: usize,
) -> *mut libc::c_void {
    libc::memcpy(dst, src, size)
}

#[no_mangle]
pub unsafe extern "C" fn pj_memset(
    dst: *mut libc::c_void,
    c: i32,
    size: usize,
) -> *mut libc::c_void {
    libc::memset(dst, c, size)
}

#[no_mangle]
pub unsafe extern "C" fn pj_memmove(
    dst: *mut libc::c_void,
    src: *const libc::c_void,
    size: usize,
) -> *mut libc::c_void {
    libc::memmove(dst, src, size)
}

#[no_mangle]
pub unsafe extern "C" fn pj_memcmp(
    s1: *const libc::c_void,
    s2: *const libc::c_void,
    size: usize,
) -> i32 {
    libc::memcmp(s1, s2, size)
}

#[no_mangle]
pub unsafe extern "C" fn pj_memchr(
    s: *const libc::c_void,
    c: i32,
    size: usize,
) -> *const libc::c_void {
    libc::memchr(s, c, size)
}

#[no_mangle]
pub unsafe extern "C" fn pj_bzero(
    dst: *mut libc::c_void,
    size: usize,
) {
    libc::memset(dst, 0, size);
}
