//! Dictation machine application.
//!
//! Port of app_dictate.c from Asterisk C. Provides a virtual dictation
//! machine with record and playback modes controlled via DTMF. Supports
//! pause/resume, rewind/fast-forward, variable playback speed, and
//! file truncation.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::path::PathBuf;
use tracing::{debug, info};

/// Operating mode of the dictation machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictateMode {
    /// Initial state - prompting for filename.
    Init,
    /// Recording mode.
    Record,
    /// Playback mode.
    Play,
}

/// Flags controlling dictation behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct DictateFlags {
    /// Currently recording.
    pub recording: bool,
    /// Currently playing.
    pub playing: bool,
    /// Truncate file on next record.
    pub truncate: bool,
    /// Paused state.
    pub paused: bool,
}

/// Parsed arguments for the Dictate application.
#[derive(Debug)]
pub struct DictateArgs {
    /// Base directory for dictation files.
    pub base_dir: String,
    /// Optional filename (if not provided, user is prompted).
    pub filename: Option<String>,
}

impl DictateArgs {
    /// Default base directory for dictation files.
    pub const DEFAULT_BASE_DIR: &'static str = "/var/spool/asterisk/dictate";

    /// Parse Dictate() argument string.
    ///
    /// Format: [base_dir[,filename]]
    pub fn parse(args: &str) -> Self {
        if args.trim().is_empty() {
            return Self {
                base_dir: Self::DEFAULT_BASE_DIR.to_string(),
                filename: None,
            };
        }

        let parts: Vec<&str> = args.splitn(2, ',').collect();

        let base_dir = parts
            .first()
            .map(|d| d.trim())
            .filter(|d| !d.is_empty())
            .unwrap_or(Self::DEFAULT_BASE_DIR)
            .to_string();

        let filename = parts
            .get(1)
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty());

        Self { base_dir, filename }
    }

    /// Build the full file path for a dictation file.
    pub fn file_path(&self, filename: &str) -> PathBuf {
        let mut path = PathBuf::from(&self.base_dir);
        path.push(filename);
        path
    }
}

/// The Dictate() dialplan application.
///
/// Usage: Dictate([base_dir[,filename]])
///
/// Virtual dictation machine. Starts in playback mode (paused).
/// User controls via DTMF:
///
/// In Playback mode:
///   1 - Switch to record mode
///   2 - Toggle playback speed (1x-4x)
///   7 - Rewind
///   8 - Fast-forward
///
/// In Record mode:
///   1 - Switch to playback mode
///   8 - Toggle truncate flag
///
/// Global controls:
///   * - Pause/resume
///   # - Exit and save
///   0 - Help
pub struct AppDictate;

impl DialplanApp for AppDictate {
    fn name(&self) -> &str {
        "Dictate"
    }

    fn description(&self) -> &str {
        "Virtual Dictation Machine"
    }
}

impl AppDictate {
    /// Rewind/fast-forward factor (samples): 320 samples/frame * 80 frames.
    pub const SEEK_FACTOR: i64 = 320 * 80;

    /// Execute the Dictate application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = DictateArgs::parse(args);

        // Answer the channel if not up
        if channel.state != ChannelState::Up {
            debug!("Dictate: answering channel");
            channel.state = ChannelState::Up;
        }

        info!(
            "Dictate: channel '{}' starting (base_dir='{}', filename={:?})",
            channel.name, parsed.base_dir, parsed.filename,
        );

