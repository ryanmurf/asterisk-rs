//! SIP REFER (call transfer) handling (port of res_pjsip_refer.c).
//!
//! Implements blind and attended call transfer using the SIP REFER method
//! (RFC 3515). Generates NOTIFY messages with sipfrag bodies to report
//! transfer progress back to the transferor.

use std::fmt;

use uuid::Uuid;

use crate::parser::{
    extract_uri, header_names, RequestLine, SipHeader, SipMessage, SipMethod, SipUri, StartLine,
    StatusLine,
};

// ---------------------------------------------------------------------------
// Transfer types and structures
// ---------------------------------------------------------------------------

/// The type of transfer being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    /// Blind (unattended) transfer: the original call is ended and a new
    /// call is placed to the Refer-To target.
    Blind,
    /// Attended transfer: the transferor has already established a call
    /// with the transfer target and wants to bridge the two calls.
    Attended,
}

/// Parsed transfer request from an incoming REFER.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    /// The URI to transfer the call to (from Refer-To header).
    pub refer_to: String,
    /// The identity of the party requesting the transfer (Referred-By).
    pub referred_by: Option<String>,
    /// Replaces header embedded in the Refer-To (for attended transfer).
    pub replaces: Option<ReplacesInfo>,
    /// Transfer type inferred from the request.
    pub transfer_type: TransferType,
    /// Call-ID of the REFER request itself.
    pub call_id: String,
}

/// Parsed Replaces parameter from a Refer-To URI.
#[derive(Debug, Clone)]
pub struct ReplacesInfo {
    /// Call-ID of the dialog to be replaced.
    pub call_id: String,
    /// To-tag of the dialog to be replaced.
    pub to_tag: String,
    /// From-tag of the dialog to be replaced.
    pub from_tag: String,
}

/// Transfer progress notification state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferProgress {
    /// Transfer initiated (100 Trying).
    Trying,
    /// Transfer target is ringing (180 Ringing).
    Ringing,
    /// Transfer completed successfully (200 OK).
    Success,
    /// Transfer failed.
    Failed(u16),
}

impl TransferProgress {
    /// SIP response code for this progress state.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Trying => 100,
            Self::Ringing => 180,
            Self::Success => 200,
            Self::Failed(code) => *code,
        }
    }

    /// Reason phrase for this progress state.
    pub fn reason(&self) -> &str {
        match self {
            Self::Trying => "Trying",
            Self::Ringing => "Ringing",
            Self::Success => "OK",
            Self::Failed(_) => "Service Unavailable",
        }
    }

    /// Whether this is a terminal state (for Subscription-State).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed(_))
    }
}

// ---------------------------------------------------------------------------
// REFER request parsing
// ---------------------------------------------------------------------------

/// Parse an incoming SIP REFER request into a `TransferRequest`.
pub fn parse_refer(request: &SipMessage) -> Result<TransferRequest, ReferError> {
    if request.method() != Some(SipMethod::Refer) {
        return Err(ReferError::NotRefer);
    }

    // Refer-To header is mandatory.
    let refer_to_raw = request
        .get_header("Refer-To")
        .or_else(|| request.get_header("r")) // compact form
        .ok_or(ReferError::MissingReferTo)?;

    let refer_to = extract_uri(refer_to_raw).unwrap_or_else(|| refer_to_raw.to_string());

    // Referred-By is optional.
    let referred_by = request
        .get_header("Referred-By")
        .or_else(|| request.get_header("b"))
        .map(|v| extract_uri(v).unwrap_or_else(|| v.to_string()));

    // Check for Replaces parameter in Refer-To URI.
    let replaces = parse_replaces_from_refer_to(refer_to_raw);

    let transfer_type = if replaces.is_some() {
        TransferType::Attended
    } else {
        TransferType::Blind
    };

    let call_id = request.call_id().unwrap_or("").to_string();

    Ok(TransferRequest {
        refer_to,
        referred_by,
        replaces,
        transfer_type,
        call_id,
    })
}

