//! CONNECTEDLINE() function - read/write connected line information.
//!
//! Port of func_connectedline.c from Asterisk C.
//!
//! Provides:
//! - CONNECTEDLINE(datatype) - read/write connected party info
//!
//! Datatypes: name, name-pres, num, num-pres, source, subaddr, subaddr-type, tag, priv-*

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Presentation values for connected line information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    Allowed,
    Restricted,
    Unavailable,
}

impl Presentation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Restricted => "restricted",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "allowed" | "0" => Self::Allowed,
            "restricted" | "1" => Self::Restricted,
            "unavailable" | "2" => Self::Unavailable,
            _ => Self::Allowed,
        }
    }

    pub fn as_int(&self) -> i32 {
        match self {
            Self::Allowed => 0,
            Self::Restricted => 1,
            Self::Unavailable => 2,
        }
    }
}

/// Connected line information source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectedLineSource {
    Unknown,
    Answer,
    Dialplan,
    Operator,
    Transfer,
}

impl ConnectedLineSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Answer => "answer",
            Self::Dialplan => "dialplan",
            Self::Operator => "operator",
            Self::Transfer => "transfer",
        }
    }

    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "answer" => Self::Answer,
            "dialplan" => Self::Dialplan,
            "operator" => Self::Operator,
            "transfer" => Self::Transfer,
            _ => Self::Unknown,
        }
    }
}

/// CONNECTEDLINE() function.
///
/// Read/write connected line party information on a channel.
///
/// Usage:
///   ${CONNECTEDLINE(name)}       - Connected party name
///   ${CONNECTEDLINE(num)}        - Connected party number
///   ${CONNECTEDLINE(name-pres)}  - Name presentation
///   ${CONNECTEDLINE(num-pres)}   - Number presentation
///   ${CONNECTEDLINE(source)}     - Source of connected line info
///   ${CONNECTEDLINE(tag)}        - Tag
///   ${CONNECTEDLINE(subaddr)}    - Sub-address
///   ${CONNECTEDLINE(subaddr-type)} - Sub-address type
///
/// Write sets the specified field value.
pub struct FuncConnectedLine;

impl DialplanFunc for FuncConnectedLine {
    fn name(&self) -> &str {
        "CONNECTEDLINE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let field = args.trim().to_lowercase();
        let key = format!("__CONNECTEDLINE_{}", field.to_uppercase().replace('-', "_"));
        Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let field = args.trim().to_lowercase();

        match field.as_str() {
            "name" | "num" | "tag" | "subaddr" | "subaddr-type" => {
                let key = format!("__CONNECTEDLINE_{}", field.to_uppercase().replace('-', "_"));
                ctx.set_variable(&key, value);
                Ok(())
            }
            "name-pres" | "num-pres" => {
                let pres = Presentation::from_str_name(value);
                let key = format!("__CONNECTEDLINE_{}", field.to_uppercase().replace('-', "_"));
                ctx.set_variable(&key, pres.as_str());
                Ok(())
            }
            "source" => {
                let source = ConnectedLineSource::from_str_name(value);
                ctx.set_variable("__CONNECTEDLINE_SOURCE", source.as_str());
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "CONNECTEDLINE: unknown datatype '{}'",
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
        let func = FuncConnectedLine;
        assert_eq!(func.read(&ctx, "name").unwrap(), "");
    }

    #[test]
    fn test_write_and_read_name() {
        let mut ctx = FuncContext::new();
        let func = FuncConnectedLine;
        func.write(&mut ctx, "name", "Alice").unwrap();
        assert_eq!(func.read(&ctx, "name").unwrap(), "Alice");
    }

    #[test]
    fn test_write_and_read_num() {
        let mut ctx = FuncContext::new();
        let func = FuncConnectedLine;
        func.write(&mut ctx, "num", "5551234").unwrap();
        assert_eq!(func.read(&ctx, "num").unwrap(), "5551234");
    }

    #[test]
    fn test_presentation() {
        let mut ctx = FuncContext::new();
        let func = FuncConnectedLine;
        func.write(&mut ctx, "name-pres", "restricted").unwrap();
        assert_eq!(func.read(&ctx, "name-pres").unwrap(), "restricted");
    }

    #[test]
    fn test_source() {
        let mut ctx = FuncContext::new();
        let func = FuncConnectedLine;
        func.write(&mut ctx, "source", "answer").unwrap();
        assert_eq!(func.read(&ctx, "source").unwrap(), "answer");
    }

    #[test]
    fn test_invalid_field() {
        let mut ctx = FuncContext::new();
        let func = FuncConnectedLine;
        assert!(func.write(&mut ctx, "bogus", "val").is_err());
    }
}
