//! AMI protocol types and parsing.
//!
//! The AMI protocol is a simple text-based protocol where messages consist
//! of key-value header lines terminated by `\r\n`, with a blank `\r\n` line
//! separating messages. This module provides types for actions, responses,
//! and events, plus parsing logic.

use std::collections::HashMap;
use std::fmt;

/// An AMI action received from a client.
///
/// Actions are requests from external managers to Asterisk.
/// Every action has a name and a set of key-value headers.
#[derive(Debug, Clone)]
pub struct AmiAction {
    /// The action name (e.g., "Login", "Originate", "Hangup").
    pub name: String,
    /// The action ID (optional, echoed in the response).
    pub action_id: Option<String>,
    /// Key-value headers sent with the action.
    pub headers: HashMap<String, String>,
}

impl AmiAction {
    /// Create a new empty action.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action_id: None,
            headers: HashMap::new(),
        }
    }

    /// Set a header on this action.
    pub fn set_header(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if key.eq_ignore_ascii_case("ActionID") {
            self.action_id = Some(value.clone());
        }
        self.headers.insert(key, value);
    }

    /// Get a header value (case-insensitive key lookup).
    pub fn get_header(&self, key: &str) -> Option<&str> {
        // Try exact match first, then case-insensitive
        if let Some(v) = self.headers.get(key) {
            return Some(v.as_str());
        }
        for (k, v) in &self.headers {
            if k.eq_ignore_ascii_case(key) {
                return Some(v.as_str());
            }
        }
        None
    }

    /// Parse an AMI action from raw protocol bytes.
    ///
    /// The input should be a complete message (all lines up to the blank line).
    pub fn parse(input: &str) -> Option<Self> {
        let mut action = None;
        let mut headers = HashMap::new();
        let mut action_id = None;

        for line in input.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                break;
            }

            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();

                if key.eq_ignore_ascii_case("Action") {
                    action = Some(value.clone());
                }
                if key.eq_ignore_ascii_case("ActionID") {
                    action_id = Some(value.clone());
                }
                headers.insert(key, value);
            }
        }

        let name = action?;
        Some(Self {
            name,
            action_id,
            headers,
        })
    }

    /// Serialize this action to AMI wire format.
    pub fn serialize(&self) -> String {
        let mut buf = format!("Action: {}\r\n", self.name);
        for (key, value) in &self.headers {
            if !key.eq_ignore_ascii_case("Action") {
                buf.push_str(&format!("{}: {}\r\n", key, value));
            }
        }
        buf.push_str("\r\n");
        buf
    }
}

/// An AMI response sent from the server to a client.
#[derive(Debug, Clone)]
pub struct AmiResponse {
    /// Whether the action succeeded.
    pub success: bool,
    /// The response message.
    pub message: String,
    /// The action ID (echoed from the request).
    pub action_id: Option<String>,
    /// Additional key-value headers in the response.
    pub headers: HashMap<String, String>,
    /// Multi-line output data (for actions like "Command").
    pub output: Vec<String>,
}

impl AmiResponse {
    /// Create a success response.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            action_id: None,
            headers: HashMap::new(),
            output: Vec::new(),
        }
    }

    /// Create an error response.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            action_id: None,
            headers: HashMap::new(),
            output: Vec::new(),
        }
    }

    /// Set the action ID on the response.
    pub fn with_action_id(mut self, id: Option<String>) -> Self {
        self.action_id = id;
        self
    }

    /// Add a header to the response.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Add output lines to the response (for Command action).
    pub fn with_output(mut self, lines: Vec<String>) -> Self {
        self.output = lines;
        self
    }

    /// Serialize this response to AMI wire format.
    pub fn serialize(&self) -> String {
        let mut buf = if self.success {
            "Response: Success\r\n".to_string()
        } else {
            "Response: Error\r\n".to_string()
        };

        if let Some(ref id) = self.action_id {
            buf.push_str(&format!("ActionID: {}\r\n", id));
        }

        if !self.message.is_empty() {
            buf.push_str(&format!("Message: {}\r\n", self.message));
        }

        for (key, value) in &self.headers {
            buf.push_str(&format!("{}: {}\r\n", key, value));
        }

        // Output lines for Command responses
        if !self.output.is_empty() {
            buf.push_str("Output: follows\r\n");
            for line in &self.output {
                buf.push_str(line);
                buf.push_str("\r\n");
            }
            buf.push_str("--END COMMAND--\r\n");
        }

        buf.push_str("\r\n");
        buf
    }
}

