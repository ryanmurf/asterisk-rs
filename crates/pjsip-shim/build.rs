use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    cc::Build::new()
        .file("src/log_wrapper.c")
        .file("/tmp/pjproject-2.16/pjlib/src/pj/ioqueue_select.c")
        // NOTE: ioqueue_common_abs.c is NOT listed here because
        // ioqueue_select.c does #include "ioqueue_common_abs.c" directly.
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
