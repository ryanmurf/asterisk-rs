//! PJSIP dialplan functions.
//!
//! Port of func_pjsip_endpoint.c, func_pjsip_contact.c, etc. from Asterisk C.
//!
//! Provides:
//! - PJSIP_DIAL_CONTACTS(endpoint[,aor[,request_user]]) - build dial string
//! - PJSIP_MEDIA_OFFER(media) - set media offer for SDP
//! - PJSIP_PARSE_URI(uri,type) - parse components of a SIP URI
//! - PJSIP_HEADER(action,header_name[,header_num]) - read/add/remove SIP headers
//!
//! These are stub implementations since they require the PJSIP stack.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// PJSIP_DIAL_CONTACTS() function.
///
/// Given an endpoint name, builds a dial string for all registered contacts.
pub struct FuncPjsipDialContacts;

impl DialplanFunc for FuncPjsipDialContacts {
    fn name(&self) -> &str {
        "PJSIP_DIAL_CONTACTS"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        let endpoint = parts.first().map(|s| s.trim()).unwrap_or("");
        if endpoint.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PJSIP_DIAL_CONTACTS requires an endpoint name".into(),
            ));
        }
        // Stub: would look up contacts for the endpoint from PJSIP registry
        // Returns dial string like "PJSIP/contact1&PJSIP/contact2"
        Ok(format!("PJSIP/{}", endpoint))
    }
}

/// PJSIP_MEDIA_OFFER() function.
///
/// Sets the media types to offer in the SDP for an outbound INVITE.
pub struct FuncPjsipMediaOffer;

impl DialplanFunc for FuncPjsipMediaOffer {
    fn name(&self) -> &str {
        "PJSIP_MEDIA_OFFER"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let media = args.trim();
        let var = format!("__PJSIP_MEDIA_OFFER_{}", media.to_uppercase());
        Ok(ctx.get_variable(&var).cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let media = args.trim();
        if media.is_empty() {
            return Err(FuncError::InvalidArgument(
                "PJSIP_MEDIA_OFFER requires a media type argument (audio/video)".into(),
            ));
        }
        let var = format!("__PJSIP_MEDIA_OFFER_{}", media.to_uppercase());
        ctx.set_variable(&var, value.trim());
        Ok(())
    }
}

/// PJSIP_PARSE_URI() function.
///
/// Parses a SIP URI and returns the requested component.
pub struct FuncPjsipParseUri;

impl DialplanFunc for FuncPjsipParseUri {
    fn name(&self) -> &str {
        "PJSIP_PARSE_URI"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "PJSIP_PARSE_URI requires uri and type arguments".into(),
            ));
        }
        let uri = parts[0].trim();
        let uri_type = parts[1].trim().to_lowercase();

        // Basic SIP URI parsing: sip:user@host:port;params
        let uri_body = uri.strip_prefix("sip:").or_else(|| uri.strip_prefix("sips:")).unwrap_or(uri);

        match uri_type.as_str() {
            "user" => {
                Ok(uri_body.split('@').next().unwrap_or("").to_string())
            }
            "host" => {
                let after_at = uri_body.split('@').nth(1).unwrap_or(uri_body);
                let host = after_at.split(':').next().unwrap_or(after_at);
                let host = host.split(';').next().unwrap_or(host);
                Ok(host.to_string())
            }
            "port" => {
                let after_at = uri_body.split('@').nth(1).unwrap_or(uri_body);
                if let Some(port_str) = after_at.split(':').nth(1) {
                    let port = port_str.split(';').next().unwrap_or(port_str);
                    Ok(port.to_string())
                } else {
                    Ok("5060".to_string())
                }
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "PJSIP_PARSE_URI: unknown type '{}', valid: user, host, port", uri_type
            ))),
        }
    }
}

/// PJSIP_HEADER() function.
///
/// Reads, adds, updates, or removes SIP headers on a channel.
pub struct FuncPjsipHeader;

impl DialplanFunc for FuncPjsipHeader {
    fn name(&self) -> &str {
        "PJSIP_HEADER"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "PJSIP_HEADER requires action and header_name".into(),
            ));
        }

        let action = parts[0].trim().to_lowercase();
        let header_name = parts[1].trim();
        let _header_num = parts.get(2).map(|s| s.trim()).unwrap_or("1");

        match action.as_str() {
            "read" => {
                let var = format!("__PJSIP_HEADER_{}", header_name.to_uppercase());
                Ok(ctx.get_variable(&var).cloned().unwrap_or_default())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "PJSIP_HEADER: read action expected 'read', got '{}'", action
            ))),
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "PJSIP_HEADER requires action and header_name".into(),
            ));
        }

        let action = parts[0].trim().to_lowercase();
        let header_name = parts[1].trim();

        match action.as_str() {
            "add" | "update" => {
                let var = format!("__PJSIP_HEADER_{}", header_name.to_uppercase());
                ctx.set_variable(&var, value.trim());
                Ok(())
            }
            "remove" => {
                let var = format!("__PJSIP_HEADER_{}", header_name.to_uppercase());
                ctx.set_variable(&var, "");
                Ok(())
            }
            _ => Err(FuncError::InvalidArgument(format!(
                "PJSIP_HEADER: action must be add, update, or remove, got '{}'", action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dial_contacts() {
        let ctx = FuncContext::new();
        let func = FuncPjsipDialContacts;
        let result = func.read(&ctx, "alice").unwrap();
        assert_eq!(result, "PJSIP/alice");
    }

    #[test]
    fn test_parse_uri_user() {
        let ctx = FuncContext::new();
        let func = FuncPjsipParseUri;
        let result = func.read(&ctx, "sip:alice@example.com:5060,user").unwrap();
        assert_eq!(result, "alice");
    }

    #[test]
    fn test_parse_uri_host() {
        let ctx = FuncContext::new();
        let func = FuncPjsipParseUri;
        let result = func.read(&ctx, "sip:alice@example.com:5060,host").unwrap();
        assert_eq!(result, "example.com");
    }

    #[test]
    fn test_parse_uri_port() {
        let ctx = FuncContext::new();
        let func = FuncPjsipParseUri;
        let result = func.read(&ctx, "sip:alice@example.com:5080,port").unwrap();
        assert_eq!(result, "5080");
    }

    #[test]
    fn test_pjsip_header_add_read() {
        let mut ctx = FuncContext::new();
        let func = FuncPjsipHeader;
        func.write(&mut ctx, "add,X-Custom", "my-value").unwrap();
        let result = func.read(&ctx, "read,X-Custom").unwrap();
        assert_eq!(result, "my-value");
    }
}
