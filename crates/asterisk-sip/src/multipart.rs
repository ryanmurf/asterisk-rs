//! Multipart MIME body support for SIP messages.
//!
//! SIP messages can carry multipart/mixed bodies (e.g., SDP + ISUP).
//! This module handles parsing and generation of multipart MIME bodies
//! as used in SIP per RFC 5621.

use uuid::Uuid;

/// A multipart MIME body consisting of multiple body parts.
#[derive(Debug, Clone)]
pub struct MultipartBody {
    /// The boundary string used to separate parts.
    pub boundary: String,
    /// The individual body parts.
    pub parts: Vec<BodyPart>,
}

/// A single part within a multipart MIME body.
#[derive(Debug, Clone)]
pub struct BodyPart {
    /// Content-Type of this part (e.g., `application/sdp`).
    pub content_type: String,
    /// Optional Content-Disposition header.
    pub content_disposition: Option<String>,
    /// The body content.
    pub body: Vec<u8>,
}

/// Error type for multipart operations.
#[derive(Debug, thiserror::Error)]
pub enum MultipartError {
    #[error("Missing boundary parameter in Content-Type")]
    MissingBoundary,
    #[error("Invalid multipart structure: {0}")]
    InvalidStructure(String),
    #[error("Invalid UTF-8 in headers: {0}")]
    InvalidUtf8(String),
}

/// Parse a multipart body from the Content-Type header and raw body bytes.
///
/// The `content_type` should be the full Content-Type header value, e.g.:
/// `multipart/mixed;boundary=unique-boundary-1`
pub fn parse_multipart(content_type: &str, body: &[u8]) -> Result<MultipartBody, MultipartError> {
    let boundary = extract_boundary(content_type)?;
    let body_str =
        std::str::from_utf8(body).map_err(|e| MultipartError::InvalidUtf8(e.to_string()))?;

    let delimiter = format!("--{}", boundary);
    let end_delimiter = format!("--{}--", boundary);

    let mut parts = Vec::new();

    // Split body by delimiter
    let sections: Vec<&str> = body_str.split(&delimiter).collect();

    for section in sections.iter().skip(1) {
        // Skip the closing delimiter
        let section = section.trim_start_matches("\r\n");
        if section.starts_with("--") || section.is_empty() {
            continue;
        }

        // Remove trailing end-delimiter if present
        let section = if let Some(stripped) = section.strip_suffix(&format!("--{}", "")) {
            stripped
        } else {
            section
        };

        // Split part into headers and body at the blank line
        let (headers_section, part_body) = if let Some(pos) = section.find("\r\n\r\n") {
            (&section[..pos], &section[pos + 4..])
        } else if let Some(pos) = section.find("\n\n") {
            (&section[..pos], &section[pos + 2..])
        } else {
            continue;
        };

        // Remove trailing delimiter markers from body
        let part_body = part_body
            .trim_end_matches("\r\n")
            .trim_end_matches(&end_delimiter)
            .trim_end_matches(&delimiter)
            .trim_end_matches("\r\n");

        let mut ct = String::new();
        let mut cd = None;

        for line in headers_section.lines() {
            let line = line.trim();
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if name.eq_ignore_ascii_case("Content-Type") {
                    ct = value.to_string();
                } else if name.eq_ignore_ascii_case("Content-Disposition") {
                    cd = Some(value.to_string());
                }
            }
        }

        if !ct.is_empty() {
            parts.push(BodyPart {
                content_type: ct,
                content_disposition: cd,
                body: part_body.as_bytes().to_vec(),
            });
        }
    }

    Ok(MultipartBody { boundary, parts })
}

/// Generate a multipart body from a list of body parts.
///
/// Returns a tuple of (content_type, body_bytes) where content_type is the
/// full Content-Type header value with boundary parameter.
pub fn generate_multipart(parts: &[BodyPart]) -> (String, Vec<u8>) {
    let boundary = format!("asterisk-boundary-{}", &Uuid::new_v4().to_string()[..8]);
    generate_multipart_with_boundary(parts, &boundary)
}

