//! SIP message parser (RFC 3261).
//!
//! Parses SIP request/response messages including:
//! - Request and status lines
//! - Header fields with folding support
//! - SIP URIs
//! - Content-Length based body extraction

use std::collections::HashMap;
use std::fmt;

/// SIP methods as defined in RFC 3261 and extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SipMethod {
    Invite,
    Ack,
    Bye,
    Cancel,
    Register,
    Options,
    Subscribe,
    Notify,
    Refer,
    Info,
    Update,
    Message,
    Prack,
    Publish,
}

impl SipMethod {
    #[inline]
    pub fn from_name(s: &str) -> Option<Self> {
        // SIP methods are case-sensitive per RFC 3261, but we accept
        // case-insensitive matching for robustness. Avoid allocation by
        // using eq_ignore_ascii_case instead of to_uppercase().
        if s.eq_ignore_ascii_case("INVITE") {
            Some(Self::Invite)
        } else if s.eq_ignore_ascii_case("ACK") {
            Some(Self::Ack)
        } else if s.eq_ignore_ascii_case("BYE") {
            Some(Self::Bye)
        } else if s.eq_ignore_ascii_case("CANCEL") {
            Some(Self::Cancel)
        } else if s.eq_ignore_ascii_case("REGISTER") {
            Some(Self::Register)
        } else if s.eq_ignore_ascii_case("OPTIONS") {
            Some(Self::Options)
        } else if s.eq_ignore_ascii_case("SUBSCRIBE") {
            Some(Self::Subscribe)
        } else if s.eq_ignore_ascii_case("NOTIFY") {
            Some(Self::Notify)
        } else if s.eq_ignore_ascii_case("REFER") {
            Some(Self::Refer)
        } else if s.eq_ignore_ascii_case("INFO") {
            Some(Self::Info)
        } else if s.eq_ignore_ascii_case("UPDATE") {
            Some(Self::Update)
        } else if s.eq_ignore_ascii_case("MESSAGE") {
            Some(Self::Message)
        } else if s.eq_ignore_ascii_case("PRACK") {
            Some(Self::Prack)
        } else if s.eq_ignore_ascii_case("PUBLISH") {
            Some(Self::Publish)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Invite => "INVITE",
            Self::Ack => "ACK",
            Self::Bye => "BYE",
            Self::Cancel => "CANCEL",
            Self::Register => "REGISTER",
            Self::Options => "OPTIONS",
            Self::Subscribe => "SUBSCRIBE",
            Self::Notify => "NOTIFY",
            Self::Refer => "REFER",
            Self::Info => "INFO",
            Self::Update => "UPDATE",
            Self::Message => "MESSAGE",
            Self::Prack => "PRACK",
            Self::Publish => "PUBLISH",
        }
    }
}

impl fmt::Display for SipMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A SIP URI: sip:user@host:port;params?headers
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipUri {
    pub scheme: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub parameters: HashMap<String, Option<String>>,
    pub headers: HashMap<String, String>,
}

impl SipUri {
    /// Parse a SIP URI from a string.
    #[inline]
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let input = input.trim();

        // Scheme -- use case-insensitive comparison to avoid allocation
        // in the common case where the scheme is already lowercase.
        let (scheme_raw, rest) = input
            .split_once(':')
            .ok_or_else(|| ParseError("Missing scheme in URI".into()))?;
        let scheme = if scheme_raw.eq_ignore_ascii_case("sip") {
            "sip".to_string()
        } else if scheme_raw.eq_ignore_ascii_case("sips") {
            "sips".to_string()
        } else if scheme_raw.eq_ignore_ascii_case("tel") {
            "tel".to_string()
        } else {
            return Err(ParseError(format!("Unknown URI scheme: {}", scheme_raw)));
        };

        // Split off headers (after ?)
        let (before_headers, headers_str) = match rest.split_once('?') {
            Some((b, h)) => (b, Some(h)),
            None => (rest, None),
        };

