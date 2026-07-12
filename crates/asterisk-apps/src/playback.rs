//! Playback application - plays audio files to a channel.
//!
//! Port of app_playback.c from Asterisk C. Plays one or more audio files
//! to the channel, optionally allowing interruption by DTMF.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::{ChannelState, Frame};
use bytes::Bytes;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, warn};

/// Media clock: 8 kHz telephony audio, 20 ms per RTP packet.
const SAMPLE_RATE_HZ: u32 = 8000;
const FRAME_MS: u32 = 20;
/// Samples in one 20 ms frame at 8 kHz (160).
const SAMPLES_PER_FRAME: u32 = SAMPLE_RATE_HZ * FRAME_MS / 1000;

/// The raw on-disk encoding of a sounds file, inferred from its extension.
///
/// rustisk does not yet transcode (see the codec-relabel limitation in the
/// RTP layer), so a file plays correctly only when its encoding matches the
/// call's negotiated codec — exactly as Asterisk requires the right format
/// (or a translator) to be available. We support the header-less raw formats
/// that map directly onto a 20 ms RTP payload; WAV/GSM decoding is future
/// work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AudioFormat {
    /// Bytes consumed per 20 ms frame.
    bytes_per_frame: usize,
    /// Nominal RTP payload type for the encoding (the driver currently stamps
    /// the negotiated PT regardless, so this is informational).
    codec_id: u32,
}

impl AudioFormat {
    /// Infer the raw encoding from a file extension, defaulting to µ-law —
    /// the most common header-less telephony recording format.
    fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            // 8-bit companded: one byte per sample.
            "ulaw" | "ul" | "pcmu" | "g711u" => AudioFormat { bytes_per_frame: 160, codec_id: 0 },
            "alaw" | "al" | "pcma" | "g711a" => AudioFormat { bytes_per_frame: 160, codec_id: 8 },
            // 16-bit signed linear, 8 kHz: two bytes per sample.
            "sln" | "slin" | "raw" | "sln8" | "s16" => AudioFormat { bytes_per_frame: 320, codec_id: 0 },
            // Unknown/no extension: assume µ-law.
            _ => AudioFormat { bytes_per_frame: 160, codec_id: 0 },
        }
    }
}

/// Split raw audio bytes into fixed-size 20 ms frames. A trailing partial
/// frame (shorter than `bytes_per_frame`) is kept and played as-is, matching
/// Asterisk's handling of a final short block.
fn split_into_frames(data: &[u8], bytes_per_frame: usize) -> Vec<Bytes> {
    if bytes_per_frame == 0 {
        return Vec::new();
    }
    data.chunks(bytes_per_frame)
        .map(Bytes::copy_from_slice)
        .collect()
}

/// The Playback() dialplan application.
///
/// Plays audio files to the channel. Multiple files can be specified,
/// separated by '&'. The channel is answered before playback unless
/// the 'noanswer' option is given.
///
/// Usage: Playback(file1[&file2[&...]][,options])
///
/// Options:
///   skip     - Do not play if channel is not answered
///   noanswer - Do not answer the channel before playing
pub struct AppPlayback;

/// Options for playback.
#[derive(Debug, Clone, Default)]
pub struct PlaybackOptions {
    /// If true, skip playback if channel is not answered
    pub skip: bool,
    /// If true, do not answer the channel before playing
    pub noanswer: bool,
}

impl PlaybackOptions {
    /// Parse comma-separated options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for opt in opts.split(',') {
            match opt.trim().to_lowercase().as_str() {
                "skip" => result.skip = true,
                "noanswer" => result.noanswer = true,
                "" => {}
                other => {
                    debug!("Playback: ignoring unknown option '{}'", other);
                }
            }
        }
        result
    }
}

/// Result of a playback operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// All files played successfully
    Success,
    /// Playback failed (file not found, channel error, etc.)
    Failed,
    /// Playback was interrupted by DTMF
    Interrupted(char),
}

impl DialplanApp for AppPlayback {
    fn name(&self) -> &str {
        "Playback"
    }

    fn description(&self) -> &str {
        "Play a file"
    }
}

impl AppPlayback {
    /// Execute the Playback application.
    ///
    /// # Arguments
    /// * `channel` - The channel to play audio to
    /// * `args` - Argument string: "file1[&file2[&...]],options"
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (files_str, options) = Self::parse_args(args);
        let filenames: Vec<&str> = files_str.split('&').filter(|s| !s.is_empty()).collect();

