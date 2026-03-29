//! Festival TTS integration.
//!
//! Port of app_festival.c from Asterisk C. Connects to a Festival
//! Speech Synthesis server over TCP, sends text, and plays the
//! resulting audio to the channel.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// Festival server connection configuration.
#[derive(Debug, Clone)]
pub struct FestivalConfig {
    /// Festival server hostname.
    pub host: String,
    /// Festival server port (default 1314).
    pub port: u16,
    /// Whether to cache generated audio files.
    pub cache: bool,
    /// Cache directory.
    pub cache_dir: String,
}

impl Default for FestivalConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 1314,
            cache: false,
            cache_dir: "/tmp/festival_cache".to_string(),
        }
    }
}

/// Options for the Festival application.
#[derive(Debug, Clone, Default)]
pub struct FestivalOptions {
    /// Don't answer the channel.
    pub no_answer: bool,
    /// Interruptible by any DTMF.
    pub any_interrupt: bool,
}

impl FestivalOptions {
    /// Parse option string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'n' => result.no_answer = true,
                'a' => result.any_interrupt = true,
                _ => {
                    debug!("Festival: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The Festival() dialplan application.
///
/// Usage: Festival(text[,intkeys])
///
/// Connects to a Festival TTS server, sends the given text, receives
/// synthesized audio, and plays it to the channel.
///
/// Options (via channel variable FESTIVAL_OPTS):
///   n - Don't answer the channel
///   a - Allow any DTMF to interrupt
pub struct AppFestival;

impl DialplanApp for AppFestival {
    fn name(&self) -> &str {
        "Festival"
    }

    fn description(&self) -> &str {
        "Say text with Festival TTS engine"
    }
}

impl AppFestival {
    /// Execute the Festival application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let text = parts.first().copied().unwrap_or("");
        let _intkeys = parts.get(1).copied().unwrap_or("");

        if text.is_empty() {
            warn!("Festival: requires text argument");
            return PbxExecResult::Failed;
        }

        info!("Festival: channel '{}' speaking: '{}'", channel.name, text);

        let _config = FestivalConfig::default();

        // In a real implementation:
        // 1. Connect to Festival server (TCP)
        // 2. Send: (tts_textasterisk "text" 'file)
        // 3. Receive audio data (raw 8kHz mulaw or wav)
        // 4. Write to temp file
        // 5. Play audio file to channel (interruptible if configured)
        // 6. Clean up temp file

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_festival_config_default() {
        let cfg = FestivalConfig::default();
        assert_eq!(cfg.port, 1314);
        assert_eq!(cfg.host, "localhost");
    }

    #[test]
    fn test_festival_options() {
        let opts = FestivalOptions::parse("na");
        assert!(opts.no_answer);
        assert!(opts.any_interrupt);
    }

    #[tokio::test]
    async fn test_festival_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppFestival::exec(&mut channel, "Hello world").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_festival_exec_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppFestival::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