        // Split off parameters (after ;)
        let (before_params, params_str) = match before_headers.split_once(';') {
            Some((b, p)) => (b, Some(p)),
            None => (before_headers, None),
        };

        // Parse user@host:port
        let (user, password, host, port) = if let Some((userinfo, hostport)) =
            before_params.split_once('@')
        {
            let (user, password) = match userinfo.split_once(':') {
                Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
                None => (Some(userinfo.to_string()), None),
            };
            let (host, port) = parse_host_port(hostport)?;
            (user, password, host, port)
        } else {
            let (host, port) = parse_host_port(before_params)?;
            (None, None, host, port)
        };

        // Parse parameters
        let mut parameters = HashMap::new();
        if let Some(params) = params_str {
            for param in params.split(';') {
                if param.is_empty() {
                    continue;
                }
                match param.split_once('=') {
                    Some((k, v)) => {
                        parameters.insert(k.to_lowercase(), Some(v.to_string()));
                    }
                    None => {
                        parameters.insert(param.to_lowercase(), None);
                    }
                }
            }
        }

        // Parse headers
        let mut headers = HashMap::new();
        if let Some(hdrs) = headers_str {
            for hdr in hdrs.split('&') {
                if let Some((k, v)) = hdr.split_once('=') {
                    headers.insert(k.to_string(), v.to_string());
                }
            }
        }

        Ok(SipUri {
            scheme,
            user,
            password,
            host,
            port,
            parameters,
            headers,
        })
    }

    /// Get a parameter value.
    pub fn get_param(&self, name: &str) -> Option<&str> {
        self.parameters
            .get(&name.to_lowercase())
            .and_then(|v| v.as_deref())
    }

    /// Get the transport parameter.
    pub fn transport(&self) -> Option<&str> {
        self.get_param("transport")
    }

    /// Check if this URI has an IPv6 host address.
    pub fn is_ipv6_host(&self) -> bool {
        self.host.contains(':')
    }

    /// Format the host for display in SIP headers.
    /// IPv6 addresses are wrapped in brackets.
    pub fn host_display(&self) -> String {
        if self.is_ipv6_host() {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }
}

impl fmt::Display for SipUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if let Some(ref user) = self.user {
            write!(f, "{}", user)?;
            if let Some(ref pass) = self.password {
                write!(f, ":{}", pass)?;
            }
            write!(f, "@")?;
        }
        // IPv6 addresses must be wrapped in brackets
        if self.is_ipv6_host() {
            write!(f, "[{}]", self.host)?;
        } else {
            write!(f, "{}", self.host)?;
        }
        if let Some(port) = self.port {
            write!(f, ":{}", port)?;
        }
        for (k, v) in &self.parameters {
            match v {
                Some(val) => write!(f, ";{}={}", k, val)?,
                None => write!(f, ";{}", k)?,
            }
        }
        let mut first_hdr = true;
        for (k, v) in &self.headers {
            write!(f, "{}", if first_hdr { "?" } else { "&" })?;
            write!(f, "{}={}", k, v)?;
            first_hdr = false;
        }
        Ok(())
    }
}

fn parse_host_port(s: &str) -> Result<(String, Option<u16>), ParseError> {
    // Handle IPv6: [::1]:port
    if s.starts_with('[') {
        if let Some(end_bracket) = s.find(']') {
            let host = s[1..end_bracket].to_string();
            let rest = &s[end_bracket + 1..];
            let port = if let Some(port_str) = rest.strip_prefix(':') {
                Some(
                    port_str
                        .parse::<u16>()
                        .map_err(|_| ParseError(format!("Invalid port: {}", port_str)))?,
                )
            } else {
                None
            };
            return Ok((host, port));
        }
    }

    match s.rsplit_once(':') {
        Some((host, port_str)) => {
            if let Ok(port) = port_str.parse::<u16>() {
                Ok((host.to_string(), Some(port)))
            } else {
                // Might be IPv6 without brackets
                Ok((s.to_string(), None))
            }
        }
        None => Ok((s.to_string(), None)),
    }
}

