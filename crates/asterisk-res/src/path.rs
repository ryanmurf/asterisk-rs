//! SIP Path header support (RFC 3327).
//!
//! Implements the SIP Path extension used by proxies to insert themselves
//! into the routing path of a registration. Stored paths are applied as
//! Route headers when routing requests to the registered endpoint.

use std::fmt;

use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum PathError {
    #[error("invalid Path header: {0}")]
    InvalidHeader(String),
    #[error("no Path stored for AOR {0}")]
    NoPath(String),
}

pub type PathResult<T> = Result<T, PathError>;

// ---------------------------------------------------------------------------
// Path header value
// ---------------------------------------------------------------------------

/// A single SIP URI from a Path header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    /// The SIP URI (e.g., `<sip:proxy.example.com;lr>`).
    pub uri: String,
}

impl PathEntry {
    pub fn new(uri: &str) -> Self {
        Self {
            uri: uri.to_string(),
        }
    }

    /// Ensure the URI has the `lr` (loose-route) parameter.
    pub fn ensure_lr(&mut self) {
        if !self.uri.contains(";lr") {
            // Insert lr before the closing >.
            if let Some(pos) = self.uri.rfind('>') {
                self.uri.insert_str(pos, ";lr");
            } else {
                self.uri.push_str(";lr");
            }
        }
    }
}

impl fmt::Display for PathEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.uri)
    }
}

// ---------------------------------------------------------------------------
// Path set (stored for a registration)
// ---------------------------------------------------------------------------

/// The set of Path headers associated with a registration.
///
/// When a REGISTER passes through proxies, each proxy adds a Path header.
/// The registrar stores these and uses them as Route headers when routing
/// subsequent requests to the registered contact.
#[derive(Debug, Clone, Default)]
pub struct PathSet {
    /// Ordered list of Path entries (first = outermost proxy).
    pub entries: Vec<PathEntry>,
}

impl PathSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a Path entry (outermost proxy should be added first).
    pub fn add(&mut self, entry: PathEntry) {
        debug!(uri = %entry.uri, "Path entry added");
        self.entries.push(entry);
    }

    /// Parse Path header values from a comma-separated string.
    ///
    /// Multiple Path headers or a single comma-separated header are handled.
    pub fn parse(header_value: &str) -> PathResult<Self> {
        let mut set = PathSet::new();
        for part in header_value.split(',') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Basic validation: should contain "sip:" or "sips:".
            if !trimmed.contains("sip:") && !trimmed.contains("sips:") {
                return Err(PathError::InvalidHeader(trimmed.to_string()));
            }
            set.entries.push(PathEntry::new(trimmed));
        }
        Ok(set)
    }

    /// Format the stored paths as Route headers for outbound routing.
    ///
    /// Returns a list of Route header values in the correct order.
    pub fn to_route_headers(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| format!("Route: {}", e.uri))
            .collect()
    }

    /// Format as a single Path header value (comma-separated).
    pub fn to_header_value(&self) -> String {
        self.entries
            .iter()
            .map(|e| e.uri.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether any Path entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of stored path entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// Path header construction for proxies
// ---------------------------------------------------------------------------

/// Build a Path header value for a proxy inserting itself into the path.
///
/// `local_uri` is the proxy's own SIP URI. The `lr` parameter is ensured.
pub fn build_path_header(local_uri: &str) -> String {
    let mut entry = PathEntry::new(local_uri);
    entry.ensure_lr();
    format!("Path: {}", entry.uri)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_entry_basic() {
        let entry = PathEntry::new("<sip:proxy.example.com;lr>");
        assert_eq!(entry.uri, "<sip:proxy.example.com;lr>");
    }

    #[test]
    fn test_path_entry_ensure_lr() {
        let mut entry = PathEntry::new("<sip:proxy.example.com>");
        entry.ensure_lr();
        assert!(entry.uri.contains(";lr"));
        assert_eq!(entry.uri, "<sip:proxy.example.com;lr>");
    }

    #[test]
    fn test_path_entry_ensure_lr_already_present() {
        let mut entry = PathEntry::new("<sip:proxy.example.com;lr>");
        entry.ensure_lr();
        // Should not double-add.
        assert_eq!(
            entry.uri.matches(";lr").count(),
            1
        );
    }

    #[test]
    fn test_path_set_parse() {
        let input = "<sip:proxy1.example.com;lr>, <sip:proxy2.example.com;lr>";
        let set = PathSet::parse(input).unwrap();
        assert_eq!(set.len(), 2);
        assert_eq!(set.entries[0].uri, "<sip:proxy1.example.com;lr>");
        assert_eq!(set.entries[1].uri, "<sip:proxy2.example.com;lr>");
    }

    #[test]
    fn test_path_set_parse_invalid() {
        let result = PathSet::parse("not-a-sip-uri");
        assert!(result.is_err());
    }

    #[test]
    fn test_to_route_headers() {
        let mut set = PathSet::new();
        set.add(PathEntry::new("<sip:p1.example.com;lr>"));
        set.add(PathEntry::new("<sip:p2.example.com;lr>"));

        let routes = set.to_route_headers();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0], "Route: <sip:p1.example.com;lr>");
        assert_eq!(routes[1], "Route: <sip:p2.example.com;lr>");
    }

    #[test]
    fn test_to_header_value() {
        let mut set = PathSet::new();
        set.add(PathEntry::new("<sip:p1.example.com;lr>"));
        set.add(PathEntry::new("<sip:p2.example.com;lr>"));
        assert_eq!(
            set.to_header_value(),
            "<sip:p1.example.com;lr>, <sip:p2.example.com;lr>"
        );
    }

    #[test]
    fn test_build_path_header() {
        let header = build_path_header("<sip:myproxy.example.com>");
        assert_eq!(header, "Path: <sip:myproxy.example.com;lr>");
    }
}
