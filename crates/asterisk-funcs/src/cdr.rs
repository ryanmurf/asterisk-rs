//! CDR() function - read/write CDR (Call Detail Record) variables.
//!
//! Provides dialplan access to CDR fields for the current call.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// CDR() function.
///
/// Usage:
///   ${CDR(field)} - Read a CDR field
///   Set(CDR(field)=value) - Write a CDR field (where supported)
///
/// Supported fields: src, dst, dcontext, channel, dstchannel, lastapp,
///   lastdata, start, answer, end, duration, billsec, disposition,
///   amaflags, accountcode, uniqueid, linkedid, userfield, sequence
pub struct FuncCdr;

impl DialplanFunc for FuncCdr {
    fn name(&self) -> &str {
        "CDR"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let field = args.trim().to_lowercase();
        let cdr_key = format!("CDR_{}", field.to_uppercase());

        match field.as_str() {
            "src" => Ok(ctx
                .caller_number
                .clone()
                .unwrap_or_default()),
            "dst" => Ok(ctx.extension.clone()),
            "dcontext" => Ok(ctx.context.clone()),
            "channel" => Ok(ctx.channel_name.clone()),
            "dstchannel" => Ok(ctx
                .variables
                .get("CDR_DSTCHANNEL")
                .cloned()
                .unwrap_or_default()),
            "lastapp" => Ok(ctx
                .variables
                .get("CDR_LASTAPP")
                .cloned()
                .unwrap_or_default()),
            "lastdata" => Ok(ctx
                .variables
                .get("CDR_LASTDATA")
                .cloned()
                .unwrap_or_default()),
            "start" | "answer" | "end" => Ok(ctx
                .variables
                .get(&cdr_key)
                .cloned()
                .unwrap_or_default()),
            "duration" | "billsec" => Ok(ctx
                .variables
                .get(&cdr_key)
                .cloned()
                .unwrap_or_else(|| "0".to_string())),
            "disposition" => Ok(ctx
                .variables
                .get("CDR_DISPOSITION")
                .cloned()
                .unwrap_or_else(|| "NO ANSWER".to_string())),
            "amaflags" => Ok(ctx
                .variables
                .get("CDR_AMAFLAGS")
                .cloned()
                .unwrap_or_else(|| "DOCUMENTATION".to_string())),
            "accountcode" => Ok(ctx.account_code.clone()),
            "uniqueid" => Ok(ctx.channel_uniqueid.clone()),
            "linkedid" => Ok(ctx.channel_linkedid.clone()),
            "userfield" => Ok(ctx
                .variables
                .get("CDR_USERFIELD")
                .cloned()
                .unwrap_or_default()),
            "sequence" => Ok(ctx
                .variables
                .get("CDR_SEQUENCE")
                .cloned()
                .unwrap_or_else(|| "0".to_string())),
            _ => {
                // Check for user-defined CDR variables
                Ok(ctx.variables.get(&cdr_key).cloned().unwrap_or_default())
            }
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let field = args.trim().to_lowercase();

        match field.as_str() {
            "accountcode" => {
                ctx.account_code = value.to_string();
                Ok(())
            }
            "userfield" => {
                ctx.variables
                    .insert("CDR_USERFIELD".to_string(), value.to_string());
                Ok(())
            }
            "amaflags" => {
                ctx.variables
                    .insert("CDR_AMAFLAGS".to_string(), value.to_string());
                Ok(())
            }
            // Read-only fields
            "src" | "dst" | "dcontext" | "channel" | "dstchannel" | "start" | "answer"
            | "end" | "duration" | "billsec" | "disposition" | "uniqueid" | "linkedid"
            | "sequence" => Err(FuncError::ReadOnly(format!("CDR({})", field))),
            // Custom CDR variables
            _ => {
                let cdr_key = format!("CDR_{}", field.to_uppercase());
                ctx.variables.insert(cdr_key, value.to_string());
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_src() {
        let mut ctx = FuncContext::new();
        ctx.caller_number = Some("5551234".to_string());
        let func = FuncCdr;
        assert_eq!(func.read(&ctx, "src").unwrap(), "5551234");
    }

    #[test]
    fn test_read_dst() {
        let mut ctx = FuncContext::new();
        ctx.extension = "100".to_string();
        let func = FuncCdr;
        assert_eq!(func.read(&ctx, "dst").unwrap(), "100");
    }

    #[test]
    fn test_write_userfield() {
        let mut ctx = FuncContext::new();
        let func = FuncCdr;
        func.write(&mut ctx, "userfield", "test-data").unwrap();
        assert_eq!(func.read(&ctx, "userfield").unwrap(), "test-data");
    }

    #[test]
    fn test_readonly_field() {
        let mut ctx = FuncContext::new();
        let func = FuncCdr;
        assert!(func.write(&mut ctx, "duration", "100").is_err());
    }
}