/// Parse Replaces info from a Refer-To header value.
///
/// Example: `<sip:bob@host?Replaces=call-id%3Bto-tag%3Dabc%3Bfrom-tag%3Ddef>`
fn parse_replaces_from_refer_to(header_value: &str) -> Option<ReplacesInfo> {
    // Look for ?Replaces= or &Replaces= in the header value.
    let lower = header_value.to_lowercase();
    let replaces_start = lower.find("replaces=")?;
    let after = &header_value[replaces_start + 9..];

    // The value may be URL-encoded. Trim at '>' or '&' or end.
    let end = after
        .find('>')
        .or_else(|| after.find('&'))
        .unwrap_or(after.len());
    let encoded = &after[..end];

    // URL-decode.
    let decoded = url_decode(encoded);

    // Parse: call-id;to-tag=X;from-tag=Y
    let mut parts = decoded.splitn(2, ';');
    let call_id = parts.next()?.trim().to_string();
    if call_id.is_empty() {
        return None;
    }

    let mut to_tag = String::new();
    let mut from_tag = String::new();

    if let Some(params) = parts.next() {
        for param in params.split(';') {
            let param = param.trim();
            if let Some(v) = param.strip_prefix("to-tag=") {
                to_tag = v.to_string();
            } else if let Some(v) = param.strip_prefix("from-tag=") {
                from_tag = v.to_string();
            }
        }
    }

    Some(ReplacesInfo {
        call_id,
        to_tag,
        from_tag,
    })
}

/// Simple URL decoder (%XX -> char).
fn url_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hi = chars.next().unwrap_or('0');
            let lo = chars.next().unwrap_or('0');
            let byte =
                u8::from_str_radix(&format!("{}{}", hi, lo), 16).unwrap_or(b'?');
            result.push(byte as char);
        } else {
            result.push(c);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// REFER response / NOTIFY generation
// ---------------------------------------------------------------------------

/// Build a 202 Accepted response to a REFER request.
pub fn build_refer_accepted(request: &SipMessage) -> SipMessage {
    

    // Per RFC 3515, we must include a Require: norefersub or rely on
    // implicit subscription.  Most implementations just send 202 with
    // implicit subscription.
    request
        .create_response(202, "Accepted")
        .unwrap_or_else(|_| make_error(request, 500, "Internal Server Error"))
}

/// Build a NOTIFY with sipfrag body to report transfer progress.
///
/// `refer_request` is the original REFER request (used for dialog info).
pub fn build_transfer_notify(
    refer_request: &SipMessage,
    progress: TransferProgress,
    local_tag: &str,
    notify_cseq: u32,
) -> SipMessage {
    let branch = format!(
        "z9hG4bK{}",
        &Uuid::new_v4().to_string().replace('-', "")[..16]
    );

    // Build sipfrag body.
    let sipfrag = format!(
        "SIP/2.0 {} {}",
        progress.status_code(),
        progress.reason()
    );

    let sub_state = if progress.is_terminal() {
        "terminated;reason=noresource".to_string()
    } else {
        "active".to_string()
    };

    // Target URI from the Contact of the REFER sender.
    let remote_contact = refer_request
        .get_header(header_names::CONTACT)
        .and_then(extract_uri)
        .unwrap_or_else(|| "sip:localhost".to_string());

    let target_uri = SipUri::parse(&remote_contact).unwrap_or_else(|_| SipUri {
        scheme: "sip".to_string(),
        user: None,
        password: None,
        host: "localhost".to_string(),
        port: Some(5060),
        parameters: Default::default(),
        headers: Default::default(),
    });

    let from_hdr = refer_request.to_header().unwrap_or("").to_string();
    let to_hdr = refer_request.from_header().unwrap_or("").to_string();
    let call_id = refer_request.call_id().unwrap_or("").to_string();

    let headers = vec![
        SipHeader {
            name: header_names::VIA.to_string(),
            value: format!("SIP/2.0/UDP placeholder;branch={}", branch),
        },
        SipHeader {
            name: header_names::MAX_FORWARDS.to_string(),
            value: "70".to_string(),
        },
        SipHeader {
            name: header_names::FROM.to_string(),
            value: if from_hdr.contains("tag=") {
                from_hdr
            } else {
                format!("{};tag={}", from_hdr, local_tag)
            },
        },
        SipHeader {
            name: header_names::TO.to_string(),
            value: to_hdr,
        },
        SipHeader {
            name: header_names::CALL_ID.to_string(),
            value: call_id,
        },
        SipHeader {
            name: header_names::CSEQ.to_string(),
            value: format!("{} NOTIFY", notify_cseq),
        },
        SipHeader {
            name: "Event".to_string(),
            value: "refer".to_string(),
        },
        SipHeader {
            name: "Subscription-State".to_string(),
            value: sub_state,
        },
        SipHeader {
            name: header_names::CONTENT_TYPE.to_string(),
            value: "message/sipfrag;version=2.0".to_string(),
        },
        SipHeader {
            name: header_names::CONTENT_LENGTH.to_string(),
            value: sipfrag.len().to_string(),
        },
    ];

    SipMessage {
        start_line: StartLine::Request(RequestLine {
            method: SipMethod::Notify,
            uri: target_uri,
            version: "SIP/2.0".to_string(),
        }),
        headers,
        body: sipfrag,
    }
}

/// Build a REFER request (when we are initiating a transfer).
pub fn build_refer(
    refer_to: &str,
    dialog_remote_target: &str,
    call_id: &str,
    from_tag: &str,
    remote_tag: &str,
    cseq: u32,
    local_addr: &str,
) -> SipMessage {
    let branch = format!(
        "z9hG4bK{}",
        &Uuid::new_v4().to_string().replace('-', "")[..16]
    );

    let target_uri = SipUri::parse(dialog_remote_target).unwrap_or_else(|_| SipUri {
        scheme: "sip".to_string(),
        user: None,
        password: None,
        host: "localhost".to_string(),
        port: Some(5060),
        parameters: Default::default(),
        headers: Default::default(),
    });

    let headers = vec![
        SipHeader {
            name: header_names::VIA.to_string(),
            value: format!("SIP/2.0/UDP {};branch={}", local_addr, branch),
        },
        SipHeader {
            name: header_names::MAX_FORWARDS.to_string(),
            value: "70".to_string(),
        },
        SipHeader {
            name: header_names::FROM.to_string(),
            value: format!("<sip:asterisk@{}>;tag={}", local_addr, from_tag),
        },
        SipHeader {
            name: header_names::TO.to_string(),
            value: format!("<{}>;tag={}", dialog_remote_target, remote_tag),
        },
        SipHeader {
            name: header_names::CALL_ID.to_string(),
            value: call_id.to_string(),
        },
        SipHeader {
            name: header_names::CSEQ.to_string(),
            value: format!("{} REFER", cseq),
        },
        SipHeader {
            name: header_names::CONTACT.to_string(),
            value: format!("<sip:asterisk@{}>", local_addr),
        },
        SipHeader {
            name: "Refer-To".to_string(),
            value: format!("<{}>", refer_to),
        },
        SipHeader {
            name: header_names::CONTENT_LENGTH.to_string(),
            value: "0".to_string(),
        },
    ];

    SipMessage {
        start_line: StartLine::Request(RequestLine {
            method: SipMethod::Refer,
            uri: target_uri,
            version: "SIP/2.0".to_string(),
        }),
        headers,
        body: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ReferError {
    NotRefer,
    MissingReferTo,
    InvalidUri(String),
}

impl fmt::Display for ReferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRefer => write!(f, "Not a REFER request"),
            Self::MissingReferTo => write!(f, "Missing Refer-To header"),
            Self::InvalidUri(u) => write!(f, "Invalid URI: {}", u),
        }
    }
}