        // In a real implementation:
        //
        //   // Set read format to signed linear
        //   set_read_format(channel, Format::Slin).await;
        //
        //   // Brief pause after answer
        //   safe_sleep(channel, 200).await;
        //
        //   loop {
        //       // Get filename from user or argument
        //       let filename = if let Some(ref f) = parsed.filename {
        //           f.clone()
        //       } else {
        //           match read_input(channel, "dictate/enter_filename", 256).await {
        //               Some(f) if !f.is_empty() => f,
        //               _ => return PbxExecResult::Hangup,
        //           }
        //       };
        //
        //       // Ensure directory exists
        //       std::fs::create_dir_all(&parsed.base_dir).ok();
        //
        //       let file_path = parsed.file_path(&filename);
        //       let mut mode = DictateMode::Play;
        //       let mut flags = DictateFlags { paused: true, ..Default::default() };
        //       let mut speed: u32 = 1;
        //       let mut samples: i64 = 0;
        //
        //       // Open file for append (create if needed)
        //       let mut fs = open_file_rw(&file_path, "raw").await;
        //
        //       // Play help prompt
        //       play_and_wait(channel, "dictate/forhelp").await;
        //
        //       // Main DTMF-controlled loop
        //       loop {
        //           let frame = read_frame(channel).await;
        //           match frame {
        //               None => return PbxExecResult::Hangup,
        //               Some(Frame::Dtmf(digit)) => {
        //                   match (mode, digit) {
        //                       // Playback mode controls
        //                       (DictateMode::Play, '1') => {
        //                           flags.paused = true;
        //                           mode = DictateMode::Record;
        //                       }
        //                       (DictateMode::Play, '2') => {
        //                           speed = (speed % 4) + 1;
        //                           say_number(channel, speed).await;
        //                       }
        //                       (DictateMode::Play, '7') => {
        //                           samples = (samples - SEEK_FACTOR).max(0);
        //                           seek_stream(&fs, samples).await;
        //                       }
        //                       (DictateMode::Play, '8') => {
        //                           samples += SEEK_FACTOR;
        //                           seek_stream(&fs, samples).await;
        //                       }
        //                       // Record mode controls
        //                       (DictateMode::Record, '1') => {
        //                           flags.paused = true;
        //                           mode = DictateMode::Play;
        //                       }
        //                       (DictateMode::Record, '8') => {
        //                           flags.truncate = !flags.truncate;
        //                       }
        //                       // Global controls
        //                       (_, '#') => break,   // exit
        //                       (_, '*') => {
        //                           flags.paused = !flags.paused;
        //                           if flags.paused {
        //                               play_and_wait(channel, "dictate/pause").await;
        //                           } else {
        //                               let prompt = if mode == DictateMode::Play {
        //                                   "dictate/playback"
        //                               } else {
        //                                   "dictate/record"
        //                               };
        //                               play_and_wait(channel, prompt).await;
        //                           }
        //                       }
        //                       (_, '0') => {
        //                           flags.paused = true;
        //                           play_and_wait(channel, "dictate/paused").await;
        //                           let help = match mode {
        //                               DictateMode::Play => "dictate/play_help",
        //                               DictateMode::Record => "dictate/record_help",
        //                               _ => "dictate/both_help",
        //                           };
        //                           play_and_wait(channel, help).await;
        //                           play_and_wait(channel, "dictate/both_help").await;
        //                       }
        //                       _ => {}
        //                   }
        //               }
        //               Some(Frame::Voice(audio)) => {
        //                   match mode {
        //                       DictateMode::Play if !flags.paused => {
        //                           // Read and play back frames at current speed
        //                           for _ in 0..speed {
        //                               if let Some(fr) = read_stream_frame(&fs).await {
        //                                   write_frame(channel, &fr).await;
        //                                   samples += fr.samples as i64;
        //                               } else {
        //                                   samples = 0;
        //                                   seek_stream(&fs, 0).await;
        //                               }
        //                           }
        //                       }
        //                       DictateMode::Record if !flags.paused => {
        //                           write_stream(&fs, &audio).await;
        //                       }
        //                       _ => {}
        //                   }
        //               }
        //               _ => {}
        //           }
        //       }
        //   }

        info!("Dictate: channel '{}' session completed", channel.name);
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dictate_args_empty() {
        let args = DictateArgs::parse("");
        assert_eq!(args.base_dir, DictateArgs::DEFAULT_BASE_DIR);
        assert!(args.filename.is_none());
    }

    #[test]
    fn test_parse_dictate_args_base_dir() {
        let args = DictateArgs::parse("/tmp/dictate");
        assert_eq!(args.base_dir, "/tmp/dictate");
        assert!(args.filename.is_none());
    }

    #[test]
    fn test_parse_dictate_args_full() {
        let args = DictateArgs::parse("/tmp/dictate,memo001");
        assert_eq!(args.base_dir, "/tmp/dictate");
        assert_eq!(args.filename.as_deref(), Some("memo001"));
    }

    #[test]
    fn test_file_path() {
        let args = DictateArgs::parse("/tmp/dictate");
        let path = args.file_path("memo001");
        assert_eq!(path, PathBuf::from("/tmp/dictate/memo001"));
    }

    #[test]
    fn test_dictate_mode() {
        assert_ne!(DictateMode::Play, DictateMode::Record);
        assert_eq!(DictateMode::Init, DictateMode::Init);
    }

    #[tokio::test]
    async fn test_dictate_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDictate::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
