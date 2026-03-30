//! pjlib test framework implementation.
//!
//! pjproject's pjlib-test uses a test harness with runners, suites, and cases.
//! We implement these so the test binary can link against our library.

use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Condvar};

/// Tracks whether we are currently inside pj_test_run.
static IS_UNDER_TEST: AtomicBool = AtomicBool::new(false);

/// Fixed-size captured log entry (no heap allocation).
const MAX_CAPTURED_MSG: usize = 256;
const MAX_TC_NAME: usize = 32;
const MAX_CAPTURED_LOGS: usize = 64;

#[derive(Copy, Clone)]
struct CapturedLog {
    tc_name: [u8; MAX_TC_NAME],
    tc_name_len: usize,
    result: i32,
    level: i32,
    msg: [u8; MAX_CAPTURED_MSG],
    msg_len: usize,
}

const EMPTY_LOG: CapturedLog = CapturedLog {
    tc_name: [0u8; MAX_TC_NAME],
    tc_name_len: 0,
    result: 0,
    level: 0,
    msg: [0u8; MAX_CAPTURED_MSG],
    msg_len: 0,
};

/// Global log capture buffer -- uses a static array to avoid heap allocation.
static mut CAPTURED_LOGS: [CapturedLog; MAX_CAPTURED_LOGS] = [EMPTY_LOG; MAX_CAPTURED_LOGS];
static mut CAPTURED_LOG_COUNT: usize = 0;

/// Currently running test case info (set during pj_test_run).
/// Uses thread-local storage for proper per-thread tracking during
/// parallel test execution.
std::thread_local! {
    static CURRENT_TC_NAME_BUF: std::cell::RefCell<[u8; MAX_TC_NAME]> =
        std::cell::RefCell::new([0u8; MAX_TC_NAME]);
    static CURRENT_TC_NAME_LEN: std::cell::Cell<usize> = std::cell::Cell::new(0);
    static CURRENT_TC_LOG_BUF_SIZE: std::cell::Cell<u32> = std::cell::Cell::new(0);
}

/// Set the current test case info on the current thread.
unsafe fn set_current_tc(name: &[u8], log_buf_size: u32) {
    CURRENT_TC_NAME_BUF.with(|buf| {
        let mut b = buf.borrow_mut();
        let copy_len = name.len().min(MAX_TC_NAME - 1);
        b[..copy_len].copy_from_slice(&name[..copy_len]);
        b[copy_len] = 0;
    });
    CURRENT_TC_NAME_LEN.with(|len| len.set(name.len().min(MAX_TC_NAME - 1)));
    CURRENT_TC_LOG_BUF_SIZE.with(|sz| sz.set(log_buf_size));
}

/// Clear the current test case info on the current thread.
unsafe fn clear_current_tc() {
    CURRENT_TC_NAME_LEN.with(|len| len.set(0));
    CURRENT_TC_LOG_BUF_SIZE.with(|sz| sz.set(0));
}

/// Global mutex for protecting CAPTURED_LOGS during parallel execution.
static LOG_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Log intercept callback used during pj_test_run.
/// Uses thread-local storage for test case tracking and a mutex for the global log buffer.
unsafe extern "C" fn test_log_intercept(level: i32, data: *const libc::c_char, len: i32) {
    if data.is_null() || len <= 0 {
        return;
    }

    // Only capture if the test case has a log buffer configured
    let buf_size = CURRENT_TC_LOG_BUF_SIZE.with(|sz| sz.get());
    if buf_size < 10 {
        return;
    }

    let name_len = CURRENT_TC_NAME_LEN.with(|l| l.get());

    // Lock the global capture buffer for thread safety
    let _guard = LOG_CAPTURE_LOCK.lock();

    let idx = CAPTURED_LOG_COUNT;
    if idx >= MAX_CAPTURED_LOGS {
        return;
    }

    let msg_len = (len as usize).min(MAX_CAPTURED_MSG - 1);
    std::ptr::copy_nonoverlapping(data as *const u8, CAPTURED_LOGS[idx].msg.as_mut_ptr(), msg_len);
    CAPTURED_LOGS[idx].msg[msg_len] = 0;
    CAPTURED_LOGS[idx].msg_len = msg_len;
    CAPTURED_LOGS[idx].level = level;
    CAPTURED_LOGS[idx].result = 0;

    CURRENT_TC_NAME_BUF.with(|buf| {
        let b = buf.borrow();
        let copy_len = name_len.min(MAX_TC_NAME - 1);
        std::ptr::copy_nonoverlapping(
            b.as_ptr(),
            CAPTURED_LOGS[idx].tc_name.as_mut_ptr(),
            copy_len,
        );
        CAPTURED_LOGS[idx].tc_name[copy_len] = 0;
        CAPTURED_LOGS[idx].tc_name_len = copy_len;
    });

    CAPTURED_LOG_COUNT = idx + 1;
}

