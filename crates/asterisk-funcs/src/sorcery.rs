//! SORCERY() function - query sorcery data objects.
//!
//! Port of func_sorcery.c from Asterisk C.
//!
//! Provides:
//! - SORCERY(module,object_type,id,field) - Query a sorcery object field
//!
//! Sorcery is Asterisk's data abstraction layer. This function queries
//! objects through the sorcery framework.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// SORCERY() function.
///
/// Usage: SORCERY(module,object_type,id,field)
///
/// Examples:
///   ${SORCERY(res_pjsip,endpoint,alice,context)}
///   ${SORCERY(res_pjsip,aor,alice,max_contacts)}
///
/// In this port, lookups are simulated via context variables keyed by
/// the sorcery path: __SORCERY_{module}_{type}_{id}_{field}
pub struct FuncSorcery;

impl DialplanFunc for FuncSorcery {
    fn name(&self) -> &str {
        "SORCERY"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(4, ',').collect();
        if parts.len() < 4 {
            return Err(FuncError::InvalidArgument(
                "SORCERY: requires module,object_type,id,field arguments".to_string(),
            ));
        }

        let module = parts[0].trim();
        let obj_type = parts[1].trim();
        let id = parts[2].trim();
        let field = parts[3].trim();

        if module.is_empty() || obj_type.is_empty() || id.is_empty() || field.is_empty() {
            return Err(FuncError::InvalidArgument(
                "SORCERY: all arguments must be non-empty".to_string(),
            ));
        }

        let key = format!(
            "__SORCERY_{}_{}_{}_{}", module, obj_type, id, field
        );
        Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sorcery_empty() {
        let ctx = FuncContext::new();
        let func = FuncSorcery;
        let result = func.read(&ctx, "res_pjsip,endpoint,alice,context").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_sorcery_with_data() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__SORCERY_res_pjsip_endpoint_alice_context", "default");
        let func = FuncSorcery;
        assert_eq!(
            func.read(&ctx, "res_pjsip,endpoint,alice,context").unwrap(),
            "default"
        );
    }

    #[test]
    fn test_sorcery_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncSorcery;
        assert!(func.read(&ctx, "res_pjsip,endpoint,alice").is_err());
        assert!(func.read(&ctx, "res_pjsip,endpoint").is_err());
    }
}
