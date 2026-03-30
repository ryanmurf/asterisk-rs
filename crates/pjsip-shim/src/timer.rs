//! pj_timer_heap -- timer heap operations.
//!
//! pjproject uses a timer heap for scheduling delayed callbacks.
//! We implement a minimal version backed by BTreeMap.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::*;

/// Timer entry callback.
pub type pj_timer_heap_callback =
    unsafe extern "C" fn(timer_heap: *mut pj_timer_heap_t, entry: *mut pj_timer_entry);

/// Timer entry -- matches C layout with PJ_TIMER_USE_COPY=1 (32 bytes).
///
/// C layout:
///   offset  0: user_data (void*, 8)
///   offset  8: id (int, 4)
///   offset 12: padding (4)
///   offset 16: cb (function pointer, 8)
///   offset 24: _timer_id (pj_timer_id_t = int, 4)
///   offset 28: padding (4)
///   total: 32
#[repr(C)]
pub struct pj_timer_entry {
    pub user_data: *mut libc::c_void,  // offset 0
    pub id: i32,                       // offset 8
    _pad0: i32,                        // offset 12
    pub cb: Option<pj_timer_heap_callback>, // offset 16
    pub _timer_id: i32,                // offset 24
    _pad1: i32,                        // offset 28
}

/// Time value (seconds + milliseconds).
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct pj_time_val {
    pub sec: libc::c_long,
    pub msec: libc::c_long,
}

/// Opaque timer heap.
#[repr(C)]
pub struct pj_timer_heap_t {
    _opaque: [u8; 0],
}

/// Per-timer data stored in the heap (since PJ_TIMER_USE_COPY=1 means
/// _timer_value and _grp_lock aren't in the entry struct).
struct TimerData {
    entry_ptr: *mut pj_timer_entry,
    timer_value: pj_time_val,
    grp_lock: *mut libc::c_void,
}

struct TimerHeapInner {
    data: std::sync::Mutex<TimerHeapData>,
}

struct TimerHeapData {
    next_id: i32,
    max_timed_out: u32,
    entries: BTreeMap<i32, TimerData>,
}

// ---------------------------------------------------------------------------
// pj_timer_entry_init
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_entry_init(
    entry: *mut pj_timer_entry,
    id: i32,
    user_data: *mut libc::c_void,
    cb: Option<pj_timer_heap_callback>,
) {
    if entry.is_null() {
        return;
    }
    std::ptr::write_bytes(entry as *mut u8, 0, std::mem::size_of::<pj_timer_entry>());
    (*entry).id = id;
    (*entry)._timer_id = -1; // not active initially
    (*entry).user_data = user_data;
    (*entry).cb = cb;
}

