//! Build script for asterisk-codecs.
//!
//! Emits the linker directives for the optional native codec FFI features
//! (`native-opus`, `native-gsm`, `native-speex`). Without these directives
//! the `extern "C"` bindings in `opus_ffi.rs` / `gsm_ffi.rs` / `speex_ffi.rs`
//! have no library to bind against and any build with a feature enabled
//! fails at link time with undefined symbols (issue #15).
//!
//! Resolution order per enabled feature:
//! 1. `<LIB>_LIB_DIR` environment variable (e.g. `OPUS_LIB_DIR`) -- adds the
//!    given directory to the link search path and links the library.
//! 2. `pkg-config` -- preferred when available; uses its `-L` / `-l` output.
//! 3. Fallback -- plain `cargo:rustc-link-lib=<name>`, relying on the
//!    default linker search path.
//!
//! When no native feature is enabled this script emits only
//! `rerun-if-env-changed` markers, so default builds are unaffected.

use std::env;
use std::process::Command;

/// (cargo feature env var, pkg-config package, library name, lib-dir env var)
const NATIVE_LIBS: &[(&str, &str, &str, &str)] = &[
    ("CARGO_FEATURE_NATIVE_OPUS", "opus", "opus", "OPUS_LIB_DIR"),
    ("CARGO_FEATURE_NATIVE_GSM", "gsm", "gsm", "GSM_LIB_DIR"),
    (
        "CARGO_FEATURE_NATIVE_SPEEX",
        "speex",
        "speex",
        "SPEEX_LIB_DIR",
    ),
];

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    for (feature_env, pkg, lib, dir_env) in NATIVE_LIBS {
        println!("cargo:rerun-if-env-changed={dir_env}");
        if env::var_os(feature_env).is_some() {
            link_native_lib(pkg, lib, dir_env);
        }
    }
}

/// Emit link directives for one enabled native codec library.
fn link_native_lib(pkg: &str, lib: &str, dir_env: &str) {
    // 1. Explicit override via <LIB>_LIB_DIR.
    if let Some(dir) = env::var(dir_env).ok().filter(|d| !d.is_empty()) {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib={lib}");
        return;
    }

    // 2. pkg-config, when the package is known to it.
    if emit_pkg_config(pkg) {
        return;
    }

    // 3. Fallback: assume the library is on the default linker search path.
    println!(
        "cargo:warning=pkg-config could not locate '{pkg}'; \
         falling back to '-l{lib}' on the default search path \
         (set {dir_env} to override)"
    );
    println!("cargo:rustc-link-lib={lib}");
}

/// Ask pkg-config for the link flags of `pkg` and translate them into cargo
/// directives. Returns false when pkg-config is missing, the package is
/// unknown, or the output contains no `-l` flag.
fn emit_pkg_config(pkg: &str) -> bool {
    let output = match Command::new("pkg-config").args(["--libs", pkg]).output() {
        Ok(out) if out.status.success() => out,
        _ => return false,
    };

    let flags = String::from_utf8_lossy(&output.stdout);
    let mut directives = Vec::new();
    let mut has_lib = false;
    for flag in flags.split_whitespace() {
        if let Some(dir) = flag.strip_prefix("-L") {
            directives.push(format!("cargo:rustc-link-search=native={dir}"));
        } else if let Some(name) = flag.strip_prefix("-l") {
            directives.push(format!("cargo:rustc-link-lib={name}"));
            has_lib = true;
        }
    }

    if has_lib {
        for directive in directives {
            println!("{directive}");
        }
    }
    has_lib
}
