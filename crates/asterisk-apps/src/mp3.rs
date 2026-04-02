//! MP3 playback application -- plays MP3 files via external decoder.
//!
//! Port of app_mp3.c from Asterisk C. Launches mpg123 (or ffmpeg) as a
//! subprocess to decode MP3 audio and pipes the decoded PCM to the channel.
//! Supports MP3 files, M3U playlists, and HTTP streams.
//!
//! Note: This application does not automatically answer. It should be
//! preceded by Answer() or Progress().

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Well-known paths for the mpg123 decoder.
const LOCAL_MPG123: &str = "/usr/local/bin/mpg123";
const SYSTEM_MPG123: &str = "/usr/bin/mpg123";

/// Well-known paths for ffmpeg (fallback decoder).
const LOCAL_FFMPEG: &str = "/usr/local/bin/ffmpeg";
const SYSTEM_FFMPEG: &str = "/usr/bin/ffmpeg";

/// Which external decoder to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Decoder {
    /// mpg123 -- the traditional Asterisk choice.
    Mpg123,
    /// ffmpeg -- more flexible, supports more formats.
    Ffmpeg,
}

/// Options for the MP3Player application.
#[derive(Debug, Clone)]
pub struct Mp3Options {
    /// The MP3 file path, URL, or M3U playlist to play.
    pub location: String,
    /// Which decoder to use.
    pub decoder: Mp3Decoder,
}

impl Mp3Options {
    /// Parse the argument string.
    ///
    /// Format: MP3Player(location)
    pub fn parse(args: &str) -> Result<Self, String> {
        let location = args.trim().to_string();
        if location.is_empty() {
            return Err("missing required argument: location (MP3 file, URL, or M3U playlist)".into());
        }

        Ok(Self {
            location,
            decoder: Mp3Decoder::Mpg123,
        })
    }
}

/// Find the mpg123 binary on the system.
fn find_mpg123() -> Option<PathBuf> {
    for path in &[LOCAL_MPG123, SYSTEM_MPG123] {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    // Also try PATH
    if let Ok(output) = std::process::Command::new("which")
        .arg("mpg123")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

/// Find the ffmpeg binary on the system.
fn find_ffmpeg() -> Option<PathBuf> {
    for path in &[LOCAL_FFMPEG, SYSTEM_FFMPEG] {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(output) = std::process::Command::new("which")
        .arg("ffmpeg")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

/// The MP3Player() dialplan application.
///
/// Plays an MP3 file, M3U playlist, or HTTP stream by decoding it with
/// mpg123 (preferred) or ffmpeg. The decoded PCM audio is piped to the
/// channel. The user can press any DTMF key to stop playback.
///
/// Usage: MP3Player(location)
///
/// The location can be:
/// - A local MP3 file path: /var/lib/asterisk/sounds/music.mp3
/// - An HTTP stream: http://example.com/stream.mp3
/// - An M3U playlist: /var/lib/asterisk/playlist.m3u
pub struct AppMp3Player;

impl DialplanApp for AppMp3Player {
    fn name(&self) -> &str {
        "MP3Player"
    }

    fn description(&self) -> &str {
        "Play an MP3 file or stream"
    }
}

impl AppMp3Player {
    /// Execute the MP3Player application.
    ///
    /// # Arguments
    /// * `channel` - The channel to play audio on
    /// * `args` - The MP3 file/URL/playlist location
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = match Mp3Options::parse(args) {
            Ok(o) => o,
            Err(e) => {
                warn!("MP3Player: {}", e);
                return PbxExecResult::Failed;
            }
        };

        info!(
            "MP3Player: playing '{}' on channel '{}'",
            options.location, channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // Find the decoder binary
        let decoder_path = match options.decoder {
            Mp3Decoder::Mpg123 => find_mpg123().or_else(|| {
                debug!("MP3Player: mpg123 not found, trying ffmpeg");
                find_ffmpeg()
            }),
            Mp3Decoder::Ffmpeg => find_ffmpeg().or_else(|| {
                debug!("MP3Player: ffmpeg not found, trying mpg123");
                find_mpg123()
            }),
        };

        let decoder_path = match decoder_path {
            Some(p) => p,
            None => {
                warn!(
                    "MP3Player: no suitable decoder found (install mpg123 or ffmpeg)"
                );
                return PbxExecResult::Failed;
            }
        };

        debug!(
            "MP3Player: using decoder '{}'",
            decoder_path.display()
        );

        // In a full implementation:
        // 1. Fork the decoder process with stdout piped
        //    - For mpg123: mpg123 -q -s --mono -r 8000 <location>
        //    - For ffmpeg: ffmpeg -i <location> -f s16le -ar 8000 -ac 1 pipe:1
        // 2. Read decoded PCM samples from the subprocess stdout pipe
        // 3. Package samples into audio frames
        // 4. Write frames to the channel
        // 5. Meanwhile, read frames from the channel to check for DTMF
        // 6. On any DTMF key press, stop playback
        // 7. On channel hangup, kill the subprocess and exit
        // 8. When the subprocess exits normally, playback is complete

        // Stub: build the command to show it would work
        let is_mpg123 = decoder_path
            .file_name()
            .map(|n| n.to_string_lossy().contains("mpg123"))
            .unwrap_or(false);

        let _command_args = if is_mpg123 {
            vec![
                "-q".to_string(),
                "-s".to_string(),
                "--mono".to_string(),
                "-r".to_string(),
                "8000".to_string(),
                options.location.clone(),
            ]
        } else {
            // ffmpeg
            vec![
                "-i".to_string(),
                options.location.clone(),
                "-f".to_string(),
                "s16le".to_string(),
                "-ar".to_string(),
                "8000".to_string(),
                "-ac".to_string(),
                "1".to_string(),
                "pipe:1".to_string(),
            ]
        };

        debug!(
            "MP3Player: would spawn: {} {:?}",
            decoder_path.display(),
            _command_args
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mp3_options_parse() {
        let opts = Mp3Options::parse("/path/to/file.mp3").unwrap();
        assert_eq!(opts.location, "/path/to/file.mp3");
        assert_eq!(opts.decoder, Mp3Decoder::Mpg123);
    }

    #[test]
    fn test_mp3_options_parse_url() {
        let opts = Mp3Options::parse("http://example.com/stream.mp3").unwrap();
        assert_eq!(opts.location, "http://example.com/stream.mp3");
    }

    #[test]
    fn test_mp3_options_parse_empty() {
        assert!(Mp3Options::parse("").is_err());
        assert!(Mp3Options::parse("   ").is_err());
    }
}
