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

/// Timer entry.
#[repr(C)]
pub struct pj_timer_entry {
    pub prev: *mut pj_timer_entry,
    pub next: *mut pj_timer_entry,
    pub _timer_id: i32,
    pub cb: Option<pj_timer_heap_callback>,
    pub user_data: *mut libc::c_void,
    pub _timer_value: pj_time_val,
    pub _grp_lock: *mut libc::c_void,
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

struct TimerHeapInner {
    next_id: i32,
    max_timed_out: u32,
    entries: BTreeMap<i32, *mut pj_timer_entry>,
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
    (*entry)._timer_id = id;
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
        next_id: 1,
        max_timed_out: 64,
        entries: BTreeMap::new(),
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
    let inner = &mut *(heap as *mut TimerHeapInner);
    let id = inner.next_id;
    inner.next_id += 1;
    (*entry)._timer_id = id;

    // Calculate absolute time
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (*entry)._timer_value.sec = now.as_secs() as libc::c_long + (*delay).sec;
    (*entry)._timer_value.msec = (now.subsec_millis() as libc::c_long) + (*delay).msec;
    // Normalize
    if (*entry)._timer_value.msec >= 1000 {
        (*entry)._timer_value.sec += (*entry)._timer_value.msec / 1000;
        (*entry)._timer_value.msec %= 1000;
    }

    inner.entries.insert(id, entry);
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
    if entry.is_null() {
        return PJ_EINVAL;
    }
    (*entry)._grp_lock = grp_lock;
    let status = pj_timer_heap_schedule(heap, entry, delay);
    if status == PJ_SUCCESS && id_val != 0 {
        (*entry)._timer_id = id_val;
    }
    status
}

#[no_mangle]
pub unsafe extern "C" fn pj_timer_heap_cancel(
    heap: *mut pj_timer_heap_t,
    entry: *mut pj_timer_entry,
) -> i32 {
    if heap.is_null() || entry.is_null() {
        return 0;
    }
    let inner = &mut *(heap as *mut TimerHeapInner);
    let id = (*entry)._timer_id;
    if inner.entries.remove(&id).is_some() {
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
    let result = pj_timer_heap_cancel(heap, entry);
    if result > 0 {
        (*entry)._timer_id = id_val;
    }
    result
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
    inner.entries.len()
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
    if let Some((_id, &entry_ptr)) = inner.entries.iter().next() {
        (*time) = (*entry_ptr)._timer_value;
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
    let inner = &mut *(heap as *mut TimerHeapInner);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_sec = now.as_secs() as libc::c_long;
    let now_msec = now.subsec_millis() as libc::c_long;

    let mut fired = 0i32;
    let max = inner.max_timed_out as i32;
    let mut to_fire = Vec::new();

    for (&id, &entry_ptr) in inner.entries.iter() {
        if fired >= max {
            break;
        }
        let tv = (*entry_ptr)._timer_value;
        if tv.sec < now_sec || (tv.sec == now_sec && tv.msec <= now_msec) {
            to_fire.push(id);
            fired += 1;
        }
    }

    for id in to_fire {
        if let Some(entry_ptr) = inner.entries.remove(&id) {
            (*entry_ptr)._timer_id = -1;
            if let Some(cb) = (*entry_ptr).cb {
                cb(heap, entry_ptr);
            }
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
    let inner = &mut *(heap as *mut TimerHeapInner);
    inner.max_timed_out = count;
}