/// Opaque test runner.
#[repr(C)]
pub struct pj_test_runner {
    _opaque: [u8; 0],
}

/// Test suite -- matches the C layout in unittest.h.
/// `tests` is a pj_test_case used as a linked-list sentinel (176 bytes).
#[repr(C)]
pub struct pj_test_suite {
    pub tests: pj_test_case,
    pub start_time: crate::time::pj_timestamp,
    pub end_time: crate::time::pj_timestamp,
}

/// Test case callback type: `int (*test_func)(void *arg)`
pub type pj_test_func = unsafe extern "C" fn(*mut libc::c_void) -> i32;

/// PJ_TEST_EXCLUSIVE flag -- run test exclusively (no parallel tests).
const PJ_TEST_EXCLUSIVE: u32 = 1;
/// PJ_TEST_FUNC_NO_ARG flag -- call test_func with no argument.
const PJ_TEST_FUNC_NO_ARG: u32 = 2;
/// PJ_EPENDING -- test result when test hasn't completed yet.
const PJ_EPENDING: i32 = 70002;

/// Test case -- must match C layout exactly (176 bytes).
///
/// C layout:
///   offset  0: prev (8)
///   offset  8: next (8)
///   offset 16: obj_name[32] (32)
///   offset 48: test_func (8)
///   offset 56: arg (8)
///   offset 64: flags (4)
///   offset 68: fb (pj_fifobuf_t, 40 bytes)
///   offset 108: padding (4)
///   offset 112: prm (4)
///   offset 116: result (4)
///   offset 120: logs (pj_test_log_item, 32 bytes)
///   offset 152: runner (8)
///   offset 160: start_time (8)
///   offset 168: end_time (8)
///   total: 176
#[repr(C)]
pub struct pj_test_case {
    pub prev: *mut pj_test_case,      // offset 0
    pub next: *mut pj_test_case,      // offset 8
    pub obj_name: [libc::c_char; 32], // offset 16
    pub test_func: Option<pj_test_func>, // offset 48
    pub arg: *mut libc::c_void,       // offset 56
    pub flags: u32,                   // offset 64
    pub log_buf_size: u32,            // offset 68: we store buf_size here (start of fb area)
    _pad0: [u8; 44],                  // offset 72: rest of fb + pad + prm
    pub result: i32,                  // offset 116
    _pad1: [u8; 56],                  // offset 120: logs + runner + timestamps
}

/// Test stat -- must match the C layout in unittest.h.
#[repr(C)]
pub struct pj_test_stat {
    pub duration: crate::timer::pj_time_val,
    pub ntests: u32,
    pub nruns: u32,
    pub nfailed: u32,
    pub failed_names: [*const libc::c_char; 32],
}

impl Default for pj_test_stat {
    fn default() -> Self {
        Self {
            duration: crate::timer::pj_time_val { sec: 0, msec: 0 },
            ntests: 0,
            nruns: 0,
            nfailed: 0,
            failed_names: [std::ptr::null(); 32],
        }
    }
}

/// Runner parameter -- must match C layout (12 bytes).
#[repr(C)]
pub struct pj_test_runner_param {
    pub stop_on_error: i32,     // pj_bool_t
    pub nthreads: u32,          // unsigned
    pub verbosity: u32,         // unsigned
}

