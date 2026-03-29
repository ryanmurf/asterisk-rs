//! MODULE() / IFMODULE() functions - module information.
//!
//! Port of func_module.c from Asterisk C.
//!
//! Provides:
//! - IFMODULE(modulename.so) - check if module is loaded (returns "1" or "0")
//! - MODULE(field) - get module info
//!
//! MODULE fields:
//! - "load" - module load status

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// IFMODULE() function - checks if a module is loaded.
pub struct FuncIfModule;

impl DialplanFunc for FuncIfModule {
    fn name(&self) -> &str {
        "IFMODULE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let module_name = args.trim();
        if module_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "IFMODULE requires a module name argument".into(),
            ));
        }
        // Stub: would check if the named module is loaded
        // For now, return "0" (not loaded)
        Ok("0".to_string())
    }
}

/// MODULE() function - get module information.
pub struct FuncModule;

impl DialplanFunc for FuncModule {
    fn name(&self) -> &str {
        "MODULE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let field = args.trim().to_lowercase();
        match field.as_str() {
            "load" | "running" | "loadtype" | "count" => {
                // Stub: would query module manager
                Ok(String::new())
            }
            "" => Err(FuncError::InvalidArgument(
                "MODULE requires a field argument".into(),
            )),
            _ => Err(FuncError::InvalidArgument(format!(
                "MODULE: unknown field '{}', valid: load, running, loadtype, count", field
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ifmodule() {
        let ctx = FuncContext::new();
        let func = FuncIfModule;
        let result = func.read(&ctx, "chan_sip.so").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_ifmodule_empty() {
        let ctx = FuncContext::new();
        let func = FuncIfModule;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_module_load() {
        let ctx = FuncContext::new();
        let func = FuncModule;
        let result = func.read(&ctx, "load");
        assert!(result.is_ok());
    }

    #[test]
    fn test_module_invalid() {
        let ctx = FuncContext::new();
        let func = FuncModule;
        assert!(func.read(&ctx, "invalid_field").is_err());
    }
}
