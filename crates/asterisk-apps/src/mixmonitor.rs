//! MixMonitor application - record/monitor active calls.
//!
//! Port of app_mixmonitor.c from Asterisk C. Records both sides of a call
//! by attaching an audiohook to the channel and mixing the read/write audio
//! streams into an output file. Supports volume adjustment, separate
//! receive/transmit files, post-recording command execution, and more.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Default monitoring directory for recordings.
const DEFAULT_MONITOR_DIR: &str = "/var/spool/asterisk/monitor";

/// MixMonitor status values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixMonitorStatus {
    /// Recording is active.
    Active,
    /// Recording completed normally.
    Completed,
    /// Recording was stopped via StopMixMonitor.
    Stopped,
    /// Recording failed to start.
    Failed,
    /// Channel hung up during recording.
    Hangup,
}

impl MixMonitorStatus {
    /// String representation for channel variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Completed => "COMPLETED",
            Self::Stopped => "STOPPED",
            Self::Failed => "FAILED",
            Self::Hangup => "HANGUP",
        }
    }
}

/// Volume adjustment factor (range: -4 to +4, 0 = no change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeAdjust(i8);

impl VolumeAdjust {
    /// Create a new volume adjustment, clamped to [-4, 4].
    pub fn new(val: i8) -> Self {
        Self(val.clamp(-4, 4))
    }

    /// No adjustment.
    pub fn none() -> Self {
        Self(0)
    }

    /// Get the raw value.
    pub fn value(&self) -> i8 {
        self.0
    }
}

impl Default for VolumeAdjust {
    fn default() -> Self {
        Self::none()
    }
}

/// Options for the MixMonitor application.
#[derive(Debug, Clone, Default)]
pub struct MixMonitorOptions {
    /// Append to existing file instead of overwriting.
    pub append: bool,
    /// Only record audio while the channel is bridged.
    pub bridge_only: bool,
    /// Play a periodic beep (interval in seconds, 0 = disabled).
    pub beep_interval: u32,
    /// Delete the recording file when done.
    pub delete_on_completion: bool,
    /// Volume adjustment for the "heard" (read) audio.
    pub read_volume: VolumeAdjust,
    /// Volume adjustment for the "spoken" (write) audio.
    pub write_volume: VolumeAdjust,
    /// Volume adjustment for both directions.
    pub both_volume: VolumeAdjust,
    /// Separate file for recording receive audio.
    pub receive_file: Option<String>,
    /// Separate file for recording transmit audio.
    pub transmit_file: Option<String>,
    /// Interleave audio as stereo (requires .raw extension).
    pub stereo: bool,
    /// Do not insert silence for synchronization of r/t files.
    pub no_sync_silence: bool,
    /// Channel variable to store the MixMonitor ID in.
    pub id_variable: Option<String>,
    /// Play a beep when recording starts.
    pub play_beep: bool,
    /// Use real CallerID (not connected line) for voicemail CID.
    pub real_callerid: bool,
}

impl MixMonitorOptions {
    /// Parse the options string from MixMonitor arguments.
    ///
    /// Options: a, b, B(interval), c, d, v(x), V(x), W(x), r(file), t(file), D, n, i(var), p
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                'a' => result.append = true,
                'b' => result.bridge_only = true,
                'B' => {
                    // B(interval) - periodic beep
                    let arg = Self::extract_paren_arg(&mut chars);
                    result.beep_interval = arg
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(15);
                }
                'c' => result.real_callerid = true,
                'd' => result.delete_on_completion = true,
                'v' => {
                    // v(x) - read volume
                    let arg = Self::extract_paren_arg(&mut chars);
                    if let Some(val) = arg.and_then(|s| s.parse::<i8>().ok()) {
                        result.read_volume = VolumeAdjust::new(val);
                    }
                }
                'V' => {
                    // V(x) - write volume
                    let arg = Self::extract_paren_arg(&mut chars);
                    if let Some(val) = arg.and_then(|s| s.parse::<i8>().ok()) {
                        result.write_volume = VolumeAdjust::new(val);
                    }
                }
                'W' => {
                    // W(x) - both volumes
                    let arg = Self::extract_paren_arg(&mut chars);
                    if let Some(val) = arg.and_then(|s| s.parse::<i8>().ok()) {
                        result.both_volume = VolumeAdjust::new(val);
                    }
                }
                'r' => {
                    // r(file) - receive file
                    result.receive_file = Self::extract_paren_arg(&mut chars);
                }
                't' => {
                    // t(file) - transmit file
                    result.transmit_file = Self::extract_paren_arg(&mut chars);
                }
                'D' => result.stereo = true,
                'n' => result.no_sync_silence = true,
                'i' => {
                    // i(chanvar)
                    result.id_variable = Self::extract_paren_arg(&mut chars);
                }
                'p' => result.play_beep = true,
                _ => {
                    debug!("MixMonitor: ignoring unknown option '{}'", ch);
                }
            }
        }

        // If both_volume is set, apply to read and write if they haven't been set individually
        if result.both_volume.value() != 0 {
            if result.read_volume.value() == 0 {
                result.read_volume = result.both_volume;
            }
            if result.write_volume.value() == 0 {
                result.write_volume = result.both_volume;
            }
        }

        result
    }

    /// Extract a parenthesized argument from the char iterator.
    fn extract_paren_arg(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        if chars.peek() == Some(&'(') {
            chars.next(); // consume '('
            let mut arg = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                arg.push(c);
            }
            if arg.is_empty() {
                None
            } else {
                Some(arg)
            }
        } else {
            None
        }
    }
}

