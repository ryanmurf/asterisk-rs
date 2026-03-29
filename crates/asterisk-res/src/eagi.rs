//! Enhanced AGI (EAGI) - AGI with live audio access.
//!
//! Port of the EAGI portions of res_agi.c from Asterisk C.
//!
//! EAGI extends AGI by providing the script with a readable file descriptor
//! (fd 3) that streams the audio from the channel in real time. This allows
//! AGI scripts to perform custom audio processing (speech recognition,
//! DTMF detection, recording, etc.) while controlling the dialplan.
//!
//! The audio format on fd 3 is signed linear 16-bit, 8000 Hz, mono.

use crate::agi::{AgiEnvironment, AgiError, AgiResponse, AgiResult};
use std::collections::HashMap;
use tracing::{debug, info};

/// Audio format used on the EAGI audio pipe (fd 3).
pub const EAGI_AUDIO_FORMAT: &str = "slin";
/// Sample rate for EAGI audio (8000 Hz).
pub const EAGI_SAMPLE_RATE: u32 = 8000;
/// Bits per sample (16-bit signed linear).
pub const EAGI_BITS_PER_SAMPLE: u8 = 16;
/// Bytes per sample.
pub const EAGI_BYTES_PER_SAMPLE: u8 = 2;
/// Bytes per 20ms frame at 8kHz/16bit.
pub const EAGI_FRAME_SIZE: usize = 320;

/// The file descriptor number used for EAGI audio.
pub const EAGI_AUDIO_FD: i32 = 3;

/// EAGI session extending a standard AGI session.
///
/// Includes all AGI commands plus access to the live audio stream
/// via the audio pipe.
#[derive(Debug)]
pub struct EagiSession {
    /// AGI environment variables
    pub env: AgiEnvironment,
    /// Whether the audio pipe is active
    pub audio_active: bool,
    /// Accumulated audio bytes for testing
    audio_buffer: Vec<u8>,
    /// Pending commands
    command_queue: Vec<String>,
    /// Responses for queued commands
    response_queue: Vec<AgiResponse>,
}

impl EagiSession {
    /// Create a new EAGI session.
    pub fn new(env: AgiEnvironment) -> Self {
        info!(
            "Starting EAGI session for channel '{}' with audio on fd {}",
            env.channel, EAGI_AUDIO_FD,
        );
        Self {
            env,
            audio_active: true,
            audio_buffer: Vec::new(),
            command_queue: Vec::new(),
            response_queue: Vec::new(),
        }
    }

    /// Check if this is an EAGI session (always true).
    pub fn is_enhanced(&self) -> bool {
        true
    }

    /// Read audio data from the audio pipe.
    ///
    /// In production, this reads from fd 3. Here we return from the buffer.
    /// Returns the number of bytes read, or 0 if no data available.
    pub fn read_audio(&mut self, buf: &mut [u8]) -> AgiResult<usize> {
        if !self.audio_active {
            return Err(AgiError::Protocol("EAGI audio pipe not active".into()));
        }

        let available = self.audio_buffer.len().min(buf.len());
        if available == 0 {
            return Ok(0);
        }

        buf[..available].copy_from_slice(&self.audio_buffer[..available]);
        self.audio_buffer.drain(..available);
        debug!("EAGI: read {} audio bytes", available);
        Ok(available)
    }

    /// Feed audio data into the session (for testing/simulation).
    pub fn feed_audio(&mut self, data: &[u8]) {
        self.audio_buffer.extend_from_slice(data);
    }

    /// Stop the audio pipe.
    pub fn stop_audio(&mut self) {
        self.audio_active = false;
        self.audio_buffer.clear();
        debug!("EAGI: audio pipe stopped");
    }

    /// Send an AGI command and get the response.
    ///
    /// This is the same as standard AGI but the audio pipe remains active
    /// during command execution.
    pub fn execute_command(&mut self, command: &str) -> AgiResult<AgiResponse> {
        debug!("EAGI command: {}", command);
        self.command_queue.push(command.to_string());

        // In production, this would write to stdin and read from stdout.
        // For the port, return a queued response or default success.
        if let Some(response) = self.response_queue.pop() {
            Ok(response)
        } else {
            Ok(AgiResponse::success("0"))
        }
    }

    /// Queue a response for the next command (for testing).
    pub fn queue_response(&mut self, response: AgiResponse) {
        self.response_queue.push(response);
    }

