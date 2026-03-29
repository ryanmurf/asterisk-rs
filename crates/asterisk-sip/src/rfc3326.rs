//! SIP Reason header (RFC 3326) for hangup cause mapping.
//!
//! Port of `res/res_pjsip_rfc3326.c`. Parses the `Reason` header from
//! BYE/CANCEL requests and responses to extract Q.850 and SIP cause codes,
//! and generates Reason headers on outbound messages.

use tracing::debug;

// ---------------------------------------------------------------------------
// Reason header parsing
// ---------------------------------------------------------------------------

/// Parsed Reason header information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonHeader {
    /// Q.850 cause code (if present).
    pub q850_cause: Option<u32>,
    /// SIP cause code (if present).
    pub sip_cause: Option<u32>,
    /// Reason text (if present).
    pub text: Option<String>,
}

impl ReasonHeader {
    pub fn new() -> Self {
        Self {
            q850_cause: None,
            sip_cause: None,
            text: None,
        }
    }

    /// Get the effective hangup cause code.
    ///
    /// Prefers Q.850 codes over SIP codes (matching C behaviour).
    /// Q.850 codes are masked to 7 bits.
    pub fn effective_cause(&self) -> Option<u32> {
        if let Some(q850) = self.q850_cause {
            Some(q850 & 0x7f)
        } else {
            self.sip_cause.map(|c| sip_to_hangup_cause(c))
        }
    }
}

impl Default for ReasonHeader {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse one or more `Reason` header values.
///
/// Multiple Reason headers may be present (Q.850 and SIP protocol).
/// Format: `Q.850;cause=16;text="Normal call clearing"`
///         `SIP;cause=200;text="Call completed elsewhere"`
pub fn parse_reason_headers(headers: &[&str]) -> ReasonHeader {
    let mut result = ReasonHeader::new();

    for header_value in headers {
        let value = header_value.trim();

        let is_q850 = value.len() >= 5
            && value[..5].eq_ignore_ascii_case("Q.850");
        let is_sip = value.len() >= 3
            && value[..3].eq_ignore_ascii_case("SIP");

        if !is_q850 && !is_sip {
            continue;
        }

        // Extract cause=N
        if let Some(cause_str) = extract_param(value, "cause") {
            if let Ok(code) = cause_str.parse::<u32>() {
                if is_q850 {
                    result.q850_cause = Some(code);
                } else {
                    result.sip_cause = Some(code);
                }
                debug!(protocol = if is_q850 { "Q.850" } else { "SIP" }, cause = code, "Parsed Reason header");
            }
        }

        // Extract text="..."
        if let Some(text) = extract_param(value, "text") {
            let text = text.trim_matches('"');
            result.text = Some(text.to_string());
        }
    }

    result
}

/// Extract a parameter value from a Reason header.
fn extract_param<'a>(header: &'a str, param_name: &str) -> Option<&'a str> {
    let search = format!("{}=", param_name);
    let pos = header.to_lowercase().find(&search)?;
    let start = pos + search.len();
    let rest = &header[start..];

    // Find end of value (semicolon or end of string)
    let end = rest
        .find(';')
        .unwrap_or(rest.len());

    // Handle quoted values
    if rest.starts_with('"') {
        let close = rest[1..].find('"').map(|p| p + 2).unwrap_or(end);
        Some(&rest[..close])
    } else {
        Some(rest[..end].trim())
    }
}

// ---------------------------------------------------------------------------
// Reason header generation
// ---------------------------------------------------------------------------

/// Generate a Reason header value for outgoing BYE/CANCEL.
///
/// Mirrors `rfc3326_add_reason_header()` from the C source.
pub fn generate_reason_header(hangup_cause: u32) -> Option<String> {
    match hangup_cause {
        // "Answered elsewhere" gets a SIP Reason
        26 => Some(
            "SIP;cause=200;text=\"Call completed elsewhere\"".to_string(),
        ),
        // Normal causes get a Q.850 Reason
        cause if cause > 0 => {
            let text = q850_cause_text(cause);
            Some(format!("Q.850;cause={};text=\"{}\"", cause, text))
        }
        _ => None,
    }
}

/// Map a SIP response code to a hangup cause code.
///
/// Simplified mapping of common SIP response codes to Q.850 causes.
pub fn sip_to_hangup_cause(sip_code: u32) -> u32 {
    match sip_code {
        200 => 16, // Normal clearing
        401 | 407 => 21, // Call rejected (auth failure)
        403 => 21, // Call rejected
        404 => 1,  // Unallocated number
        408 => 19, // No answer from user
        480 => 20, // Subscriber absent
        486 => 17, // User busy
        487 => 16, // Normal clearing (request terminated)
        488 => 88, // Incompatible destination
        500 => 38, // Network out of order
        503 => 34, // No circuit available
        _ => 16,   // Normal clearing (default)
    }
}

/// Get a human-readable description for a Q.850 cause code.
fn q850_cause_text(cause: u32) -> &'static str {
    match cause {
        1 => "Unallocated number",
        16 => "Normal call clearing",
        17 => "User busy",
        18 => "No user responding",
        19 => "No answer from user",
        20 => "Subscriber absent",
        21 => "Call rejected",
        26 => "Non-selected user clearing",
        27 => "Destination out of order",
        34 => "No circuit available",
        38 => "Network out of order",
        88 => "Incompatible destination",
        _ => "Unknown cause",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_q850() {
        let headers = vec!["Q.850;cause=16;text=\"Normal call clearing\""];
        let result = parse_reason_headers(&headers);
        assert_eq!(result.q850_cause, Some(16));
        assert_eq!(result.effective_cause(), Some(16));
    }

    #[test]
    fn test_parse_sip() {
        let headers = vec!["SIP;cause=200;text=\"Call completed elsewhere\""];
        let result = parse_reason_headers(&headers);
        assert_eq!(result.sip_cause, Some(200));
        assert_eq!(result.effective_cause(), Some(16)); // 200 -> 16
    }

    #[test]
    fn test_parse_both() {
        let headers = vec![
            "Q.850;cause=17",
            "SIP;cause=486",
        ];
        let result = parse_reason_headers(&headers);
        assert_eq!(result.q850_cause, Some(17));
        assert_eq!(result.sip_cause, Some(486));
        // Q.850 takes precedence
        assert_eq!(result.effective_cause(), Some(17));
    }

    #[test]
    fn test_parse_empty() {
        let result = parse_reason_headers(&[]);
        assert_eq!(result.effective_cause(), None);
    }

    #[test]
    fn test_generate_reason_normal() {
        let header = generate_reason_header(16).unwrap();
        assert!(header.contains("Q.850"));
        assert!(header.contains("cause=16"));
    }

    #[test]
    fn test_generate_reason_answered_elsewhere() {
        let header = generate_reason_header(26).unwrap();
        assert!(header.contains("SIP"));
        assert!(header.contains("cause=200"));
    }

    #[test]
    fn test_generate_reason_zero() {
        assert!(generate_reason_header(0).is_none());
    }

    #[test]
    fn test_sip_to_hangup_cause() {
        assert_eq!(sip_to_hangup_cause(486), 17); // Busy
        assert_eq!(sip_to_hangup_cause(404), 1);  // Unallocated
        assert_eq!(sip_to_hangup_cause(408), 19); // No answer
    }
}