/// Represents an active MixMonitor recording session.
///
/// This session tracks the state of an ongoing recording, including
/// the output file, options, and timing information.
#[derive(Debug)]
pub struct MixMonitorSession {
    /// Unique identifier for this recording session.
    pub id: String,
    /// The channel name being recorded.
    pub channel_name: String,
    /// The primary output file path.
    pub filename: PathBuf,
    /// Recording options.
    pub options: MixMonitorOptions,
    /// Command to execute after recording completes.
    pub post_command: Option<String>,
    /// Current status.
    pub status: MixMonitorStatus,
    /// When the recording started.
    pub started: Instant,
    /// Total bytes written.
    pub bytes_written: u64,
    /// Whether the session has been requested to stop.
    pub stop_requested: bool,
}

impl MixMonitorSession {
    /// Create a new MixMonitor session.
    pub fn new(
        channel_name: &str,
        filename: PathBuf,
        options: MixMonitorOptions,
        post_command: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel_name: channel_name.to_string(),
            filename,
            options,
            post_command,
            status: MixMonitorStatus::Active,
            started: Instant::now(),
            bytes_written: 0,
            stop_requested: false,
        }
    }

    /// Get the duration of the recording so far.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Request the session to stop.
    pub fn request_stop(&mut self) {
        self.stop_requested = true;
    }
}

/// Parsed arguments for the MixMonitor application.
#[derive(Debug)]
pub struct MixMonitorArgs {
    /// Output filename (with extension).
    pub filename: String,
    /// Parsed options.
    pub options: MixMonitorOptions,
    /// Command to execute after recording finishes.
    pub post_command: Option<String>,
}

impl MixMonitorArgs {
    /// Parse MixMonitor() argument string.
    ///
    /// Format: filename[,options[,command]]
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(3, ',').collect();

        let filename = parts.first()?.trim().to_string();
        if filename.is_empty() {
            return None;
        }

        let options = parts
            .get(1)
            .map(|o| MixMonitorOptions::parse(o.trim()))
            .unwrap_or_default();

        let post_command = parts
            .get(2)
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());

        Some(Self {
            filename,
            options,
            post_command,
        })
    }

    /// Build the full output file path.
    pub fn output_path(&self) -> PathBuf {
        if self.filename.starts_with('/') {
            PathBuf::from(&self.filename)
        } else {
            let mut p = PathBuf::from(DEFAULT_MONITOR_DIR);
            p.push(&self.filename);
            p
        }
    }
}

/// The MixMonitor() dialplan application.
///
/// Usage: MixMonitor(filename[,options[,command]])
///
/// Records both sides of a call by attaching an audiohook to the channel.
/// The read and write audio streams are mixed and written to the output file.
pub struct AppMixMonitor;

impl DialplanApp for AppMixMonitor {
    fn name(&self) -> &str {
        "MixMonitor"
    }

    fn description(&self) -> &str {
        "Record a call and mix the audio during the recording"
    }
}