    /// Get the list of executed commands (for testing).
    pub fn executed_commands(&self) -> &[String] {
        &self.command_queue
    }

    /// Get the size of buffered audio data.
    pub fn audio_buffer_size(&self) -> usize {
        self.audio_buffer.len()
    }
}

/// Generate the EAGI environment variables.
///
/// These are the same as standard AGI variables plus EAGI-specific ones.
pub fn eagi_env_vars(env: &AgiEnvironment) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("agi_enhanced".to_string(), "1.0".to_string());
    vars.insert("agi_channel".to_string(), env.channel.clone());
    vars.insert("agi_uniqueid".to_string(), env.uniqueid.clone());
    vars.insert("agi_callerid".to_string(), env.callerid.clone());
    vars.insert("agi_calleridname".to_string(), env.calleridname.clone());
    vars.insert("agi_context".to_string(), env.context.clone());
    vars.insert("agi_extension".to_string(), env.extension.clone());
    vars.insert("agi_priority".to_string(), env.priority.to_string());
    vars.insert("agi_type".to_string(), env.channel_type.clone());
    vars.insert("agi_accountcode".to_string(), env.accountcode.clone());
    vars.insert("agi_request".to_string(), env.request.clone());
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_env() -> AgiEnvironment {
        AgiEnvironment {
            request: "agi://localhost/test.agi".to_string(),
            channel: "SIP/alice-001".to_string(),
            language: "en".to_string(),
            channel_type: "SIP".to_string(),
            uniqueid: "12345.1".to_string(),
            version: "0.1.0".to_string(),
            callerid: "1001".to_string(),
            calleridname: "Alice".to_string(),
            callingpres: "0".to_string(),
            callingani2: "0".to_string(),
            callington: "0".to_string(),
            callingtns: "0".to_string(),
            dnid: String::new(),
            rdnis: String::new(),
            context: "default".to_string(),
            extension: "100".to_string(),
            priority: "1".to_string(),
            enhanced: "1.0".to_string(),
            accountcode: String::new(),
            threadid: String::new(),
            arguments: Vec::new(),
            extra: HashMap::new(),
        }
    }

    #[test]
    fn test_eagi_session_creation() {
        let session = EagiSession::new(test_env());
        assert!(session.is_enhanced());
        assert!(session.audio_active);
    }

    #[test]
    fn test_eagi_audio_read_write() {
        let mut session = EagiSession::new(test_env());

        // Feed 320 bytes (one frame) of silence
        let silence = vec![0u8; EAGI_FRAME_SIZE];
        session.feed_audio(&silence);
        assert_eq!(session.audio_buffer_size(), EAGI_FRAME_SIZE);

        // Read the audio
        let mut buf = vec![0u8; EAGI_FRAME_SIZE];
        let read = session.read_audio(&mut buf).unwrap();
        assert_eq!(read, EAGI_FRAME_SIZE);
        assert_eq!(session.audio_buffer_size(), 0);
    }

    #[test]
    fn test_eagi_audio_partial_read() {
        let mut session = EagiSession::new(test_env());
        session.feed_audio(&[1u8; 100]);

        let mut buf = vec![0u8; 50];
        let read = session.read_audio(&mut buf).unwrap();
        assert_eq!(read, 50);
        assert_eq!(session.audio_buffer_size(), 50);
    }

    #[test]
    fn test_eagi_stop_audio() {
        let mut session = EagiSession::new(test_env());
        session.feed_audio(&[0u8; 100]);
        session.stop_audio();
        assert!(!session.audio_active);

        let mut buf = vec![0u8; 100];
        assert!(session.read_audio(&mut buf).is_err());
    }

    #[test]
    fn test_eagi_command_execution() {
        let mut session = EagiSession::new(test_env());
        let response = session.execute_command("STREAM FILE hello \"\"").unwrap();
        assert_eq!(response.code, 200);
        assert_eq!(session.executed_commands().len(), 1);
    }

    #[test]
    fn test_eagi_env_vars() {
        let env = test_env();
        let vars = eagi_env_vars(&env);
        assert_eq!(vars.get("agi_enhanced").unwrap(), "1.0");
        assert_eq!(vars.get("agi_channel").unwrap(), "SIP/alice-001");
        assert_eq!(vars.get("agi_callerid").unwrap(), "1001");
    }
}
