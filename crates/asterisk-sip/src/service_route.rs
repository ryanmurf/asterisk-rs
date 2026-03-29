//! SIP Service-Route support — RFC 3608.
//!
//! The Service-Route header is returned by the registrar in a 200 OK
//! response to REGISTER. The UA stores these routes and applies them as
//! pre-loaded Route headers on all subsequent outbound requests for the
//! duration of the registration.

use crate::parser::{header_names, SipHeader, SipMessage};

/// Stored Service-Route set for a registration.
#[derive(Debug, Clone, Default)]
pub struct ServiceRouteSet {
    /// Ordered list of Service-Route header values.
    /// These become Route headers on outbound requests.
    routes: Vec<String>,
}

impl ServiceRouteSet {
    /// Create an empty service route set.
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
        }
    }

    /// Parse and store Service-Route headers from a REGISTER 200 OK response.
    ///
    /// Replaces any previously stored routes.
    pub fn update_from_response(&mut self, msg: &SipMessage) {
        let service_routes = msg.get_headers("Service-Route");
        self.routes = service_routes.iter().map(|s| s.to_string()).collect();
    }

    /// Check if there are any stored service routes.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Get the number of stored service routes.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Get the stored routes as a slice.
    pub fn routes(&self) -> &[String] {
        &self.routes
    }

    /// Apply the stored service routes as Route headers on an outbound request.
    ///
    /// This inserts the Service-Route values as Route headers at the beginning
    /// of the header list (before any existing Route headers).
    pub fn apply_to_request(&self, msg: &mut SipMessage) {
        if self.routes.is_empty() {
            return;
        }

        // Build Route headers from stored Service-Route values
        let route_headers: Vec<SipHeader> = self
            .routes
            .iter()
            .map(|r| SipHeader {
                name: header_names::ROUTE.to_string(),
                value: r.clone(),
            })
            .collect();

        // Find the position of existing Route headers (if any) to insert before them
        let insert_pos = msg
            .headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case(header_names::ROUTE))
            .unwrap_or(msg.headers.len());

        // Insert service route headers
        for (i, header) in route_headers.into_iter().enumerate() {
            msg.headers.insert(insert_pos + i, header);
        }
    }

    /// Clear stored service routes (e.g., on re-registration or unregister).
    pub fn clear(&mut self) {
        self.routes.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_service_route() {
        let response = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>;tag=def\r\n\
              Call-ID: sr-test\r\n\
              CSeq: 1 REGISTER\r\n\
              Service-Route: <sip:proxy1.example.com;lr>\r\n\
              Service-Route: <sip:proxy2.example.com;lr>\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let mut sr = ServiceRouteSet::new();
        assert!(sr.is_empty());

        sr.update_from_response(&response);
        assert_eq!(sr.len(), 2);
        assert_eq!(sr.routes()[0], "<sip:proxy1.example.com;lr>");
        assert_eq!(sr.routes()[1], "<sip:proxy2.example.com;lr>");
    }

    #[test]
    fn test_apply_service_route() {
        let mut sr = ServiceRouteSet::new();
        sr.routes = vec![
            "<sip:proxy1.example.com;lr>".to_string(),
            "<sip:proxy2.example.com;lr>".to_string(),
        ];

        let mut msg = SipMessage::parse(
            b"INVITE sip:bob@example.com SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK456\r\n\
              From: <sip:alice@example.com>;tag=xyz\r\n\
              To: <sip:bob@example.com>\r\n\
              Call-ID: invite-test\r\n\
              CSeq: 1 INVITE\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        sr.apply_to_request(&mut msg);

        let route_headers = msg.get_headers("Route");
        assert_eq!(route_headers.len(), 2);
        assert_eq!(route_headers[0], "<sip:proxy1.example.com;lr>");
        assert_eq!(route_headers[1], "<sip:proxy2.example.com;lr>");
    }

    #[test]
    fn test_service_route_clear() {
        let mut sr = ServiceRouteSet::new();
        sr.routes = vec!["<sip:proxy@example.com;lr>".to_string()];
        assert!(!sr.is_empty());

        sr.clear();
        assert!(sr.is_empty());
    }
}