/// An AMI event sent from the server to clients.
///
/// Events are asynchronous notifications about state changes in Asterisk
/// (new channels, hangups, bridge events, etc.).
#[derive(Debug, Clone)]
pub struct AmiEvent {
    /// The event name (e.g., "Newchannel", "Hangup", "Bridge").
    pub name: String,
    /// The event category for filtering.
    pub category: u32,
    /// Key-value headers in the event.
    pub headers: HashMap<String, String>,
}

impl AmiEvent {
    /// Create a new event.
    pub fn new(name: impl Into<String>, category: u32) -> Self {
        Self {
            name: name.into(),
            category,
            headers: HashMap::new(),
        }
    }

    /// Create a new event with a set of headers in one go.
    ///
    /// Convenience constructor used by channel lifecycle event emitters.
    /// The category defaults to `SYSTEM | CALL | USER` (0x43) so that the
    /// event passes through any session whose read permission includes at
    /// least one of those categories.  Call sites that need a specific
    /// category should use [`Self::new`] + [`Self::with_header`] instead,
    /// or the dedicated [`Self::new_with_headers_cat`] constructor.
    pub fn new_with_headers(name: impl Into<String>, headers: &[(&str, &str)]) -> Self {
        // Default category covers system + call + user so that events like
        // FullyBooted, Newchannel, and UserEvent all pass the filter.
        let mut event = Self::new(name, 0x43);
        for &(k, v) in headers {
            event.headers.insert(k.to_string(), v.to_string());
        }
        event
    }

    /// Like [`new_with_headers`] but allows specifying an explicit category.
    pub fn new_with_headers_cat(name: impl Into<String>, category: u32, headers: &[(&str, &str)]) -> Self {
        let mut event = Self::new(name, category);
        for &(k, v) in headers {
            event.headers.insert(k.to_string(), v.to_string());
        }
        event
    }

    /// Add a header to the event (mutable, non-consuming).
    pub fn add_header(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.headers.insert(key.into(), value.into());
    }

    /// Add a header to the event.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Serialize this event to AMI wire format.
    pub fn serialize(&self) -> String {
        let mut buf = format!("Event: {}\r\n", self.name);
        for (key, value) in &self.headers {
            buf.push_str(&format!("{}: {}\r\n", key, value));
        }
        buf.push_str("\r\n");
        buf
    }

    /// Parse an AMI event from raw text.
    pub fn parse(input: &str) -> Option<Self> {
        let mut event_name = None;
        let mut headers = HashMap::new();

        for line in input.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                break;
            }

            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();

                if key.eq_ignore_ascii_case("Event") {
                    event_name = Some(value.clone());
                }
                headers.insert(key, value);
            }
        }

        let name = event_name?;
        Some(Self {
            name,
            category: 0,
            headers,
        })
    }
}

impl fmt::Display for AmiAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Action({})", self.name)
    }
}

impl fmt::Display for AmiResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Response({}, {})",
            if self.success { "Success" } else { "Error" },
            self.message
        )
    }
}

impl fmt::Display for AmiEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Event({})", self.name)
    }
}

