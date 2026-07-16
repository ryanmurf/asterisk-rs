//! SIP dialog management (RFC 3261 Section 12).
//!
//! A dialog represents a peer-to-peer SIP relationship between two UAs
//! that persists for some time. Dialogs are identified by Call-ID,
//! local tag, and remote tag.

use crate::parser::{extract_tag, extract_uri, SipMessage, SipMethod, header_names};

/// Why a message was rejected as not belonging to an established dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogValidationError {
    CallId,
    LocalTag,
    RemoteTag,
    RouteSet,
    CSeq,
}

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

    /// Validate a request received from the remote dialog participant before
    /// the transaction user mutates call state.
    ///
    /// Remote requests carry our tag in `To` and the peer's tag in `From`.
    /// ACK reuses the CSeq of the INVITE it acknowledges; every other
    /// in-dialog request must advance the peer's sequence number. A Route
    /// header is optional by the time the request reaches the endpoint (the
    /// last proxy may consume it), but a supplied route set may not conflict
    /// with the one established by Record-Route.
    pub fn validate_remote_request(
        &mut self,
        request: &SipMessage,
    ) -> Result<(), DialogValidationError> {
        if request.call_id() != Some(self.call_id.as_str()) {
            return Err(DialogValidationError::CallId);
        }
        if request.to_header().and_then(extract_tag).as_deref()
            != Some(self.local_tag.as_str())
        {
            return Err(DialogValidationError::LocalTag);
        }
        if request.from_header().and_then(extract_tag).as_deref()
            != Some(self.remote_tag.as_str())
        {
            return Err(DialogValidationError::RemoteTag);
        }

        let routes = normalized_headers(request, header_names::ROUTE);
        if !routes.is_empty() && routes != normalized_route_set(&self.route_set) {
            return Err(DialogValidationError::RouteSet);
        }

        let cseq = cseq_number(request).ok_or(DialogValidationError::CSeq)?;
        match request.method() {
            Some(SipMethod::Ack) if self.remote_seq == Some(cseq) => {}
            Some(SipMethod::Ack) => return Err(DialogValidationError::CSeq),
            Some(_) if self.remote_seq.is_none_or(|previous| cseq > previous) => {
                self.remote_seq = Some(cseq);
            }
            _ => return Err(DialogValidationError::CSeq),
        }
        Ok(())
    }

    /// Validate a response received for a locally generated in-dialog
    /// request. Record-Route arrives in wire order, while a UAC stores its
    /// route set in reverse order.
    pub fn validate_remote_response(
        &self,
        response: &SipMessage,
        expected_cseq: u32,
    ) -> Result<(), DialogValidationError> {
        if response.call_id() != Some(self.call_id.as_str()) {
            return Err(DialogValidationError::CallId);
        }
        if response.from_header().and_then(extract_tag).as_deref()
            != Some(self.local_tag.as_str())
        {
            return Err(DialogValidationError::LocalTag);
        }
        if response.to_header().and_then(extract_tag).as_deref()
            != Some(self.remote_tag.as_str())
        {
            return Err(DialogValidationError::RemoteTag);
        }
        if cseq_number(response) != Some(expected_cseq) {
            return Err(DialogValidationError::CSeq);
        }

        let mut record_routes = normalized_headers(response, header_names::RECORD_ROUTE);
        if !record_routes.is_empty() {
            if self.is_uac {
                record_routes.reverse();
            }
            if record_routes != normalized_route_set(&self.route_set) {
                return Err(DialogValidationError::RouteSet);
            }
        }
        Ok(())
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

fn cseq_number(message: &SipMessage) -> Option<u32> {
    message.cseq()?.split_whitespace().next()?.parse().ok()
}

fn normalized_headers(message: &SipMessage, name: &str) -> Vec<String> {
    message
        .get_headers(name)
        .into_iter()
        .map(normalize_route)
        .collect()
}

fn normalized_route_set(routes: &[String]) -> Vec<String> {
    routes.iter().map(|route| normalize_route(route)).collect()
}

fn normalize_route(route: &str) -> String {
    route.split_whitespace().collect::<String>().to_ascii_lowercase()
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

    fn inbound_dialog() -> Dialog {
        Dialog {
            call_id: "dialog-1".to_string(),
            local_tag: "local-tag".to_string(),
            remote_tag: "remote-tag".to_string(),
            local_seq: 0,
            remote_seq: Some(10),
            local_uri: "sip:local@example.com".to_string(),
            remote_uri: "sip:remote@example.com".to_string(),
            remote_target: "sip:remote@192.0.2.10".to_string(),
            route_set: vec!["<sip:proxy.example.com;lr>".to_string()],
            state: DialogState::Confirmed,
            is_uac: false,
        }
    }

    fn remote_request(method: &str, cseq: u32) -> SipMessage {
        SipMessage::parse(
            format!(
                "{method} sip:local@example.com SIP/2.0\r\n\
                 Via: SIP/2.0/UDP 192.0.2.10;branch=z9hG4bKremote\r\n\
                 From: <sip:remote@example.com>;tag=remote-tag\r\n\
                 To: <sip:local@example.com>;tag=local-tag\r\n\
                 Call-ID: dialog-1\r\n\
                 CSeq: {cseq} {method}\r\n\
                 Route: <sip:proxy.example.com;lr>\r\n\
                 Content-Length: 0\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn validates_remote_request_identity_route_and_monotonic_cseq() {
        let mut dialog = inbound_dialog();
        dialog.validate_remote_request(&remote_request("BYE", 11)).unwrap();
        assert_eq!(dialog.remote_seq, Some(11));

        let stale = remote_request("UPDATE", 11);
        assert_eq!(
            dialog.validate_remote_request(&stale),
            Err(DialogValidationError::CSeq)
        );

        let mut wrong_tag = remote_request("INVITE", 12);
        wrong_tag
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::FROM))
            .unwrap()
            .value = "<sip:remote@example.com>;tag=forged".to_string();
        assert_eq!(
            dialog.validate_remote_request(&wrong_tag),
            Err(DialogValidationError::RemoteTag)
        );
        assert_eq!(dialog.remote_seq, Some(11));

        let mut wrong_route = remote_request("INVITE", 12);
        wrong_route
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::ROUTE))
            .unwrap()
            .value = "<sip:attacker.example.com;lr>".to_string();
        assert_eq!(
            dialog.validate_remote_request(&wrong_route),
            Err(DialogValidationError::RouteSet)
        );
        assert_eq!(dialog.remote_seq, Some(11));
    }

    #[test]
    fn ack_must_reuse_latest_remote_invite_cseq_without_advancing_it() {
        let mut dialog = inbound_dialog();
        dialog.validate_remote_request(&remote_request("ACK", 10)).unwrap();
        assert_eq!(dialog.remote_seq, Some(10));
        assert_eq!(
            dialog.validate_remote_request(&remote_request("ACK", 11)),
            Err(DialogValidationError::CSeq)
        );
    }

    #[test]
    fn response_must_match_tags_route_set_and_local_cseq() {
        let mut dialog = inbound_dialog();
        dialog.is_uac = true;
        dialog.local_seq = 12;
        let response = SipMessage::parse(
            b"SIP/2.0 200 OK\r\n\
              Via: SIP/2.0/UDP 192.0.2.20;branch=z9hG4bKlocal\r\n\
              From: <sip:local@example.com>;tag=local-tag\r\n\
              To: <sip:remote@example.com>;tag=remote-tag\r\n\
              Call-ID: dialog-1\r\n\
              CSeq: 12 INVITE\r\n\
              Record-Route: <sip:proxy.example.com;lr>\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .unwrap();
        dialog.validate_remote_response(&response, 12).unwrap();

        let mut forged = response.clone();
        forged
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(header_names::RECORD_ROUTE))
            .unwrap()
            .value = "<sip:attacker.example.com;lr>".to_string();
        assert_eq!(
            dialog.validate_remote_response(&forged, 12),
            Err(DialogValidationError::RouteSet)
        );
    }
}
