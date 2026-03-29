//! Controllable playback application.
//!
//! Port of app_controlplayback.c from Asterisk C. Plays back a sound
//! file with DTMF controls for fast-forward, rewind, stop, pause,
//! restart, and reverse.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// DTMF key bindings for playback control.
#[derive(Debug, Clone)]
pub struct ControlKeys {
    /// DTMF digit to skip forward (default: #).
    pub forward: char,
    /// DTMF digit to rewind (default: *).
    pub rewind: char,
    /// DTMF digit to stop playback (default: none).
    pub stop: Option<char>,
    /// DTMF digit to pause/resume (default: none).
    pub pause: Option<char>,
    /// DTMF digit to restart from beginning (default: none).
    pub restart: Option<char>,
    /// DTMF digit to reverse playback direction (default: none).
    pub reverse: Option<char>,
}

impl Default for ControlKeys {
    fn default() -> Self {
        Self {
            forward: '#',
            rewind: '*',
            stop: None,
            pause: None,
            restart: None,
            reverse: None,
        }
    }
}

/// Options for the ControlPlayback application.
#[derive(Debug, Clone)]
pub struct ControlPlaybackOptions {
    /// File to play.
    pub filename: String,
    /// Skip duration in milliseconds (default: 3000).
    pub skip_ms: u32,
    /// DTMF key bindings.
    pub keys: ControlKeys,
    /// Starting offset in milliseconds.
    pub offset_ms: u32,
}

impl ControlPlaybackOptions {
    /// Parse from pipe-separated arguments.
    ///
    /// Format: filename[,skipms[,ff[,rew[,stop[,pause[,restart[,reverse]]]]]]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.split(',').collect();
        let filename = parts.first().copied().unwrap_or("").to_string();
        let skip_ms = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(3000);

        let ff = parts.get(2).and_then(|s| s.chars().next()).unwrap_or('#');
        let rew = parts.get(3).and_then(|s| s.chars().next()).unwrap_or('*');
        let stop = parts.get(4).and_then(|s| s.chars().next());
        let pause = parts.get(5).and_then(|s| s.chars().next());
        let restart = parts.get(6).and_then(|s| s.chars().next());
        let reverse = parts.get(7).and_then(|s| s.chars().next());

        Self {
            filename,
            skip_ms,
            keys: ControlKeys {
                forward: ff,
                rewind: rew,
                stop,
                pause,
                restart,
                reverse,
            },
            offset_ms: 0,
        }
    }
}

/// The ControlPlayback() dialplan application.
///
/// Usage: ControlPlayback(file[,skipms[,ff[,rew[,stop[,pause[,restart[,reverse]]]]]]])
///
/// Plays back a sound file with DTMF-based controls.
///
/// Sets CPLAYBACKSTATUS:
///   SUCCESS   - Playback completed normally
///   USERSTOPPED - User pressed stop key
///   ERROR     - Playback failed
pub struct AppControlPlayback;

impl DialplanApp for AppControlPlayback {
    fn name(&self) -> &str {
        "ControlPlayback"
    }

    fn description(&self) -> &str {
        "Play a file with FF, REW, and control via DTMF"
    }
}

impl AppControlPlayback {
    /// Execute the ControlPlayback application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = ControlPlaybackOptions::parse(args);

        if options.filename.is_empty() {
            warn!("ControlPlayback: requires filename argument");
            return PbxExecResult::Failed;
        }

        info!(
            "ControlPlayback: channel '{}' playing '{}' (skip={}ms)",
            channel.name, options.filename, options.skip_ms,
        );

        // In a real implementation:
        // 1. Answer channel if not already up
        // 2. Start streaming audio file
        // 3. Monitor for DTMF:
        //    - forward key: skip ahead by skip_ms
        //    - rewind key: skip back by skip_ms
        //    - stop key: stop playback, set CPLAYBACKSTATUS=USERSTOPPED
        //    - pause key: pause/resume
        //    - restart key: seek to beginning
        //    - reverse key: toggle reverse playback
        // 4. On completion, set CPLAYBACKSTATUS=SUCCESS

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controlplayback_options_default() {
        let opts = ControlPlaybackOptions::parse("hello-world");
        assert_eq!(opts.filename, "hello-world");
        assert_eq!(opts.skip_ms, 3000);
        assert_eq!(opts.keys.forward, '#');
        assert_eq!(opts.keys.rewind, '*');
    }

    #[test]
    fn test_controlplayback_options_custom() {
        let opts = ControlPlaybackOptions::parse("hello-world,5000,6,4,5,7,8,9");
        assert_eq!(opts.skip_ms, 5000);
        assert_eq!(opts.keys.forward, '6');
        assert_eq!(opts.keys.rewind, '4');
        assert_eq!(opts.keys.stop, Some('5'));
        assert_eq!(opts.keys.pause, Some('7'));
        assert_eq!(opts.keys.restart, Some('8'));
        assert_eq!(opts.keys.reverse, Some('9'));
    }

    #[tokio::test]
    async fn test_controlplayback_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppControlPlayback::exec(&mut channel, "hello-world").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
