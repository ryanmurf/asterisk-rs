//! pjlib test framework stubs.
//!
//! pjproject's pjlib-test uses a test harness with runners, suites, and cases.
//! We stub these so the test binary can link against our library.

use crate::types::*;

/// Opaque test runner.
#[repr(C)]
pub struct pj_test_runner {
    _opaque: [u8; 0],
}

/// Opaque test suite.
#[repr(C)]
pub struct pj_test_suite {
    pub tests: pj_list_head,
}

/// Linked-list head used by pj_test_suite.
#[repr(C)]
pub struct pj_list_head {
    pub prev: *mut libc::c_void,
    pub next: *mut libc::c_void,
}

/// Test case callback type.
pub type pj_test_func = unsafe extern "C" fn(*mut pj_test_case) -> i32;

/// Test case.
#[repr(C)]
pub struct pj_test_case {
    pub prev: *mut pj_test_case,
    pub next: *mut pj_test_case,
    pub obj_name: [libc::c_char; 32],
    pub flags: u32,
    pub test_func: Option<pj_test_func>,
    pub result: i32,
    pub _pad: [u8; 64],
}

/// Test stat.
#[repr(C)]
#[derive(Default)]
pub struct pj_test_stat {
    pub ntests: u32,
    pub nruns: u32,
    pub nfailed: u32,
    pub duration: u32,
}

/// Runner parameter.
#[repr(C)]
pub struct pj_test_runner_param {
    pub log_level: i32,
    pub nthreads: i32,
    _pad: [u8; 64],
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
) {
    if tc.is_null() {
        return;
    }
    std::ptr::write_bytes(tc as *mut u8, 0, std::mem::size_of::<pj_test_case>());
    (*tc).prev = tc;
    (*tc).next = tc;
    (*tc).flags = flags;
    (*tc).test_func = test_func;
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
        (*suite).tests.prev = &mut (*suite).tests as *mut _ as *mut _;
        (*suite).tests.next = &mut (*suite).tests as *mut _ as *mut _;
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
    // Insert at tail of the suite's test list
    let sentinel = &mut (*suite).tests as *mut pj_list_head as *mut pj_test_case;
    let prev = (*sentinel).prev;
    (*tc).prev = prev;
    (*tc).next = sentinel;
    (*(prev as *mut pj_test_case)).next = tc;
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
    // Return a static suite
    static mut ROOT_SUITE: pj_test_suite = pj_test_suite {
        tests: pj_list_head {
            prev: std::ptr::null_mut(),
            next: std::ptr::null_mut(),
        },
    };
    // Init circular list if needed
    if ROOT_SUITE.tests.prev.is_null() {
        ROOT_SUITE.tests.prev = std::ptr::addr_of_mut!(ROOT_SUITE.tests) as *mut _;
        ROOT_SUITE.tests.next = std::ptr::addr_of_mut!(ROOT_SUITE.tests) as *mut _;
    }
    std::ptr::addr_of_mut!(ROOT_SUITE)
}

// ---------------------------------------------------------------------------
// Runner creation
// ---------------------------------------------------------------------------

struct RunnerInner;

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_text_runner(
    pool: *mut pj_pool_t,
    _flags: u32,
) -> *mut pj_test_runner {
    let _ = pool;
    Box::into_raw(Box::new(RunnerInner)) as *mut pj_test_runner
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_create_basic_runner(
    pool: *mut pj_pool_t,
) -> *mut pj_test_runner {
    pj_test_create_text_runner(pool, 0)
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
    // Walk the test case list and run each one
    let sentinel = &mut (*suite).tests as *mut pj_list_head as *mut pj_test_case;
    let mut cur = (*sentinel).next as *mut pj_test_case;
    while cur != sentinel {
        if let Some(func) = (*cur).test_func {
            (*cur).result = func(cur);
        }
        cur = (*cur).next;
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
    let sentinel = &(*suite).tests as *const pj_list_head as *const pj_test_case;
    let mut cur = (*sentinel).next as *const pj_test_case;
    while cur != sentinel {
        (*stat).ntests += 1;
        (*stat).nruns += 1;
        if (*cur).result != 0 {
            (*stat).nfailed += 1;
        }
        cur = (*cur).next;
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_display_stat(
    _stat: *const pj_test_stat,
    _title: *const libc::c_char,
    _log_sender: *const libc::c_char,
) {
    // no-op: we don't display stats
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
pub unsafe extern "C" fn pj_test_destroy_runner(runner: *mut pj_test_runner) {
    if !runner.is_null() {
        let _ = Box::from_raw(runner as *mut RunnerInner);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_runner_destroy(runner: *mut pj_test_runner) {
    pj_test_destroy_runner(runner);
}

// ---------------------------------------------------------------------------
// Additional test framework functions needed by pjlib-test
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pj_test_display_log_messages(
    _suite: *const pj_test_suite,
    _level: i32,
) {
    // no-op
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
    0
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_runner_param_default(
    param: *mut pj_test_runner_param,
) {
    if param.is_null() {
        return;
    }
    std::ptr::write_bytes(param as *mut u8, 0, std::mem::size_of::<pj_test_runner_param>());
    (*param).log_level = 3;
    (*param).nthreads = 0;
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_init(
    suite: *mut pj_test_suite,
) {
    if suite.is_null() {
        return;
    }
    (*suite).tests.prev = &mut (*suite).tests as *mut _ as *mut _;
    (*suite).tests.next = &mut (*suite).tests as *mut _ as *mut _;
}

#[no_mangle]
pub unsafe extern "C" fn pj_test_suite_shuffle(
    suite: *mut pj_test_suite,
    _seed: u32,
) {
    // no-op: we don't shuffle
    let _ = suite;
}
