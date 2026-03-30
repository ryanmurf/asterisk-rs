//! pj_time -- time functions using std::time.
//!
//! Provides gettimeofday, elapsed time calculations, time encoding/decoding,
//! and high-resolution timestamps.

use crate::timer::pj_time_val;
use crate::types::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// Parsed time structure matching pj_parsed_time.
#[repr(C)]
#[derive(Default)]
pub struct pj_parsed_time {
    pub wday: i32,
    pub day: i32,
    pub mon: i32,
    pub year: i32,
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub msec: i32,
}

/// High-resolution timestamp.
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct pj_timestamp {
    pub u64_val: u64,
}

// ---------------------------------------------------------------------------
// pj_gettimeofday
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_gettimeofday(tv: *mut pj_time_val) -> pj_status_t {
    if tv.is_null() {
        return PJ_EINVAL;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (*tv).sec = now.as_secs() as libc::c_long;
    (*tv).msec = now.subsec_millis() as libc::c_long;
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// pj_time_val_normalize
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_time_val_normalize(tv: *mut pj_time_val) {
    if tv.is_null() {
        return;
    }
    if (*tv).msec >= 1000 {
        (*tv).sec += (*tv).msec / 1000;
        (*tv).msec %= 1000;
    } else if (*tv).msec < 0 {
        // Borrow from seconds
        let borrow = (-(*tv).msec + 999) / 1000;
        (*tv).sec -= borrow;
        (*tv).msec += borrow * 1000;
    }
}

// ---------------------------------------------------------------------------
// pj_time_encode / pj_time_decode
// ---------------------------------------------------------------------------

/// Encode a parsed time into a pj_time_val (simplified).
#[no_mangle]
pub unsafe extern "C" fn pj_time_encode(
    pt: *const pj_parsed_time,
    tv: *mut pj_time_val,
) -> pj_status_t {
    if pt.is_null() || tv.is_null() {
        return PJ_EINVAL;
    }
    // Simplified encoding using a rough calculation
    let days_since_epoch = {
        let y = (*pt).year as i64;
        let m = (*pt).mon as i64; // 0-based
        let d = (*pt).day as i64;
        // Rough calculation
        (y - 1970) * 365 + (y - 1969) / 4 + m * 30 + d
    };
    let secs = days_since_epoch * 86400
        + (*pt).hour as i64 * 3600
        + (*pt).min as i64 * 60
        + (*pt).sec as i64;
    (*tv).sec = secs as libc::c_long;
    (*tv).msec = (*pt).msec as libc::c_long;
    PJ_SUCCESS
}

/// Decode a pj_time_val into a parsed time (simplified).
#[no_mangle]
pub unsafe extern "C" fn pj_time_decode(
    tv: *const pj_time_val,
    pt: *mut pj_parsed_time,
) -> pj_status_t {
    if tv.is_null() || pt.is_null() {
        return PJ_EINVAL;
    }
    let secs = (*tv).sec as i64;
    let days = secs / 86400;
    let remaining = secs % 86400;

    (*pt).hour = (remaining / 3600) as i32;
    (*pt).min = ((remaining % 3600) / 60) as i32;
    (*pt).sec = (remaining % 60) as i32;
    (*pt).msec = (*tv).msec as i32;
    (*pt).wday = ((days + 4) % 7) as i32; // Jan 1 1970 was Thursday

    // Rough year/month/day from day count
    let mut y = 1970i32;
    let mut d = days;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366i64
        } else {
            365i64
        };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        y += 1;
    }
    (*pt).year = y;

    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_months = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0i32;
    for &dm in &days_in_months {
        if d < dm {
            break;
        }
        d -= dm;
        m += 1;
    }
    (*pt).mon = m;
    (*pt).day = d as i32;

    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Local / GMT conversion (stubs)
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_time_local_to_gmt(tv: *mut pj_time_val) -> pj_status_t {
    // Stub: assume UTC
    let _ = tv;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_time_gmt_to_local(tv: *mut pj_time_val) -> pj_status_t {
    // Stub: assume UTC
    let _ = tv;
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Elapsed time
// ---------------------------------------------------------------------------

/// pj_elapsed_time returns a pj_time_val (sec/msec) from two pj_timestamps.
/// C signature: pj_time_val pj_elapsed_time(const pj_timestamp *start,
///                                          const pj_timestamp *stop);
/// Returns by value (sec, msec struct).
#[no_mangle]
pub unsafe extern "C" fn pj_elapsed_time(
    start: *const pj_timestamp,
    stop: *const pj_timestamp,
) -> pj_time_val {
    if start.is_null() || stop.is_null() {
        return pj_time_val { sec: 0, msec: 0 };
    }
    let start_ns = (*start).u64_val;
    let stop_ns = (*stop).u64_val;
    let diff_ns = if stop_ns > start_ns { stop_ns - start_ns } else { 0 };
    let sec = (diff_ns / 1_000_000_000) as libc::c_long;
    let msec = ((diff_ns % 1_000_000_000) / 1_000_000) as libc::c_long;
    pj_time_val { sec, msec }
}

#[no_mangle]
pub unsafe extern "C" fn pj_elapsed_msec(
    start: *const pj_timestamp,
    stop: *const pj_timestamp,
) -> u32 {
    if start.is_null() || stop.is_null() {
        return 0;
    }
    let start_ns = (*start).u64_val;
    let stop_ns = (*stop).u64_val;
    let diff_ns = if stop_ns > start_ns { stop_ns - start_ns } else { 0 };
    (diff_ns / 1_000_000) as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_elapsed_usec(
    start: *const pj_timestamp,
    stop: *const pj_timestamp,
) -> u32 {
    if start.is_null() || stop.is_null() {
        return 0;
    }
    let start_ns = (*start).u64_val;
    let stop_ns = (*stop).u64_val;
    let diff_ns = if stop_ns > start_ns { stop_ns - start_ns } else { 0 };
    (diff_ns / 1_000) as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_elapsed_nanosec(
    start: *const pj_timestamp,
    stop: *const pj_timestamp,
) -> u32 {
    if start.is_null() || stop.is_null() {
        return 0;
    }
    let start_ns = (*start).u64_val;
    let stop_ns = (*stop).u64_val;
    let diff_ns = if stop_ns > start_ns { stop_ns - start_ns } else { 0 };
    diff_ns as u32
}

// ---------------------------------------------------------------------------
// High-resolution timestamp
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_get_timestamp(ts: *mut pj_timestamp) -> pj_status_t {
    if ts.is_null() {
        return PJ_EINVAL;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (*ts).u64_val = now.as_nanos() as u64;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_get_timestamp_freq(freq: *mut pj_timestamp) -> pj_status_t {
    if freq.is_null() {
        return PJ_EINVAL;
    }
    (*freq).u64_val = 1_000_000_000; // nanosecond frequency
    PJ_SUCCESS
}

// ---------------------------------------------------------------------------
// Additional time functions needed by pjlib-test
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_elapsed_cycle(
    start: *const pj_timestamp,
    stop: *const pj_timestamp,
) -> u32 {
    if start.is_null() || stop.is_null() {
        return 0;
    }
    ((*stop).u64_val.wrapping_sub((*start).u64_val)) as u32
}

#[no_mangle]
pub unsafe extern "C" fn pj_gettickcount(tv: *mut pj_time_val) -> pj_status_t {
    if tv.is_null() {
        return PJ_EINVAL;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (*tv).sec = now.as_secs() as libc::c_long;
    (*tv).msec = now.subsec_millis() as libc::c_long;
    PJ_SUCCESS
}
