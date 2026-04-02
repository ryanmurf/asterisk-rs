//! SIP Geolocation header support (RFC 6442).
//!
//! Port of `res/res_pjsip_geolocation.c`. Stub implementation for
//! passing Geolocation and Geolocation-Routing headers through SIP
//! signalling. The actual geolocation data (PIDF-LO) is handled by
//! the `geolocation_res` module in `asterisk-res`.


// ---------------------------------------------------------------------------
// Geolocation header
// ---------------------------------------------------------------------------

/// Geolocation header value from a SIP message.
///
/// Per RFC 6442, the Geolocation header contains a URI reference
/// pointing to a PIDF-LO document, either by reference (HTTP URI)
/// or by value (CID URI for inline body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeolocationHeader {
    /// URI pointing to the PIDF-LO location object.
    pub location_uri: String,
    /// Routing allowed flag from Geolocation-Routing header.
    pub routing_allowed: bool,
}

impl GeolocationHeader {
    pub fn new(location_uri: &str) -> Self {
        Self {
            location_uri: location_uri.to_string(),
            routing_allowed: true,
        }
    }

    pub fn with_routing(mut self, allowed: bool) -> Self {
        self.routing_allowed = allowed;
        self
    }

    /// Check if this is a by-reference geolocation (HTTP/HTTPS URI).
    pub fn is_by_reference(&self) -> bool {
        self.location_uri.starts_with("http://")
            || self.location_uri.starts_with("https://")
    }

    /// Check if this is a by-value geolocation (CID URI for MIME body).
    pub fn is_by_value(&self) -> bool {
        self.location_uri.starts_with("cid:")
    }
}

/// Parse a Geolocation header value.
///
/// Format: `<https://example.com/location>` or `<cid:loc123@example.com>`
pub fn parse_geolocation_header(value: &str) -> Option<GeolocationHeader> {
    let trimmed = value.trim();
    // Extract URI from angle brackets if present
    let uri = if trimmed.starts_with('<') {
        let end = trimmed.find('>')?;
        &trimmed[1..end]
    } else {
        trimmed
    };

    if uri.is_empty() {
        return None;
    }

    Some(GeolocationHeader::new(uri))
}

/// Parse a Geolocation-Routing header value.
///
/// Format: `yes` or `no`
pub fn parse_geolocation_routing(value: &str) -> bool {
    let trimmed = value.trim().to_lowercase();
    trimmed == "yes" || trimmed == "true"
}

/// Generate Geolocation header value for outgoing messages.
pub fn generate_geolocation_header(geo: &GeolocationHeader) -> String {
    format!("<{}>", geo.location_uri)
}

/// Generate Geolocation-Routing header value.
pub fn generate_geolocation_routing(allowed: bool) -> &'static str {
    if allowed { "yes" } else { "no" }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_by_reference() {
        let geo = parse_geolocation_header("<https://example.com/location>").unwrap();
        assert_eq!(geo.location_uri, "https://example.com/location");
        assert!(geo.is_by_reference());
        assert!(!geo.is_by_value());
    }

    #[test]
    fn test_parse_by_value() {
        let geo = parse_geolocation_header("<cid:loc123@example.com>").unwrap();
        assert!(geo.is_by_value());
    }

    #[test]
    fn test_parse_no_brackets() {
        let geo = parse_geolocation_header("https://example.com/loc").unwrap();
        assert_eq!(geo.location_uri, "https://example.com/loc");
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_geolocation_header("").is_none());
        assert!(parse_geolocation_header("<>").is_none());
    }

    #[test]
    fn test_routing() {
        assert!(parse_geolocation_routing("yes"));
        assert!(parse_geolocation_routing("  Yes  "));
        assert!(!parse_geolocation_routing("no"));
    }

    #[test]
    fn test_generate() {
        let geo = GeolocationHeader::new("https://example.com/loc");
        assert_eq!(
            generate_geolocation_header(&geo),
            "<https://example.com/loc>"
        );
        assert_eq!(generate_geolocation_routing(true), "yes");
        assert_eq!(generate_geolocation_routing(false), "no");
    }
}
