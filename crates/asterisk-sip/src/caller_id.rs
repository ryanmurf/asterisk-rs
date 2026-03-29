//! Caller ID from SIP headers.
//!
//! Port of `res/res_pjsip_caller_id.c`. Extracts caller identification
//! from From, P-Asserted-Identity (PAI), Remote-Party-ID (RPID), and
//! OLI parameters. Maps between SIP identity headers and the internal
//! caller ID / connected line representations.

use std::fmt;

use tracing::debug;

// ---------------------------------------------------------------------------
// Caller ID presentation
// ---------------------------------------------------------------------------

/// Caller ID presentation mode.
///
/// Maps to `ast_party_id_presentation` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerPresentation {
    /// Allowed, passed the screening.
    Allowed,
    /// Allowed but not screened.
    AllowedNotScreened,
    /// Restricted (privacy requested).
    Restricted,
    /// Restricted, not screened.
    RestrictedNotScreened,
    /// Unavailable.
    Unavailable,
}

impl CallerPresentation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::AllowedNotScreened => "allowed_not_screened",
            Self::Restricted => "restricted",
            Self::RestrictedNotScreened => "restricted_not_screened",
            Self::Unavailable => "unavailable",
        }
    }

    /// Determine presentation from SIP `privacy` parameter.
    pub fn from_privacy(privacy: &str) -> Self {
        match privacy.to_lowercase().as_str() {
            "full" | "name" | "uri" => Self::Restricted,
            "off" | "none" => Self::Allowed,
            _ => Self::Allowed,
        }
    }

    /// Determine presentation from RPID `screen` and `privacy` params.
    pub fn from_rpid_params(screen: &str, privacy: &str) -> Self {
        let screened = screen.eq_ignore_ascii_case("yes");
        let restricted = privacy.eq_ignore_ascii_case("full")
            || privacy.eq_ignore_ascii_case("name")
            || privacy.eq_ignore_ascii_case("uri");

        match (restricted, screened) {
            (true, true) => Self::Restricted,
            (true, false) => Self::RestrictedNotScreened,
            (false, true) => Self::Allowed,
            (false, false) => Self::AllowedNotScreened,
        }
    }
}

impl fmt::Display for CallerPresentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Caller ID
// ---------------------------------------------------------------------------

/// Caller identification extracted from SIP headers.
#[derive(Debug, Clone)]
pub struct CallerId {
    /// Caller name (display name from SIP URI).
    pub name: Option<String>,
    /// Caller number (user part of SIP URI).
    pub number: Option<String>,
    /// ANI2 / Originating Line Information.
    pub ani2: Option<i32>,
    /// Presentation mode.
    pub presentation: CallerPresentation,
    /// Source header this was extracted from.
    pub source: CallerIdSource,
}

/// Which SIP header the caller ID was extracted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdSource {
    /// From header.
    From,
    /// P-Asserted-Identity.
    PAI,
    /// Remote-Party-ID.
    RPID,
    /// Unknown.
    Unknown,
}

impl CallerId {
    pub fn new() -> Self {
        Self {
            name: None,
            number: None,
            ani2: None,
            presentation: CallerPresentation::Allowed,
            source: CallerIdSource::Unknown,
        }
    }

    /// Extract caller ID from a From header value.
    pub fn from_from_header(display_name: &str, user: &str) -> Self {
        Self {
            name: if display_name.is_empty() {
                None
            } else {
                Some(display_name.to_string())
            },
            number: if user.is_empty() {
                None
            } else {
                Some(user.to_string())
            },
            ani2: None,
            presentation: CallerPresentation::Allowed,
            source: CallerIdSource::From,
        }
    }

    /// Extract caller ID from a P-Asserted-Identity header value.
    ///
    /// PAI format: `"Display Name" <sip:user@host>` or `<sip:user@host>`
    pub fn from_pai(display_name: &str, user: &str, privacy: &str) -> Self {
        Self {
            name: if display_name.is_empty() {
                None
            } else {
                Some(display_name.to_string())
            },
            number: if user.is_empty() {
                None
            } else {
                Some(user.to_string())
            },
            ani2: None,
            presentation: CallerPresentation::from_privacy(privacy),
            source: CallerIdSource::PAI,
        }
    }