/// SIP request line.
#[derive(Debug, Clone)]
pub struct RequestLine {
    pub method: SipMethod,
    pub uri: SipUri,
    pub version: String,
}

/// SIP status (response) line.
#[derive(Debug, Clone)]
pub struct StatusLine {
    pub version: String,
    pub status_code: u16,
    pub reason_phrase: String,
}

/// The first line of a SIP message.
#[derive(Debug, Clone)]
pub enum StartLine {
    Request(RequestLine),
    Response(StatusLine),
}

/// A single SIP header.
#[derive(Debug, Clone)]
pub struct SipHeader {
    pub name: String,
    pub value: String,
}

/// Well-known SIP header name constants.
pub mod header_names {
    pub const VIA: &str = "Via";
    pub const FROM: &str = "From";
    pub const TO: &str = "To";
    pub const CALL_ID: &str = "Call-ID";
    pub const CSEQ: &str = "CSeq";
    pub const CONTACT: &str = "Contact";
    pub const MAX_FORWARDS: &str = "Max-Forwards";
    pub const CONTENT_TYPE: &str = "Content-Type";
    pub const CONTENT_LENGTH: &str = "Content-Length";
    pub const ROUTE: &str = "Route";
    pub const RECORD_ROUTE: &str = "Record-Route";
    pub const EXPIRES: &str = "Expires";
    pub const WWW_AUTHENTICATE: &str = "WWW-Authenticate";
    pub const PROXY_AUTHENTICATE: &str = "Proxy-Authenticate";
    pub const AUTHORIZATION: &str = "Authorization";
    pub const PROXY_AUTHORIZATION: &str = "Proxy-Authorization";
    pub const ALLOW: &str = "Allow";
    pub const SUPPORTED: &str = "Supported";
    pub const REQUIRE: &str = "Require";
    pub const USER_AGENT: &str = "User-Agent";
    pub const SERVER: &str = "Server";
    pub const RETRY_AFTER: &str = "Retry-After";
    pub const SERVICE_ROUTE: &str = "Service-Route";
    pub const SESSION_EXPIRES: &str = "Session-Expires";
    pub const MIN_SE: &str = "Min-SE";
    pub const FLOW_TIMER: &str = "Flow-Timer";
}

/// Compact header name mapping (RFC 3261 Section 7.3.3).
#[inline]
fn expand_compact_header(name: &str) -> &str {
    match name {
        "i" => header_names::CALL_ID,
        "m" => header_names::CONTACT,
        "e" => "Content-Encoding",
        "l" => header_names::CONTENT_LENGTH,
        "c" => header_names::CONTENT_TYPE,
        "f" => header_names::FROM,
        "s" => "Subject",
        "k" => header_names::SUPPORTED,
        "t" => header_names::TO,
        "v" => header_names::VIA,
        other => other,
    }
}

/// A parsed SIP message.
#[derive(Debug, Clone)]
pub struct SipMessage {
    pub start_line: StartLine,
    pub headers: Vec<SipHeader>,
    pub body: String,
}

/// Parse error.
#[derive(Debug, Clone)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SIP parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