impl AppMixMonitor {
    /// Execute the MixMonitor application.
    ///
    /// # Arguments
    /// * `channel` - The channel to record
    /// * `args` - Argument string: "filename[,options[,command]]"
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match MixMonitorArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("MixMonitor: requires at least a filename argument");
                return PbxExecResult::Failed;
            }
        };

        let output_path = parsed.output_path();

        info!(
            "MixMonitor: starting recording on channel '{}' to {:?}",
            channel.name, output_path
        );

        // Create the recording session
        let session = MixMonitorSession::new(
            &channel.name,
            output_path.clone(),
            parsed.options.clone(),
            parsed.post_command.clone(),
        );
        let session_id = session.id.clone();

        // Store the session ID in a channel variable if requested
        if let Some(ref var_name) = parsed.options.id_variable {
            channel.set_variable(var_name, &session_id);
        }

        // Store the session on the channel's datastore for StopMixMonitor to find
        let session = Arc::new(Mutex::new(session));
        channel
            .datastores
            .insert("mixmonitor_session".to_string(), Box::new(session.clone()));

        // Play beep if requested
        if parsed.options.play_beep {
            debug!("MixMonitor: playing start beep on channel '{}'", channel.name);
            // In production: play_file(channel, "beep").await;
        }

        // In a real implementation, we would attach an audiohook to the channel:
        //
        //   let audiohook = AudioHook::new(AudioHookType::Spy, "MixMonitor");
        //   audiohook.set_read_volume(parsed.options.read_volume.value());
        //   audiohook.set_write_volume(parsed.options.write_volume.value());
        //
        //   // Open output file(s)
        //   let file_writer = FileWriter::open(&output_path, parsed.options.append)?;
        //   let rx_writer = parsed.options.receive_file.as_ref()
        //       .map(|f| FileWriter::open(f, false))
        //       .transpose()?;
        //   let tx_writer = parsed.options.transmit_file.as_ref()
        //       .map(|f| FileWriter::open(f, false))
        //       .transpose()?;
        //
        //   // Spawn recording task
        //   tokio::spawn(async move {
        //       loop {
        //           let (read_frame, write_frame) = audiohook.read_mixed().await;
        //
        //           // Check if channel is still up and stop not requested
        //           if session.lock().stop_requested {
        //               session.lock().status = MixMonitorStatus::Stopped;
        //               break;
        //           }
        //
        //           // Mix frames and write to file
        //           if !parsed.options.stereo {
        //               let mixed = mix_frames(&read_frame, &write_frame);
        //               file_writer.write(&mixed)?;
        //           } else {
        //               // Interleave for stereo output
        //               file_writer.write_stereo(&read_frame, &write_frame)?;
        //           }
        //
        //           // Write to separate r/t files if configured
        //           if let Some(ref w) = rx_writer { w.write(&read_frame)?; }
        //           if let Some(ref w) = tx_writer { w.write(&write_frame)?; }
        //
        //           // Periodic beep
        //           if parsed.options.beep_interval > 0 {
        //               let elapsed = session.lock().elapsed();
        //               if elapsed.as_secs() % parsed.options.beep_interval as u64 == 0 {
        //                   play_beep(channel).await;
        //               }
        //           }
        //       }
        //
        //       // Execute post-recording command
        //       if let Some(ref cmd) = parsed.post_command {
        //           std::process::Command::new("sh").arg("-c").arg(cmd).spawn();
        //       }
        //
        //       // Delete file if requested
        //       if parsed.options.delete_on_completion {
        //           std::fs::remove_file(&output_path).ok();
        //       }
        //   });
        //
        //   channel.attach_audiohook(audiohook);

        info!(
            "MixMonitor: recording session '{}' started on channel '{}'",
            session_id, channel.name
        );

        // MixMonitor returns immediately -- recording continues asynchronously
        PbxExecResult::Success
    }
}

/// The StopMixMonitor() dialplan application.
///
/// Usage: StopMixMonitor([MixMonitorID])
///
/// Stops an active MixMonitor recording on the channel.
pub struct AppStopMixMonitor;

impl DialplanApp for AppStopMixMonitor {
    fn name(&self) -> &str {
        "StopMixMonitor"
    }

    fn description(&self) -> &str {
        "Stop an active MixMonitor recording"
    }
}

