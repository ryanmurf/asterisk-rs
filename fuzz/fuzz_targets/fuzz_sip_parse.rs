#![no_main]
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

/// Minimal SIP message representation for fuzzing
#[derive(Debug)]
pub struct SipMessage {
    pub method: Option<String>,
    pub uri: Option<String>,
    pub version: String,
    pub status_code: Option<u16>,
    pub reason_phrase: Option<String>,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Vec<u8>,
}

/// Parse error for SIP messages
#[derive(Debug)]
pub struct ParseError(pub String);

impl SipMessage {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let text = std::str::from_utf8(data)
            .map_err(|e| ParseError(format!("Invalid UTF-8: {}", e)))?;
        parse_message(text)
    }
}

fn parse_message(text: &str) -> Result<SipMessage, ParseError> {
    // Split headers from body at the blank line (\r\n\r\n or \n\n)
    let (header_section, body) = if let Some(pos) = text.find("\r\n\r\n") {
        (text[..pos].to_string(), text[pos + 4..].as_bytes())
    } else if let Some(pos) = text.find("\n\n") {
        (text[..pos].to_string(), text[pos + 2..].as_bytes())
    } else {
        (text.to_string(), &[] as &[u8])
    };

    let lines: Vec<&str> = header_section.lines().collect();
    if lines.is_empty() {
        return Err(ParseError("Empty message".to_string()));
    }

    let first_line = lines[0];
    let (method, uri, version, status_code, reason_phrase) = parse_first_line(first_line)?;

    // Parse headers with folding support
    let mut headers = HashMap::new();
    let mut current_header: Option<(String, String)> = None;

    for line in &lines[1..] {
        if line.is_empty() {
            break;
        }

        // Handle header folding (continuation lines start with whitespace)
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, ref mut value)) = current_header {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else {
            // Finish previous header
            if let Some((name, value)) = current_header.take() {
                headers.entry(name.to_lowercase()).or_insert_with(Vec::new).push(value);
            }

            // Parse new header
            if let Some(colon_pos) = line.find(':') {
                let name = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                current_header = Some((name, value));
            }
        }
    }

    // Don't forget the last header
    if let Some((name, value)) = current_header {
        headers.entry(name.to_lowercase()).or_insert_with(Vec::new).push(value);
    }

    Ok(SipMessage {
        method,
        uri,
        version,
        status_code,
        reason_phrase,
        headers,
        body: body.to_vec(),
    })
}

fn parse_first_line(line: &str) -> Result<(Option<String>, Option<String>, String, Option<u16>, Option<String>), ParseError> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    
    if parts.len() < 2 {
        return Err(ParseError("Invalid first line".to_string()));
    }

    // Check if it's a request (METHOD URI VERSION) or response (VERSION STATUS REASON)
    if parts[0].starts_with("SIP/") {
        // Response: SIP/2.0 200 OK
        if parts.len() < 3 {
            return Err(ParseError("Invalid response line".to_string()));
        }
        
        let version = parts[0].to_string();
        let status_code = parts[1].parse::<u16>()
            .map_err(|_| ParseError("Invalid status code".to_string()))?;
        let reason_phrase = parts[2..].join(" ");
        
        Ok((None, None, version, Some(status_code), Some(reason_phrase)))
    } else {
        // Request: INVITE sip:user@example.com SIP/2.0
        if parts.len() < 3 {
            return Err(ParseError("Invalid request line".to_string()));
        }
        
        let method = parts[0].to_string();
        let uri = parts[1].to_string();
        let version = parts[2].to_string();
        
        Ok((Some(method), Some(uri), version, None, None))
    }
}

fuzz_target!(|data: &[u8]| {
    // Ensure we don't panic on any input
    let _ = SipMessage::parse(data);
});