        if filenames.is_empty() {
            warn!("Playback: no files specified");
            return PbxExecResult::Failed;
        }

        // Check if we should skip playback
        if options.skip && channel.state != ChannelState::Up {
            debug!("Playback: skipping - channel not answered and 'skip' option set");
            return PbxExecResult::Success;
        }

        // Answer the channel if needed (unless 'noanswer' option)
        if !options.noanswer && channel.state != ChannelState::Up {
            debug!("Playback: answering channel before playback");
            channel.state = ChannelState::Up;
        }

        // Play each file in sequence
        let mut overall_status = PlaybackStatus::Success;
        for filename in &filenames {
            let filename = filename.trim();
            debug!("Playback: playing file '{}' to channel '{}'", filename, channel.name);

            match Self::play_file(channel, filename).await {
                PlaybackStatus::Success => {
                    debug!("Playback: file '{}' played successfully", filename);
                }
                PlaybackStatus::Failed => {
                    warn!("Playback: failed to play file '{}'", filename);
                    overall_status = PlaybackStatus::Failed;
                    // Continue trying other files (Asterisk behavior)
                }
                PlaybackStatus::Interrupted(digit) => {
                    debug!(
                        "Playback: interrupted by DTMF '{}' during file '{}'",
                        digit, filename
                    );
                    // Stop playback on DTMF interrupt
                    break;
                }
            }
        }