impl std::error::Error for ReferError {}

fn make_error(request: &SipMessage, code: u16, reason: &str) -> SipMessage {
    request
        .create_response(code, reason)
        .unwrap_or_else(|_| SipMessage {
            start_line: StartLine::Response(StatusLine {
                version: "SIP/2.0".to_string(),
                status_code: code,
                reason_phrase: reason.to_string(),
            }),
            headers: Vec::new(),
            body: String::new(),
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_blind_refer() {
        let msg = SipMessage::parse(
            b"REFER sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: refer-test-123\r\n\
              CSeq: 1 REFER\r\n\
              Refer-To: <sip:carol@example.com>\r\n\
              Contact: <sip:bob@10.0.0.2>\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let xfer = parse_refer(&msg).unwrap();
        assert_eq!(xfer.refer_to, "sip:carol@example.com");
        assert_eq!(xfer.transfer_type, TransferType::Blind);
        assert!(xfer.replaces.is_none());
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(url_decode("call-id%3Bto-tag%3Dabc"), "call-id;to-tag=abc");
    }

    #[test]
    fn test_transfer_notify_sipfrag() {
        let refer = SipMessage::parse(
            b"REFER sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: refer-test-456\r\n\
              CSeq: 1 REFER\r\n\
              Refer-To: <sip:carol@example.com>\r\n\
              Contact: <sip:bob@10.0.0.2>\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let notify = build_transfer_notify(&refer, TransferProgress::Success, "localtag", 1);
        assert_eq!(notify.method(), Some(SipMethod::Notify));
        assert_eq!(notify.body, "SIP/2.0 200 OK");
        assert!(notify
            .get_header("Subscription-State")
            .unwrap()
            .contains("terminated"));
    }
}
