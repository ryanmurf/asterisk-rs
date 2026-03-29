//! CALLERID() function - read/write caller ID data.
//!
//! Port of func_callerid.c from Asterisk C.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// CALLERID() function.
///
/// Read/write CallerID data on the channel.
///
/// Usage:
///   CALLERID(name)     - Get/set caller name
///   CALLERID(num)      - Get/set caller number
///   CALLERID(all)      - Get "name" <number> format
///   CALLERID(ANI-num)  - Get/set ANI
///   CALLERID(RDNIS)    - Get/set RDNIS
///   CALLERID(DNID)     - Get/set DNID
///   CALLERID(pres)     - Get/set presentation
pub struct FuncCallerId;

impl DialplanFunc for FuncCallerId {
    fn name(&self) -> &str {
        "CALLERID"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let datatype = args.trim().to_lowercase();

        match datatype.as_str() {
            "name" => Ok(ctx.caller_name.clone().unwrap_or_default()),
            "num" | "number" => Ok(ctx.caller_number.clone().unwrap_or_default()),
            "all" => {
                let name = ctx.caller_name.as_deref().unwrap_or("");
                let num = ctx.caller_number.as_deref().unwrap_or("");
                if name.is_empty() && num.is_empty() {
                    Ok(String::new())
                } else if name.is_empty() {
                    Ok(num.to_string())
                } else if num.is_empty() {
                    Ok(format!("\"{}\"", name))
                } else {
                    Ok(format!("\"{}\" <{}>", name, num))
                }
            }
            "ani" | "ani-num" => Ok(ctx.ani.clone().unwrap_or_default()),
            "rdnis" => Ok(ctx.rdnis.clone().unwrap_or_default()),
            "dnid" => Ok(ctx.dnid.clone().unwrap_or_default()),
            "pres" | "name-pres" | "num-pres" => {
                // Default presentation
                Ok("allowed".to_string())
            }
            "name-valid" | "num-valid" => {
                // Return "1" if the respective field has a value
                let valid = match datatype.as_str() {
                    "name-valid" => ctx.caller_name.is_some(),
                    "num-valid" => ctx.caller_number.is_some(),
                    _ => false,
                };
                Ok(if valid { "1".to_string() } else { "0".to_string() })
            }
            "tag" => Ok(String::new()),
            _ => Err(FuncError::InvalidArgument(format!(
                "CALLERID: unknown datatype '{}'",
                datatype
            ))),
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let datatype = args.trim().to_lowercase();

        match datatype.as_str() {
            "name" => {
                ctx.caller_name = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            "num" | "number" => {
                ctx.caller_number = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            "all" => {
                // Parse "name" <number> format
                let (name, num) = parse_callerid_all(value);
                ctx.caller_name = name;
                ctx.caller_number = num;
                Ok(())
            }
            "ani" | "ani-num" => {
                ctx.ani = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            "rdnis" => {
                ctx.rdnis = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            "dnid" => {
                ctx.dnid = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "CALLERID: unknown datatype '{}' for write",
                datatype
            ))),
        }
    }
}

/// Parse a "Name" <Number> string into separate name and number parts.
fn parse_callerid_all(s: &str) -> (Option<String>, Option<String>) {
    let s = s.trim();
    if s.is_empty() {
        return (None, None);
    }

    // Try to parse "name" <number> format
    if let Some(lt_pos) = s.rfind('<') {
        if let Some(gt_pos) = s.rfind('>') {
            if gt_pos > lt_pos {
                let number = s[lt_pos + 1..gt_pos].trim().to_string();
                let name_part = s[..lt_pos].trim();
                let name = if name_part.starts_with('"') && name_part.ends_with('"') {
                    name_part[1..name_part.len() - 1].to_string()
                } else {
                    name_part.to_string()
                };
                let name = if name.is_empty() { None } else { Some(name) };
                let number = if number.is_empty() {
                    None
                } else {
                    Some(number)
                };
                return (name, number);
            }
        }
    }

    // No angle brackets - treat as number only
    (None, Some(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_name() {
        let mut ctx = FuncContext::new();
        ctx.caller_name = Some("Alice".to_string());
        let func = FuncCallerId;
        assert_eq!(func.read(&ctx, "name").unwrap(), "Alice");
    }

    #[test]
    fn test_read_num() {
        let mut ctx = FuncContext::new();
        ctx.caller_number = Some("5551234".to_string());
        let func = FuncCallerId;
        assert_eq!(func.read(&ctx, "num").unwrap(), "5551234");
    }

    #[test]
    fn test_read_all() {
        let mut ctx = FuncContext::new();
        ctx.caller_name = Some("Alice".to_string());
        ctx.caller_number = Some("5551234".to_string());
        let func = FuncCallerId;
        assert_eq!(func.read(&ctx, "all").unwrap(), "\"Alice\" <5551234>");
    }

    #[test]
    fn test_write_name() {
        let mut ctx = FuncContext::new();
        let func = FuncCallerId;
        func.write(&mut ctx, "name", "Bob").unwrap();
        assert_eq!(ctx.caller_name.as_deref(), Some("Bob"));
    }

    #[test]
    fn test_parse_callerid_all() {
        let (name, num) = parse_callerid_all("\"Alice\" <5551234>");
        assert_eq!(name.as_deref(), Some("Alice"));
        assert_eq!(num.as_deref(), Some("5551234"));

        let (name, num) = parse_callerid_all("5551234");
        assert!(name.is_none());
        assert_eq!(num.as_deref(), Some("5551234"));

        let (name, num) = parse_callerid_all("");
        assert!(name.is_none());
        assert!(num.is_none());
    }
}
