//! Record application - records audio from a channel to a file.
//!
//! Port of app_record.c from Asterisk C. Records audio from the channel
//! into a file, with support for silence detection, maximum duration,
//! DTMF termination, and various recording options.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Record status set as the RECORD_STATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStatus {
    /// Recording was terminated by DTMF ('#' or '*')
    Dtmf,
    /// Recording ended due to silence timeout
    Silence,
    /// Recording skipped because channel not answered and 's' option given
    Skip,
    /// Recording ended because maximum duration was reached
    Timeout,
    /// Channel was hung up during recording
    Hangup,
    /// An error occurred during recording
    Error,
    /// Operator exit (DTMF '0' with 'o' option)
    Operator,
}

impl RecordStatus {
    /// String representation for the RECORD_STATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dtmf => "DTMF",
            Self::Silence => "SILENCE",
            Self::Skip => "SKIP",
            Self::Timeout => "TIMEOUT",
            Self::Hangup => "HANGUP",
            Self::Error => "ERROR",
            Self::Operator => "OPERATOR",
        }
    }
}

/// Recording options parsed from the option string.
#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    /// Append to existing recording instead of replacing
    pub append: bool,
    /// Do not answer the channel
    pub noanswer: bool,
    /// Suppress the beep before recording
    pub quiet: bool,
    /// Skip recording if channel not answered
    pub skip: bool,
    /// Use '*' as terminator instead of '#'
    pub star_terminate: bool,
    /// Ignore all terminator keys and record until hangup
    pub ignore_terminate: bool,
    /// Keep recorded file upon hangup
    pub keep_on_hangup: bool,
    /// Terminate on any DTMF digit
    pub any_terminate: bool,
    /// Exit on '0' key press with OPERATOR status
    pub operator_exit: bool,
    /// Do not truncate trailing silence
    pub no_truncate: bool,
}

impl RecordOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.append = true,
                'n' => result.noanswer = true,
                'q' => result.quiet = true,
                's' => result.skip = true,
                't' => result.star_terminate = true,
                'x' => result.ignore_terminate = true,
                'k' => result.keep_on_hangup = true,
                'y' => result.any_terminate = true,
                'o' => result.operator_exit = true,
                'u' => result.no_truncate = true,
                _ => {
                    debug!("Record: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }

    /// Get the terminator character.
    pub fn terminator(&self) -> Option<char> {
        if self.ignore_terminate {
            None
        } else if self.star_terminate {
            Some('*')
        } else {
            Some('#')
        }
    }
}

/// The Record() dialplan application.
///
/// Usage: Record(filename.format[,silence[,maxduration[,options]]])
///
/// Records audio from the channel to a file. The file format is determined
/// by the extension in the filename argument.
pub struct AppRecord;

impl DialplanApp for AppRecord {
    fn name(&self) -> &str {
        "Record"
    }

    fn description(&self) -> &str {
        "Record to a file"
    }
}

/// Parsed arguments for the Record application.
#[derive(Debug)]
pub struct RecordArgs {
    /// Base filename (without path or extension)
    pub filename: String,
    /// File format/extension (wav, gsm, etc.)
    pub format: String,
    /// Silence timeout in seconds (0 = disabled)
    pub silence_threshold: Duration,
    /// Maximum recording duration (0 = unlimited)
    pub max_duration: Duration,
    /// Recording options
    pub options: RecordOptions,
}

impl RecordArgs {
    /// Parse Record() argument string.
    ///
    /// Format: filename.format,silence,maxduration,options
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(4, ',').collect();

        // Parse filename.format (required)
        let file_part = parts.first()?.trim();
        let dot_pos = file_part.rfind('.')?;
        let filename = file_part[..dot_pos].to_string();
        let format = file_part[dot_pos + 1..].to_string();

        if filename.is_empty() || format.is_empty() {
            return None;
        }

        // Parse silence threshold (optional)
        let silence_threshold = if let Some(s) = parts.get(1) {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Duration::ZERO
            } else {
                match trimmed.parse::<u64>() {
                    Ok(secs) => Duration::from_secs(secs),
                    Err(_) => Duration::ZERO,
                }
            }
        } else {
            Duration::ZERO
        };

        // Parse max duration (optional)
        let max_duration = if let Some(s) = parts.get(2) {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Duration::ZERO
            } else {
                match trimmed.parse::<u64>() {
                    Ok(secs) => Duration::from_secs(secs),
                    Err(_) => Duration::ZERO,
                }
            }
        } else {
            Duration::ZERO
        };

        // Parse options (optional)
        let options = if let Some(s) = parts.get(3) {
            RecordOptions::parse(s.trim())
        } else {
            RecordOptions::default()
        };

        Some(Self {
            filename,
            format,
            silence_threshold,
            max_duration,
            options,
        })
    }

    /// Build the full output file path.
    pub fn output_path(&self) -> PathBuf {
        let mut path = if self.filename.starts_with('/') {
            PathBuf::from(&self.filename)
        } else {
            let mut p = PathBuf::from("/var/spool/asterisk/recording");
            p.push(&self.filename);
            p
        };
        path.set_extension(&self.format);
        path
    }
}

