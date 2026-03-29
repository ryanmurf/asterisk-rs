//! AEAP (Asterisk External Application Protocol) speech engine interface.
//!
//! Port of `res/res_speech_aeap.c`. Provides a speech recognition engine
//! that delegates to an external application via the AEAP WebSocket
//! protocol. The external application handles the actual speech-to-text
//! processing.

use std::collections::HashMap;

use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// AEAP speech protocol version.
pub const SPEECH_AEAP_VERSION: &str = "0.1.0";

/// AEAP protocol name for speech-to-text.
pub const SPEECH_PROTOCOL: &str = "speech_to_text";

/// Default connection timeout in milliseconds.
pub const CONNECTION_TIMEOUT_MS: u64 = 2000;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AeapSpeechError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("request timeout")]
    Timeout,
    #[error("protocol error: {0}")]
    ProtocolError(String),
    #[error("AEAP speech error: {0}")]
    Other(String),
}

pub type AeapSpeechResult<T> = Result<T, AeapSpeechError>;

// ---------------------------------------------------------------------------
// AEAP speech engine configuration
// ---------------------------------------------------------------------------

/// Configuration for an AEAP speech engine instance.
///
/// Loaded from sorcery configuration (speech.conf).
#[derive(Debug, Clone)]
pub struct AeapSpeechConfig {
    /// Engine name.
    pub name: String,
    /// WebSocket server URL for the AEAP endpoint.
    pub server_url: String,
    /// AEAP codec/format to use.
    pub codec: String,
    /// Connection timeout in milliseconds.
    pub timeout_ms: u64,
    /// Custom parameters passed to the external application.
    pub custom_params: HashMap<String, String>,
}

impl AeapSpeechConfig {
    pub fn new(name: &str, server_url: &str) -> Self {
        Self {
            name: name.to_string(),
            server_url: server_url.to_string(),
            codec: "slin".to_string(),
            timeout_ms: CONNECTION_TIMEOUT_MS,
            custom_params: HashMap::new(),
        }
    }

    /// Add a custom parameter.
    pub fn set_param(&mut self, key: &str, value: &str) {
        self.custom_params.insert(key.to_string(), value.to_string());
    }
}

// ---------------------------------------------------------------------------
// AEAP message types
// ---------------------------------------------------------------------------

/// AEAP request types for speech operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeapSpeechRequest {
    /// Setup/create a new speech session.
    Setup,
    /// Load a grammar.
    LoadGrammar,
    /// Unload a grammar.
    UnloadGrammar,
    /// Get recognition results.
    GetResults,
    /// Set an engine parameter.
    Set,
    /// Get an engine parameter.
    Get,
}

impl AeapSpeechRequest {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::LoadGrammar => "load_grammar",
            Self::UnloadGrammar => "unload_grammar",
            Self::GetResults => "get_results",
            Self::Set => "set",
            Self::Get => "get",
        }
    }
}

// ---------------------------------------------------------------------------
// AEAP speech session (stub)
// ---------------------------------------------------------------------------

/// An AEAP speech session connected to an external application.
///
/// Stub implementation - actual WebSocket communication requires
/// async runtime integration.
#[derive(Debug)]
pub struct AeapSpeechSession {
    /// Configuration.
    pub config: AeapSpeechConfig,
    /// Whether the session is connected.
    pub connected: bool,
}

impl AeapSpeechSession {
    /// Create a new session (not yet connected).
    pub fn new(config: AeapSpeechConfig) -> Self {
        Self {
            config,
            connected: false,
        }
    }

    /// Connect to the AEAP server.
    ///
    /// Stub - requires WebSocket client implementation.
    pub fn connect(&mut self) -> AeapSpeechResult<()> {
        debug!(
            url = %self.config.server_url,
            engine = %self.config.name,
            "AEAP speech connect (stub)"
        );
        Err(AeapSpeechError::ConnectionFailed(
            "WebSocket client not implemented".to_string(),
        ))
    }

    /// Send audio data to the AEAP server.
    pub fn send_audio(&self, _data: &[u8]) -> AeapSpeechResult<()> {
        if !self.connected {
            return Err(AeapSpeechError::ConnectionFailed("not connected".into()));
        }
        Ok(())
    }

    /// Disconnect from the AEAP server.
    pub fn disconnect(&mut self) {
        self.connected = false;
        debug!(engine = %self.config.name, "AEAP speech disconnected");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config() {
        let mut config = AeapSpeechConfig::new("my-engine", "ws://localhost:9099/speech");
        config.set_param("language", "en-US");
        assert_eq!(config.name, "my-engine");
        assert_eq!(config.custom_params.get("language").unwrap(), "en-US");
    }

    #[test]
    fn test_request_types() {
        assert_eq!(AeapSpeechRequest::Setup.as_str(), "setup");
        assert_eq!(AeapSpeechRequest::GetResults.as_str(), "get_results");
    }

    #[test]
    fn test_session_not_connected() {
        let config = AeapSpeechConfig::new("test", "ws://localhost:9099");
        let session = AeapSpeechSession::new(config);
        assert!(!session.connected);
        assert!(session.send_audio(&[0u8; 320]).is_err());
    }

    #[test]
    fn test_connect_stub() {
        let config = AeapSpeechConfig::new("test", "ws://localhost:9099");
        let mut session = AeapSpeechSession::new(config);
        assert!(session.connect().is_err());
    }
}