/// Generate a multipart body with a specific boundary string.
pub fn generate_multipart_with_boundary(parts: &[BodyPart], boundary: &str) -> (String, Vec<u8>) {
    let content_type = format!("multipart/mixed;boundary={}", boundary);
    let mut body = Vec::new();

    for part in parts {
        // Delimiter
        body.extend_from_slice(b"--");
        body.extend_from_slice(boundary.as_bytes());
        body.extend_from_slice(b"\r\n");

        // Part headers
        body.extend_from_slice(
            format!("Content-Type: {}\r\n", part.content_type).as_bytes(),
        );
        if let Some(ref cd) = part.content_disposition {
            body.extend_from_slice(
                format!("Content-Disposition: {}\r\n", cd).as_bytes(),
            );
        }

        // Blank line between headers and body
        body.extend_from_slice(b"\r\n");

        // Part body
        body.extend_from_slice(&part.body);
        body.extend_from_slice(b"\r\n");
    }

    // Closing delimiter
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");

    (content_type, body)
}

/// Extract the boundary parameter from a Content-Type header value.
fn extract_boundary(content_type: &str) -> Result<String, MultipartError> {
    for param in content_type.split(';') {
        let param = param.trim();
        if let Some(val) = param.strip_prefix("boundary=") {
            let val = val.trim().trim_matches('"');
            if val.is_empty() {
                return Err(MultipartError::MissingBoundary);
            }
            return Ok(val.to_string());
        }
    }
    Err(MultipartError::MissingBoundary)
}

