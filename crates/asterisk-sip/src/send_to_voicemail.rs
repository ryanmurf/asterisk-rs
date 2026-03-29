//! Redirect to voicemail via SIP.
//!
//! Port of `res/res_pjsip_send_to_voicemail.c`. Detects SIP REFER
//! requests with a Diversion header containing `reason=send_to_vm`
//! and sets the appropriate channel variables and redirecting information
//! so the call is directed to voicemail.

use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Custom SIP header used by Digium phones for call features.
pub const SEND_TO_VM_HEADER: &str = "X-Digium-Call-Feature";

/// Header value indicating send-to-voicemail.
pub const SEND_TO_VM_HEADER_VALUE: &str = "feature_send_to_vm";

/// Redirecting reason value for send-to-voicemail.
pub const SEND_TO_VM_REASON: &str = "send_to_vm";

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Check whether a SIP REFER contains a send-to-voicemail indication.
///
/// Looks for a Diversion header with `reason=send_to_vm` or the
/// custom `X-Digium-Call-Feature: feature_send_to_vm` header.
pub fn is_send_to_voicemail(headers: &[(String, String)]) -> bool {
    for (name, value) in headers {
        let lower_name = name.to_lowercase();

        // Check for custom feature header
        if lower_name == "x-digium-call-feature"
            && value.trim().eq_ignore_ascii_case(SEND_TO_VM_HEADER_VALUE)
        {
            debug!("Send-to-voicemail detected via X-Digium-Call-Feature header");
            return true;
        }

        // Check Diversion header for reason=send_to_vm
        if lower_name == "diversion" && contains_send_to_vm_reason(value) {
            debug!("Send-to-voicemail detected via Diversion header");
            return true;
        }
    }
    false
}

/// Check whether a Diversion header value contains reason=send_to_vm.
fn contains_send_to_vm_reason(diversion_value: &str) -> bool {
    // Parse parameters from the Diversion header value
    for param in diversion_value.split(';') {
        let param = param.trim();
        if let Some(value) = param.strip_prefix("reason=") {
            let reason = value.trim().trim_matches('"');
            if reason.eq_ignore_ascii_case(SEND_TO_VM_REASON) {
                return true;
            }
        }
    }
    false
}

/// Result of processing a send-to-voicemail request.
#[derive(Debug, Clone)]
pub struct SendToVmResult {
    /// Whether send-to-voicemail was detected.
    pub detected: bool,
    /// The original redirecting number (from Diversion header).
    pub redirecting_number: Option<String>,
    /// The voicemail context to redirect to.
    pub vm_context: Option<String>,
}

impl SendToVmResult {
    pub fn not_detected() -> Self {
        Self {
            detected: false,
            redirecting_number: None,
            vm_context: None,
        }
    }

    pub fn detected(redirecting_number: Option<String>) -> Self {
        Self {
            detected: true,
            redirecting_number,
            vm_context: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_via_custom_header() {
        let headers = vec![(
            "X-Digium-Call-Feature".to_string(),
            "feature_send_to_vm".to_string(),
        )];
        assert!(is_send_to_voicemail(&headers));
    }

    #[test]
    fn test_detect_via_diversion() {
        let headers = vec![(
            "Diversion".to_string(),
            "<sip:1001@example.com>;reason=send_to_vm".to_string(),
        )];
        assert!(is_send_to_voicemail(&headers));
    }

    #[test]
    fn test_detect_via_diversion_quoted() {
        let headers = vec![(
            "Diversion".to_string(),
            "<sip:1001@example.com>;reason=\"send_to_vm\"".to_string(),
        )];
        assert!(is_send_to_voicemail(&headers));
    }

    #[test]
    fn test_no_detection() {
        let headers = vec![(
            "Diversion".to_string(),
            "<sip:1001@example.com>;reason=no-answer".to_string(),
        )];
        assert!(!is_send_to_voicemail(&headers));
    }

    #[test]
    fn test_empty_headers() {
        assert!(!is_send_to_voicemail(&[]));
    }

    #[test]
    fn test_result_types() {
        let r = SendToVmResult::not_detected();
        assert!(!r.detected);

        let r = SendToVmResult::detected(Some("1001".to_string()));
        assert!(r.detected);
        assert_eq!(r.redirecting_number.as_deref(), Some("1001"));
    }
}
