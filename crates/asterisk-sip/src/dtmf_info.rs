//! DTMF via SIP INFO (port of res_pjsip_dtmf_info.c).
//!
//! Parses incoming SIP INFO requests carrying DTMF events and converts
//! them to internal DTMF frames. Supports the `application/dtmf-relay`
//! and `application/dtmf` content types.


use crate::parser::{header_names, SipMessage, SipMethod};

// ---------------------------------------------------------------------------
// DTMF event
// ---------------------------------------------------------------------------

/// A DTMF event parsed from a SIP INFO request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtmfInfoEvent {
    /// The DTMF digit (0-9, *, #, A-D, !).
    pub digit: char,
    /// Duration in milliseconds.
    pub duration_ms: u32,
}

// ---------------------------------------------------------------------------
// Content type detection
// ---------------------------------------------------------------------------

/// Supported DTMF INFO content types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DtmfContentType {
    /// `application/dtmf` -- body contains just the digit.
    Dtmf,
    /// `application/dtmf-relay` -- body contains Signal= and Duration= lines.
    DtmfRelay,
    /// `application/hook-flash` -- flash event.
    HookFlash,
    /// Not a DTMF content type.
    Unknown,
}

fn classify_content_type(ct: &str) -> DtmfContentType {
    let ct = ct.trim().to_lowercase();
    if ct == "application/dtmf" {
        DtmfContentType::Dtmf
    } else if ct == "application/dtmf-relay" {
        DtmfContentType::DtmfRelay
    } else if ct == "application/hook-flash" {
        DtmfContentType::HookFlash
    } else {
        DtmfContentType::Unknown
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a character as a DTMF event.
///
/// Handles both literal digits and numeric event codes (0-16 per RFC 4733).
fn get_event(c: &str) -> Option<char> {
    let c = c.trim();
    if c.is_empty() {
        return None;
    }

    let first = c.chars().next()?;

    // Direct digit characters.
    if first.is_ascii_digit()
        || first == '*'
        || first == '#'
        || first == '!'
        || ('A'..='D').contains(&first)
        || ('a'..='d').contains(&first)
    {
        // Could be a multi-character numeric event code like "10", "11".
        if let Ok(event) = c.parse::<u32>() {
            return match event {
                0..=9 => Some((b'0' + event as u8) as char),
                10 => Some('*'),
                11 => Some('#'),
                12 => Some('A'),
                13 => Some('B'),
                14 => Some('C'),
                15 => Some('D'),
                16 => Some('!'),
                _ => None,
            };
        }
        return Some(first);
    }

    None
}

/// Parse a SIP INFO request containing DTMF information.
///
/// Returns `None` if the request is not a DTMF INFO, or if parsing fails.
pub fn parse_dtmf_info(request: &SipMessage) -> Option<DtmfInfoEvent> {
    if request.method() != Some(SipMethod::Info) {
        return None;
    }

    let content_type = request
        .get_header(header_names::CONTENT_TYPE)
        .unwrap_or("");

    let ct = classify_content_type(content_type);
    if ct == DtmfContentType::Unknown {
        return None;
    }

    // Hook flash -> special '!' digit.
    if ct == DtmfContentType::HookFlash {
        return Some(DtmfInfoEvent {
            digit: '!',
            duration_ms: 0,
        });
    }

    let body = request.body.trim();
    if body.is_empty() {
        // Empty body is acceptable per the C code (returns 200 OK with no event).
        return None;
    }

    match ct {
        DtmfContentType::Dtmf => {
            // Body is directly the event character/code.
            let digit = get_event(body)?;
            Some(DtmfInfoEvent {
                digit,
                duration_ms: 100,
            })
        }
        DtmfContentType::DtmfRelay => {
            // Body format:
            //   Signal=5
            //   Duration=160
            let mut event: Option<char> = None;
            let mut duration: u32 = 100;

            for line in body.lines() {
                let line = line.trim();
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    if key.eq_ignore_ascii_case("signal") {
                        event = get_event(value);
                    } else if key.eq_ignore_ascii_case("duration") {
                        duration = value.parse().unwrap_or(100);
                    }
                }
            }

            let digit = event?;
            Some(DtmfInfoEvent {
                digit,
                duration_ms: duration,
            })
        }
        _ => None,
    }
}

/// Build a SIP INFO request carrying a DTMF event.
pub fn build_dtmf_info(
    digit: char,
    duration_ms: u32,
) -> (String, String) {
    // Use application/dtmf-relay format.
    let content_type = "application/dtmf-relay".to_string();
    let body = format!("Signal={}\r\nDuration={}\r\n", digit, duration_ms);
    (content_type, body)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dtmf_relay() {
        let msg = SipMessage::parse(
            b"INFO sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: dtmf-test-123\r\n\
              CSeq: 1 INFO\r\n\
              Content-Type: application/dtmf-relay\r\n\
              Content-Length: 29\r\n\
              \r\n\
              Signal=5\r\nDuration=160\r\n",
        )
        .unwrap();

        let event = parse_dtmf_info(&msg).unwrap();
        assert_eq!(event.digit, '5');
        assert_eq!(event.duration_ms, 160);
    }

    #[test]
    fn test_parse_dtmf_direct() {
        let msg = SipMessage::parse(
            b"INFO sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: dtmf-test-456\r\n\
              CSeq: 1 INFO\r\n\
              Content-Type: application/dtmf\r\n\
              Content-Length: 1\r\n\
              \r\n\
              #",
        )
        .unwrap();

        let event = parse_dtmf_info(&msg).unwrap();
        assert_eq!(event.digit, '#');
    }

    #[test]
    fn test_parse_dtmf_star_event_10() {
        let msg = SipMessage::parse(
            b"INFO sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: dtmf-test-789\r\n\
              CSeq: 1 INFO\r\n\
              Content-Type: application/dtmf-relay\r\n\
              Content-Length: 28\r\n\
              \r\n\
              Signal=10\r\nDuration=80\r\n",
        )
        .unwrap();

        let event = parse_dtmf_info(&msg).unwrap();
        assert_eq!(event.digit, '*');
        assert_eq!(event.duration_ms, 80);
    }

    #[test]
    fn test_get_event_chars() {
        assert_eq!(get_event("5"), Some('5'));
        assert_eq!(get_event("*"), Some('*'));
        assert_eq!(get_event("#"), Some('#'));
        assert_eq!(get_event("A"), Some('A'));
        assert_eq!(get_event("10"), Some('*'));
        assert_eq!(get_event("11"), Some('#'));
        assert_eq!(get_event("16"), Some('!'));
    }

    #[test]
    fn test_not_dtmf_info() {
        let msg = SipMessage::parse(
            b"INFO sip:alice@10.0.0.1 SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.2;branch=z9hG4bK123\r\n\
              From: Bob <sip:bob@example.com>;tag=abc\r\n\
              To: Alice <sip:alice@example.com>;tag=def\r\n\
              Call-ID: dtmf-test-000\r\n\
              CSeq: 1 INFO\r\n\
              Content-Type: application/xml\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        assert!(parse_dtmf_info(&msg).is_none());
    }
}
