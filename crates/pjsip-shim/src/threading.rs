//! pj_thread / pj_mutex / pj_sem / pj_rwmutex / pj_lock -- threading primitives.
//!
//! These wrap std::thread, parking_lot, and friends to provide the C-callable
//! threading API that pjproject expects.

use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};

// ============================================================================
// Threads
// ============================================================================

/// Opaque thread descriptor.
#[repr(C)]
pub struct pj_thread_t {
    _opaque: [u8; 0],
}

/// Thread procedure type.
pub type pj_thread_proc = unsafe extern "C" fn(arg: *mut libc::c_void) -> i32;

struct ThreadInner {
    name: String,
    handle: Option<std::thread::JoinHandle<i32>>,
    _registered: bool,
}

// Thread-local for "this thread" lookup.
// We store as usize to avoid Send issues with raw pointers in thread locals.
std::thread_local! {
    static CURRENT_THREAD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

static MAIN_REGISTERED: AtomicBool = AtomicBool::new(false);
static mut MAIN_THREAD: *mut pj_thread_t = std::ptr::null_mut();

#[no_mangle]
pub unsafe extern "C" fn pj_thread_create(
    _pool: *mut pj_pool_t,
    name: *const libc::c_char,
    proc_: Option<pj_thread_proc>,
    arg: *mut libc::c_void,
    _stack_size: usize,
    _flags: u32,
    p_thread: *mut *mut pj_thread_t,
) -> pj_status_t {
    if p_thread.is_null() {
        return PJ_EINVAL;
    }
    let proc_ = match proc_ {
        Some(f) => f,
        None => return PJ_EINVAL,
    };

    let name_str = if name.is_null() {
        "pj-thread".to_string()
    } else {
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    };

    // SAFETY: arg is a raw pointer passed to the thread; the caller is
    // responsible for ensuring it outlives the thread.
    let arg_send = arg as usize;

    let inner = Box::new(ThreadInner {
        name: name_str.clone(),
        handle: None,
        _registered: false,
    });
    let thread_ptr = Box::into_raw(inner) as *mut pj_thread_t;

    // Convert everything to usize to avoid Send issues with raw pointers.
    let thread_ptr_usize = thread_ptr as usize;
    let proc_usize = proc_ as usize;

    let builder = std::thread::Builder::new().name(name_str);
    let handle = builder
        .spawn(move || {
            CURRENT_THREAD.with(|c| {
                c.set(thread_ptr_usize);
            });
            let func: pj_thread_proc = std::mem::transmute(proc_usize);
            func(arg_send as *mut libc::c_void)
        });

    match handle {
        Ok(jh) => {
            let inner = &mut *(thread_ptr as *mut ThreadInner);
            inner.handle = Some(jh);
            *p_thread = thread_ptr;
            PJ_SUCCESS
        }
        Err(_) => {
            let _ = Box::from_raw(thread_ptr as *mut ThreadInner);
            PJ_ENOMEM
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_destroy(thread: *mut pj_thread_t) {
    if !thread.is_null() {
        let _ = Box::from_raw(thread as *mut ThreadInner);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_register(
    name: *const libc::c_char,
    _desc: *mut libc::c_void, // pj_thread_desc (stack space)
    p_thread: *mut *mut pj_thread_t,
) -> pj_status_t {
    if p_thread.is_null() {
        return PJ_EINVAL;
    }
    let name_str = if name.is_null() {
        "registered".to_string()
    } else {
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    };
    let inner = Box::new(ThreadInner {
        name: name_str,
        handle: None,
        _registered: true,
    });
    let ptr = Box::into_raw(inner) as *mut pj_thread_t;
    CURRENT_THREAD.with(|c| {
        c.set(ptr as usize);
    });
    *p_thread = ptr;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_is_registered() -> pj_bool_t {
    let ptr = CURRENT_THREAD.with(|c| c.get());
    if ptr == 0 { PJ_FALSE } else { PJ_TRUE }
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_this() -> *mut pj_thread_t {
    let ptr = CURRENT_THREAD.with(|c| c.get());
    if ptr != 0 {
        return ptr as *mut pj_thread_t;
    }
    // Auto-register main thread
    if !MAIN_REGISTERED.swap(true, Ordering::SeqCst) {
        let inner = Box::new(ThreadInner {
            name: "main".to_string(),
            handle: None,
            _registered: true,
        });
        MAIN_THREAD = Box::into_raw(inner) as *mut pj_thread_t;
        CURRENT_THREAD.with(|c| {
            c.set(MAIN_THREAD as usize);
        });
    }
    MAIN_THREAD
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_get_name(thread: *mut pj_thread_t) -> *const libc::c_char {
    if thread.is_null() {
        return b"unknown\0".as_ptr() as *const _;
    }
    let inner = &*(thread as *const ThreadInner);
    // Return a pointer to the name.  This is safe as long as the thread
    // object is alive (which it should be while the caller uses the name).
    inner.name.as_ptr() as *const libc::c_char
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_join(thread: *mut pj_thread_t) -> pj_status_t {
    if thread.is_null() {
        return PJ_EINVAL;
    }
    let inner = &mut *(thread as *mut ThreadInner);
    if let Some(handle) = inner.handle.take() {
        match handle.join() {
            Ok(_) => PJ_SUCCESS,
            Err(_) => PJ_EINVALIDOP,
        }
    } else {
        PJ_SUCCESS
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_thread_sleep(msec: u32) -> pj_status_t {
    std::thread::sleep(std::time::Duration::from_millis(msec as u64));
    PJ_SUCCESS
}

// ============================================================================
// Mutex
// ============================================================================

#[repr(C)]
pub struct pj_mutex_t {
    _opaque: [u8; 0],
}

enum MutexInner {
    Simple(parking_lot::Mutex<()>),
    Recursive(parking_lot::ReentrantMutex<()>),
}

struct MutexWrapper {
    inner: MutexInner,
    _simple_guard: Option<parking_lot::MutexGuard<'static, ()>>,
    _reentrant_guard: Option<parking_lot::ReentrantMutexGuard<'static, ()>>,
    lock_count: std::sync::atomic::AtomicI32,
}

fn create_mutex(recursive: bool) -> *mut pj_mutex_t {
    let wrapper = Box::new(MutexWrapper {
        inner: if recursive {
            MutexInner::Recursive(parking_lot::ReentrantMutex::new(()))
        } else {
            MutexInner::Simple(parking_lot::Mutex::new(()))
        },
        _simple_guard: None,
        _reentrant_guard: None,
        lock_count: std::sync::atomic::AtomicI32::new(0),
    });
    Box::into_raw(wrapper) as *mut pj_mutex_t
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_create(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    mutex_type: i32,
    p_mutex: *mut *mut pj_mutex_t,
) -> pj_status_t {
    if p_mutex.is_null() {
        return PJ_EINVAL;
    }
    // mutex_type: 1 = simple, 2 = recursive
    *p_mutex = create_mutex(mutex_type == 2);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_create_simple(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_mutex: *mut *mut pj_mutex_t,
) -> pj_status_t {
    if p_mutex.is_null() {
        return PJ_EINVAL;
    }
    *p_mutex = create_mutex(false);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_create_recursive(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_mutex: *mut *mut pj_mutex_t,
) -> pj_status_t {
    if p_mutex.is_null() {
        return PJ_EINVAL;
    }
    *p_mutex = create_mutex(true);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_lock(mutex: *mut pj_mutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let wrapper = &mut *(mutex as *mut MutexWrapper);
    match &wrapper.inner {
        MutexInner::Simple(m) => {
            // We can't store the guard properly, so we use a raw approach:
            // parking_lot::Mutex::lock returns a guard. We leak it on lock
            // and reconstruct on unlock. This is sound but ugly.
            let guard = m.lock();
            std::mem::forget(guard);
        }
        MutexInner::Recursive(m) => {
            let guard = m.lock();
            std::mem::forget(guard);
        }
    }
    wrapper.lock_count.fetch_add(1, Ordering::SeqCst);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_trylock(mutex: *mut pj_mutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let wrapper = &mut *(mutex as *mut MutexWrapper);
    let locked = match &wrapper.inner {
        MutexInner::Simple(m) => {
            if let Some(guard) = m.try_lock() {
                std::mem::forget(guard);
                true
            } else {
                false
            }
        }
        MutexInner::Recursive(m) => {
            if let Some(guard) = m.try_lock() {
                std::mem::forget(guard);
                true
            } else {
                false
            }
        }
    };
    if locked {
        wrapper.lock_count.fetch_add(1, Ordering::SeqCst);
        PJ_SUCCESS
    } else {
        PJ_EBUSY
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_unlock(mutex: *mut pj_mutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let wrapper = &mut *(mutex as *mut MutexWrapper);
    let prev = wrapper.lock_count.fetch_sub(1, Ordering::SeqCst);
    if prev <= 0 {
        wrapper.lock_count.fetch_add(1, Ordering::SeqCst); // restore
        return PJ_EINVALIDOP;
    }
    match &wrapper.inner {
        MutexInner::Simple(m) => {
            m.force_unlock();
        }
        MutexInner::Recursive(m) => {
            m.force_unlock();
        }
    }
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_mutex_destroy(mutex: *mut pj_mutex_t) -> pj_status_t {
    if !mutex.is_null() {
        let _ = Box::from_raw(mutex as *mut MutexWrapper);
    }
    PJ_SUCCESS
}

// ============================================================================
// Semaphore
// ============================================================================

#[repr(C)]
pub struct pj_sem_t {
    _opaque: [u8; 0],
}

struct SemInner {
    count: parking_lot::Mutex<i32>,
    condvar: parking_lot::Condvar,
}

#[no_mangle]
pub unsafe extern "C" fn pj_sem_create(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    initial: u32,
    _max: u32,
    p_sem: *mut *mut pj_sem_t,
) -> pj_status_t {
    if p_sem.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(SemInner {
        count: parking_lot::Mutex::new(initial as i32),
        condvar: parking_lot::Condvar::new(),
    });
    *p_sem = Box::into_raw(inner) as *mut pj_sem_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sem_wait(sem: *mut pj_sem_t) -> pj_status_t {
    if sem.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(sem as *const SemInner);
    let mut count = inner.count.lock();
    while *count <= 0 {
        inner.condvar.wait(&mut count);
    }
    *count -= 1;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sem_trywait(sem: *mut pj_sem_t) -> pj_status_t {
    if sem.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(sem as *const SemInner);
    let mut count = inner.count.lock();
    if *count > 0 {
        *count -= 1;
        PJ_SUCCESS
    } else {
        PJ_EBUSY
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_sem_post(sem: *mut pj_sem_t) -> pj_status_t {
    if sem.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(sem as *const SemInner);
    let mut count = inner.count.lock();
    *count += 1;
    inner.condvar.notify_one();
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_sem_destroy(sem: *mut pj_sem_t) -> pj_status_t {
    if !sem.is_null() {
        let _ = Box::from_raw(sem as *mut SemInner);
    }
    PJ_SUCCESS
}

// ============================================================================
// RW Mutex
// ============================================================================

#[repr(C)]
pub struct pj_rwmutex_t {
    _opaque: [u8; 0],
}

struct RwMutexInner {
    lock: parking_lot::RwLock<()>,
    // Track lock state to know what to unlock
    read_count: std::sync::atomic::AtomicI32,
    write_locked: AtomicBool,
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_create(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_mutex: *mut *mut pj_rwmutex_t,
) -> pj_status_t {
    if p_mutex.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(RwMutexInner {
        lock: parking_lot::RwLock::new(()),
        read_count: std::sync::atomic::AtomicI32::new(0),
        write_locked: AtomicBool::new(false),
    });
    *p_mutex = Box::into_raw(inner) as *mut pj_rwmutex_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_lock_read(mutex: *mut pj_rwmutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(mutex as *const RwMutexInner);
    let guard = inner.lock.read();
    std::mem::forget(guard);
    inner.read_count.fetch_add(1, Ordering::SeqCst);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_lock_write(mutex: *mut pj_rwmutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(mutex as *const RwMutexInner);
    let guard = inner.lock.write();
    std::mem::forget(guard);
    inner.write_locked.store(true, Ordering::SeqCst);
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_unlock_read(mutex: *mut pj_rwmutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(mutex as *const RwMutexInner);
    inner.read_count.fetch_sub(1, Ordering::SeqCst);
    inner.lock.force_unlock_read();
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_unlock_write(mutex: *mut pj_rwmutex_t) -> pj_status_t {
    if mutex.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(mutex as *const RwMutexInner);
    inner.write_locked.store(false, Ordering::SeqCst);
    inner.lock.force_unlock_write();
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_rwmutex_destroy(mutex: *mut pj_rwmutex_t) -> pj_status_t {
    if !mutex.is_null() {
        let _ = Box::from_raw(mutex as *mut RwMutexInner);
    }
    PJ_SUCCESS
}

// ============================================================================
// Lock abstraction
// ============================================================================

/// Opaque lock.
#[repr(C)]
pub struct pj_lock_t {
    _opaque: [u8; 0],
}

/// Lock operations vtable (matches pjproject's pj_lock_t internally).
struct LockInner {
    mutex: *mut pj_mutex_t,
    is_null: bool,
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_create_null_mutex(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_lock: *mut *mut pj_lock_t,
) -> pj_status_t {
    if p_lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = Box::new(LockInner {
        mutex: std::ptr::null_mut(),
        is_null: true,
    });
    *p_lock = Box::into_raw(inner) as *mut pj_lock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_create_simple_mutex(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_lock: *mut *mut pj_lock_t,
) -> pj_status_t {
    if p_lock.is_null() {
        return PJ_EINVAL;
    }
    let mutex = create_mutex(false);
    let inner = Box::new(LockInner {
        mutex,
        is_null: false,
    });
    *p_lock = Box::into_raw(inner) as *mut pj_lock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_create_recursive_mutex(
    _pool: *mut pj_pool_t,
    _name: *const libc::c_char,
    p_lock: *mut *mut pj_lock_t,
) -> pj_status_t {
    if p_lock.is_null() {
        return PJ_EINVAL;
    }
    let mutex = create_mutex(true);
    let inner = Box::new(LockInner {
        mutex,
        is_null: false,
    });
    *p_lock = Box::into_raw(inner) as *mut pj_lock_t;
    PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_acquire(lock: *mut pj_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const LockInner);
    if inner.is_null {
        return PJ_SUCCESS;
    }
    pj_mutex_lock(inner.mutex)
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_tryacquire(lock: *mut pj_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const LockInner);
    if inner.is_null {
        return PJ_SUCCESS;
    }
    pj_mutex_trylock(inner.mutex)
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_release(lock: *mut pj_lock_t) -> pj_status_t {
    if lock.is_null() {
        return PJ_EINVAL;
    }
    let inner = &*(lock as *const LockInner);
    if inner.is_null {
        return PJ_SUCCESS;
    }
    pj_mutex_unlock(inner.mutex)
}

#[no_mangle]
pub unsafe extern "C" fn pj_lock_destroy(lock: *mut pj_lock_t) -> pj_status_t {
    if !lock.is_null() {
        let inner = Box::from_raw(lock as *mut LockInner);
        if !inner.is_null && !inner.mutex.is_null() {
            pj_mutex_destroy(inner.mutex);
        }
    }
    PJ_SUCCESS
}