// ---------------------------------------------------------------------------
// pj_test_case_init
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_case_init(
    tc: *mut pj_test_case,
    obj_name: *const libc::c_char,
    flags: u32,
    test_func: Option<pj_test_func>,
    arg: *mut libc::c_void,
    _fifobuf_buf: *mut libc::c_void,
    buf_size: u32,
    _prm: *const libc::c_void,
) {
    if tc.is_null() {
        return;
    }
    std::ptr::write_bytes(tc as *mut u8, 0, std::mem::size_of::<pj_test_case>());
    (*tc).prev = tc;
    (*tc).next = tc;
    (*tc).flags = flags;
    (*tc).test_func = test_func;
    (*tc).arg = arg;
    (*tc).log_buf_size = buf_size;
    (*tc).result = 0;
    if !obj_name.is_null() {
        let len = libc::strlen(obj_name).min(31);
        std::ptr::copy_nonoverlapping(obj_name, (*tc).obj_name.as_mut_ptr(), len);
        (*tc).obj_name[len] = 0;
    }
}

// ---------------------------------------------------------------------------
// Suite operations
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_create(
    pool: *mut pj_pool_t,
) -> *mut pj_test_suite {
    let suite = crate::pool::pj_pool_alloc(pool, std::mem::size_of::<pj_test_suite>())
        as *mut pj_test_suite;
    if !suite.is_null() {
        std::ptr::write_bytes(suite as *mut u8, 0, std::mem::size_of::<pj_test_suite>());
        let sentinel = &mut (*suite).tests as *mut pj_test_case;
        (*sentinel).prev = sentinel;
        (*sentinel).next = sentinel;
    }
    suite
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_add_case(
    suite: *mut pj_test_suite,
    tc: *mut pj_test_case,
) {
    if suite.is_null() || tc.is_null() {
        return;
    }
    // Insert at tail of the suite's test list (circular doubly-linked)
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    let prev = (*sentinel).prev;
    (*tc).prev = prev;
    (*tc).next = sentinel;
    (*prev).next = tc;
    (*sentinel).prev = tc;
}

// ---------------------------------------------------------------------------
// Init / get root suite
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_init() {
    // no-op
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_get_root_suite() -> *mut pj_test_suite {
    // Return a static suite (zeroed, then init sentinel on first access)
    static mut ROOT_SUITE_BUF: [u8; 256] = [0u8; 256]; // oversized to fit pj_test_suite
    let suite = ROOT_SUITE_BUF.as_mut_ptr() as *mut pj_test_suite;
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    if (*sentinel).prev.is_null() {
        (*sentinel).prev = sentinel;
        (*sentinel).next = sentinel;
    }
    suite
}

// ---------------------------------------------------------------------------
// Runner creation
// ---------------------------------------------------------------------------

/// Magic value to identify heap-allocated runners created by us.
const RUNNER_MAGIC: u32 = 0x504A5452; // "PJTR"

#[repr(C)]
struct RunnerInner {
    magic: u32,
    nthreads: u32,
    stop_on_error: bool,
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_text_runner(
    _pool: *mut pj_pool_t,
    prm: *const pj_test_runner_param,
    p_runner: *mut *mut pj_test_runner,
) -> pj_status_t {
    if p_runner.is_null() {
        return -1; // PJ_EINVAL
    }
    let nthreads = if prm.is_null() { 0 } else { (*prm).nthreads };
    let stop_on_error = if prm.is_null() { false } else { (*prm).stop_on_error != 0 };
    let runner = Box::into_raw(Box::new(RunnerInner { magic: RUNNER_MAGIC, nthreads, stop_on_error })) as *mut pj_test_runner;
    *p_runner = runner;
    0 // PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_basic_runner(
    _pool: *mut pj_pool_t,
) -> *mut pj_test_runner {
    Box::into_raw(Box::new(RunnerInner { magic: RUNNER_MAGIC, nthreads: 0, stop_on_error: false })) as *mut pj_test_runner
}

// ---------------------------------------------------------------------------
// pj_test_run
// ---------------------------------------------------------------------------

/// Data needed to run a single test case on a thread.
/// Uses usize for pointer fields to satisfy Send requirement.
struct TestCaseWork {
    func: pj_test_func,
    arg: usize,  // *mut c_void cast to usize for Send
    flags: u32,
}

// SAFETY: test case pointers and function pointers are safe to send across
// threads since pjlib's test framework is designed for multi-threaded use.
unsafe impl Send for TestCaseWork {}

/// Run a single test case and store the result.
unsafe fn run_one_test_case(tc: *mut pj_test_case) {
    if let Some(func) = (*tc).test_func {
        if (*tc).flags & PJ_TEST_FUNC_NO_ARG != 0 {
            let no_arg_func: unsafe extern "C" fn() -> i32 =
                std::mem::transmute(func);
            (*tc).result = no_arg_func();
        } else {
            (*tc).result = func((*tc).arg);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_run(
    runner: *mut pj_test_runner,
    suite: *mut pj_test_suite,
) {
    if suite.is_null() {
        return;
    }

    // Determine nthreads from runner.
    // Only read nthreads if the runner was created by us (has our magic number).
    // Stack-allocated runners from pj_test_init_basic_runner won't have the magic.
    let nthreads = if runner.is_null() {
        0u32
    } else {
        let inner = runner as *const RunnerInner;
        if (*inner).magic == RUNNER_MAGIC {
            (*inner).nthreads
        } else {
            0u32 // basic runner (stack-allocated) -- run sequentially
        }
    };

    IS_UNDER_TEST.store(true, Ordering::SeqCst);
    crate::time::pj_get_timestamp(&mut (*suite).start_time);

    // Clear previous captured logs
    CAPTURED_LOG_COUNT = 0;

    // Save current log writer and install our intercept
    let orig_writer = crate::logging::pj_log_get_log_func();
    crate::logging::pj_log_set_log_func(Some(test_log_intercept));

    // Mark all tests as pending
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    {
        let mut tc = (*sentinel).next;
        while tc != sentinel {
            (*tc).result = PJ_EPENDING;
            tc = (*tc).next;
        }
    }

    if nthreads > 0 {
        // Multi-threaded execution: respect PJ_TEST_EXCLUSIVE
        run_tests_threaded(suite);
    } else {
        // Sequential execution
        run_tests_sequential(suite);
    }

    // Clear current test info
    clear_current_tc();

    // Restore original log writer
    crate::logging::pj_log_set_log_func(orig_writer);

    crate::time::pj_get_timestamp(&mut (*suite).end_time);
    IS_UNDER_TEST.store(false, Ordering::SeqCst);
}

/// Sequential test execution (basic runner, nthreads=0).
unsafe fn run_tests_sequential(suite: *mut pj_test_suite) {
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    let mut cur = (*sentinel).next;
    while cur != sentinel {
        let name = std::ffi::CStr::from_ptr((*cur).obj_name.as_ptr());
        let name_str = name.to_str().unwrap_or("<unknown>").to_string();
        eprintln!("[test_run] Running: {}", name_str);

        set_current_tc(name_str.as_bytes(), (*cur).log_buf_size);

        run_one_test_case(cur);

        // Patch the result into captured logs for this test case
        let result = (*cur).result;
        {
            let name_bytes = name_str.as_bytes();
            for i in 0..CAPTURED_LOG_COUNT {
                let log = &mut CAPTURED_LOGS[i];
                if log.tc_name_len == name_bytes.len()
                    && &log.tc_name[..log.tc_name_len] == name_bytes
                {
                    log.result = result;
                }
            }
        }

        let status = if result == 0 { "OK" } else { "FAILED" };
        eprintln!("[test_run]   {} => {} (rc={})", name_str, status, result);

        cur = (*cur).next;
    }
}

/// Multi-threaded test execution (text runner, nthreads>0).
/// Groups consecutive tests by exclusivity:
/// - PJ_TEST_EXCLUSIVE tests run one at a time sequentially
/// - Non-exclusive tests run in parallel using OS threads
unsafe fn run_tests_threaded(suite: *mut pj_test_suite) {
    let sentinel = &mut (*suite).tests as *mut pj_test_case;

    // Collect all test cases into an ordered vec
    let mut all_tests: Vec<*mut pj_test_case> = Vec::new();
    let mut cur = (*sentinel).next;
    while cur != sentinel {
        all_tests.push(cur);
        cur = (*cur).next;
    }

    let mut i = 0;
    while i < all_tests.len() {
        let tc = all_tests[i];
        if (*tc).flags & PJ_TEST_EXCLUSIVE != 0 {
            // Run this exclusive test sequentially
            let name = std::ffi::CStr::from_ptr((*tc).obj_name.as_ptr());
            let name_str = name.to_str().unwrap_or("<unknown>").to_string();
            eprintln!("[test_run] Running: {} (exclusive)", name_str);

            set_current_tc(name_str.as_bytes(), (*tc).log_buf_size);
            run_one_test_case(tc);

            // Patch result into captured logs
            let result = (*tc).result;
            {
                let name_bytes = name_str.as_bytes();
                for j in 0..CAPTURED_LOG_COUNT {
                    let log = &mut CAPTURED_LOGS[j];
                    if log.tc_name_len == name_bytes.len()
                        && &log.tc_name[..log.tc_name_len] == name_bytes
                    {
                        log.result = result;
                    }
                }
            }

            let status = if result == 0 { "OK" } else { "FAILED" };
            eprintln!("[test_run]   {} => {} (rc={})", name_str, status, result);
            i += 1;
        } else {
            // Collect consecutive non-exclusive tests
            let start = i;
            while i < all_tests.len() && (*all_tests[i]).flags & PJ_TEST_EXCLUSIVE == 0 {
                i += 1;
            }
            let parallel_group = &all_tests[start..i];

            eprintln!(
                "[test_run] Running {} parallel tests",
                parallel_group.len()
            );

            // Run parallel tests on OS threads.
            // Each thread sets its own thread-local test case info for log capture.
            let mut handles: Vec<(usize, std::thread::JoinHandle<i32>)> = Vec::new();

            for (idx, &tc_ptr) in parallel_group.iter().enumerate() {
                let func = (*tc_ptr).test_func;
                let arg = (*tc_ptr).arg as usize;
                let flags = (*tc_ptr).flags;
                let log_buf_size = (*tc_ptr).log_buf_size;
                // Copy test name for the thread
                let mut tc_name_buf = [0u8; MAX_TC_NAME];
                let tc_name = std::ffi::CStr::from_ptr((*tc_ptr).obj_name.as_ptr());
                let tc_name_bytes = tc_name.to_bytes();
                let copy_len = tc_name_bytes.len().min(MAX_TC_NAME - 1);
                tc_name_buf[..copy_len].copy_from_slice(&tc_name_bytes[..copy_len]);

                if let Some(f) = func {
                    let work = TestCaseWork {
                        func: f,
                        arg,
                        flags,
                    };
                    let handle = std::thread::spawn(move || {
                        // Set thread-local test case info for log capture
                        unsafe { set_current_tc(&tc_name_buf[..copy_len], log_buf_size) };
                        let result = if work.flags & PJ_TEST_FUNC_NO_ARG != 0 {
                            let no_arg_func: unsafe extern "C" fn() -> i32 =
                                unsafe { std::mem::transmute(work.func) };
                            unsafe { no_arg_func() }
                        } else {
                            unsafe { (work.func)(work.arg as *mut libc::c_void) }
                        };
                        unsafe { clear_current_tc() };
                        result
                    });
                    handles.push((idx, handle));
                } else {
                    (*tc_ptr).result = 0;
                }
            }

            // Wait for all threads and collect results
            for (idx, handle) in handles {
                if let Ok(result) = handle.join() {
                    let tc_ptr = parallel_group[idx];
                    (*tc_ptr).result = result;
                    let name = std::ffi::CStr::from_ptr((*tc_ptr).obj_name.as_ptr());
                    let name_str = name.to_str().unwrap_or("<unknown>");

                    // Patch result into captured logs
                    let name_bytes = name_str.as_bytes();
                    for j in 0..CAPTURED_LOG_COUNT {
                        let log = &mut CAPTURED_LOGS[j];
                        if log.tc_name_len == name_bytes.len()
                            && &log.tc_name[..log.tc_name_len] == name_bytes
                        {
                            log.result = result;
                        }
                    }

                    let status = if result == 0 { "OK" } else { "FAILED" };
                    eprintln!("[test_run]   {} => {} (rc={})", name_str, status, result);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stats & display
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_get_stat(
    suite: *const pj_test_suite,
    stat: *mut pj_test_stat,
) {
    if stat.is_null() {
        return;
    }
    std::ptr::write_bytes(stat as *mut u8, 0, std::mem::size_of::<pj_test_stat>());
    if suite.is_null() {
        return;
    }

    // Compute duration from timestamps (nanoseconds since epoch stored as u64)
    let start_ns = (*suite).start_time.u64_val;
    let end_ns = (*suite).end_time.u64_val;
    if end_ns > start_ns {
        let diff_ns = end_ns - start_ns;
        (*stat).duration.sec = (diff_ns / 1_000_000_000) as libc::c_long;
        (*stat).duration.msec = ((diff_ns % 1_000_000_000) / 1_000_000) as libc::c_long;
    }

    let sentinel = &(*suite).tests as *const pj_test_case;
    let mut cur = (*sentinel).next as *const pj_test_case;
    while cur != sentinel {
        (*stat).ntests += 1;
        (*stat).nruns += 1;
        if (*cur).result != 0 {
            if ((*stat).nfailed as usize) < 32 {
                (*stat).failed_names[(*stat).nfailed as usize] =
                    (*cur).obj_name.as_ptr();
            }
            (*stat).nfailed += 1;
        }
        cur = (*cur).next;
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_display_stat(
    stat: *const pj_test_stat,
    title: *const libc::c_char,
    log_sender: *const libc::c_char,
) {
    if stat.is_null() {
        return;
    }
    let title_str = if title.is_null() {
        "tests"
    } else {
        std::ffi::CStr::from_ptr(title).to_str().unwrap_or("tests")
    };
    let sender_str = if log_sender.is_null() {
        "test"
    } else {
        std::ffi::CStr::from_ptr(log_sender).to_str().unwrap_or("test")
    };
    eprintln!(
        "[{}] Unit test statistics for {}: total={}, run={}, failed={}, duration={}m{}.{:03}s",
        sender_str, title_str,
        (*stat).ntests, (*stat).nruns, (*stat).nfailed,
        (*stat).duration.sec / 60,
        (*stat).duration.sec % 60,
        (*stat).duration.msec,
    );
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_display_log(
    _suite: *const pj_test_suite,
    _log_level: i32,
) {
    // no-op
}

// ---------------------------------------------------------------------------
// Runner destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_destroy_runner(_runner: *mut pj_test_runner) {
    // No-op: runners may be stack-allocated (basic runner) or heap-allocated
    // (text runner). We can't safely distinguish, so just do nothing.
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_runner_destroy(runner: *mut pj_test_runner) {
    pj_test_destroy_runner(runner);
}

// ---------------------------------------------------------------------------
// Additional test framework functions needed by pjlib-test
// ---------------------------------------------------------------------------

/// pj_test_select_tests flags
const PJ_TEST_FAILED_TESTS: u32 = 1;
const PJ_TEST_SUCCESSFUL_TESTS: u32 = 2;
const PJ_TEST_ALL_TESTS: u32 = 3;

#[no_mangle]
pub unsafe extern "C" fn pj_test_display_log_messages(
    _suite: *const pj_test_suite,
    flags: u32,
) {
    // Get the current log writer to replay through
    let writer = crate::logging::pj_log_get_log_func();

    for i in 0..CAPTURED_LOG_COUNT {
        let log = &CAPTURED_LOGS[i];

        // Filter by test result based on flags
        let select = flags & PJ_TEST_ALL_TESTS;
        let include = match select {
            PJ_TEST_ALL_TESTS => true,
            PJ_TEST_FAILED_TESTS => log.result != 0,
            PJ_TEST_SUCCESSFUL_TESTS => log.result == 0,
            _ => false,
        };
        if !include {
            continue;
        }

        // Replay through the current log writer
        if let Some(w) = writer {
            w(log.level, log.msg.as_ptr() as *const libc::c_char, log.msg_len as i32);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_init_basic_runner(
    runner: *mut pj_test_runner,
    _param: *const pj_test_runner_param,
) {
    // no-op -- runner is already initialized
    let _ = runner;
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_is_under_test() -> i32 {
    if IS_UNDER_TEST.load(Ordering::SeqCst) { 1 } else { 0 }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_runner_param_default(
    param: *mut pj_test_runner_param,
) {
    if param.is_null() {
        return;
    }
    std::ptr::write_bytes(param as *mut u8, 0, std::mem::size_of::<pj_test_runner_param>());
    (*param).stop_on_error = 0;
    (*param).nthreads = 0;
    (*param).verbosity = 0;
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_init(
    suite: *mut pj_test_suite,
) {
    if suite.is_null() {
        return;
    }
    std::ptr::write_bytes(suite as *mut u8, 0, std::mem::size_of::<pj_test_suite>());
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    (*sentinel).prev = sentinel;
    (*sentinel).next = sentinel;
}

/// PJ_TEST_KEEP_FIRST flag
const PJ_TEST_KEEP_FIRST: u32 = 8;
/// PJ_TEST_KEEP_LAST flag
const PJ_TEST_KEEP_LAST: u32 = 16;

/// LCG random number generator matching pjproject's rand_int().
/// Returns (A * seed + C) % M where M = 2^31, A = 1103515245, C = 12345.
fn rand_int(seed: u32) -> u32 {
    const M: u32 = 1u32 << 31; // 2147483648
    const A: u32 = 1103515245;
    const C: u32 = 12345;
    (A.wrapping_mul(seed).wrapping_add(C)) % M
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_shuffle(
    suite: *mut pj_test_suite,
    seed: i32,
) {
    if suite.is_null() {
        return;
    }

    // Also call pj_srand for RNG consistency (like C does)
    crate::misc::pj_srand(seed as u32);
    let mut rand_val: u32 = if seed >= 0 { seed as u32 } else { crate::misc::pj_rand() as u32 };

    let sentinel = &mut (*suite).tests as *mut pj_test_case;

    // Move all tests to a temporary source list
    // (replicating pj_list_merge_last behavior)
    let mut src_tests: Vec<*mut pj_test_case> = Vec::new();
    let mut cur = (*sentinel).next;
    while cur != sentinel {
        let next = (*cur).next;
        src_tests.push(cur);
        cur = next;
    }

    // Re-initialize the suite list as empty
    (*sentinel).prev = sentinel;
    (*sentinel).next = sentinel;

    // Move KEEP_FIRST tests to destination first (preserving order)
    let mut i = 0;
    while i < src_tests.len() {
        if (*src_tests[i]).flags & PJ_TEST_KEEP_FIRST != 0 {
            let tc = src_tests.remove(i);
            let prev = (*sentinel).prev;
            (*tc).prev = prev;
            (*tc).next = sentinel;
            (*prev).next = tc;
            (*sentinel).prev = tc;
        } else {
            i += 1;
        }
    }

    // Count non-KEEP_LAST tests (these are the "movable" ones)
    let total_initial = src_tests.len();
    let mut movable = 0usize;
    for &tc in &src_tests {
        if (*tc).flags & PJ_TEST_KEEP_LAST == 0 {
            movable += 1;
        }
    }

    // Shuffle non-KEEP_LAST tests using random step selection
    // This matches the C algorithm exactly
    while movable > 0 {
        let total = src_tests.len();
        if total == 0 {
            break;
        }
        rand_val = rand_int(rand_val);
        let step = (rand_val as usize) % total;

        let tc = src_tests[step];

        // Skip KEEP_LAST tests (they stay in the source list)
        if (*tc).flags & PJ_TEST_KEEP_LAST != 0 {
            continue;
        }

        // Remove from source and add to destination
        src_tests.remove(step);
        let prev = (*sentinel).prev;
        (*tc).prev = prev;
        (*tc).next = sentinel;
        (*prev).next = tc;
        (*sentinel).prev = tc;
        movable -= 1;
    }

    // Move remaining KEEP_LAST tests to destination (preserving order)
    for tc in src_tests {
        let prev = (*sentinel).prev;
        (*tc).prev = prev;
        (*tc).next = sentinel;
        (*prev).next = tc;
        (*sentinel).prev = tc;
    }
}
