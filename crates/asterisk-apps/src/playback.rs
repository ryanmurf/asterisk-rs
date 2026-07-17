//! Playback application - plays audio files to a channel.
//!
//! Port of app_playback.c from Asterisk C. Plays one or more audio files
//! to the channel, optionally allowing interruption by DTMF.

use crate::{DialplanApp, PbxExecResult};
use asterisk_codecs::alaw_table::linear_to_alaw_fast;
use asterisk_codecs::ulaw_table::linear_to_mulaw_fast;
use asterisk_core::channel::Channel;
use asterisk_formats::wav::WavFormat8k;
use asterisk_formats::FileFormat;
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

/// The on-disk encoding of a sounds file, inferred from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioFormat {
    /// Headerless RTP-ready payload bytes.
    Raw {
        /// Bytes consumed per 20 ms frame.
        bytes_per_frame: usize,
        /// Nominal RTP payload type for the encoding.
        codec_id: u32,
    },
    /// RIFF/WAVE containing 8 kHz mono signed 16-bit little-endian PCM.
    WavPcm16,
}

impl AudioFormat {
    /// Infer the encoding from a file extension, defaulting to raw µ-law.
    fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            // 8-bit companded: one byte per sample.
            "ulaw" | "ul" | "pcmu" | "g711u" => AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 0,
            },
            "alaw" | "al" | "pcma" | "g711a" => AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 8,
            },
            // 16-bit signed linear, 8 kHz: two bytes per sample.
            "sln" | "slin" | "raw" | "sln8" | "s16" => AudioFormat::Raw {
                bytes_per_frame: 320,
                codec_id: 0,
            },
            "wav" | "wave" => AudioFormat::WavPcm16,
            // Unknown/no extension: assume µ-law.
            _ => AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 0,
            },
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

/// Decode a supported file into RTP-ready voice frames.
fn load_audio_frames(
    path: &Path,
    format: AudioFormat,
    negotiated_codec: Option<u32>,
) -> Result<Vec<Frame>, String> {
    match format {
        AudioFormat::Raw {
            bytes_per_frame,
            codec_id,
        } => {
            let data = std::fs::read(path).map_err(|error| error.to_string())?;
            if data.is_empty() {
                return Err("file is empty".to_string());
            }
            let bytes_per_sample = bytes_per_frame / SAMPLES_PER_FRAME as usize;
            Ok(split_into_frames(&data, bytes_per_frame)
                .into_iter()
                .map(|data| {
                    Frame::voice(codec_id, (data.len() / bytes_per_sample) as u32, data)
                })
                .collect())
        }
        AudioFormat::WavPcm16 => load_wav_frames(path, negotiated_codec),
    }
}

