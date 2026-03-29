//! Dialplan existence-check functions.
//!
//! Port of func_dialplan.c from Asterisk C.
//!
//! Provides:
//! - VALID_EXTEN(context,exten,priority) - check if extension exists
//! - DIALPLAN_EXISTS(context,exten,priority) - alias for VALID_EXTEN

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// VALID_EXTEN() function.
///
/// Checks whether a given extension exists in the dialplan.
///
/// Usage: VALID_EXTEN(context,extension,priority)
///
/// Returns "1" if the extension exists, "0" otherwise.
///
/// If context is empty, the current context is used.
/// If priority is empty, "1" is assumed.
pub struct FuncValidExten;

impl DialplanFunc for FuncValidExten {
    fn name(&self) -> &str {
        "VALID_EXTEN"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (context, exten, priority) = parse_dialplan_args(args, ctx)?;

        // Check via channel variables for a registered extension map.
        // In a full implementation this would query the PBX dialplan engine.
        // For now we check the variable __DIALPLAN_<context>_<exten>_<priority>.
        let key = format!("__DIALPLAN_{}_{}_{}", context, exten, priority);
        if ctx.get_variable(&key).is_some() {
            Ok("1".to_string())
        } else {
            // Also check without priority (extension exists at all)
            let key_no_pri = format!("__DIALPLAN_{}_{}", context, exten);
            if ctx.get_variable(&key_no_pri).is_some() {
                Ok("1".to_string())
            } else {
                Ok("0".to_string())
            }
        }
    }
}

/// DIALPLAN_EXISTS() function.
///
/// Identical to VALID_EXTEN() - checks whether an extension exists.
///
/// Usage: DIALPLAN_EXISTS(context,extension,priority)
pub struct FuncDialplanExists;

impl DialplanFunc for FuncDialplanExists {
    fn name(&self) -> &str {
        "DIALPLAN_EXISTS"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        // Delegate to same logic as VALID_EXTEN
        let valid_exten = FuncValidExten;
        valid_exten.read(ctx, args)
    }
}

/// Parse dialplan function arguments: context,extension,priority.
///
/// If context is empty, uses the current context from the FuncContext.
/// If priority is empty, defaults to "1".
fn parse_dialplan_args(
    args: &str,
    ctx: &FuncContext,
) -> Result<(String, String, String), FuncError> {
    let parts: Vec<&str> = args.splitn(3, ',').collect();

    let context = parts
        .first()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| ctx.context.clone());

    let exten = parts
        .get(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FuncError::InvalidArgument(
                "VALID_EXTEN/DIALPLAN_EXISTS: extension argument is required".to_string(),
            )
        })?;

    let priority = parts
        .get(2)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "1".to_string());

    Ok((context, exten, priority))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_exten_not_found() {
        let ctx = FuncContext::new();
        let func = FuncValidExten;
        assert_eq!(func.read(&ctx, "default,100,1").unwrap(), "0");
    }

    #[test]
    fn test_valid_exten_found() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__DIALPLAN_default_100_1", "1");
        let func = FuncValidExten;
        assert_eq!(func.read(&ctx, "default,100,1").unwrap(), "1");
    }

    #[test]
    fn test_dialplan_exists_alias() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__DIALPLAN_myctx_200_1", "1");
        let func = FuncDialplanExists;
        assert_eq!(func.read(&ctx, "myctx,200,1").unwrap(), "1");
    }

    #[test]
    fn test_default_context_and_priority() {
        let mut ctx = FuncContext::new();
        ctx.context = "incoming".to_string();
        ctx.set_variable("__DIALPLAN_incoming_s_1", "1");
        let func = FuncValidExten;
        assert_eq!(func.read(&ctx, ",s,").unwrap(), "1");
    }

    #[test]
    fn test_missing_extension() {
        let ctx = FuncContext::new();
        let func = FuncValidExten;
        assert!(func.read(&ctx, "default").is_err());
    }
}
