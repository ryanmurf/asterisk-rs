//! SIP message tracing utilities.
//!
//! Provides functions to inject and extract tracing context from SIP messages,
//! enabling distributed tracing across SIP hops.

use crate::parser::SipMessage;
use std::collections::HashMap;

/// Inject tracing headers into a SIP message.
/// 
/// This adds custom X-Trace-* headers to the SIP message that can be used
/// to propagate trace context across SIP hops.
///
/// # Arguments
/// 
/// * `message` - Mutable reference to the SIP message
/// * `trace_id` - W3C trace ID as a hex string
/// * `span_id` - W3C span ID as a hex string  
/// * `trace_flags` - W3C trace flags (typically 0x01 for sampled)
pub fn inject_trace_headers(
    message: &mut SipMessage, 
    trace_id: &str,
    span_id: &str, 
    trace_flags: u8
) {
    use crate::parser::SipHeader;
    
    // Add trace headers to the message
    message.headers.push(SipHeader {
        name: "X-Trace-Id".to_string(),
        value: trace_id.to_string(),
    });
    
    message.headers.push(SipHeader {
        name: "X-Span-Id".to_string(), 
        value: span_id.to_string(),
    });
    
    message.headers.push(SipHeader {
        name: "X-Trace-Flags".to_string(),
        value: trace_flags.to_string(),
    });
}

/// Extract tracing headers from a SIP message.
///
/// Extracts W3C trace context from custom X-Trace-* headers in the SIP message.
///
/// # Arguments
///
/// * `message` - Reference to the SIP message
///
/// # Returns
///
/// Returns a HashMap of trace headers if present, or None if no trace headers found.
pub fn extract_trace_headers(message: &SipMessage) -> Option<HashMap<String, String>> {
    let mut headers = HashMap::new();
    
    if let Some(trace_id) = message.get_header("X-Trace-Id") {
        headers.insert("X-Trace-Id".to_string(), trace_id.to_string());
    }
    
    if let Some(span_id) = message.get_header("X-Span-Id") {
        headers.insert("X-Span-Id".to_string(), span_id.to_string());
    }
    
    if let Some(trace_flags) = message.get_header("X-Trace-Flags") {
        headers.insert("X-Trace-Flags".to_string(), trace_flags.to_string());
    }
    
    if let Some(trace_state) = message.get_header("X-Trace-State") {
        headers.insert("X-Trace-State".to_string(), trace_state.to_string());
    }
    
    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

/// Create a unique span ID for SIP transactions.
///
/// Generates a unique identifier that can be used as a span ID for tracing.
pub fn generate_span_id() -> String {
    use std::fmt::Write;
    
    // Generate a random 64-bit span ID
    let mut rng_bytes = [0u8; 8];
    getrandom::getrandom(&mut rng_bytes).unwrap_or_else(|_| {
        // Fallback to system time if getrandom fails
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        rng_bytes.copy_from_slice(&nanos.to_le_bytes());
    });
    
    let span_id = u64::from_le_bytes(rng_bytes);
    
    // Format as 16-character hex string (8 bytes)
    let mut result = String::with_capacity(16);
    write!(&mut result, "{:016x}", span_id).unwrap();
    result
}

/// Create a unique trace ID for SIP calls.
///
/// Generates a unique identifier that can be used as a trace ID for tracing.
pub fn generate_trace_id() -> String {
    use std::fmt::Write;
    
    // Generate a random 128-bit trace ID
    let mut rng_bytes = [0u8; 16];
    getrandom::getrandom(&mut rng_bytes).unwrap_or_else(|_| {
        // Fallback to system time + random component if getrandom fails
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        
        let bytes1 = (nanos as u64).to_le_bytes();
        let bytes2 = ((nanos >> 64) as u64).to_le_bytes();
        
        rng_bytes[0..8].copy_from_slice(&bytes1);
        rng_bytes[8..16].copy_from_slice(&bytes2);
    });
    
    // Format as 32-character hex string (16 bytes)
    let mut result = String::with_capacity(32);
    for &byte in &rng_bytes {
        write!(&mut result, "{:02x}", byte).unwrap();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::*;

    #[test]
    fn test_trace_header_roundtrip() {
        // Create a simple SIP message
        let mut message = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri: SipUri::parse("sip:test@example.com").unwrap(),
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![],
            body: String::new(),
        };
        
        let trace_id = generate_trace_id();
        let span_id = generate_span_id();
        let trace_flags = 0x01;
        
        // Inject headers
        inject_trace_headers(&mut message, &trace_id, &span_id, trace_flags);
        
        // Extract headers
        let extracted = extract_trace_headers(&message).unwrap();
        
        assert_eq!(extracted.get("X-Trace-Id"), Some(&trace_id));
        assert_eq!(extracted.get("X-Span-Id"), Some(&span_id));
        assert_eq!(extracted.get("X-Trace-Flags"), Some(&"1".to_string()));
    }
    
    #[test]
    fn test_generate_ids() {
        let trace_id1 = generate_trace_id();
        let trace_id2 = generate_trace_id();
        let span_id1 = generate_span_id();
        let span_id2 = generate_span_id();
        
        // Should be different
        assert_ne!(trace_id1, trace_id2);
        assert_ne!(span_id1, span_id2);
        
        // Should be correct length
        assert_eq!(trace_id1.len(), 32);
        assert_eq!(span_id1.len(), 16);
        
        // Should be valid hex
        assert!(u128::from_str_radix(&trace_id1, 16).is_ok());
        assert!(u64::from_str_radix(&span_id1, 16).is_ok());
    }
}