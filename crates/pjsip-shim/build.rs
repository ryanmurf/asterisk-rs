use std::path::{Path, PathBuf};

/// List of exported C symbols (macOS) / whole-archive symbols that the
/// pjproject C layer provides.  Only relevant when the `pjproject-cffi`
/// feature is enabled.
#[cfg(target_os = "macos")]
const EXPORTED_SYMBOLS: &[&str] = &[
    // Log wrapper symbols
    "pj_log_1", "pj_log_2", "pj_log_3", "pj_log_4", "pj_log_5",
    "pj_perror_1", "pj_perror_2", "pj_perror_3", "pj_perror_4", "pj_perror_5",
    "pj_perror",
    "pj_push_exception_handler_", "pj_pop_exception_handler_", "pj_throw_exception_",
    "pj_push_exception_handler", "pj_pop_exception_handler", "pj_throw_exception",
    // C ioqueue symbols
    "pj_ioqueue_name",
    "pj_ioqueue_create", "pj_ioqueue_create2", "pj_ioqueue_destroy",
    "pj_ioqueue_register_sock", "pj_ioqueue_register_sock2",
    "pj_ioqueue_unregister",
    "pj_ioqueue_poll",
    "pj_ioqueue_get_user_data", "pj_ioqueue_set_user_data",
    "pj_ioqueue_recv", "pj_ioqueue_recvfrom",
    "pj_ioqueue_send", "pj_ioqueue_sendto",
    "pj_ioqueue_accept", "pj_ioqueue_connect",
    "pj_ioqueue_op_key_init", "pj_ioqueue_is_pending",
    "pj_ioqueue_post_completion",
    "pj_ioqueue_set_lock",
    "pj_ioqueue_set_default_concurrency", "pj_ioqueue_set_concurrency",
    "pj_ioqueue_lock_key", "pj_ioqueue_trylock_key", "pj_ioqueue_unlock_key",
    "pj_ioqueue_clear_key",
    "pj_ioqueue_get_os_handle",
    "pj_ioqueue_cfg_default",
    // C os_core_unix symbols (threads, mutexes, atomics, etc.)
    "pj_init", "pj_shutdown", "pj_atexit", "pj_getpid",
    "pj_thread_create", "pj_thread_create2",
    "pj_thread_register", "pj_thread_this", "pj_thread_get_name",
    "pj_thread_join", "pj_thread_destroy", "pj_thread_sleep",
    "pj_thread_resume", "pj_thread_is_registered",
    "pj_thread_attach", "pj_thread_unregister",
    "pj_thread_get_prio", "pj_thread_set_prio",
    "pj_thread_get_prio_min", "pj_thread_get_prio_max",
    "pj_thread_get_os_handle",
    "pj_thread_local_alloc", "pj_thread_local_free",
    "pj_thread_local_set", "pj_thread_local_get",
    "pj_mutex_create", "pj_mutex_create_simple", "pj_mutex_create_recursive",
    "pj_mutex_lock", "pj_mutex_unlock", "pj_mutex_trylock",
    "pj_mutex_destroy", "pj_mutex_is_locked",
    "pj_rwmutex_create", "pj_rwmutex_lock_read", "pj_rwmutex_lock_write",
    "pj_rwmutex_unlock_read", "pj_rwmutex_unlock_write", "pj_rwmutex_destroy",
    "pj_sem_create", "pj_sem_wait", "pj_sem_trywait",
    "pj_sem_post", "pj_sem_destroy",
    "pj_atomic_create", "pj_atomic_destroy",
    "pj_atomic_set", "pj_atomic_get",
    "pj_atomic_inc", "pj_atomic_inc_and_get",
    "pj_atomic_dec", "pj_atomic_dec_and_get",
    "pj_atomic_add", "pj_atomic_add_and_get",
    "pj_enter_critical_section", "pj_leave_critical_section",
    "pj_event_create", "pj_event_wait", "pj_event_trywait",
    "pj_event_set", "pj_event_pulse", "pj_event_reset", "pj_event_destroy",
    "pj_barrier_create", "pj_barrier_wait", "pj_barrier_destroy",
    "pj_set_cloexec_flag", "pj_term_set_color", "pj_term_get_color",
    // C lock symbols
    "pj_lock_create_simple_mutex", "pj_lock_create_recursive_mutex",
    "pj_lock_create_null_mutex", "pj_lock_create_semaphore",
    "pj_lock_acquire", "pj_lock_tryacquire", "pj_lock_release", "pj_lock_destroy",
    "pj_grp_lock_config_default",
    "pj_grp_lock_create", "pj_grp_lock_create_w_handler",
    "pj_grp_lock_destroy",
    "pj_grp_lock_acquire", "pj_grp_lock_tryacquire", "pj_grp_lock_release",
    "pj_grp_lock_replace",
    "pj_grp_lock_add_handler", "pj_grp_lock_del_handler",
    "pj_grp_lock_add_ref", "pj_grp_lock_dec_ref", "pj_grp_lock_get_ref",
    "pj_grp_lock_chain_lock", "pj_grp_lock_unchain_lock",
    "pj_grp_lock_dump",
    // C timestamp symbols
    "pj_get_timestamp", "pj_get_timestamp_freq",
    // Stubs
    "PJ_NO_MEMORY_EXCEPTION", "PJ_VERSION",
    "pj_NO_MEMORY_EXCEPTION", "pj_get_version",
    "pj_log_init", "pj_errno_clear_handlers",
];