        match overall_status {
            PlaybackStatus::Success => PbxExecResult::Success,
            PlaybackStatus::Failed => PbxExecResult::Failed,
            PlaybackStatus::Interrupted(_) => PbxExecResult::Success,
        }
    }

    /// Parse the argument string into file list and options.
    fn parse_args(args: &str) -> (&str, PlaybackOptions) {
        // Split on comma for files vs options
        // But files can contain '&' for multiple file separation
        if let Some(comma_pos) = args.rfind(',') {
            // Check if the part after comma looks like options
            let potential_opts = &args[comma_pos + 1..];
            let potential_opts_lower = potential_opts.trim().to_lowercase();
            if potential_opts_lower.contains("skip")
                || potential_opts_lower.contains("noanswer")
                || potential_opts_lower.contains("say")
                || potential_opts_lower.contains("mix")
            {
                let files = &args[..comma_pos];
                let options = PlaybackOptions::parse(potential_opts);
                return (files, options);
            }
        }
        // No options found, entire string is file list
        (args, PlaybackOptions::default())
    }

    /// Play a single audio file to the channel.
    ///
    /// Opens the resolved file, splits it into 20 ms frames according to its
    /// raw encoding, and writes each frame to the channel's technology driver
    /// (`write_frame` → RTP) paced at 20 ms — the same media pump `Echo()`
    /// uses. Returns:
    /// * `Failed` if the file cannot be opened/read, is empty, or the channel
    ///   has no media plane (no tech driver) to deliver audio to;
    /// * `Success` once every frame has been written;
    /// * `Interrupted` is reserved for DTMF barge-in (not yet detected here).
    ///
    /// Before the fix this resolved a path, wrote nothing, and always
    /// returned `Success` — so IVRs fell straight through their prompts and
    /// `PLAYBACKSTATUS` failure handling could never trigger (issue #29).
    async fn play_file(channel: &mut Channel, filename: &str) -> PlaybackStatus {
        let path = Self::resolve_file_path(filename);

        // Open + read the file. An unopenable file is a hard failure — this is
        // the behaviour PLAYBACKSTATUS branching depends on.
        let data = match std::fs::read(&path) {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => {
                warn!("Playback: file '{}' ({:?}) is empty", filename, path);
                return PlaybackStatus::Failed;
            }
            Err(e) => {
                warn!("Playback: cannot open '{}' ({:?}): {}", filename, path, e);
                return PlaybackStatus::Failed;
            }
        };

        let format = AudioFormat::from_path(&path);
        let frames = split_into_frames(&data, format.bytes_per_frame);

        // The technology driver is looked up by the channel-name prefix
        // (e.g. "PJSIP/alice-00000001" -> "PJSIP"), exactly like Echo(). No
        // driver means no media plane, so the audio cannot be delivered.
        let tech = channel.name.split('/').next().unwrap_or("").to_string();
        let driver = match asterisk_core::channel::tech_registry::TECH_REGISTRY.find(&tech) {
            Some(d) => d,
            None => {
                warn!(
                    "Playback: channel '{}' has no '{}' media driver; cannot play '{}'",
                    channel.name, tech, filename
                );
                return PlaybackStatus::Failed;
            }
        };

        debug!(
            "Playback: streaming '{}' ({} frames, {} B/frame) to '{}'",
            filename,
            frames.len(),
            format.bytes_per_frame,
            channel.name
        );

        // Pace the frames at the media clock. `interval` fires immediately on
        // the first tick, then every FRAME_MS, giving a steady cadence
        // without accumulating the per-write processing time as drift.
        let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS as u64));
        for chunk in frames {
            // Stop if the caller hung up mid-prompt (local flag or a hangup
            // set on the shared store copy by AMI / a remote BYE).
            if channel.state == ChannelState::Down || channel.check_hangup() {
                debug!("Playback: channel '{}' hung up during playback", channel.name);
                break;
            }

            ticker.tick().await;

            let frame = Frame::voice(format.codec_id, SAMPLES_PER_FRAME, chunk);
            if let Err(e) = driver.write_frame(channel, &frame).await {
                warn!("Playback: write_frame error on '{}': {}", channel.name, e);
                return PlaybackStatus::Failed;
            }
        }

        PlaybackStatus::Success
    }

    /// Resolve a filename to a full path.
    ///
    /// If the filename starts with '/', it's treated as absolute.
    /// Otherwise, it's looked up in the sounds directory.
    fn resolve_file_path(filename: &str) -> PathBuf {
        if filename.starts_with('/') {
            PathBuf::from(filename)
        } else {
            // Default sounds directory
            let mut path = PathBuf::from("/var/lib/asterisk/sounds");
            path.push(filename);
            path
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_no_options() {
        let (files, opts) = AppPlayback::parse_args("hello-world");
        assert_eq!(files, "hello-world");
        assert!(!opts.skip);
        assert!(!opts.noanswer);
    }

    #[test]
    fn test_parse_args_with_options() {
        let (files, opts) = AppPlayback::parse_args("hello-world,skip");
        assert_eq!(files, "hello-world");
        assert!(opts.skip);
    }

    #[test]
    fn test_parse_args_multiple_files() {
        let (files, opts) = AppPlayback::parse_args("file1&file2&file3,noanswer");
        assert_eq!(files, "file1&file2&file3");
        assert!(opts.noanswer);
    }

    #[test]
    fn test_resolve_absolute_path() {
        let path = AppPlayback::resolve_file_path("/custom/sounds/greeting");
        assert_eq!(path, PathBuf::from("/custom/sounds/greeting"));
    }

    #[test]
    fn test_resolve_relative_path() {
        let path = AppPlayback::resolve_file_path("en/hello-world");
        assert_eq!(
            path,
            PathBuf::from("/var/lib/asterisk/sounds/en/hello-world")
        );
    }

    // --- issue #29: real media pump --------------------------------------

    #[test]
    fn test_audio_format_from_extension() {
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.ulaw")),
            AudioFormat { bytes_per_frame: 160, codec_id: 0 }
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.alaw")),
            AudioFormat { bytes_per_frame: 160, codec_id: 8 }
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.sln")),
            AudioFormat { bytes_per_frame: 320, codec_id: 0 }
        );
        // Unknown / no extension defaults to µ-law.
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting")),
            AudioFormat { bytes_per_frame: 160, codec_id: 0 }
        );
    }

    #[test]
    fn test_split_into_frames() {
        // Exactly 5 full µ-law frames.
        let data = vec![0x7Fu8; 160 * 5];
        let frames = split_into_frames(&data, 160);
        assert_eq!(frames.len(), 5);
        assert!(frames.iter().all(|f| f.len() == 160));

        // A trailing partial frame is preserved.
        let data = vec![0u8; 160 * 2 + 40];
        let frames = split_into_frames(&data, 160);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[2].len(), 40);

        // Empty input → no frames.
        assert!(split_into_frames(&[], 160).is_empty());
    }

    #[tokio::test]
    async fn test_play_nonexistent_file_returns_failed() {
        // The core PLAYBACKSTATUS-branching fix: an unopenable file must
        // return Failed, not Success. No media driver is needed — the failure
        // happens at file open.
        let mut channel = Channel::new("SIP/test-nofile");
        let result =
            AppPlayback::exec(&mut channel, "/nonexistent/rustisk/does-not-exist.ulaw").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
