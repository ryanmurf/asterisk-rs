#![no_main]
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

/// Minimal SDP session description for fuzzing
#[derive(Debug, Default)]
pub struct SessionDescription {
    pub version: u8,
    pub origin: Option<String>,
    pub session_name: Option<String>,
    pub connection: Option<String>,
    pub timing: Vec<String>,
    pub media: Vec<MediaDescription>,
    pub attributes: HashMap<String, Option<String>>,
}

#[derive(Debug)]
pub struct MediaDescription {
    pub media_type: String,
    pub port: u16,
    pub protocol: String,
    pub formats: Vec<String>,
    pub connection: Option<String>,
    pub attributes: HashMap<String, Option<String>>,
}

#[derive(Debug)]
pub struct SdpError(pub String);

impl SessionDescription {
    pub fn parse(text: &str) -> Result<Self, SdpError> {
        let mut sdp = SessionDescription::default();
        let mut current_media: Option<MediaDescription> = None;

        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.len() < 2 || line.as_bytes()[1] != b'=' {
                continue;
            }

            let field_type = line.as_bytes()[0] as char;
            let value = &line[2..];

            match field_type {
                'v' => {
                    // Version
                    sdp.version = value.parse().unwrap_or(0);
                }
                'o' => {
                    // Origin
                    sdp.origin = Some(value.to_string());
                }
                's' => {
                    // Session name
                    sdp.session_name = Some(value.to_string());
                }
                'c' => {
                    // Connection
                    if let Some(ref mut media) = current_media {
                        media.connection = Some(value.to_string());
                    } else {
                        sdp.connection = Some(value.to_string());
                    }
                }
                't' => {
                    // Timing
                    sdp.timing.push(value.to_string());
                }
                'm' => {
                    // Media description
                    // Save previous media if exists
                    if let Some(media) = current_media.take() {
                        sdp.media.push(media);
                    }

                    // Parse media line: "audio 5004 RTP/AVP 0"
                    let parts: Vec<&str> = value.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let media_type = parts[0].to_string();
                        let port = parts[1].parse().unwrap_or(0);
                        let protocol = parts[2].to_string();
                        let formats = parts[3..].iter().map(|s| s.to_string()).collect();

                        current_media = Some(MediaDescription {
                            media_type,
                            port,
                            protocol,
                            formats,
                            connection: None,
                            attributes: HashMap::new(),
                        });
                    }
                }
                'a' => {
                    // Attribute
                    let (name, value) = if let Some(colon_pos) = value.find(':') {
                        (value[..colon_pos].to_string(), Some(value[colon_pos + 1..].to_string()))
                    } else {
                        (value.to_string(), None)
                    };

                    if let Some(ref mut media) = current_media {
                        media.attributes.insert(name, value);
                    } else {
                        sdp.attributes.insert(name, value);
                    }
                }
                _ => {
                    // Unknown field, ignore for fuzzing
                }
            }
        }

        // Don't forget the last media description
        if let Some(media) = current_media {
            sdp.media.push(media);
        }

        Ok(sdp)
    }
}

fuzz_target!(|data: &[u8]| {
    // Convert bytes to string for SDP parsing
    if let Ok(text) = std::str::from_utf8(data) {
        // Ensure we don't panic on any input
        let _ = SessionDescription::parse(text);
    }
});