// ---------------------------------------------------------------------------
// Heap create / destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_create(
    _pool: *mut pj_pool_t,
    _max_count: usize,
    p_heap: *mut *mut pj_timer_heap_t,
) -> pj_status_t {
    if p_heap.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(TimerHeapInner {
        data: std::sync::Mutex::new(TimerHeapData {
            next_id: 1,
            max_timed_out: 64,
            entries: BTreeMap::new(),
        }),
    });
    *p_heap = Box::into_raw(inner) as *mut pj_timer_heap_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_destroy(heap: *mut pj_timer_heap_t) {
    if !heap.is_null() {
        let _ = Box::from_raw(heap as *mut TimerHeapInner);
    }
}

// ---------------------------------------------------------------------------
// Schedule / cancel
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_schedule(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
    delay: *const pj_time_val,
) -> pj_status_t {
    if heap.is_null() || entry.is_null() || delay.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let mut data = inner.data.lock().unwrap();

    let id = data.next_id;
    data.next_id += 1;
    (*entry)._timer_id = id;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut tv = pj_time_val {
        sec: now.as_secs() as libc::c_long + (*delay).sec,
        msec: (now.subsec_millis() as libc::c_long) + (*delay).msec,
    };
    if tv.msec >= 1000 {
        tv.sec += tv.msec / 1000;
        tv.msec %= 1000;
    }

    data.entries.insert(id, TimerData {
        entry_ptr: entry,
        timer_value: tv,
        grp_lock: std::ptr::null_mut(),
    });

    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_schedule_w_grp_lock(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
    delay: *const pj_time_val,
    id_val: i32,
    grp_lock: *mut libc::c_void,
) -> pj_status_t {
    if heap.is_null() || entry.is_null() || delay.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let mut data = inner.data.lock().unwrap();

    let id = data.next_id;
    data.next_id += 1;
    (*entry)._timer_id = id;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut tv = pj_time_val {
        sec: now.as_secs() as libc::c_long + (*delay).sec,
        msec: (now.subsec_millis() as libc::c_long) + (*delay).msec,
    };
    if tv.msec >= 1000 {
        tv.sec += tv.msec / 1000;
        tv.msec %= 1000;
    }

    let mut td = TimerData {
        entry_ptr: entry,
        timer_value: tv,
        grp_lock,
    };

    if id_val != 0 {
        (*entry)._timer_id = id_val;
        data.entries.insert(id_val, td);
    } else {
        data.entries.insert(id, td);
    }

    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_cancel(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
) -> i32 {
    if heap.is_null() || entry.is_null() {
        return 0;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let mut data = inner.data.lock().unwrap();
    let id = (*entry)._timer_id;
    if data.entries.remove(&id).is_some() {
        (*entry)._timer_id = -1;
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_cancel_if_active(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
    id_val: i32,
) -> i32 {
    if heap.is_null() || entry.is_null() {
        return 0;
    }
    if (*entry)._timer_id < 0 {
        return 0;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let mut data = inner.data.lock().unwrap();
    let id = (*entry)._timer_id;
    if data.entries.remove(&id).is_some() {
        (*entry)._timer_id = id_val;
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_count(heap: *mut pj_timer_heap_t) -> usize {
    if heap.is_null() {
        return 0;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let data = inner.data.lock().unwrap();
    data.entries.len()
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_earliest_time(
    heap: *mut pj_timer_heap_t,
    time: *mut pj_time_val,
) -> pj_status_t {
    if heap.is_null() || time.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let data = inner.data.lock().unwrap();
    if let Some((_id, td)) = data.entries.iter().next() {
        (*time) = td.timer_value;
        PJ_SUCCESS
    } else {
        PJ_ENOTFOUND
    }
}

// ---------------------------------------------------------------------------
// Poll
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_poll(
    heap: *mut pj_timer_heap_t,
    _next_delay: *mut pj_time_val,
) -> i32 {
    if heap.is_null() {
        return 0;
    }
    let inner = &*(heap as *const TimerHeapInner);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_sec = now.as_secs() as libc::c_long;
    let now_msec = now.subsec_millis() as libc::c_long;

    // Collect expired entries under the lock
    let to_fire: Vec<(i32, *mut pj_timer_entry)>;
    {
        let mut data = inner.data.lock().unwrap();
        let max = data.max_timed_out as i32;
        let mut fired = 0i32;
        let mut ids = Vec::new();

        for (&id, td) in data.entries.iter() {
            if fired >= max {
                break;
            }
            let tv = td.timer_value;
            if tv.sec < now_sec || (tv.sec == now_sec && tv.msec <= now_msec) {
                ids.push(id);
                fired += 1;
            }
        }

        to_fire = ids.iter().filter_map(|&id| {
            data.entries.remove(&id).map(|td| (id, td.entry_ptr))
        }).collect();
    }
    // Release the lock before calling callbacks to avoid deadlocks

    let fired = to_fire.len() as i32;
    for (_id, entry_ptr) in to_fire {
        (*entry_ptr)._timer_id = -1;
        if let Some(cb) = (*entry_ptr).cb {
            cb(heap, entry_ptr);
        }
    }

    fired
}

// ---------------------------------------------------------------------------
// Set max timed out per poll
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_set_max_timed_out_per_poll(
    heap: *mut pj_timer_heap_t,
    count: u32,
) {
    if heap.is_null() {
        return;
    }
    let inner = &*(heap as *const TimerHeapInner);
    let mut data = inner.data.lock().unwrap();
    data.max_timed_out = count;
}

// ---------------------------------------------------------------------------
// Additional timer heap functions needed by pjlib-test
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_mem_size(_count: usize) -> usize {
    // Return a reasonable fixed size
    std::mem::size_of::<TimerHeapInner>() + 256
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_schedule_dbg(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
    delay: *const pj_time_val,
    _src_file: *const libc::c_char,
    _src_line: i32,
) -> pj_status_t {
    pj_timer_heap_schedule(heap, entry, delay)
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_schedule_w_grp_lock_dbg(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
    delay: *const pj_time_val,
    id_val: i32,
    grp_lock: *mut libc::c_void,
    _src_file: *const libc::c_char,
    _src_line: i32,
) -> pj_status_t {
    pj_timer_heap_schedule_w_grp_lock(heap, entry, delay, id_val, grp_lock)
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_set_lock(
    _heap: *mut pj_timer_heap_t,
    _lock: *mut libc::c_void,
    _auto_del: i32,
) {
    // no-op: our timer heap doesn't need an external lock
}