/// Check if a Content-Type header value indicates a multipart body.
pub fn is_multipart(content_type: &str) -> bool {
    content_type
        .trim()
        .to_lowercase()
        .starts_with("multipart/")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_multipart() {
        let content_type = "multipart/mixed;boundary=unique-boundary";
        let body = b"--unique-boundary\r\n\
Content-Type: application/sdp\r\n\
\r\n\
v=0\r\n\
o=- 0 0 IN IP4 10.0.0.1\r\n\
s=test\r\n\
\r\n\
--unique-boundary\r\n\
Content-Type: application/isup\r\n\
Content-Disposition: signal;handling=optional\r\n\
\r\n\
ISUP-DATA-HERE\r\n\
--unique-boundary--\r\n";

        let result = parse_multipart(content_type, body).unwrap();
        assert_eq!(result.boundary, "unique-boundary");
        assert_eq!(result.parts.len(), 2);
        assert_eq!(result.parts[0].content_type, "application/sdp");
        assert_eq!(result.parts[1].content_type, "application/isup");
        assert_eq!(
            result.parts[1].content_disposition.as_deref(),
            Some("signal;handling=optional")
        );
    }

    #[test]
    fn test_generate_multipart() {
        let parts = vec![
            BodyPart {
                content_type: "application/sdp".to_string(),
                content_disposition: None,
                body: b"v=0\r\no=- 0 0 IN IP4 10.0.0.1\r\ns=test".to_vec(),
            },
            BodyPart {
                content_type: "application/isup".to_string(),
                content_disposition: Some("signal;handling=optional".to_string()),
                body: b"ISUP-DATA".to_vec(),
            },
        ];

        let (ct, body) = generate_multipart(&parts);
        assert!(ct.starts_with("multipart/mixed;boundary="));
        assert!(is_multipart(&ct));

        // Parse what we generated to verify roundtrip
        let parsed = parse_multipart(&ct, &body).unwrap();
        assert_eq!(parsed.parts.len(), 2);
        assert_eq!(parsed.parts[0].content_type, "application/sdp");
        assert_eq!(parsed.parts[1].content_type, "application/isup");
        assert_eq!(parsed.parts[0].body, b"v=0\r\no=- 0 0 IN IP4 10.0.0.1\r\ns=test");
        assert_eq!(parsed.parts[1].body, b"ISUP-DATA");
    }

    #[test]
    fn test_generate_multipart_roundtrip() {
        let parts = vec![BodyPart {
            content_type: "text/plain".to_string(),
            content_disposition: None,
            body: b"Hello, World!".to_vec(),
        }];

        let (ct, body) = generate_multipart(&parts);
        let parsed = parse_multipart(&ct, &body).unwrap();
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].body, b"Hello, World!");
    }

    #[test]
    fn test_extract_boundary() {
        assert_eq!(
            extract_boundary("multipart/mixed;boundary=abc123").unwrap(),
            "abc123"
        );
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=\"quoted-boundary\"").unwrap(),
            "quoted-boundary"
        );
        assert!(extract_boundary("application/sdp").is_err());
    }

    #[test]
    fn test_is_multipart() {
        assert!(is_multipart("multipart/mixed;boundary=abc"));
        assert!(is_multipart("Multipart/Mixed;boundary=abc"));
        assert!(!is_multipart("application/sdp"));
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL MULTIPART TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_multipart_missing_final_boundary() {
        let content_type = "multipart/mixed;boundary=test-boundary";
        let body = b"--test-boundary\r\n\
Content-Type: text/plain\r\n\
\r\n\
Hello\r\n";
        // Missing --test-boundary-- at the end
        // Should still parse the part (graceful handling)
        let result = parse_multipart(content_type, body);
        assert!(result.is_ok());
        let mp = result.unwrap();
        assert_eq!(mp.parts.len(), 1);
    }

    #[test]
    fn test_multipart_empty_parts() {
        let content_type = "multipart/mixed;boundary=empty-test";
        let body = b"--empty-test\r\n\
Content-Type: text/plain\r\n\
\r\n\
\r\n\
--empty-test--\r\n";
        let result = parse_multipart(content_type, body).unwrap();
        // The part body should be empty or just whitespace
        assert_eq!(result.parts.len(), 1);
    }

    #[test]
    fn test_multipart_missing_boundary_param() {
        let content_type = "multipart/mixed";
        let body = b"some body";
        let result = parse_multipart(content_type, body);
        assert!(result.is_err());
    }

    #[test]
    fn test_multipart_empty_boundary_param() {
        let content_type = "multipart/mixed;boundary=";
        let body = b"some body";
        let result = parse_multipart(content_type, body);
        assert!(result.is_err());
    }

    #[test]
    fn test_multipart_boundary_in_quoted_param() {
        let content_type = "multipart/mixed;boundary=\"my-boundary\"";
        let body = b"--my-boundary\r\n\
Content-Type: text/plain\r\n\
\r\n\
content\r\n\
--my-boundary--\r\n";
        let result = parse_multipart(content_type, body).unwrap();
        assert_eq!(result.boundary, "my-boundary");
        assert_eq!(result.parts.len(), 1);
    }

    #[test]
    fn test_multipart_generate_roundtrip_multiple() {
        let parts = vec![
            BodyPart {
                content_type: "application/sdp".to_string(),
                content_disposition: None,
                body: b"v=0\r\ns=test".to_vec(),
            },
            BodyPart {
                content_type: "application/pidf+xml".to_string(),
                content_disposition: Some("render;handling=optional".to_string()),
                body: b"<xml>data</xml>".to_vec(),
            },
            BodyPart {
                content_type: "text/plain".to_string(),
                content_disposition: None,
                body: b"plain text".to_vec(),
            },
        ];

        let (ct, body) = generate_multipart(&parts);
        let parsed = parse_multipart(&ct, &body).unwrap();
        assert_eq!(parsed.parts.len(), 3);
        assert_eq!(parsed.parts[0].content_type, "application/sdp");
        assert_eq!(parsed.parts[1].content_type, "application/pidf+xml");
        assert_eq!(parsed.parts[2].content_type, "text/plain");
        assert_eq!(parsed.parts[0].body, b"v=0\r\ns=test");
        assert_eq!(parsed.parts[2].body, b"plain text");
    }

    #[test]
    fn test_multipart_specific_boundary_roundtrip() {
        let parts = vec![BodyPart {
            content_type: "text/plain".to_string(),
            content_disposition: None,
            body: b"test data".to_vec(),
        }];

        let (ct, body) = generate_multipart_with_boundary(&parts, "custom-boundary-123");
        assert!(ct.contains("custom-boundary-123"));
        let parsed = parse_multipart(&ct, &body).unwrap();
        assert_eq!(parsed.boundary, "custom-boundary-123");
        assert_eq!(parsed.parts.len(), 1);
        assert_eq!(parsed.parts[0].body, b"test data");
    }
}