impl AppStopMixMonitor {
    /// Execute the StopMixMonitor application.
    ///
    /// # Arguments
    /// * `channel` - The channel to stop recording on
    /// * `args` - Optional MixMonitor ID to stop a specific recording
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let _specific_id = if args.trim().is_empty() {
            None
        } else {
            Some(args.trim().to_string())
        };

        // Look up the active session on the channel's datastore
        if let Some(session_box) = channel.datastores.get("mixmonitor_session") {
            if let Some(session) = session_box.downcast_ref::<Arc<Mutex<MixMonitorSession>>>() {
                let mut sess = session.lock();
                sess.request_stop();
                sess.status = MixMonitorStatus::Stopped;
                info!(
                    "StopMixMonitor: stopped recording session '{}' on channel '{}'",
                    sess.id, channel.name
                );

                // In a real implementation, we would also detach the audiohook:
                //   channel.detach_audiohook("MixMonitor");
            }
        } else {
            debug!(
                "StopMixMonitor: no active MixMonitor found on channel '{}'",
                channel.name
            );
        }

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mixmonitor_args_basic() {
        let args = MixMonitorArgs::parse("recording.wav").unwrap();
        assert_eq!(args.filename, "recording.wav");
        assert!(args.post_command.is_none());
    }

    #[test]
    fn test_parse_mixmonitor_args_with_options() {
        let args = MixMonitorArgs::parse("recording.wav,abv(-2)V(3)").unwrap();
        assert_eq!(args.filename, "recording.wav");
        assert!(args.options.append);
        assert!(args.options.bridge_only);
        assert_eq!(args.options.read_volume.value(), -2);
        assert_eq!(args.options.write_volume.value(), 3);
    }

    #[test]
    fn test_parse_mixmonitor_args_with_command() {
        let args = MixMonitorArgs::parse("rec.wav,a,/usr/bin/process.sh ${FILENAME}").unwrap();
        assert_eq!(args.post_command.as_deref(), Some("/usr/bin/process.sh ${FILENAME}"));
    }

    #[test]
    fn test_parse_mixmonitor_args_empty() {
        assert!(MixMonitorArgs::parse("").is_none());
    }

    #[test]
    fn test_options_volume_clamp() {
        let v = VolumeAdjust::new(10);
        assert_eq!(v.value(), 4);
        let v = VolumeAdjust::new(-10);
        assert_eq!(v.value(), -4);
    }

    #[test]
    fn test_options_beep_interval() {
        let opts = MixMonitorOptions::parse("B(30)");
        assert_eq!(opts.beep_interval, 30);
    }

    #[test]
    fn test_options_id_variable() {
        let opts = MixMonitorOptions::parse("i(MIXMON_ID)");
        assert_eq!(opts.id_variable.as_deref(), Some("MIXMON_ID"));
    }

    #[test]
    fn test_options_both_volume() {
        let opts = MixMonitorOptions::parse("W(2)");
        assert_eq!(opts.read_volume.value(), 2);
        assert_eq!(opts.write_volume.value(), 2);
    }

    #[test]
    fn test_output_path_relative() {
        let args = MixMonitorArgs::parse("call.wav").unwrap();
        assert_eq!(
            args.output_path(),
            PathBuf::from("/var/spool/asterisk/monitor/call.wav")
        );
    }

    #[test]
    fn test_output_path_absolute() {
        let args = MixMonitorArgs::parse("/tmp/call.wav").unwrap();
        assert_eq!(args.output_path(), PathBuf::from("/tmp/call.wav"));
    }

    #[test]
    fn test_session_creation() {
        let session = MixMonitorSession::new(
            "SIP/test-001",
            PathBuf::from("/tmp/test.wav"),
            MixMonitorOptions::default(),
            None,
        );
        assert_eq!(session.status, MixMonitorStatus::Active);
        assert!(!session.stop_requested);
        assert_eq!(session.bytes_written, 0);
    }

    #[tokio::test]
    async fn test_mixmonitor_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMixMonitor::exec(&mut channel, "test.wav,a").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_mixmonitor_empty_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppMixMonitor::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_stop_mixmonitor() {
        let mut channel = Channel::new("SIP/test-001");
        // Start a recording first
        let _ = AppMixMonitor::exec(&mut channel, "test.wav,i(MID)").await;
        // Now stop it
        let result = AppStopMixMonitor::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
