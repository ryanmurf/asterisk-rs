//! SIP Diversion header handling (port of res_pjsip_diversion.c).
//!
//! Parses and generates the Diversion header (RFC 5806) for call
//! forwarding scenarios. Maps between SIP diversion reasons and
//! internal redirecting reason codes.

use std::fmt;


use crate::parser::{extract_uri, SipHeader, SipMessage};

// ---------------------------------------------------------------------------
// Redirecting reason mapping
// ---------------------------------------------------------------------------

/// Internal redirecting reason codes (mirrors Asterisk's AST_REDIRECTING_REASON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectingReason {
    Unknown,
    UserBusy,
    NoAnswer,
    Unavailable,
    Unconditional,
    TimeOfDay,
    DoNotDisturb,
    Deflection,
    FollowMe,
    OutOfOrder,
    Away,
    CallFwdDte,
    SendToVm,
}

impl RedirectingReason {
    /// Convert from SIP Diversion reason string.
    pub fn from_sip_reason(reason: &str) -> Self {
        match reason.trim().to_lowercase().as_str() {
            "user-busy" => Self::UserBusy,
            "no-answer" => Self::NoAnswer,
            "unavailable" => Self::Unavailable,
            "unconditional" => Self::Unconditional,
            "time-of-day" => Self::TimeOfDay,
            "do-not-disturb" => Self::DoNotDisturb,
            "deflection" => Self::Deflection,
            "follow-me" => Self::FollowMe,
            "out-of-service" => Self::OutOfOrder,
            "away" => Self::Away,
            "cf_dte" => Self::CallFwdDte,
            "send_to_vm" => Self::SendToVm,
            _ => Self::Unknown,
        }
    }

    /// Convert to SIP Diversion reason string.
    pub fn to_sip_reason(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UserBusy => "user-busy",
            Self::NoAnswer => "no-answer",
            Self::Unavailable => "unavailable",
            Self::Unconditional => "unconditional",
            Self::TimeOfDay => "time-of-day",
            Self::DoNotDisturb => "do-not-disturb",
            Self::Deflection => "deflection",
            Self::FollowMe => "follow-me",
            Self::OutOfOrder => "out-of-service",
            Self::Away => "away",
            Self::CallFwdDte => "cf_dte",
            Self::SendToVm => "send_to_vm",
        }
    }

    /// Map reason to a SIP response cause code.
    pub fn to_cause_code(&self) -> u16 {
        match self {
            Self::Unconditional => 302,
            Self::UserBusy => 486,
            Self::NoAnswer => 408,
            Self::Deflection => 480,
            Self::Unavailable => 503,
            _ => 404,
        }
    }

    /// Map from SIP cause code to redirecting reason.
    pub fn from_cause_code(cause: u16) -> Self {
        match cause {
            302 => Self::Unconditional,
            486 => Self::UserBusy,
            408 => Self::NoAnswer,
            480 | 487 => Self::Deflection,
            503 => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for RedirectingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sip_reason())
    }
}

// ---------------------------------------------------------------------------
// Diversion info
// ---------------------------------------------------------------------------

/// Parsed information from a SIP Diversion header.
#[derive(Debug, Clone)]
pub struct DiversionInfo {
    /// The URI of the diverting party (who forwarded the call).
    pub diverting_uri: String,
    /// Display name of the diverting party (if present).
    pub diverting_name: Option<String>,
    /// Reason for the diversion.
    pub reason: RedirectingReason,
    /// The raw reason string (for non-standard reasons).
    pub reason_str: String,
    /// Privacy indicator from the Diversion header.
    pub privacy: Option<String>,
    /// Counter (number of times the call has been diverted).
    pub counter: Option<u32>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a Diversion header value.
///
/// Format: `"Display Name" <sip:user@host>;reason=no-answer;counter=1;privacy=off`
pub fn parse_diversion_header(value: &str) -> Option<DiversionInfo> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    // Extract display name (if in quotes).
    let diverting_name = if value.starts_with('"') {
        value
            .find('"')
            .and_then(|start| value[start + 1..].find('"').map(|end| start + 1 + end))
            .map(|end| value[1..end].to_string())
    } else {
        None
    };

    // Extract URI.
    let diverting_uri = extract_uri(value).unwrap_or_default();

    // Parse parameters.
    let mut reason = RedirectingReason::Unknown;
    let mut reason_str = "unknown".to_string();
    let mut privacy = None;
    let mut counter = None;

    // Find parameters after '>' (or after the URI if no angle brackets).
    let params_start = value.find('>').map(|i| i + 1).unwrap_or(0);
    let params_section = &value[params_start..];

    for param in params_section.split(';') {
        let param = param.trim();
        if let Some((key, val)) = param.split_once('=') {
            let key = key.trim().to_lowercase();
            let val = val.trim().trim_matches('"');
            match key.as_str() {
                "reason" => {
                    reason_str = val.to_string();
                    reason = RedirectingReason::from_sip_reason(val);
                }
                "privacy" => {
                    privacy = Some(val.to_string());
                }
                "counter" => {
                    counter = val.parse().ok();
                }
                _ => {}
            }
        }
    }

