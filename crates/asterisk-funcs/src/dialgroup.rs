//! DIALGROUP() function - manage groups of dial targets.
//!
//! Port of func_dialgroup.c from Asterisk C.
//!
//! Provides:
//! - DIALGROUP(group_name[,op]) - Manage groups of dial targets
//!
//! Read returns the dial string for the group (e.g., "SIP/alice&SIP/bob").
//! Write adds/removes members from the group.
//!
//! Operations:
//! - default (no op): Set(DIALGROUP(grp)=SIP/alice&SIP/bob) replaces group
//! - add: Set(DIALGROUP(grp,add)=SIP/charlie) adds to group
//! - del: Set(DIALGROUP(grp,del)=SIP/alice) removes from group

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// DIALGROUP() function.
///
/// Manages groups of dial destinations for use with the Dial() application.
///
/// Usage:
///   ${DIALGROUP(group)}           - Get group as dial string
///   Set(DIALGROUP(group)=targets) - Replace group members
///   Set(DIALGROUP(group,add)=target) - Add a target
///   Set(DIALGROUP(group,del)=target) - Remove a target
pub struct FuncDialGroup;

impl FuncDialGroup {
    fn group_key(group: &str) -> String {
        format!("__DIALGROUP_{}", group)
    }
}

impl DialplanFunc for FuncDialGroup {
    fn name(&self) -> &str {
        "DIALGROUP"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let group = args.split(',').next().unwrap_or("").trim();
        if group.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DIALGROUP: group name is required".to_string(),
            ));
        }
        let key = Self::group_key(group);
        Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let group = parts[0].trim();
        if group.is_empty() {
            return Err(FuncError::InvalidArgument(
                "DIALGROUP: group name is required".to_string(),
            ));
        }
        let op = parts.get(1).map(|s| s.trim().to_lowercase());
        let key = Self::group_key(group);

        match op.as_deref() {
            None | Some("") => {
                // Replace entire group
                ctx.set_variable(&key, value.trim());
                Ok(())
            }
            Some("add") => {
                let target = value.trim();
                if target.is_empty() {
                    return Ok(());
                }
                let current = ctx.get_variable(&key).cloned().unwrap_or_default();
                let members: Vec<&str> = current
                    .split('&')
                    .filter(|s| !s.is_empty())
                    .collect();
                if members.contains(&target) {
                    return Ok(()); // already in group
                }
                let new_val = if current.is_empty() {
                    target.to_string()
                } else {
                    format!("{}&{}", current, target)
                };
                ctx.set_variable(&key, &new_val);
                Ok(())
            }
            Some("del") => {
                let target = value.trim();
                let current = ctx.get_variable(&key).cloned().unwrap_or_default();
                let members: Vec<&str> = current
                    .split('&')
                    .filter(|s| !s.is_empty() && *s != target)
                    .collect();
                ctx.set_variable(&key, &members.join("&"));
                Ok(())
            }
            Some(other) => Err(FuncError::InvalidArgument(format!(
                "DIALGROUP: unknown operation '{}', expected add/del",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_group() {
        let ctx = FuncContext::new();
        let func = FuncDialGroup;
        assert_eq!(func.read(&ctx, "sales").unwrap(), "");
    }

    #[test]
    fn test_set_group() {
        let mut ctx = FuncContext::new();
        let func = FuncDialGroup;
        func.write(&mut ctx, "sales", "SIP/alice&SIP/bob").unwrap();
        assert_eq!(func.read(&ctx, "sales").unwrap(), "SIP/alice&SIP/bob");
    }

    #[test]
    fn test_add_member() {
        let mut ctx = FuncContext::new();
        let func = FuncDialGroup;
        func.write(&mut ctx, "sales", "SIP/alice").unwrap();
        func.write(&mut ctx, "sales,add", "SIP/bob").unwrap();
        assert_eq!(func.read(&ctx, "sales").unwrap(), "SIP/alice&SIP/bob");
    }

    #[test]
    fn test_add_duplicate() {
        let mut ctx = FuncContext::new();
        let func = FuncDialGroup;
        func.write(&mut ctx, "sales", "SIP/alice").unwrap();
        func.write(&mut ctx, "sales,add", "SIP/alice").unwrap();
        assert_eq!(func.read(&ctx, "sales").unwrap(), "SIP/alice");
    }

    #[test]
    fn test_del_member() {
        let mut ctx = FuncContext::new();
        let func = FuncDialGroup;
        func.write(&mut ctx, "sales", "SIP/alice&SIP/bob&SIP/charlie").unwrap();
        func.write(&mut ctx, "sales,del", "SIP/bob").unwrap();
        assert_eq!(func.read(&ctx, "sales").unwrap(), "SIP/alice&SIP/charlie");
    }

    #[test]
    fn test_invalid_op() {
        let mut ctx = FuncContext::new();
        let func = FuncDialGroup;
        assert!(func.write(&mut ctx, "sales,bogus", "x").is_err());
    }
}