impl SipMessage {
    /// Parse a complete SIP message from bytes.
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let text = std::str::from_utf8(data)
            .map_err(|e| ParseError(format!("Invalid UTF-8: {}", e)))?;
        parse_message(text)
    }

    /// Get the first header with the given name (case-insensitive).
    #[inline]
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// Get all headers with the given name.
    #[inline]
    pub fn get_headers(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
            .collect()
    }

    /// Get the Call-ID header.
    #[inline]
    pub fn call_id(&self) -> Option<&str> {
        self.get_header(header_names::CALL_ID)
    }

    /// Get the CSeq header.
    #[inline]
    pub fn cseq(&self) -> Option<&str> {
        self.get_header(header_names::CSEQ)
    }

    /// Get the From header.
    #[inline]
    pub fn from_header(&self) -> Option<&str> {
        self.get_header(header_names::FROM)
    }

    /// Get the To header.
    #[inline]
    pub fn to_header(&self) -> Option<&str> {
        self.get_header(header_names::TO)
    }

    /// Check if this is a request.
    #[inline]
    pub fn is_request(&self) -> bool {
        matches!(self.start_line, StartLine::Request(_))
    }

    /// Check if this is a response.
    #[inline]
    pub fn is_response(&self) -> bool {
        matches!(self.start_line, StartLine::Response(_))
    }

    /// Get the method (for requests).
    #[inline]
    pub fn method(&self) -> Option<SipMethod> {
        match &self.start_line {
            StartLine::Request(r) => Some(r.method),
            _ => None,
        }
    }

    /// Get the status code (for responses).
    #[inline]
    pub fn status_code(&self) -> Option<u16> {
        match &self.start_line {
            StartLine::Response(r) => Some(r.status_code),
            _ => None,
        }
    }

    /// Create a response to a request.
    /// Create a new SIP request message.
    pub fn new_request(method: SipMethod, uri: &str) -> Self {
        let parsed_uri = SipUri::parse(uri).unwrap_or_else(|_| SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: uri.to_string(),
            port: None,
            parameters: HashMap::new(),
            headers: HashMap::new(),
        });
        SipMessage {
            start_line: StartLine::Request(RequestLine {
                method,
                uri: parsed_uri,
                version: "SIP/2.0".to_string(),
            }),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// Create a new SIP response message.
    pub fn new_response(status_code: u16, reason: &str) -> Self {
        SipMessage {
            start_line: StartLine::Response(StatusLine {
                version: "SIP/2.0".to_string(),
                status_code,
                reason_phrase: reason.to_string(),
            }),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// Add a header to this message.
    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push(SipHeader {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    /// Create a response to a request.
    pub fn create_response(&self, status_code: u16, reason: &str) -> Result<SipMessage, ParseError> {
        if !self.is_request() {
            return Err(ParseError("Cannot create response to a response".into()));
        }

        let status_line = StatusLine {
            version: "SIP/2.0".to_string(),
            status_code,
            reason_phrase: reason.to_string(),
        };

        // Copy Via, From, To, Call-ID, CSeq from request
        let mut headers = Vec::new();
        for hdr_name in &[header_names::VIA, header_names::FROM, header_names::TO, header_names::CALL_ID, header_names::CSEQ] {
            for val in self.get_headers(hdr_name) {
                headers.push(SipHeader {
                    name: hdr_name.to_string(),
                    value: val.to_string(),
                });
            }
        }

        // Add Content-Length: 0
        headers.push(SipHeader {
            name: header_names::CONTENT_LENGTH.to_string(),
            value: "0".to_string(),
        });

        Ok(SipMessage {
            start_line: StartLine::Response(status_line),
            headers,
            body: String::new(),
        })
    }
}

impl fmt::Display for SipMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Start line
        match &self.start_line {
            StartLine::Request(r) => {
                write!(f, "{} {} {}\r\n", r.method, r.uri, r.version)?;
            }
            StartLine::Response(r) => {
                write!(f, "{} {} {}\r\n", r.version, r.status_code, r.reason_phrase)?;
            }
        }

        // Headers
        for header in &self.headers {
            write!(f, "{}: {}\r\n", header.name, header.value)?;
        }

        // Blank line
        write!(f, "\r\n")?;

        // Body
        write!(f, "{}", self.body)
    }
}

/// Maximum allowed Content-Length to prevent DoS via unbounded allocation.
const MAX_CONTENT_LENGTH: usize = 65536;

/// Maximum number of headers allowed per SIP message to prevent DoS.
const MAX_HEADER_COUNT: usize = 256;

/// Maximum length of a single header value to prevent DoS.
const MAX_HEADER_VALUE_LEN: usize = 8192;

/// Parse a SIP message from text.
pub fn parse_message(text: &str) -> Result<SipMessage, ParseError> {
    // Split headers from body at the blank line (\r\n\r\n)
    let (header_section, body) = match text.find("\r\n\r\n") {
        Some(pos) => (&text[..pos], &text[pos + 4..]),
        None => {
            // Try Unix line endings
            match text.find("\n\n") {
                Some(pos) => (&text[..pos], &text[pos + 2..]),
                None => (text, ""),
            }
        }
    };

    let lines: Vec<&str> = header_section.split('\n').collect();

    if lines.is_empty() {
        return Err(ParseError("Empty message".into()));
    }

    // Parse start line
    let first_line = lines[0].trim_end_matches('\r').trim();
    if first_line.is_empty() {
        return Err(ParseError("Empty start line".into()));
    }
    let start_line = parse_start_line(first_line)?;

    // Parse headers with folding support
    let mut headers = Vec::new();
    let mut i = 1;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');

        if line.is_empty() {
            break;
        }

        // Header folding: if line starts with whitespace, append to previous header
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = headers.last_mut() {
                let h: &mut SipHeader = last;
                h.value.push(' ');
                h.value.push_str(line.trim());
                // Enforce max header value length
                if h.value.len() > MAX_HEADER_VALUE_LEN {
                    return Err(ParseError(format!(
                        "Header '{}' value exceeds maximum length ({})",
                        h.name, MAX_HEADER_VALUE_LEN
                    )));
                }
            }
            i += 1;
            continue;
        }

        // Enforce max header count
        if headers.len() >= MAX_HEADER_COUNT {
            return Err(ParseError(format!(
                "Too many headers (max {})",
                MAX_HEADER_COUNT
            )));
        }

        // Parse "Name: Value"
        if let Some((name, value)) = line.split_once(':') {
            let name = expand_compact_header(name.trim()).to_string();
            let value = value.trim().to_string();
            if value.len() > MAX_HEADER_VALUE_LEN {
                return Err(ParseError(format!(
                    "Header '{}' value exceeds maximum length ({})",
                    name, MAX_HEADER_VALUE_LEN
                )));
            }
            headers.push(SipHeader { name, value });
        }

        i += 1;
    }

    // Handle Content-Length based body extraction
    let content_length = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .and_then(|h| h.value.trim().parse::<usize>().ok());

    let body = if let Some(cl) = content_length {
        // Enforce maximum content length to prevent DoS
        if cl > MAX_CONTENT_LENGTH {
            return Err(ParseError(format!(
                "Content-Length {} exceeds maximum allowed ({})",
                cl, MAX_CONTENT_LENGTH
            )));
        }
        if cl > 0 && body.len() >= cl {
            body[..cl].to_string()
        } else {
            body.to_string()
        }
    } else {
        body.to_string()
    };

    Ok(SipMessage {
        start_line,
        headers,
        body,
    })
}

