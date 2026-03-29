//! Playback application - plays audio files to a channel.
//!
//! Port of app_playback.c from Asterisk C. Plays one or more audio files
//! to the channel, optionally allowing interruption by DTMF.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::path::PathBuf;
use tracing::{debug, info, warn};

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
    /// In a real implementation, this would:
    /// 1. Open the file using format detection (wav, gsm, alaw, ulaw, etc.)
    /// 2. Read audio frames from the file
    /// 3. Optionally transcode to the channel's native format
    /// 4. Write frames to the channel at the correct rate
    /// 5. Monitor for DTMF interrupts
    async fn play_file(channel: &Channel, filename: &str) -> PlaybackStatus {
        // Resolve the file path
        let path = Self::resolve_file_path(filename);

        // In a full implementation, we would:
        //   let file = FormatRegistry::open(&path)?;
        //   while let Some(frame) = file.read_frame()? {
        //       if let Some(dtmf) = channel.check_dtmf()? {
        //           return PlaybackStatus::Interrupted(dtmf);
        //       }
        //       channel.write_frame(frame)?;
        //       // Wait appropriate time for frame duration
        //       tokio::time::sleep(frame_duration).await;
        //   }

        info!(
            "Playback: would play '{}' (resolved: {:?}) to channel '{}'",
            filename, path, channel.name
        );

        // For now, simulate successful playback
        // The actual frame reading/writing will be implemented when the
        // format and file I/O subsystems are complete
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
}
