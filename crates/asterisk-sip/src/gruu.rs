//! GRUU (Globally Routable User Agent URI) support — RFC 5627.
//!
//! A GRUU is a SIP URI that routes to a specific UA instance. There are two
//! types:
//! - **Public GRUU:** persistent, tied to an AOR, used for call routing to a
//!   specific device.
//! - **Temp GRUU:** ephemeral, changes each registration, used for privacy.
//!
//! During REGISTER, the UA includes `+sip.instance` in the Contact header.
//! The registrar returns `pub-gruu=` and `temp-gruu=` parameters in the 200 OK
//! Contact header.

use uuid::Uuid;

use crate::parser::{header_names, SipHeader, SipMessage, SipUri};

/// GRUU state for a single registration.
#[derive(Debug, Clone)]
pub struct Gruu {
    /// Public GRUU URI (persistent, tied to AOR).
    pub public_gruu: Option<SipUri>,
    /// Temporary GRUU URI (ephemeral, changes each registration).
    pub temp_gruu: Option<SipUri>,
    /// Instance ID in `urn:uuid:<uuid>` format, stable per device.
    pub instance_id: String,
}

impl Gruu {
    /// Create a new GRUU with a random instance ID.
    pub fn new() -> Self {
        Self {
            public_gruu: None,
            temp_gruu: None,
            instance_id: format!("urn:uuid:{}", Uuid::new_v4()),
        }
    }

    /// Create a GRUU with a specific instance ID.
    pub fn with_instance_id(instance_id: String) -> Self {
        Self {
            public_gruu: None,
            temp_gruu: None,
            instance_id,
        }
    }
}

impl Default for Gruu {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a Contact header value that includes GRUU parameters.
///
/// Adds `+sip.instance` to the Contact header for use in REGISTER requests.
/// This tells the registrar we support GRUU and provides our instance ID.
///
/// Example output:
/// `<sip:user@10.0.0.1:5060>;+sip.instance="<urn:uuid:...>"`
pub fn build_contact_with_gruu(contact_uri: &str, gruu: &Gruu) -> SipHeader {
    let value = format!(
        "<{}>;+sip.instance=\"<{}>\"",
        contact_uri, gruu.instance_id
    );
    SipHeader {
        name: header_names::CONTACT.to_string(),
        value,
    }
}

/// Split a header value by semicolons, but do not split inside quoted strings.
fn split_params_respecting_quotes(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut in_angle = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '<' if !in_quotes => {
                in_angle = true;
                current.push(ch);
            }
            '>' if !in_quotes => {
                in_angle = false;
                current.push(ch);
            }
            ';' if !in_quotes && !in_angle => {
                parts.push(std::mem::take(&mut current));
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Extract GRUU information from a 200 OK response to REGISTER.
///
/// Looks for `pub-gruu=` and `temp-gruu=` parameters in the Contact header
/// of the registration response.
pub fn extract_gruu_from_response(msg: &SipMessage, instance_id: &str) -> Option<Gruu> {
    // Find the Contact header that matches our instance ID.
    let contacts = msg.get_headers(header_names::CONTACT);

    for contact_val in contacts {
        // Check if this Contact contains our instance ID.
        if !contact_val.contains(instance_id) {
            continue;
        }

        let mut public_gruu = None;
        let mut temp_gruu = None;

        // Parse parameters after the URI, respecting quoted strings.
        // Contact: <sip:...>;pub-gruu="sip:...;gr=...";temp-gruu="sip:...;gr"
        // We must not split on semicolons inside quoted values.
        let params = split_params_respecting_quotes(contact_val);
        for part in &params {
            let trimmed = part.trim();

            if let Some(val) = trimmed.strip_prefix("pub-gruu=") {
                let uri_str = val.trim_matches('"');
                public_gruu = SipUri::parse(uri_str).ok();
            } else if let Some(val) = trimmed.strip_prefix("temp-gruu=") {
                let uri_str = val.trim_matches('"');
                temp_gruu = SipUri::parse(uri_str).ok();
            }
        }

        if public_gruu.is_some() || temp_gruu.is_some() {
            return Some(Gruu {
                public_gruu,
                temp_gruu,
                instance_id: instance_id.to_string(),
            });
        }
    }

    None
}

/// Check if a SIP URI is a GRUU.
///
/// A GRUU typically contains a `gr` parameter (public GRUU) or has the
/// `gr` parameter with no value (temp GRUU).
pub fn is_gruu(uri: &SipUri) -> bool {
    uri.parameters.contains_key("gr")
}

/// Build a `Supported` header value that includes `gruu`.
pub fn supported_gruu() -> SipHeader {
    SipHeader {
        name: header_names::SUPPORTED.to_string(),
        value: "gruu".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gruu_creation() {
        let gruu = Gruu::new();
        assert!(gruu.instance_id.starts_with("urn:uuid:"));
        assert!(gruu.public_gruu.is_none());
        assert!(gruu.temp_gruu.is_none());
    }

    #[test]
    fn test_build_contact_with_gruu() {
        let gruu = Gruu::with_instance_id("urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6".to_string());
        let header = build_contact_with_gruu("sip:alice@10.0.0.1:5060", &gruu);
        assert_eq!(header.name, "Contact");
        assert!(header.value.contains("+sip.instance"));
        assert!(header.value.contains("urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6"));
    }

    #[test]
    fn test_extract_gruu_from_response() {
        let instance_id = "urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6";
        let response = SipMessage::parse(
            format!(
                "SIP/2.0 200 OK\r\n\
                 Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
                 From: <sip:alice@example.com>;tag=abc\r\n\
                 To: <sip:alice@example.com>;tag=def\r\n\
                 Call-ID: gruu-test\r\n\
                 CSeq: 1 REGISTER\r\n\
                 Contact: <sip:alice@10.0.0.1>;+sip.instance=\"<{}>\";pub-gruu=\"sip:alice@example.com;gr=urn:uuid:f81d4fae\";temp-gruu=\"sip:tgruu.1@example.com;gr\"\r\n\
                 Content-Length: 0\r\n\
                 \r\n",
                instance_id
            )
            .as_bytes(),
        )
        .unwrap();

        let gruu = extract_gruu_from_response(&response, instance_id).unwrap();
        assert!(gruu.public_gruu.is_some());
        assert!(gruu.temp_gruu.is_some());

        let pub_gruu = gruu.public_gruu.unwrap();
        assert_eq!(pub_gruu.host, "example.com");
        assert!(is_gruu(&pub_gruu));

        let temp_gruu = gruu.temp_gruu.unwrap();
        assert!(is_gruu(&temp_gruu));
    }

    #[test]
    fn test_is_gruu() {
        let gruu_uri = SipUri::parse("sip:alice@example.com;gr=urn:uuid:abc123").unwrap();
        assert!(is_gruu(&gruu_uri));

        let normal_uri = SipUri::parse("sip:alice@example.com").unwrap();
        assert!(!is_gruu(&normal_uri));
    }
}
