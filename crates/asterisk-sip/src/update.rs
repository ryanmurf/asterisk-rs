//! UPDATE method (RFC 3311) -- Mid-dialog updates without re-INVITE.
//!
//! The UPDATE method allows a UAC to update parameters of a session
//! (such as codec set, hold state, or session timers) without the
//! devices of a re-INVITE. UPDATE uses the existing dialog route set
//! and doesn't create a new transaction in the same way re-INVITE does.
//!
//! Use cases:
//! - Session timer refresh
//! - Hold/resume
//! - Codec renegotiation
//! - Early media parameter updates (before final response)

use crate::dialog::Dialog;
use crate::parser::{SipMessage, SipMethod, StartLine, RequestLine, SipUri, SipHeader};
use crate::sdp::SessionDescription;

/// Build an UPDATE request within an existing dialog.
///
/// The UPDATE request uses the dialog's route set and remote target,
/// just like a re-INVITE would, but is simpler because it does not
/// affect the dialog state machine.
///
/// `sdp` is optional -- an UPDATE may or may not carry SDP. When it
/// does carry SDP, it is an offer that expects a 200 OK with an SDP
/// answer.
pub fn build_update(
    dialog: &Dialog,
    cseq: u32,
    sdp: Option<&SessionDescription>,
) -> SipMessage {
    let request_uri = if dialog.remote_target.is_empty() {
        &dialog.remote_uri
    } else {
        &dialog.remote_target
    };

    let mut msg = SipMessage::new_request(SipMethod::Update, request_uri);

    // From/To with tags.
    if dialog.is_uac {
        msg.add_header(
            "From",
            &format!("<{}>;tag={}", dialog.local_uri, dialog.local_tag),
        );
        msg.add_header(
            "To",
            &format!("<{}>;tag={}", dialog.remote_uri, dialog.remote_tag),
        );
    } else {
        msg.add_header(
            "From",
            &format!("<{}>;tag={}", dialog.local_uri, dialog.local_tag),
        );
        msg.add_header(
            "To",
            &format!("<{}>;tag={}", dialog.remote_uri, dialog.remote_tag),
        );
    }

    msg.add_header("Call-ID", &dialog.call_id);
    msg.add_header("CSeq", &format!("{} UPDATE", cseq));
    msg.add_header("Max-Forwards", "70");

    // Route set.
    for route in &dialog.route_set {
        msg.add_header("Route", route);
    }

    // SDP body.
    if let Some(sdp) = sdp {
        let sdp_str = sdp.to_string();
        msg.add_header("Content-Type", "application/sdp");
        msg.add_header("Content-Length", &sdp_str.len().to_string());
        msg.body = sdp_str;
    } else {
        msg.add_header("Content-Length", "0");
    }

    msg
}

/// Build a 200 OK response to an UPDATE request.
///
/// If the UPDATE contained an SDP offer, the response should include
/// an SDP answer.
pub fn build_update_response(
    request: &SipMessage,
    sdp_answer: Option<&SessionDescription>,
) -> SipMessage {
    let mut response = SipMessage::new_response(200, "OK");

    // Copy headers from request.
    if let Some(via) = request.get_header("Via") {
        response.add_header("Via", via);
    }
    if let Some(from) = request.get_header("From") {
        response.add_header("From", from);
    }
    if let Some(to) = request.get_header("To") {
        response.add_header("To", to);
    }
    if let Some(call_id) = request.get_header("Call-ID") {
        response.add_header("Call-ID", call_id);
    }
    if let Some(cseq) = request.get_header("CSeq") {
        response.add_header("CSeq", cseq);
    }

    // SDP answer.
    if let Some(sdp) = sdp_answer {
        let sdp_str = sdp.to_string();
        response.add_header("Content-Type", "application/sdp");
        response.add_header("Content-Length", &sdp_str.len().to_string());
        response.body = sdp_str;
    } else {
        response.add_header("Content-Length", "0");
    }

    response
}

/// Check whether a SIP message supports the UPDATE method.
///
/// Looks for `UPDATE` in the `Allow` header.
pub fn supports_update(msg: &SipMessage) -> bool {
    msg.get_headers("Allow")
        .into_iter()
        .any(|val| {
            val.split(',')
                .any(|method| method.trim().eq_ignore_ascii_case("UPDATE"))
        })
}