fn parse_start_line(line: &str) -> Result<StartLine, ParseError> {
    // Check if it's a response: "SIP/2.0 200 OK"
    if line.starts_with("SIP/") {
        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        if parts.len() < 3 {
            return Err(ParseError(format!("Invalid status line: {}", line)));
        }
        let status_code = parts[1]
            .parse::<u16>()
            .map_err(|_| ParseError(format!("Invalid status code: {}", parts[1])))?;
        return Ok(StartLine::Response(StatusLine {
            version: parts[0].to_string(),
            status_code,
            reason_phrase: parts[2].to_string(),
        }));
    }

    // It's a request: "INVITE sip:user@host SIP/2.0"
    let parts: Vec<&str> = line.splitn(3, ' ').collect();
    if parts.len() < 3 {
        return Err(ParseError(format!("Invalid request line: {}", line)));
    }

    let method = SipMethod::from_name(parts[0])
        .ok_or_else(|| ParseError(format!("Unknown SIP method: {}", parts[0])))?;
    let uri = SipUri::parse(parts[1])?;

    Ok(StartLine::Request(RequestLine {
        method,
        uri,
        version: parts[2].to_string(),
    }))
}

/// Extract the tag parameter from a From/To header value.
pub fn extract_tag(header_value: &str) -> Option<String> {
    // Look for ";tag=..." in the header value
    for part in header_value.split(';') {
        let trimmed = part.trim();
        if let Some(tag) = trimmed.strip_prefix("tag=") {
            return Some(tag.to_string());
        }
    }
    None
}

