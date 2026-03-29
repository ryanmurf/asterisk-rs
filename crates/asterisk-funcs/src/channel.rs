//! CHANNEL() function - read/write channel properties.
//!
//! Port of func_channel.c from Asterisk C.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// CHANNEL() function.
///
/// Read/write various pieces of information about the channel.
///
/// Usage:
///   CHANNEL(state)      - R/O channel state
///   CHANNEL(name)       - R/O channel name
///   CHANNEL(uniqueid)   - R/O unique ID
///   CHANNEL(linkedid)   - R/O linked ID
///   CHANNEL(accountcode)- R/W account code
///   CHANNEL(context)    - R/O current context
///   CHANNEL(exten)      - R/O current extension
///   CHANNEL(priority)   - R/O current priority
///   CHANNEL(hangupsource) - R/O hangup source
///   CHANNEL(secure_bridge_signaling) - R/O if secure
///   CHANNEL(secure_bridge_media) - R/O if secure media
pub struct FuncChannel;

impl DialplanFunc for FuncChannel {
    fn name(&self) -> &str {
        "CHANNEL"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let item = args.trim().to_lowercase();

        match item.as_str() {
            "state" | "channelstate" => Ok(ctx.channel_state.clone()),
            "name" | "channelname" => Ok(ctx.channel_name.clone()),
            "uniqueid" => Ok(ctx.channel_uniqueid.clone()),
            "linkedid" => Ok(ctx.channel_linkedid.clone()),
            "accountcode" => Ok(ctx.account_code.clone()),
            "context" => Ok(ctx.context.clone()),
            "exten" | "extension" => Ok(ctx.extension.clone()),
            "priority" => Ok(ctx.priority.to_string()),
            "language" => {
                Ok(ctx.variables.get("CHANNEL_LANGUAGE").cloned().unwrap_or_else(|| "en".to_string()))
            }
            "musicclass" => {
                Ok(ctx.variables.get("CHANNEL_MUSICCLASS").cloned().unwrap_or_else(|| "default".to_string()))
            }
            "amaflags" => {
                Ok(ctx.variables.get("CHANNEL_AMAFLAGS").cloned().unwrap_or_else(|| "3".to_string()))
            }
            "hangupsource" => {
                Ok(ctx.variables.get("CHANNEL_HANGUPSOURCE").cloned().unwrap_or_default())
            }
            "secure_bridge_signaling" => Ok("0".to_string()),
            "secure_bridge_media" => Ok("0".to_string()),
            "audioreadformat" | "audiowriteformat" | "audionativeformat" => {
                Ok("ulaw".to_string())
            }
            "callgroup" | "pickupgroup" => Ok("".to_string()),
            "dtmf_features" => {
                Ok(ctx.variables.get("CHANNEL_DTMF_FEATURES").cloned().unwrap_or_default())
            }
            "max_forwards" => {
                Ok(ctx.variables.get("MAX_FORWARDS").cloned().unwrap_or_else(|| "70".to_string()))
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "CHANNEL: unknown item '{}'",
                item
            ))),
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let item = args.trim().to_lowercase();

        match item.as_str() {
            "accountcode" => {
                ctx.account_code = value.to_string();
                Ok(())
            }
            "language" => {
                ctx.variables.insert("CHANNEL_LANGUAGE".to_string(), value.to_string());
                Ok(())
            }
            "musicclass" => {
                ctx.variables.insert("CHANNEL_MUSICCLASS".to_string(), value.to_string());
                Ok(())
            }
            "amaflags" => {
                ctx.variables.insert("CHANNEL_AMAFLAGS".to_string(), value.to_string());
                Ok(())
            }
            "dtmf_features" => {
                ctx.variables.insert("CHANNEL_DTMF_FEATURES".to_string(), value.to_string());
                Ok(())
            }
            "max_forwards" => {
                ctx.variables.insert("MAX_FORWARDS".to_string(), value.to_string());
                Ok(())
            }
            "state" | "name" | "uniqueid" | "linkedid" | "context" | "exten" | "priority" => {
                Err(FuncError::ReadOnly(format!("CHANNEL({})", item)))
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "CHANNEL: unknown item '{}' for write",
                item
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_channel_name() {
        let mut ctx = FuncContext::new();
        ctx.channel_name = "SIP/alice-00000001".to_string();
        let func = FuncChannel;
        assert_eq!(func.read(&ctx, "name").unwrap(), "SIP/alice-00000001");
    }

    #[test]
    fn test_read_channel_state() {
        let ctx = FuncContext::new();
        let func = FuncChannel;
        assert_eq!(func.read(&ctx, "state").unwrap(), "Down");
    }

    #[test]
    fn test_write_accountcode() {
        let mut ctx = FuncContext::new();
        let func = FuncChannel;
        func.write(&mut ctx, "accountcode", "ACCT-001").unwrap();
        assert_eq!(ctx.account_code, "ACCT-001");
    }

    #[test]
    fn test_readonly_fields() {
        let mut ctx = FuncContext::new();
        let func = FuncChannel;
        assert!(func.write(&mut ctx, "name", "test").is_err());
        assert!(func.write(&mut ctx, "uniqueid", "test").is_err());
    }
}