/// Read a complete AMI message from a buffered input.
///
/// Returns the message text and the number of bytes consumed.
/// Returns None if a complete message is not yet available.
pub fn read_message(buffer: &str) -> Option<(&str, usize)> {
    // Look for the double CRLF that terminates a message
    if let Some(pos) = buffer.find("\r\n\r\n") {
        let end = pos + 4;
        Some((&buffer[..end], end))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_action() {
        let input = "Action: Login\r\nUsername: admin\r\nSecret: password\r\nActionID: 1234\r\n\r\n";
        let action = AmiAction::parse(input).unwrap();
        assert_eq!(action.name, "Login");
        assert_eq!(action.action_id.as_deref(), Some("1234"));
        assert_eq!(action.get_header("Username"), Some("admin"));
        assert_eq!(action.get_header("Secret"), Some("password"));
    }

    #[test]
    fn test_parse_action_case_insensitive() {
        let input = "Action: Ping\r\nactionid: 42\r\n\r\n";
        let action = AmiAction::parse(input).unwrap();
        assert_eq!(action.name, "Ping");
        assert_eq!(action.get_header("ActionID"), Some("42"));
    }

    #[test]
    fn test_parse_action_empty() {
        assert!(AmiAction::parse("").is_none());
        assert!(AmiAction::parse("NoAction: value\r\n\r\n").is_none());
    }

    #[test]
    fn test_serialize_action() {
        let mut action = AmiAction::new("Ping");
        action.set_header("ActionID", "123");
        let serialized = action.serialize();
        assert!(serialized.starts_with("Action: Ping\r\n"));
        assert!(serialized.ends_with("\r\n\r\n"));
        assert!(serialized.contains("ActionID: 123\r\n"));
    }

    #[test]
    fn test_serialize_success_response() {
        let resp = AmiResponse::success("Authentication accepted")
            .with_action_id(Some("42".to_string()));
        let s = resp.serialize();
        assert!(s.contains("Response: Success\r\n"));
        assert!(s.contains("ActionID: 42\r\n"));
        assert!(s.contains("Message: Authentication accepted\r\n"));
        assert!(s.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_serialize_error_response() {
        let resp = AmiResponse::error("Permission denied");
        let s = resp.serialize();
        assert!(s.contains("Response: Error\r\n"));
        assert!(s.contains("Message: Permission denied\r\n"));
    }

    #[test]
    fn test_serialize_response_with_output() {
        let resp = AmiResponse::success("Command output follows")
            .with_output(vec!["line1".to_string(), "line2".to_string()]);
        let s = resp.serialize();
        assert!(s.contains("Output: follows\r\n"));
        assert!(s.contains("line1\r\n"));
        assert!(s.contains("--END COMMAND--\r\n"));
    }

    #[test]
    fn test_serialize_event() {
        let event = AmiEvent::new("Newchannel", 0x01)
            .with_header("Channel", "SIP/1234-00000001")
            .with_header("ChannelState", "6");
        let s = event.serialize();
        assert!(s.starts_with("Event: Newchannel\r\n"));
        assert!(s.contains("Channel: SIP/1234-00000001\r\n"));
        assert!(s.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_parse_event() {
        let input = "Event: Hangup\r\nChannel: SIP/test-001\r\nCause: 16\r\n\r\n";
        let event = AmiEvent::parse(input).unwrap();
        assert_eq!(event.name, "Hangup");
        assert_eq!(event.headers.get("Channel").unwrap(), "SIP/test-001");
        assert_eq!(event.headers.get("Cause").unwrap(), "16");
    }

    #[test]
    fn test_read_message() {
        let buf = "Action: Ping\r\nActionID: 1\r\n\r\nAction: Logoff\r\n\r\n";
        let (msg, consumed) = read_message(buf).unwrap();
        assert_eq!(msg, "Action: Ping\r\nActionID: 1\r\n\r\n");
        assert_eq!(consumed, 29);

        // Parse the remaining part
        let remaining = &buf[consumed..];
        let (msg2, _) = read_message(remaining).unwrap();
        assert_eq!(msg2, "Action: Logoff\r\n\r\n");
    }

    #[test]
    fn test_read_message_incomplete() {
        let buf = "Action: Ping\r\nActionID: 1\r\n";
        assert!(read_message(buf).is_none());
    }
}