/// Decode 8 kHz signed-linear WAV frames and encode each sample as the
/// G.711 codec negotiated for this call. RFC 3551 assigns PCMU to PT 0 and
/// PCMA to PT 8; relabeling little-endian PCM as either payload is invalid.
fn load_wav_frames(path: &Path, negotiated_codec: Option<u32>) -> Result<Vec<Frame>, String> {
    let codec_id = match negotiated_codec {
        Some(codec @ (0 | 8)) => codec,
        Some(codec) => return Err(format!("unsupported negotiated codec PT {codec}")),
        None => return Err("channel has no negotiated audio codec".to_string()),
    };

    let mut stream = WavFormat8k::new()
        .open(path)
        .map_err(|error| error.to_string())?;
    if stream.sample_rate() != SAMPLE_RATE_HZ {
        return Err(format!(
            "expected {SAMPLE_RATE_HZ} Hz WAV, got {} Hz",
            stream.sample_rate()
        ));
    }

    let mut frames = Vec::new();
    while let Some(frame) = stream.read_frame().map_err(|error| error.to_string())? {
        let Frame::Voice { data, samples, .. } = frame else {
            return Err("WAV reader returned a non-voice frame".to_string());
        };
        if data.len() % 2 != 0 {
            return Err("WAV contains a partial 16-bit sample".to_string());
        }

        let payload: Vec<u8> = data
            .chunks_exact(2)
            .map(|bytes| {
                let sample = i16::from_le_bytes([bytes[0], bytes[1]]);
                if codec_id == 0 {
                    linear_to_mulaw_fast(sample)
                } else {
                    linear_to_alaw_fast(sample)
                }
            })
            .collect();
        frames.push(Frame::voice(codec_id, samples, Bytes::from(payload)));
    }

    if frames.is_empty() {
        return Err("WAV contains no audio samples".to_string());
    }
    Ok(frames)
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
            channel.answer();
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
                PlaybackStatus::Interrupted(_) => {
                    // REVIEW-BUNDLEB: never log the digit value -- a digit
                    // pressed during a prompt is PIN material once barge-in
                    // lands (log that an interruption occurred, not which
                    // digit caused it).
                    debug!("Playback: interrupted by DTMF during file '{}'", filename);
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
    /// Opens the resolved file, decodes it into 20 ms frames, and writes each
    /// frame to the channel's technology driver
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

        // Check the file before looking up the media plane. An unopenable or
        // empty file is a hard failure — this is the behaviour PLAYBACKSTATUS
        // branching depends on.
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() > 0 => {}
            Ok(_) => {
                warn!("Playback: file '{}' ({:?}) is empty", filename, path);
                return PlaybackStatus::Failed;
            }
            Err(e) => {
                warn!("Playback: cannot open '{}' ({:?}): {}", filename, path, e);
                return PlaybackStatus::Failed;
            }
        }

        let format = AudioFormat::from_path(&path);

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

        let negotiated_codec = driver.audio_format(channel).await;
        let frames = match load_audio_frames(&path, format, negotiated_codec) {
            Ok(frames) => frames,
            Err(error) => {
                warn!(
                    "Playback: cannot decode '{}' ({:?}): {}",
                    filename, path, error
                );
                return PlaybackStatus::Failed;
            }
        };

        debug!(
            "Playback: streaming '{}' ({} frames, codec PT {:?}) to '{}'",
            filename,
            frames.len(),
            negotiated_codec,
            channel.name
        );

        // Pace the frames at the media clock. `interval` fires immediately on
        // the first tick, then every FRAME_MS, giving a steady cadence
        // without accumulating the per-write processing time as drift.
        let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS as u64));
        for frame in frames {
            // Stop if the caller hung up mid-prompt (local flag or a hangup
            // set on the shared store copy by AMI / a remote BYE).
            if channel.state == ChannelState::Down || channel.check_hangup() {
                debug!("Playback: channel '{}' hung up during playback", channel.name);
                break;
            }

            ticker.tick().await;

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
    use asterisk_core::channel::ChannelDriver;
    use asterisk_types::{AsteriskError, AsteriskResult};
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    static TEST_DRIVER_ID: AtomicU32 = AtomicU32::new(1);

    #[derive(Debug)]
    struct MockPlaybackDriver {
        name: String,
        codec_id: u32,
        frames: Mutex<Vec<Frame>>,
    }

    #[async_trait::async_trait]
    impl ChannelDriver for MockPlaybackDriver {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Playback test driver"
        }

        async fn request(
            &self,
            dest: &str,
            _caller: Option<&Channel>,
        ) -> AsteriskResult<Channel> {
            Ok(Channel::new(format!("{}/{}", self.name, dest)))
        }

        async fn call(
            &self,
            _channel: &mut Channel,
            _dest: &str,
            _timeout: i32,
        ) -> AsteriskResult<()> {
            Ok(())
        }

        async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.set_state(ChannelState::Down);
            Ok(())
        }

        async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.answer();
            Ok(())
        }

        async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
            Err(AsteriskError::Internal(
                "Playback test driver does not receive frames".to_string(),
            ))
        }

        async fn write_frame(
            &self,
            _channel: &mut Channel,
            frame: &Frame,
        ) -> AsteriskResult<()> {
            self.frames.lock().push(frame.clone());
            Ok(())
        }

        async fn audio_format(&self, _channel: &Channel) -> Option<u32> {
            Some(self.codec_id)
        }
    }

    fn install_mock_driver(codec_id: u32) -> Arc<MockPlaybackDriver> {
        let id = TEST_DRIVER_ID.fetch_add(1, Ordering::Relaxed);
        let driver = Arc::new(MockPlaybackDriver {
            name: format!("PLAYBACKTEST{id}"),
            codec_id,
            frames: Mutex::new(Vec::new()),
        });
        asterisk_core::channel::tech_registry::TECH_REGISTRY.register(driver.clone());
        driver
    }

    fn write_test_wav_at_rate(samples: &[i16], sample_rate: u32) -> PathBuf {
        let data_size = (samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data_size as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/playback-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, wav).unwrap();
        path
    }

    fn write_test_wav(samples: &[i16]) -> PathBuf {
        write_test_wav_at_rate(samples, SAMPLE_RATE_HZ)
    }

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
            AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 0,
            }
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.alaw")),
            AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 8,
            }
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.sln")),
            AudioFormat::Raw {
                bytes_per_frame: 320,
                codec_id: 0,
            }
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting.wav")),
            AudioFormat::WavPcm16
        );
        // Unknown / no extension defaults to µ-law.
        assert_eq!(
            AudioFormat::from_path(Path::new("greeting")),
            AudioFormat::Raw {
                bytes_per_frame: 160,
                codec_id: 0,
            }
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

    #[tokio::test]
    async fn wav_pcm_is_encoded_to_each_negotiated_g711_codec() {
        let samples: Vec<i16> = (0..SAMPLES_PER_FRAME)
            .map(|sample| (sample as i16 - 80) * 200)
            .collect();
        let path = write_test_wav(&samples);

        for codec_id in [0, 8] {
            let driver = install_mock_driver(codec_id);
            let mut channel = Channel::new(format!("{}/call", driver.name));
            channel.state = ChannelState::Up;

            let result = AppPlayback::exec(&mut channel, path.to_str().unwrap()).await;

            assert_eq!(result, PbxExecResult::Success);
            let frames = driver.frames.lock();
            assert_eq!(frames.len(), 1);
            let Frame::Voice {
                codec_id: actual_codec,
                samples: actual_samples,
                data,
                ..
            } = &frames[0]
            else {
                panic!("Playback must write a voice frame");
            };
            let expected: Vec<u8> = samples
                .iter()
                .map(|sample| {
                    if codec_id == 0 {
                        linear_to_mulaw_fast(*sample)
                    } else {
                        linear_to_alaw_fast(*sample)
                    }
                })
                .collect();
            assert_eq!(*actual_codec, codec_id);
            assert_eq!(*actual_samples, SAMPLES_PER_FRAME);
            assert_eq!(data.as_ref(), expected);
            assert_ne!(
                &data[..4],
                b"RIFF",
                "the WAV header reached the media plane"
            );
            drop(frames);
            asterisk_core::channel::tech_registry::TECH_REGISTRY.unregister(&driver.name);
        }

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wav_rejects_wrong_sample_rate_and_unsupported_negotiated_codec() {
        let samples = vec![0i16; SAMPLES_PER_FRAME as usize];
        let path = write_test_wav_at_rate(&samples, 16000);
        let rate_error = load_wav_frames(&path, Some(0)).unwrap_err();
        assert!(rate_error.contains("expected 8000 Hz WAV, got 16000 Hz"));
        std::fs::remove_file(path).unwrap();

        let path = write_test_wav(&samples);
        let codec_error = load_wav_frames(&path, Some(9)).unwrap_err();
        assert!(codec_error.contains("unsupported negotiated codec PT 9"));
        std::fs::remove_file(path).unwrap();
    }
}