fn main() {
    // The pjproject C compatibility layer (real ioqueue / threading /
    // locking compiled from pjproject's own C sources) is opt-in via the
    // `pjproject-cffi` feature.  Without it, the crate builds as a
    // pure-Rust cdylib/staticlib and `cargo build` works out of the box
    // with no external pjproject checkout.  See README "pjproject C layer".
    if std::env::var_os("CARGO_FEATURE_PJPROJECT_CFFI").is_none() {
        // Feature off: nothing to compile, no external pjproject dependency.
        //
        // The Rust FFI modules still *declare* the pjproject C symbols
        // (ioqueue/threading/locking), and functions like the activesock
        // layer reference them. Those code paths are inert in this build,
        // but the cdylib must still link. macOS's two-level namespace
        // linker rejects undefined symbols, so allow them to be resolved
        // dynamically at load time; Linux shared objects permit undefined
        // symbols by default and need no flag. The staticlib never links
        // here, so it is unaffected.
        #[cfg(target_os = "macos")]
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
        return;
    }

    build_pjproject_cffi();
}

/// Compile the pjproject C sources and wire them into the cdylib.
/// Only called when the `pjproject-cffi` feature is enabled.
fn build_pjproject_cffi() {
    println!("cargo:rerun-if-env-changed=PJPROJECT_DIR");

    let pj_dir = locate_pjproject().unwrap_or_else(|| {
        panic!(
            "\n\
             ============================================================\n\
             pjsip-shim: the `pjproject-cffi` feature is enabled but the\n\
             pjproject 2.17 source tree could not be found.\n\
             \n\
             Provide it in one of these ways:\n\
               1. Run  scripts/fetch-pjproject.sh  (downloads it into\n\
                  crates/pjsip-shim/vendor/pjproject-2.17), or\n\
               2. Set  PJPROJECT_DIR=/path/to/pjproject-2.17  when building.\n\
             \n\
             The directory must contain pjlib/include/pj/types.h.\n\
             ============================================================\n"
        )
    });

    println!("cargo:warning=pjsip-shim: building pjproject C layer from {}", pj_dir.display());

    let include = pj_dir.join("pjlib/include");
    let src = pj_dir.join("pjlib/src/pj");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let c_files = [
        "src/log_wrapper.c".to_string(),
        "src/pjlib_stubs.c".to_string(),
        src.join("ioqueue_select.c").to_string_lossy().into_owned(),
        // NOTE: ioqueue_common_abs.c is NOT listed here because
        // ioqueue_select.c does #include "ioqueue_common_abs.c" directly.
        src.join("os_core_unix.c").to_string_lossy().into_owned(),
        src.join("lock.c").to_string_lossy().into_owned(),
        src.join("os_timestamp_posix.c").to_string_lossy().into_owned(),
    ];

    let mut build = cc::Build::new();
    for f in &c_files {
        build.file(f);
    }
    build
        .include(&include)
        .define("PJ_AUTOCONF", "1")
        // The test binary was compiled without PJ_AUTOCONF, using os_darwinos.h
        // which sets PJ_IOQUEUE_MAX_HANDLES=1024. Match it here.
        .define("PJ_IOQUEUE_MAX_HANDLES", "1024")
        // Raise FD_SETSIZE so select() can handle fd >= 1024.
        // On macOS the default is 1024 which is not enough when
        // stdin/stdout/stderr consume fds 0-2 and we open 1024 sockets.
        // Must be set *before* system headers define fd_set.
        .define("FD_SETSIZE", "2048")
        .warnings(false)
        // Suppress cc's automatic `cargo:rustc-link-lib` directive. We
        // re-link the archive below with the +whole-archive modifier; if cc
        // also emitted its default `-l`, the archive would be linked twice
        // and GNU ld would report duplicate symbols.
        .cargo_metadata(false)
        .compile("pjsip_c_parts");

    // cargo_metadata(false) silenced cc's search-path and rerun hints too;
    // re-emit the ones we still need.
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    for f in &c_files {
        println!("cargo:rerun-if-changed={}", f);
    }

    // Link the whole C archive so *all* pj* symbols land in the shim, not
    // just the ones Rust references (external C consumers need them too).
    // cargo's +whole-archive modifier maps to `--whole-archive` on GNU ld
    // and `-force_load` on macOS ld, so one directive covers both platforms.
    println!("cargo:rustc-link-lib=static:+whole-archive=pjsip_c_parts");

    // The cdylib export list from rustc only includes #[no_mangle] Rust
    // symbols. On macOS, force-export the C functions so they are visible to
    // external code linking against our dylib.
    #[cfg(target_os = "macos")]
    for sym in EXPORTED_SYMBOLS {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-exported_symbol,_{}", sym);
    }
}

/// Find the pjproject 2.17 source tree, in priority order:
///   1. `$PJPROJECT_DIR`
///   2. the vendored copy at `crates/pjsip-shim/vendor/pjproject-2.17`
///      (populated by `scripts/fetch-pjproject.sh`)
///   3. `/tmp/pjproject-2.17` (historical default)
/// A directory only counts if `pjlib/include/pj/types.h` exists under it.
fn locate_pjproject() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(dir) = std::env::var_os("PJPROJECT_DIR") {
        candidates.push(PathBuf::from(dir));
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    candidates.push(manifest_dir.join("vendor/pjproject-2.17"));

    candidates.push(PathBuf::from("/tmp/pjproject-2.17"));

    candidates
        .into_iter()
        .find(|dir| has_pjlib_headers(dir))
}

fn has_pjlib_headers(dir: &Path) -> bool {
    dir.join("pjlib/include/pj/types.h").is_file()
}