    Some(DiversionInfo {
        diverting_uri,
        diverting_name,
        reason,
        reason_str,
        privacy,
        counter,
    })
}

/// Extract Diversion information from a SIP message.
pub fn get_diversion(msg: &SipMessage) -> Option<DiversionInfo> {
    msg.get_header("Diversion")
        .and_then(parse_diversion_header)
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Build a Diversion header value.
pub fn build_diversion_header(
    uri: &str,
    display_name: Option<&str>,
    reason: RedirectingReason,
    counter: Option<u32>,
    privacy: Option<&str>,
) -> String {
    let mut value = String::new();

    if let Some(name) = display_name {
        value.push_str(&format!("\"{}\" ", name));
    }

    value.push_str(&format!("<{}>", uri));
    value.push_str(&format!(";reason={}", reason.to_sip_reason()));

    if let Some(c) = counter {
        value.push_str(&format!(";counter={}", c));
    }

    if let Some(p) = privacy {
        value.push_str(&format!(";privacy={}", p));
    }

    value
}

/// Add a Diversion header to a SIP message.
pub fn add_diversion_header(
    msg: &mut SipMessage,
    uri: &str,
    display_name: Option<&str>,
    reason: RedirectingReason,
    counter: Option<u32>,
) {
    let value = build_diversion_header(uri, display_name, reason, counter, None);
    msg.headers.push(SipHeader {
        name: "Diversion".to_string(),
        value,
    });
}

// ---------------------------------------------------------------------------
// History-Info (RFC 7044) support
// ---------------------------------------------------------------------------

/// Parse a History-Info header value.
///
/// Format: `<sip:user@host>;index=1;cause=302`
pub fn parse_history_info(value: &str) -> Option<DiversionInfo> {
    let uri = extract_uri(value).unwrap_or_default();
    let mut reason = RedirectingReason::Unknown;
    let mut reason_str = "unknown".to_string();
    let mut counter = None;

    for param in value.split(';') {
        let param = param.trim();
        if let Some((key, val)) = param.split_once('=') {
            let key = key.trim().to_lowercase();
            let val = val.trim().trim_matches('"');
            match key.as_str() {
                "cause" => {
                    if let Ok(cause) = val.parse::<u16>() {
                        reason = RedirectingReason::from_cause_code(cause);
                        reason_str = reason.to_sip_reason().to_string();
                    }
                }
                "index" => {
                    counter = val.parse().ok();
                }
                _ => {}
            }
        }
    }

    Some(DiversionInfo {
        diverting_uri: uri,
        diverting_name: None,
        reason,
        reason_str,
        privacy: None,
        counter,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diversion() {
        let value =
            "\"Alice\" <sip:alice@example.com>;reason=no-answer;counter=1;privacy=off";
        let info = parse_diversion_header(value).unwrap();
        assert_eq!(info.diverting_uri, "sip:alice@example.com");
        assert_eq!(info.diverting_name, Some("Alice".to_string()));
        assert_eq!(info.reason, RedirectingReason::NoAnswer);
        assert_eq!(info.counter, Some(1));
        assert_eq!(info.privacy, Some("off".to_string()));
    }

    #[test]
    fn test_parse_diversion_no_name() {
        let value = "<sip:bob@example.com>;reason=user-busy";
        let info = parse_diversion_header(value).unwrap();
        assert_eq!(info.diverting_uri, "sip:bob@example.com");
        assert_eq!(info.diverting_name, None);
        assert_eq!(info.reason, RedirectingReason::UserBusy);
    }

    #[test]
    fn test_build_diversion() {
        let value = build_diversion_header(
            "sip:alice@example.com",
            Some("Alice"),
            RedirectingReason::Unconditional,
            Some(1),
            None,
        );
        assert!(value.contains("sip:alice@example.com"));
        assert!(value.contains("reason=unconditional"));
        assert!(value.contains("counter=1"));
        assert!(value.contains("\"Alice\""));
    }

    #[test]
    fn test_reason_roundtrip() {
        for reason in &[
            RedirectingReason::UserBusy,
            RedirectingReason::NoAnswer,
            RedirectingReason::Unconditional,
            RedirectingReason::Unavailable,
        ] {
            let sip = reason.to_sip_reason();
            let back = RedirectingReason::from_sip_reason(sip);
            assert_eq!(*reason, back);
        }
    }

    #[test]
    fn test_cause_code_mapping() {
        assert_eq!(RedirectingReason::UserBusy.to_cause_code(), 486);
        assert_eq!(RedirectingReason::from_cause_code(486), RedirectingReason::UserBusy);
        assert_eq!(RedirectingReason::from_cause_code(302), RedirectingReason::Unconditional);
    }
}
