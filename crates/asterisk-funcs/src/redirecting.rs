//! REDIRECTING() function - read/write redirecting call information.
//!
//! Port of func_redirecting.c from Asterisk C.
//!
//! Provides:
//! - REDIRECTING(datatype) - read/write call redirecting info
//!
//! Datatypes: from-name, from-num, from-pres, to-name, to-num, to-pres,
//!            reason, count, priv-from-name, priv-from-num, priv-to-name, priv-to-num

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Redirecting reason codes (Q.931/SIP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectReason {
    Unknown,
    UserBusy,
    NoReply,
    Unconditional,
    TimeOfDay,
    DoNotDisturb,
    Deflection,
    FollowMe,
    OutOfOrder,
    Away,
    CallFwdByDTE,
}

impl RedirectReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UserBusy => "user_busy",
            Self::NoReply => "no_reply",
            Self::Unconditional => "unconditional",
            Self::TimeOfDay => "time_of_day",
            Self::DoNotDisturb => "do_not_disturb",
            Self::Deflection => "deflection",
            Self::FollowMe => "follow_me",
            Self::OutOfOrder => "out_of_order",
            Self::Away => "away",
            Self::CallFwdByDTE => "call_fwd_by_dte",
        }
    }

    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "user_busy" | "busy" => Self::UserBusy,
            "no_reply" | "noreply" => Self::NoReply,
            "unconditional" | "cfu" => Self::Unconditional,
            "time_of_day" => Self::TimeOfDay,
            "do_not_disturb" | "dnd" => Self::DoNotDisturb,
            "deflection" => Self::Deflection,
            "follow_me" => Self::FollowMe,
            "out_of_order" => Self::OutOfOrder,
            "away" => Self::Away,
            "call_fwd_by_dte" => Self::CallFwdByDTE,
            _ => Self::Unknown,
        }
    }
}

/// REDIRECTING() function.
///
/// Read/write call redirecting information.
///
/// Usage:
///   ${REDIRECTING(from-name)}   - Redirecting-from name
///   ${REDIRECTING(from-num)}    - Redirecting-from number
///   ${REDIRECTING(to-name)}     - Redirecting-to name
///   ${REDIRECTING(to-num)}      - Redirecting-to number
///   ${REDIRECTING(reason)}      - Redirect reason
///   ${REDIRECTING(count)}       - Number of redirections
///   ${REDIRECTING(from-pres)}   - From-party presentation
///   ${REDIRECTING(to-pres)}     - To-party presentation
pub struct FuncRedirecting;

impl DialplanFunc for FuncRedirecting {
    fn name(&self) -> &str {
        "REDIRECTING"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let field = args.trim().to_lowercase();
        let key = format!("__REDIRECTING_{}", field.to_uppercase().replace('-', "_"));
        Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let field = args.trim().to_lowercase();

        match field.as_str() {
            "from-name" | "from-num" | "from-pres" | "to-name" | "to-num" | "to-pres"
            | "priv-from-name" | "priv-from-num" | "priv-to-name" | "priv-to-num" => {
                let key = format!("__REDIRECTING_{}", field.to_uppercase().replace('-', "_"));
                ctx.set_variable(&key, value);
                Ok(())
            }
            "reason" => {
                let reason = RedirectReason::from_str_name(value);
                ctx.set_variable("__REDIRECTING_REASON", reason.as_str());
                Ok(())
            }
            "count" => {
                let _count: u32 = value.parse().map_err(|_| {
                    FuncError::InvalidArgument(format!(
                        "REDIRECTING(count): invalid count '{}'",
                        value
                    ))
                })?;
                ctx.set_variable("__REDIRECTING_COUNT", value);
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "REDIRECTING: unknown datatype '{}'",
                field
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_empty() {
        let ctx = FuncContext::new();
        let func = FuncRedirecting;
        assert_eq!(func.read(&ctx, "from-name").unwrap(), "");
    }

    #[test]
    fn test_write_and_read() {
        let mut ctx = FuncContext::new();
        let func = FuncRedirecting;
        func.write(&mut ctx, "from-name", "Alice").unwrap();
        func.write(&mut ctx, "from-num", "5551234").unwrap();
        func.write(&mut ctx, "to-num", "5559876").unwrap();
        assert_eq!(func.read(&ctx, "from-name").unwrap(), "Alice");
        assert_eq!(func.read(&ctx, "from-num").unwrap(), "5551234");
        assert_eq!(func.read(&ctx, "to-num").unwrap(), "5559876");
    }

    #[test]
    fn test_reason() {
        let mut ctx = FuncContext::new();
        let func = FuncRedirecting;
        func.write(&mut ctx, "reason", "unconditional").unwrap();
        assert_eq!(func.read(&ctx, "reason").unwrap(), "unconditional");
    }

    #[test]
    fn test_count() {
        let mut ctx = FuncContext::new();
        let func = FuncRedirecting;
        func.write(&mut ctx, "count", "3").unwrap();
        assert_eq!(func.read(&ctx, "count").unwrap(), "3");
    }

    #[test]
    fn test_invalid_count() {
        let mut ctx = FuncContext::new();
        let func = FuncRedirecting;
        assert!(func.write(&mut ctx, "count", "abc").is_err());
    }

    #[test]
    fn test_invalid_field() {
        let mut ctx = FuncContext::new();
        let func = FuncRedirecting;
        assert!(func.write(&mut ctx, "bogus", "val").is_err());
    }
}