/// Parse SDP from an UPDATE request or its 200 OK response.
pub fn parse_update_sdp(msg: &SipMessage) -> Option<SessionDescription> {
    if msg.body.is_empty() {
        return None;
    }

    // Check Content-Type.
    if let Some(ct) = msg.get_header("Content-Type") {
        if !ct.to_lowercase().contains("application/sdp") {
            return None;
        }
    }

    SessionDescription::parse(&msg.body).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Dialog, DialogState};
    use crate::sdp::SessionDescription;
    use asterisk_codecs::Codec;

    fn test_dialog() -> Dialog {
        Dialog {
            call_id: "test-call-id@example.com".to_string(),
            local_tag: "local-tag-1".to_string(),
            remote_tag: "remote-tag-2".to_string(),
            local_seq: 1,
            remote_seq: Some(1),
            local_uri: "sip:alice@example.com".to_string(),
            remote_uri: "sip:bob@example.com".to_string(),
            remote_target: "sip:bob@10.0.0.1:5060".to_string(),
            route_set: Vec::new(),
            state: DialogState::Confirmed,
            is_uac: true,
        }
    }

    #[test]
    fn test_build_update_no_sdp() {
        let dialog = test_dialog();
        let msg = build_update(&dialog, 2, None);

        assert_eq!(msg.get_header("CSeq"), Some("2 UPDATE"));
        assert_eq!(
            msg.get_header("Call-ID"),
            Some("test-call-id@example.com")
        );
        assert_eq!(msg.get_header("Content-Length"), Some("0"));
        assert!(msg.body.is_empty());
    }

    #[test]
    fn test_build_update_with_sdp() {
        let dialog = test_dialog();
        let codecs = vec![Codec::new("PCMU", 0, 8000)];
        let sdp = SessionDescription::create_offer("10.0.0.1", 20000, &codecs);

        let msg = build_update(&dialog, 3, Some(&sdp));

        assert_eq!(msg.get_header("CSeq"), Some("3 UPDATE"));
        assert_eq!(
            msg.get_header("Content-Type"),
            Some("application/sdp")
        );
        assert!(!msg.body.is_empty());
        assert!(msg.body.contains("m=audio"));
    }

    #[test]
    fn test_build_update_response() {
        let mut request = SipMessage::new_request(SipMethod::Update, "sip:bob@10.0.0.1");
        request.add_header("Via", "SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK-test");
        request.add_header("From", "<sip:alice@example.com>;tag=abc");
        request.add_header("To", "<sip:bob@example.com>;tag=def");
        request.add_header("Call-ID", "test-call@example.com");
        request.add_header("CSeq", "2 UPDATE");

        let response = build_update_response(&request, None);

        assert_eq!(response.status_code(), Some(200));
        assert_eq!(
            response.get_header("Call-ID"),
            Some("test-call@example.com")
        );
        assert_eq!(response.get_header("CSeq"), Some("2 UPDATE"));
    }

    #[test]
    fn test_build_update_response_with_sdp_answer() {
        let mut request = SipMessage::new_request(SipMethod::Update, "sip:bob@10.0.0.1");
        request.add_header("Via", "SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bK-test");
        request.add_header("From", "<sip:alice@example.com>;tag=abc");
        request.add_header("To", "<sip:bob@example.com>;tag=def");
        request.add_header("Call-ID", "test-call@example.com");
        request.add_header("CSeq", "2 UPDATE");

        let codecs = vec![Codec::new("PCMU", 0, 8000)];
        let sdp = SessionDescription::create_offer("10.0.0.2", 30000, &codecs);

        let response = build_update_response(&request, Some(&sdp));

        assert_eq!(
            response.get_header("Content-Type"),
            Some("application/sdp")
        );
        assert!(!response.body.is_empty());
    }

    #[test]
    fn test_supports_update() {
        let mut msg = SipMessage::new_response(200, "OK");
        msg.add_header("Allow", "INVITE, ACK, BYE, CANCEL, UPDATE, OPTIONS");
        assert!(supports_update(&msg));
    }

    #[test]
    fn test_supports_update_not_present() {
        let mut msg = SipMessage::new_response(200, "OK");
        msg.add_header("Allow", "INVITE, ACK, BYE, CANCEL, OPTIONS");
        assert!(!supports_update(&msg));
    }

    #[test]
    fn test_parse_update_sdp() {
        let mut msg = SipMessage::new_request(SipMethod::Update, "sip:bob@10.0.0.1");
        msg.add_header("Content-Type", "application/sdp");

        let sdp_text = "v=0\r\n\
            o=- 1 1 IN IP4 10.0.0.1\r\n\
            s=Test\r\n\
            c=IN IP4 10.0.0.1\r\n\
            t=0 0\r\n\
            m=audio 20000 RTP/AVP 0\r\n\
            a=rtpmap:0 PCMU/8000\r\n";

        msg.body = sdp_text.to_string();

        let sdp = parse_update_sdp(&msg).unwrap();
        assert_eq!(sdp.media_descriptions.len(), 1);
        assert_eq!(sdp.media_descriptions[0].port, 20000);
    }

    #[test]
    fn test_parse_update_sdp_empty_body() {
        let msg = SipMessage::new_request(SipMethod::Update, "sip:bob@10.0.0.1");
        assert!(parse_update_sdp(&msg).is_none());
    }

    #[test]
    fn test_update_uses_remote_target() {
        let dialog = test_dialog();
        let msg = build_update(&dialog, 2, None);

        // Should use remote_target as request URI, not remote_uri.
        match &msg.start_line {
            StartLine::Request(rl) => {
                assert_eq!(rl.uri.to_string(), "sip:bob@10.0.0.1:5060");
            }
            _ => panic!("Expected request start line"),
        }
    }

    #[test]
    fn test_update_empty_remote_target_uses_remote_uri() {
        let mut dialog = test_dialog();
        dialog.remote_target = String::new();

        let msg = build_update(&dialog, 2, None);

        match &msg.start_line {
            StartLine::Request(rl) => {
                assert_eq!(rl.uri.to_string(), "sip:bob@example.com");
            }
            _ => panic!("Expected request start line"),
        }
    }
}
