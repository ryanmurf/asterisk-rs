use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    cc::Build::new()
        .file("src/log_wrapper.c")
        .file("src/pjlib_stubs.c")
        .file("/tmp/pjproject-2.16/pjlib/src/pj/ioqueue_select.c")
        // NOTE: ioqueue_common_abs.c is NOT listed here because
        // ioqueue_select.c does #include "ioqueue_common_abs.c" directly.
        .file("/tmp/pjproject-2.16/pjlib/src/pj/os_core_unix.c")
        .file("/tmp/pjproject-2.16/pjlib/src/pj/lock.c")
        .file("/tmp/pjproject-2.16/pjlib/src/pj/os_timestamp_posix.c")
        .include("/tmp/pjproject-2.16/pjlib/include")
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
        .compile("pjsip_c_parts");

    // Force-load the archive so the linker includes all symbols,
    // even those not referenced by Rust code.
    #[cfg(target_os = "macos")]
    {
        println!(
            "cargo:rustc-link-arg=-Wl,-force_load,{}/libpjsip_c_parts.a",
            out_dir.display()
        );

        // The cdylib export list from rustc only includes #[no_mangle]
        // Rust symbols.  Force-export the C variadic functions so they
        // are visible to external code that links against our dylib.
        let symbols = [
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
        for sym in &symbols {
            println!("cargo:rustc-cdylib-link-arg=-Wl,-exported_symbol,_{}", sym);
        }
    }

    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--whole-archive");
        println!(
            "cargo:rustc-cdylib-link-arg={}/libpjsip_c_parts.a",
            out_dir.display()
        );
        println!("cargo:rustc-cdylib-link-arg=-Wl,--no-whole-archive");
    }
}