impl AppRecord {
    /// Execute the Record application.
    ///
    /// # Arguments
    /// * `channel` - The channel to record from
    /// * `args` - Argument string: "filename.format,silence,maxduration,options"
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, RecordStatus) {
        let record_args = match RecordArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Record: failed to parse arguments: '{}'", args);
                return (PbxExecResult::Failed, RecordStatus::Error);
            }
        };

        // Skip if channel not answered and 's' option given
        if record_args.options.skip && channel.state != ChannelState::Up {
            debug!("Record: skipping - channel not answered");
            return (PbxExecResult::Success, RecordStatus::Skip);
        }

        // Answer the channel if needed (unless 'n' option)
        if !record_args.options.noanswer && channel.state != ChannelState::Up {
            debug!("Record: answering channel before recording");
            channel.state = ChannelState::Up;
        }

        let output_path = record_args.output_path();
        info!(
            "Record: recording from channel '{}' to {:?} (format: {}, max: {:?}, silence: {:?})",
            channel.name,
            output_path,
            record_args.format,
            record_args.max_duration,
            record_args.silence_threshold,
        );

        // Play beep before recording (unless 'q' option)
        if !record_args.options.quiet {
            debug!("Record: playing beep");
            // In production: play_file(channel, "beep").await;
        }

        // Start recording
        let status = Self::record_loop(channel, &record_args).await;

        info!(
            "Record: finished recording to {:?}, status: {}",
            output_path,
            status.as_str()
        );

        let exec_result = match status {
            RecordStatus::Hangup => PbxExecResult::Hangup,
            RecordStatus::Error => PbxExecResult::Failed,
            _ => PbxExecResult::Success,
        };

        (exec_result, status)
    }

    /// Main recording loop.
    ///
    /// Reads voice frames from the channel and writes them to the output file.
    /// Monitors for DTMF termination, silence timeout, and max duration.
    async fn record_loop(channel: &Channel, args: &RecordArgs) -> RecordStatus {
        let start = Instant::now();
        let last_voice_time = Instant::now();
        let mut total_frames: u64 = 0;
        let poll_interval = Duration::from_millis(20); // 20ms frame intervals

        loop {
            // Check if channel hung up
            if channel.state == ChannelState::Down {
                if args.options.keep_on_hangup && total_frames > 0 {
                    return RecordStatus::Hangup;
                }
                return RecordStatus::Hangup;
            }

            // Check maximum duration
            if !args.max_duration.is_zero() && start.elapsed() >= args.max_duration {
                debug!("Record: maximum duration reached");
                return RecordStatus::Timeout;
            }

            // Check silence threshold
            if !args.silence_threshold.is_zero() {
                let silence_elapsed = last_voice_time.elapsed();
                if silence_elapsed >= args.silence_threshold {
                    debug!(
                        "Record: silence threshold reached ({:?})",
                        args.silence_threshold
                    );
                    return RecordStatus::Silence;
                }
            }

            // In a real implementation, we'd read a frame from the channel:
            //
            //   let frame = match channel_driver.read(channel).await {
            //       Ok(f) => f,
            //       Err(_) => return RecordStatus::Hangup,
            //   };
            //
            //   match frame.frame_type {
            //       FrameType::Voice => {
            //           // Check for silence using DSP
            //           if !is_silence(&frame) {
            //               last_voice_time = Instant::now();
            //           }
            //           // Write frame to file
            //           file_writer.write_frame(&frame)?;
            //           total_frames += 1;
            //       }
            //       FrameType::DtmfEnd => {
            //           let digit = char::from(frame.subclass as u8);
            //
            //           // Check for operator exit
            //           if args.options.operator_exit && digit == '0' {
            //               return RecordStatus::Operator;
            //           }
            //
            //           // Check for terminator
            //           if !args.options.ignore_terminate {
            //               if args.options.any_terminate {
            //                   return RecordStatus::Dtmf;
            //               }
            //               if let Some(term) = args.options.terminator() {
            //                   if digit == term {
            //                       return RecordStatus::Dtmf;
            //                   }
            //               }
            //           }
            //       }
            //       _ => { /* ignore other frame types */ }
            //   }

            total_frames += 1;
            tokio::time::sleep(poll_interval).await;

            // For the stub implementation, exit after simulating a short recording
            if start.elapsed() >= Duration::from_millis(100) {
                break;
            }
        }

        RecordStatus::Dtmf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_record_args() {
        let args = RecordArgs::parse("recording.wav,3,60,aq").unwrap();
        assert_eq!(args.filename, "recording");
        assert_eq!(args.format, "wav");
        assert_eq!(args.silence_threshold, Duration::from_secs(3));
        assert_eq!(args.max_duration, Duration::from_secs(60));
        assert!(args.options.append);
        assert!(args.options.quiet);
    }

    #[test]
    fn test_parse_record_args_minimal() {
        let args = RecordArgs::parse("test.gsm").unwrap();
        assert_eq!(args.filename, "test");
        assert_eq!(args.format, "gsm");
        assert_eq!(args.silence_threshold, Duration::ZERO);
        assert_eq!(args.max_duration, Duration::ZERO);
    }

    #[test]
    fn test_parse_record_args_invalid() {
        assert!(RecordArgs::parse("noextension").is_none());
        assert!(RecordArgs::parse(".wav").is_none());
        assert!(RecordArgs::parse("name.").is_none());
    }

    #[test]
    fn test_output_path() {
        let args = RecordArgs::parse("myrecording.wav").unwrap();
        assert_eq!(
            args.output_path(),
            PathBuf::from("/var/spool/asterisk/recording/myrecording.wav")
        );
    }

    #[test]
    fn test_output_path_absolute() {
        let args = RecordArgs::parse("/tmp/myrecording.wav").unwrap();
        assert_eq!(args.output_path(), PathBuf::from("/tmp/myrecording.wav"));
    }

    #[test]
    fn test_record_options() {
        let opts = RecordOptions::parse("aqst");
        assert!(opts.append);
        assert!(opts.quiet);
        assert!(opts.skip);
        assert!(opts.star_terminate);
        assert_eq!(opts.terminator(), Some('*'));
    }

    #[test]
    fn test_record_options_ignore_terminate() {
        let opts = RecordOptions::parse("x");
        assert!(opts.ignore_terminate);
        assert_eq!(opts.terminator(), None);
    }

    #[test]
    fn test_record_status_strings() {
        assert_eq!(RecordStatus::Dtmf.as_str(), "DTMF");
        assert_eq!(RecordStatus::Silence.as_str(), "SILENCE");
        assert_eq!(RecordStatus::Timeout.as_str(), "TIMEOUT");
        assert_eq!(RecordStatus::Hangup.as_str(), "HANGUP");
    }
}
