//! Handle empty SIP INFO requests.
//!
//! Port of `res/res_pjsip_empty_info.c`. Responds with 200 OK to SIP
//! INFO requests with no body/content-type. Some SBCs send empty INFO
//! as keepalives.

use tracing::debug;

// ---------------------------------------------------------------------------
// Empty INFO detection and handling
// ---------------------------------------------------------------------------

/// Check if a SIP INFO request has an empty body (no Content-Type).
///
/// Returns true if this is an empty INFO that should be auto-answered
/// with 200 OK.
pub fn is_empty_info(content_type: Option<&str>, body: &[u8]) -> bool {
    content_type.is_none() && body.is_empty()
}

/// Result of processing a SIP INFO request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoHandleResult {
    /// This module handled the request (empty INFO, responded 200 OK).
    Handled,
    /// This module did not handle the request (pass to next handler).
    NotHandled,
}

/// Process a SIP INFO request.
///
/// If the INFO has no content type, respond with 200 OK (handled by SBC
/// keepalive convention). Otherwise, let another module handle it.
pub fn handle_info(
    content_type: Option<&str>,
    body: &[u8],
) -> InfoHandleResult {
    if is_empty_info(content_type, body) {
        debug!("Handling empty SIP INFO (SBC keepalive)");
        InfoHandleResult::Handled
    } else {
        InfoHandleResult::NotHandled
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_info_detected() {
        assert!(is_empty_info(None, &[]));
        assert_eq!(handle_info(None, &[]), InfoHandleResult::Handled);
    }

    #[test]
    fn test_non_empty_info() {
        assert!(!is_empty_info(Some("application/dtmf"), &[]));
        assert_eq!(
            handle_info(Some("application/dtmf-relay"), b"Signal=1\r\n"),
            InfoHandleResult::NotHandled,
        );
    }

    #[test]
    fn test_body_present() {
        assert!(!is_empty_info(None, b"some body"));
        assert_eq!(
            handle_info(None, b"some body"),
            InfoHandleResult::NotHandled,
        );
    }
}