    /// Extract caller ID from a Remote-Party-ID header value.
    pub fn from_rpid(
        display_name: &str,
        user: &str,
        screen: &str,
        privacy: &str,
    ) -> Self {
        Self {
            name: if display_name.is_empty() {
                None
            } else {
                Some(display_name.to_string())
            },
            number: if user.is_empty() {
                None
            } else {
                Some(user.to_string())
            },
            ani2: None,
            presentation: CallerPresentation::from_rpid_params(screen, privacy),
            source: CallerIdSource::RPID,
        }
    }

    /// Whether this caller ID has a valid number.
    pub fn has_number(&self) -> bool {
        self.number.as_ref().map_or(false, |n| !n.is_empty())
    }

    /// Whether this caller ID has a valid name.
    pub fn has_name(&self) -> bool {
        self.name.as_ref().map_or(false, |n| !n.is_empty())
    }
}

impl Default for CallerId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// OLI extraction
// ---------------------------------------------------------------------------

/// Extract Originating Line Information (OLI/ANI2) from SIP URI parameters.
///
/// Looks for `isup-oli`, `ss7-oli`, or `oli` parameters.
pub fn extract_oli(params: &[(String, String)]) -> Option<i32> {
    for (name, value) in params {
        let lower = name.to_lowercase();
        if lower == "isup-oli" || lower == "ss7-oli" || lower == "oli" {
            if let Ok(oli) = value.parse::<i32>() {
                debug!(oli = oli, param = name.as_str(), "Extracted OLI");
                return Some(oli);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Connected line update decision
// ---------------------------------------------------------------------------

/// Determine if a connected line update should be queued.
///
/// Mirrors `should_queue_connected_line_update()` from the C source.
pub fn should_queue_connected_line_update(
    current_number: Option<&str>,
    new_id: &CallerId,
) -> bool {
    if !new_id.has_number() {
        return false;
    }
    match current_number {
        None | Some("") => true,
        Some(current) => current != new_id.number.as_deref().unwrap_or(""),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_from_header() {
        let cid = CallerId::from_from_header("Alice", "1001");
        assert_eq!(cid.name.as_deref(), Some("Alice"));
        assert_eq!(cid.number.as_deref(), Some("1001"));
        assert_eq!(cid.source, CallerIdSource::From);
    }

    #[test]
    fn test_from_pai() {
        let cid = CallerId::from_pai("Bob", "2001", "full");
        assert_eq!(cid.presentation, CallerPresentation::Restricted);
        assert_eq!(cid.source, CallerIdSource::PAI);
    }

    #[test]
    fn test_from_rpid() {
        let cid = CallerId::from_rpid("Carol", "3001", "yes", "off");
        assert_eq!(cid.presentation, CallerPresentation::Allowed);
        assert_eq!(cid.source, CallerIdSource::RPID);
    }

    #[test]
    fn test_rpid_presentation() {
        assert_eq!(
            CallerPresentation::from_rpid_params("no", "full"),
            CallerPresentation::RestrictedNotScreened,
        );
        assert_eq!(
            CallerPresentation::from_rpid_params("yes", "full"),
            CallerPresentation::Restricted,
        );
    }

    #[test]
    fn test_extract_oli() {
        let params = vec![
            ("transport".to_string(), "udp".to_string()),
            ("isup-oli".to_string(), "62".to_string()),
        ];
        assert_eq!(extract_oli(&params), Some(62));
    }

    #[test]
    fn test_extract_oli_not_present() {
        let params = vec![("transport".to_string(), "udp".to_string())];
        assert_eq!(extract_oli(&params), None);
    }

    #[test]
    fn test_connected_line_update_decision() {
        let cid = CallerId::from_from_header("", "1001");
        assert!(should_queue_connected_line_update(None, &cid));
        assert!(should_queue_connected_line_update(Some(""), &cid));
        assert!(should_queue_connected_line_update(Some("2001"), &cid));
        assert!(!should_queue_connected_line_update(Some("1001"), &cid));

        let empty_cid = CallerId::new();
        assert!(!should_queue_connected_line_update(None, &empty_cid));
    }
}
