//! GLOBAL() function - read/write global variables.
//!
//! Provides access to global (non-channel-specific) variables.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::collections::HashMap;
use std::sync::RwLock;

/// Global variable store, accessible across all channels.
static GLOBAL_VARS: once_cell::sync::Lazy<RwLock<HashMap<String, String>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(HashMap::new()));

/// GLOBAL() function.
///
/// Usage:
///   ${GLOBAL(varname)} - Read a global variable
///   Set(GLOBAL(varname)=value) - Write a global variable
pub struct FuncGlobal;

impl DialplanFunc for FuncGlobal {
    fn name(&self) -> &str {
        "GLOBAL"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let varname = args.trim();
        if varname.is_empty() {
            return Err(FuncError::InvalidArgument(
                "GLOBAL: variable name is required".to_string(),
            ));
        }

        let globals = GLOBAL_VARS
            .read()
            .map_err(|e| FuncError::Internal(format!("lock poisoned: {}", e)))?;
        Ok(globals.get(varname).cloned().unwrap_or_default())
    }

    fn write(&self, _ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let varname = args.trim();
        if varname.is_empty() {
            return Err(FuncError::InvalidArgument(
                "GLOBAL: variable name is required".to_string(),
            ));
        }

        let mut globals = GLOBAL_VARS
            .write()
            .map_err(|e| FuncError::Internal(format!("lock poisoned: {}", e)))?;
        if value.is_empty() {
            globals.remove(varname);
        } else {
            globals.insert(varname.to_string(), value.to_string());
        }
        Ok(())
    }
}

impl FuncGlobal {
    /// Set a global variable directly (not through the dialplan function interface).
    pub fn set_global(name: &str, value: &str) {
        if let Ok(mut globals) = GLOBAL_VARS.write() {
            if value.is_empty() {
                globals.remove(name);
            } else {
                globals.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// Get a global variable directly.
    pub fn get_global(name: &str) -> Option<String> {
        GLOBAL_VARS
            .read()
            .ok()
            .and_then(|g| g.get(name).cloned())
    }

    /// List all global variables.
    pub fn list_globals() -> Vec<(String, String)> {
        GLOBAL_VARS
            .read()
            .map(|g| g.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Clear all global variables.
    pub fn clear_globals() {
        if let Ok(mut globals) = GLOBAL_VARS.write() {
            globals.clear();
        }
    }
}

// Simple once_cell implementation
mod once_cell {
    pub mod sync {
        pub struct Lazy<T> {
            inner: std::sync::OnceLock<T>,
            init: fn() -> T,
        }

        impl<T> Lazy<T> {
            pub const fn new(init: fn() -> T) -> Self {
                Self {
                    inner: std::sync::OnceLock::new(),
                    init,
                }
            }
        }

        impl<T> std::ops::Deref for Lazy<T> {
            type Target = T;

            fn deref(&self) -> &T {
                self.inner.get_or_init(self.init)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_read_write() {
        let func = FuncGlobal;
        let mut ctx = FuncContext::new();

        // Write a global
        func.write(&mut ctx, "TESTVAR", "hello").unwrap();

        // Read it back
        assert_eq!(func.read(&ctx, "TESTVAR").unwrap(), "hello");
    }

    #[test]
    fn test_global_not_set() {
        let func = FuncGlobal;
        let ctx = FuncContext::new();
        assert_eq!(
            func.read(&ctx, "NONEXISTENT_VAR_12345").unwrap(),
            ""
        );
    }

    #[test]
    fn test_global_direct_api() {
        FuncGlobal::set_global("DIRECT_TEST", "value123");
        assert_eq!(
            FuncGlobal::get_global("DIRECT_TEST"),
            Some("value123".to_string())
        );

        FuncGlobal::set_global("DIRECT_TEST", "");
        assert_eq!(FuncGlobal::get_global("DIRECT_TEST"), None);
    }
}
