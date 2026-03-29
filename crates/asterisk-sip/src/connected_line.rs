//! Connected line updates via SIP re-INVITE/UPDATE.
//!
//! Port of `res/res_pjsip_connected_line.c`. Propagates connected line
//! identity changes (e.g., after a transfer) between SIP and the internal
//! channel representation using re-INVITE or UPDATE methods.

use tracing::debug;

// ---------------------------------------------------------------------------
// Connected line info
// ---------------------------------------------------------------------------

/// Connected line identity information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedLine {
    /// Party name.
    pub name: Option<String>,
    /// Party number.
    pub number: Option<String>,
    /// Presentation.
    pub presentation: ConnectedPresentation,
    /// Source of the update.
    pub source: ConnectedLineSource,
}

/// Presentation for connected line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectedPresentation {
    Allowed,
    Restricted,
    Unavailable,
}

impl ConnectedPresentation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Restricted => "restricted",
            Self::Unavailable => "unavailable",
        }
    }
}

/// How the connected line update is signalled in SIP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectedLineSource {
    /// Via PAI/RPID in a re-INVITE.
    ReInvite,
    /// Via PAI/RPID in an UPDATE.
    Update,
    /// From a 18x/2xx response.
    Response,
    /// From an initial INVITE.
    Invite,
    /// Unknown source.
    Unknown,
}

/// Update method preference for outgoing connected line changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectedLineMethod {
    /// Use re-INVITE.
    Invite,
    /// Use UPDATE (preferred if peer supports it).
    Update,
}

impl ConnectedLine {
    pub fn new() -> Self {
        Self {
            name: None,
            number: None,
            presentation: ConnectedPresentation::Allowed,
            source: ConnectedLineSource::Unknown,
        }
    }

    /// Create from PAI/RPID identity information.
    pub fn from_identity(
        name: &str,
        number: &str,
        restricted: bool,
        source: ConnectedLineSource,
    ) -> Self {
        Self {
            name: if name.is_empty() { None } else { Some(name.to_string()) },
            number: if number.is_empty() { None } else { Some(number.to_string()) },
            presentation: if restricted {
                ConnectedPresentation::Restricted
            } else {
                ConnectedPresentation::Allowed
            },
            source,
        }
    }

    /// Whether this connected line has usable information.
    pub fn is_valid(&self) -> bool {
        self.number.as_ref().map_or(false, |n| !n.is_empty())
    }

    /// Check if this connected line differs from another.
    pub fn differs_from(&self, other: &ConnectedLine) -> bool {
        self.number != other.number
            || self.name != other.name
            || self.presentation != other.presentation
    }
}

impl Default for ConnectedLine {
    fn default() -> Self {
        Self::new()
    }
}

/// Decide whether a connected line update should be sent.
///
/// Mirrors the logic from the C source that checks whether the
/// new connected line info differs from what was previously sent.
pub fn should_send_update(
    current: &ConnectedLine,
    new_line: &ConnectedLine,
) -> bool {
    if !new_line.is_valid() {
        return false;
    }
    current.differs_from(new_line)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connected_line_basic() {
        let cl = ConnectedLine::from_identity("Alice", "1001", false, ConnectedLineSource::ReInvite);
        assert!(cl.is_valid());
        assert_eq!(cl.presentation, ConnectedPresentation::Allowed);
    }

    #[test]
    fn test_connected_line_restricted() {
        let cl = ConnectedLine::from_identity("", "1001", true, ConnectedLineSource::Response);
        assert_eq!(cl.presentation, ConnectedPresentation::Restricted);
    }

    #[test]
    fn test_should_send_update() {
        let current = ConnectedLine::from_identity("Alice", "1001", false, ConnectedLineSource::Invite);
        let same = ConnectedLine::from_identity("Alice", "1001", false, ConnectedLineSource::ReInvite);
        let different = ConnectedLine::from_identity("Bob", "2001", false, ConnectedLineSource::ReInvite);

        assert!(!should_send_update(&current, &same));
        assert!(should_send_update(&current, &different));
    }

    #[test]
    fn test_empty_not_valid() {
        let cl = ConnectedLine::new();
        assert!(!cl.is_valid());
        assert!(!should_send_update(&cl, &ConnectedLine::new()));
    }
}
