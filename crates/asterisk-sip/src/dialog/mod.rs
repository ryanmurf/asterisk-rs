//! SIP dialog management (RFC 3261 Section 12).
//!
//! A dialog represents a peer-to-peer SIP relationship between two UAs
//! that persists for some time. Dialogs are identified by Call-ID,
//! local tag, and remote tag.

use crate::parser::{extract_tag, extract_uri, SipMessage, header_names};

/// Dialog state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogState {
    /// Dialog created from provisional response (1xx with To tag).
    Early,
    /// Dialog confirmed by 2xx response.
    Confirmed,
    /// Dialog terminated by BYE or error.
    Terminated,
}

/// A SIP dialog.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// Call-ID that identifies this dialog.
    pub call_id: String,
    /// Local tag (from From header for UAC, from To header for UAS).
    pub local_tag: String,
    /// Remote tag.
    pub remote_tag: String,
    /// Local CSeq number.
    pub local_seq: u32,
    /// Remote CSeq number.
    pub remote_seq: Option<u32>,
    /// Local URI (our contact).
    pub local_uri: String,
    /// Remote URI (their contact).
    pub remote_uri: String,
    /// Remote target (from Contact header).
    pub remote_target: String,
    /// Route set (from Record-Route headers).
    pub route_set: Vec<String>,
    /// Current dialog state.
    pub state: DialogState,
    /// Whether we are the UAC (caller) side.
    pub is_uac: bool,
}

impl Dialog {
    /// Create a dialog from a received response to an INVITE (UAC side).
    ///
    /// Per RFC 3261 Section 12.1.2, a dialog is created from a 1xx or 2xx
    /// response that contains a To tag.
    pub fn from_uac_response(
        request: &SipMessage,
        response: &SipMessage,
    ) -> Option<Self> {
        let call_id = request.call_id()?.to_string();

        let from_hdr = request.from_header()?;
        let local_tag = extract_tag(from_hdr)?;

        let to_hdr = response.to_header()?;
        let remote_tag = extract_tag(to_hdr)?;

        // Remote target from Contact header in response
        let remote_target = response
            .get_header(header_names::CONTACT)
            .and_then(extract_uri)
            .unwrap_or_default();

        // Local URI from Contact header in request
        let local_uri = request
            .get_header(header_names::CONTACT)
            .and_then(extract_uri)
            .unwrap_or_default();

        // Remote URI from To header
        let remote_uri = extract_uri(to_hdr).unwrap_or_default();

        // Route set from Record-Route headers (in reverse order for UAC)
        let mut route_set: Vec<String> = response
            .get_headers(header_names::RECORD_ROUTE)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        route_set.reverse();

        // Local CSeq from request
        let local_seq = request
            .cseq()
            .and_then(|cs| cs.split_whitespace().next())
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(1);

        let status_code = response.status_code().unwrap_or(0);
        let state = if (200..300).contains(&status_code) {
            DialogState::Confirmed
        } else if (100..200).contains(&status_code) {
            DialogState::Early
        } else {
            return None; // No dialog for error responses
        };

        Some(Dialog {
            call_id,
            local_tag,
            remote_tag,
            local_seq,
            remote_seq: None,
            local_uri,
            remote_uri,
            remote_target,
            route_set,
            state,
            is_uac: true,
        })
    }

    /// Create a dialog from a received INVITE (UAS side).
    pub fn from_uas_request(request: &SipMessage, local_tag: &str) -> Option<Self> {
        let call_id = request.call_id()?.to_string();

        let from_hdr = request.from_header()?;
        let remote_tag = extract_tag(from_hdr).unwrap_or_default();

        let remote_uri = extract_uri(from_hdr).unwrap_or_default();
        let remote_target = request
            .get_header(header_names::CONTACT)
            .and_then(extract_uri)
            .unwrap_or_default();

        // Route set from Record-Route headers (in order for UAS)
        let route_set: Vec<String> = request
            .get_headers(header_names::RECORD_ROUTE)
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let remote_seq = request
            .cseq()
            .and_then(|cs| cs.split_whitespace().next())
            .and_then(|n| n.parse::<u32>().ok());

        Some(Dialog {
            call_id,
            local_tag: local_tag.to_string(),
            remote_tag,
            local_seq: 0,
            remote_seq,
            local_uri: String::new(), // Set when we send our response with Contact
            remote_uri,
            remote_target,
            route_set,
            state: DialogState::Early,
            is_uac: false,
        })
    }

    /// Get the next local CSeq number.
    pub fn next_cseq(&mut self) -> u32 {
        self.local_seq += 1;
        self.local_seq
    }

    /// Check if a request belongs to this dialog.
    pub fn matches(&self, call_id: &str, local_tag: &str, remote_tag: &str) -> bool {
        self.call_id == call_id && self.local_tag == local_tag && self.remote_tag == remote_tag
    }

    /// Confirm an early dialog (after receiving 2xx).
    pub fn confirm(&mut self) {
        self.state = DialogState::Confirmed;
    }

    /// Terminate the dialog.
    pub fn terminate(&mut self) {
        self.state = DialogState::Terminated;
    }

    /// Check if the dialog is confirmed.
    pub fn is_confirmed(&self) -> bool {
        self.state == DialogState::Confirmed
    }

    /// Update remote target from Contact header in a request/response.
    pub fn update_remote_target(&mut self, contact_uri: &str) {
        self.remote_target = contact_uri.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SipMessage;

    #[test]
    fn test_dialog_from_invite_response() {
        let req = SipMessage::parse(
            b"INVITE sip:bob@example.com SIP/2.0\r\n\
Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
From: Alice <sip:alice@example.com>;tag=fromtag\r\n\
To: Bob <sip:bob@example.com>\r\n\
Call-ID: dialog-test-123\r\n\
CSeq: 1 INVITE\r\n\
Contact: <sip:alice@10.0.0.1>\r\n\
Content-Length: 0\r\n\
\r\n",
        )
        .unwrap();

        let resp = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
From: Alice <sip:alice@example.com>;tag=fromtag\r\n\
To: Bob <sip:bob@example.com>;tag=totag\r\n\
Call-ID: dialog-test-123\r\n\
CSeq: 1 INVITE\r\n\
Contact: <sip:bob@10.0.0.2>\r\n\
Content-Length: 0\r\n\
\r\n",
        )
        .unwrap();

        let dialog = Dialog::from_uac_response(&req, &resp).unwrap();
        assert_eq!(dialog.call_id, "dialog-test-123");
        assert_eq!(dialog.local_tag, "fromtag");
        assert_eq!(dialog.remote_tag, "totag");
        assert_eq!(dialog.state, DialogState::Confirmed);
        assert_eq!(dialog.remote_target, "sip:bob@10.0.0.2");
    }
}