/// Extract the URI from a From/To header value (may be in angle brackets).
pub fn extract_uri(header_value: &str) -> Option<String> {
    if let Some(start) = header_value.find('<') {
        if let Some(end) = header_value.find('>') {
            return Some(header_value[start + 1..end].to_string());
        }
    }
    // No angle brackets -- the whole value (minus params) is the URI
    Some(header_value.split(';').next()?.trim().to_string())
}

/// Parse a Via header value to extract branch, host, port, etc.
pub fn parse_via(value: &str) -> (String, String, Option<u16>, Option<String>) {
    // Via: SIP/2.0/UDP 10.0.0.1:5060;branch=z9hG4bK776asdhds
    let parts: Vec<&str> = value.splitn(2, ' ').collect();
    let transport = parts.first().unwrap_or(&"SIP/2.0/UDP").to_string();

    let rest = parts.get(1).unwrap_or(&"");
    let host_and_params: Vec<&str> = rest.splitn(2, ';').collect();
    let (host, port) = parse_host_port(host_and_params[0]).unwrap_or_default();

    let mut branch = None;
    if let Some(params) = host_and_params.get(1) {
        for param in params.split(';') {
            let trimmed = param.trim();
            if let Some(b) = trimmed.strip_prefix("branch=") {
                branch = Some(b.to_string());
            }
        }
    }

    (transport, host, port, branch)
}

// ---------------------------------------------------------------------------
// Retry-After header (RFC 3261 Section 20.33)
// ---------------------------------------------------------------------------

/// Parsed `Retry-After` header value.
///
/// Used in 503 (Service Unavailable) and 486 (Busy Here) responses to
/// indicate when the caller should retry.
///
/// Format: `Retry-After: 120 (I'm in a meeting) ;duration=3600`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryAfter {
    /// Number of seconds to wait before retrying.
    pub seconds: u32,
    /// Optional human-readable comment in parentheses.
    pub comment: Option<String>,
    /// Optional duration parameter (seconds the condition is expected to last).
    pub duration: Option<u32>,
}

impl RetryAfter {
    /// Parse a Retry-After header value.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }

        // Extract seconds (first token)
        let mut rest = value;
        let seconds_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let seconds: u32 = seconds_str.parse().ok()?;
        rest = rest[seconds_str.len()..].trim_start();

        // Extract optional comment in parentheses
        let mut comment = None;
        if rest.starts_with('(') {
            if let Some(end_paren) = rest.find(')') {
                comment = Some(rest[1..end_paren].to_string());
                rest = rest[end_paren + 1..].trim_start();
            }
        }

