//! pjlib test framework stubs.
//!
//! pjproject's pjlib-test uses a test harness with runners, suites, and cases.
//! We stub these so the test binary can link against our library.

use crate::types::*;
use std::sync::atomic::{AtomicBool, Ordering};

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
static mut CURRENT_TC_NAME_BUF: [u8; MAX_TC_NAME] = [0u8; MAX_TC_NAME];
static mut CURRENT_TC_NAME_LEN: usize = 0;
static mut CURRENT_TC_LOG_BUF_SIZE: u32 = 0;

/// Log intercept callback used during pj_test_run.
/// Uses only static memory -- no heap allocation.
unsafe extern "C" fn test_log_intercept(level: i32, data: *const libc::c_char, len: i32) {
    if data.is_null() || len <= 0 {
        return;
    }

    // Only capture if the test case has a log buffer configured
    if CURRENT_TC_LOG_BUF_SIZE < 10 {
        return;
    }

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

    let name_len = CURRENT_TC_NAME_LEN.min(MAX_TC_NAME - 1);
    std::ptr::copy_nonoverlapping(
        CURRENT_TC_NAME_BUF.as_ptr(),
        CAPTURED_LOGS[idx].tc_name.as_mut_ptr(),
        name_len,
    );
    CAPTURED_LOGS[idx].tc_name[name_len] = 0;
    CAPTURED_LOGS[idx].tc_name_len = name_len;

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

/// PJ_TEST_FUNC_NO_ARG flag — call test_func with no argument.
const PJ_TEST_FUNC_NO_ARG: u32 = 2;

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

struct RunnerInner;

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_text_runner(
    _pool: *mut pj_pool_t,
    _prm: *const pj_test_runner_param,
    p_runner: *mut *mut pj_test_runner,
) -> pj_status_t {
    if p_runner.is_null() {
        return -1; // PJ_EINVAL
    }
    let runner = Box::into_raw(Box::new(RunnerInner)) as *mut pj_test_runner;
    *p_runner = runner;
    0 // PJ_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_basic_runner(
    _pool: *mut pj_pool_t,
) -> *mut pj_test_runner {
    Box::into_raw(Box::new(RunnerInner)) as *mut pj_test_runner
}

// ---------------------------------------------------------------------------
// pj_test_run
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_run(
    _runner: *mut pj_test_runner,
    suite: *mut pj_test_suite,
) {
    if suite.is_null() {
        return;
    }

    IS_UNDER_TEST.store(true, Ordering::SeqCst);
    crate::time::pj_get_timestamp(&mut (*suite).start_time);

    // Clear previous captured logs
    CAPTURED_LOG_COUNT = 0;

    // Save current log writer and install our intercept
    let orig_writer = crate::logging::pj_log_get_log_func();
    crate::logging::pj_log_set_log_func(Some(test_log_intercept));

    // Walk the test case list and run each one
    let sentinel = &mut (*suite).tests as *mut pj_test_case;
    let mut cur = (*sentinel).next;
    while cur != sentinel {
        // Get the test name
        let name = std::ffi::CStr::from_ptr((*cur).obj_name.as_ptr());
        let name_str = name.to_str().unwrap_or("<unknown>").to_string();

        // Print to stderr directly (not through log system)
        eprintln!("[test_run] Running: {}", name_str);

        // Set current test case info for log capture
        {
            let name_bytes = name_str.as_bytes();
            let copy_len = name_bytes.len().min(MAX_TC_NAME - 1);
            std::ptr::copy_nonoverlapping(
                name_bytes.as_ptr(),
                CURRENT_TC_NAME_BUF.as_mut_ptr(),
                copy_len,
            );
            CURRENT_TC_NAME_BUF[copy_len] = 0;
            CURRENT_TC_NAME_LEN = copy_len;
            CURRENT_TC_LOG_BUF_SIZE = (*cur).log_buf_size;
        }

        if let Some(func) = (*cur).test_func {
            if (*cur).flags & PJ_TEST_FUNC_NO_ARG != 0 {
                // Call as int (*)(void) — no argument
                let no_arg_func: unsafe extern "C" fn() -> i32 =
                    std::mem::transmute(func);
                (*cur).result = no_arg_func();
            } else {
                (*cur).result = func((*cur).arg);
            }
        }

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

    // Clear current test info
    CURRENT_TC_NAME_LEN = 0;
    CURRENT_TC_LOG_BUF_SIZE = 0;

    // Restore original log writer
    crate::logging::pj_log_set_log_func(orig_writer);

    crate::time::pj_get_timestamp(&mut (*suite).end_time);
    IS_UNDER_TEST.store(false, Ordering::SeqCst);
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

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_shuffle(
    suite: *mut pj_test_suite,
    _seed: u32,
) {
    if suite.is_null() {
        return;
    }

    let sentinel = &mut (*suite).tests as *mut pj_test_case;

    // Collect all test cases into separate lists
    let mut keep_first: Vec<*mut pj_test_case> = Vec::new();
    let mut keep_last: Vec<*mut pj_test_case> = Vec::new();
    let mut normal: Vec<*mut pj_test_case> = Vec::new();

    let mut cur = (*sentinel).next;
    while cur != sentinel {
        let next = (*cur).next;
        if (*cur).flags & PJ_TEST_KEEP_FIRST != 0 {
            keep_first.push(cur);
        } else if (*cur).flags & PJ_TEST_KEEP_LAST != 0 {
            keep_last.push(cur);
        } else {
            normal.push(cur);
        }
        cur = next;
    }

    // Re-initialize the list as empty
    (*sentinel).prev = sentinel;
    (*sentinel).next = sentinel;

    // Re-add in order: keep_first, normal (no actual shuffling), keep_last
    for tc in keep_first.iter().chain(normal.iter()).chain(keep_last.iter()) {
        let tc = *tc;
        let prev = (*sentinel).prev;
        (*tc).prev = prev;
        (*tc).next = sentinel;
        (*prev).next = tc;
        (*sentinel).prev = tc;
    }
}
