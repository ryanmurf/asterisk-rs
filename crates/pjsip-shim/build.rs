use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    cc::Build::new()
        .file("src/log_wrapper.c")
        .compile("log_wrapper");

    // Force-load the archive so the linker includes all symbols,
    // even those not referenced by Rust code.
    #[cfg(target_os = "macos")]
    {
        println!(
            "cargo:rustc-link-arg=-Wl,-force_load,{}/liblog_wrapper.a",
            out_dir.display()
        );

        // The cdylib export list from rustc only includes #[no_mangle]
        // Rust symbols.  Force-export the C variadic functions so they
        // are visible to external code that links against our dylib.
        let symbols = [
            "pj_log_1", "pj_log_2", "pj_log_3", "pj_log_4", "pj_log_5",
            "pj_perror_1", "pj_perror_2", "pj_perror_3", "pj_perror_4", "pj_perror_5",
            "pj_perror",
            "pj_push_exception_handler_", "pj_pop_exception_handler_", "pj_throw_exception_",
            "pj_push_exception_handler", "pj_pop_exception_handler", "pj_throw_exception",
        ];
        for sym in &symbols {
            println!("cargo:rustc-cdylib-link-arg=-Wl,-exported_symbol,_{}", sym);
        }
    }

    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--whole-archive");
        println!(
            "cargo:rustc-cdylib-link-arg={}/liblog_wrapper.a",
            out_dir.display()
        );
        println!("cargo:rustc-cdylib-link-arg=-Wl,--no-whole-archive");
    }
}