        // Extract optional parameters
        let mut duration = None;
        for part in rest.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("duration=") {
                duration = val.trim().parse::<u32>().ok();
            }
        }

        Some(RetryAfter {
            seconds,
            comment,
            duration,
        })
    }

    /// Extract and parse Retry-After from a SIP message.
    pub fn from_message(msg: &SipMessage) -> Option<Self> {
        let value = msg.get_header(header_names::RETRY_AFTER)?;
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sip_request() {
        let msg = b"INVITE sip:bob@biloxi.example.com SIP/2.0\r\n\
Via: SIP/2.0/UDP pc33.atlanta.example.com;branch=z9hG4bKnashds8\r\n\
Max-Forwards: 70\r\n\
To: Bob <sip:bob@biloxi.example.com>\r\n\
From: Alice <sip:alice@atlanta.example.com>;tag=1928301774\r\n\
Call-ID: a84b4c76e66710@pc33.atlanta.example.com\r\n\
CSeq: 314159 INVITE\r\n\
Contact: <sip:alice@pc33.atlanta.example.com>\r\n\
Content-Type: application/sdp\r\n\
Content-Length: 0\r\n\
\r\n";

        let parsed = SipMessage::parse(msg).unwrap();
        assert!(parsed.is_request());
        assert_eq!(parsed.method(), Some(SipMethod::Invite));
        assert_eq!(
            parsed.call_id(),
            Some("a84b4c76e66710@pc33.atlanta.example.com")
        );
        assert_eq!(parsed.cseq(), Some("314159 INVITE"));

        let from = parsed.from_header().unwrap();
        assert!(from.contains("Alice"));
        assert_eq!(extract_tag(from), Some("1928301774".to_string()));
    }

    #[test]
    fn test_parse_sip_response() {
        let msg = b"SIP/2.0 200 OK\r\n\
Via: SIP/2.0/UDP server10.biloxi.example.com;branch=z9hG4bKnashds8\r\n\
To: Bob <sip:bob@biloxi.example.com>;tag=a6c85cf\r\n\
From: Alice <sip:alice@atlanta.example.com>;tag=1928301774\r\n\
Call-ID: a84b4c76e66710@pc33.atlanta.example.com\r\n\
CSeq: 314159 INVITE\r\n\
Contact: <sip:bob@192.0.2.4>\r\n\
Content-Length: 0\r\n\
\r\n";

        let parsed = SipMessage::parse(msg).unwrap();
        assert!(parsed.is_response());
        assert_eq!(parsed.status_code(), Some(200));
    }

    #[test]
    fn test_parse_sip_uri() {
        let uri = SipUri::parse("sip:alice@atlanta.example.com:5060;transport=tcp").unwrap();
        assert_eq!(uri.scheme, "sip");
        assert_eq!(uri.user, Some("alice".to_string()));
        assert_eq!(uri.host, "atlanta.example.com");
        assert_eq!(uri.port, Some(5060));
        assert_eq!(uri.transport(), Some("tcp"));
    }

    #[test]
    fn test_parse_sip_uri_no_user() {
        let uri = SipUri::parse("sip:atlanta.example.com").unwrap();
        assert_eq!(uri.user, None);
        assert_eq!(uri.host, "atlanta.example.com");
    }

    #[test]
    fn test_header_folding() {
        let msg = b"INVITE sip:bob@example.com SIP/2.0\r\nSubject: I know you're there,\r\n pick up the phone\r\n and talk to me!\r\nCall-ID: test123\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n";
        let parsed = SipMessage::parse(msg).unwrap();
        let subject = parsed.get_header("Subject").unwrap();
        assert!(subject.contains("pick up the phone"));
        assert!(subject.contains("and talk to me!"));
    }

    #[test]
    fn test_compact_headers() {
        let msg = b"INVITE sip:bob@example.com SIP/2.0\r\n\
v: SIP/2.0/UDP 10.0.0.1\r\n\
f: Alice <sip:alice@example.com>;tag=abc\r\n\
t: Bob <sip:bob@example.com>\r\n\
i: call123\r\n\
l: 0\r\n\
CSeq: 1 INVITE\r\n\
\r\n";
        let parsed = SipMessage::parse(msg).unwrap();
        assert_eq!(parsed.call_id(), Some("call123"));
        assert!(parsed.from_header().unwrap().contains("Alice"));
    }

    #[test]
    fn test_create_response() {
        let req = b"INVITE sip:bob@example.com SIP/2.0\r\n\
Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
From: <sip:alice@example.com>;tag=abc\r\n\
To: <sip:bob@example.com>\r\n\
Call-ID: testcall\r\n\
CSeq: 1 INVITE\r\n\
Content-Length: 0\r\n\
\r\n";
        let parsed = SipMessage::parse(req).unwrap();
        let resp = parsed.create_response(200, "OK").unwrap();
        assert_eq!(resp.status_code(), Some(200));
        assert_eq!(resp.call_id(), Some("testcall"));
    }

    // ---- IPv6 URI tests ----

    #[test]
    fn test_parse_ipv6_uri_with_brackets() {
        let uri = SipUri::parse("sip:user@[2001:db8::1]:5060").unwrap();
        assert_eq!(uri.scheme, "sip");
        assert_eq!(uri.user, Some("user".to_string()));
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, Some(5060));
        assert!(uri.is_ipv6_host());
    }

    #[test]
    fn test_parse_ipv6_uri_no_port() {
        let uri = SipUri::parse("sip:user@[::1]").unwrap();
        assert_eq!(uri.host, "::1");
        assert_eq!(uri.port, None);
        assert!(uri.is_ipv6_host());
    }

    #[test]
    fn test_parse_ipv6_uri_no_user() {
        let uri = SipUri::parse("sip:[2001:db8::1]:5060").unwrap();
        assert_eq!(uri.user, None);
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, Some(5060));
    }

    #[test]
    fn test_ipv6_uri_display() {
        let uri = SipUri::parse("sip:user@[2001:db8::1]:5060").unwrap();
        let displayed = uri.to_string();
        assert!(displayed.contains("[2001:db8::1]"));
        assert!(displayed.contains(":5060"));
    }

    #[test]
    fn test_ipv4_uri_not_ipv6() {
        let uri = SipUri::parse("sip:user@10.0.0.1:5060").unwrap();
        assert!(!uri.is_ipv6_host());
        let displayed = uri.to_string();
        assert!(!displayed.contains('['));
    }

    #[test]
    fn test_ipv6_host_display() {
        let uri = SipUri::parse("sip:user@[::1]").unwrap();
        assert_eq!(uri.host_display(), "[::1]");

        let uri4 = SipUri::parse("sip:user@10.0.0.1").unwrap();
        assert_eq!(uri4.host_display(), "10.0.0.1");
    }

    // ---- Retry-After tests ----

    #[test]
    fn test_retry_after_simple() {
        let ra = RetryAfter::parse("120").unwrap();
        assert_eq!(ra.seconds, 120);
        assert!(ra.comment.is_none());
        assert!(ra.duration.is_none());
    }

    #[test]
    fn test_retry_after_with_comment() {
        let ra = RetryAfter::parse("120 (I'm in a meeting)").unwrap();
        assert_eq!(ra.seconds, 120);
        assert_eq!(ra.comment.as_deref(), Some("I'm in a meeting"));
    }

    #[test]
    fn test_retry_after_with_duration() {
        let ra = RetryAfter::parse("120 ;duration=3600").unwrap();
        assert_eq!(ra.seconds, 120);
        assert_eq!(ra.duration, Some(3600));
    }

    #[test]
    fn test_retry_after_full() {
        let ra = RetryAfter::parse("120 (server overloaded) ;duration=3600").unwrap();
        assert_eq!(ra.seconds, 120);
        assert_eq!(ra.comment.as_deref(), Some("server overloaded"));
        assert_eq!(ra.duration, Some(3600));
    }

    #[test]
    fn test_retry_after_from_message() {
        let msg = SipMessage::parse(
            b"SIP/2.0 503 Service Unavailable\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:bob@example.com>;tag=def\r\n\
              Call-ID: retry-test\r\n\
              CSeq: 1 INVITE\r\n\
              Retry-After: 60 (system maintenance)\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let ra = RetryAfter::from_message(&msg).unwrap();
        assert_eq!(ra.seconds, 60);
        assert_eq!(ra.comment.as_deref(), Some("system maintenance"));
    }

    #[test]
    fn test_retry_after_empty() {
        assert!(RetryAfter::parse("").is_none());
        assert!(RetryAfter::parse("   ").is_none());
    }
}
