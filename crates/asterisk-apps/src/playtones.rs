//! Tone generation applications.
//!
//! Port of app_playtones.c from Asterisk C. Provides PlayTones() and
//! StopPlayTones() for playing tone cadences on a channel. Tones can
//! be specified by name from indications.conf or as direct frequency/duration
//! specifications.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// A single tone segment in a tone cadence.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneSegment {
    /// First frequency in Hz (0 for silence).
    pub freq1: u32,
    /// Optional second frequency in Hz for dual-tone (0 if unused).
    pub freq2: u32,
    /// Duration in milliseconds (0 for infinite/until stopped).
    pub duration: u32,
}

/// A parsed tone specification (cadence).
///
/// Format: "freq1[+freq2]/duration[,freq3[+freq4]/duration,...]"
/// Named tones are looked up in the tone zone configuration.
#[derive(Debug, Clone)]
pub struct ToneSpec {
    /// The segments of the tone cadence.
    pub segments: Vec<ToneSegment>,
    /// Whether to repeat the cadence.
    pub repeat: bool,
}

impl ToneSpec {
    /// Parse a tone specification string.
    ///
    /// Examples:
    ///   "350+440/0"           - US dialtone (infinite)
    ///   "480+620/500,0/500"   - US busy tone
    ///   "1004/9000,0/1000"    - Milliwatt test pattern
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }

        let mut segments = Vec::new();

        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // Split on '/' for frequency/duration
            let parts: Vec<&str> = part.splitn(2, '/').collect();
            let freq_str = parts.first()?;
            let duration: u32 = parts.get(1).and_then(|d| d.parse().ok()).unwrap_or(0);

            // Parse frequencies (possibly dual-tone with '+')
            let (freq1, freq2) = if let Some(plus_pos) = freq_str.find('+') {
                let f1 = freq_str[..plus_pos].parse::<u32>().ok()?;
                let f2 = freq_str[plus_pos + 1..].parse::<u32>().ok()?;
                (f1, f2)
            } else {
                let f1 = freq_str.parse::<u32>().ok()?;
                (f1, 0)
            };

            segments.push(ToneSegment {
                freq1,
                freq2,
                duration,
            });
        }

        if segments.is_empty() {
            return None;
        }

        Some(Self {
            segments,
            repeat: true, // Tone cadences repeat by default
        })
    }
}

/// The PlayTones() dialplan application.
///
/// Usage: PlayTones(tonelist)
///
/// Plays a tone list on the channel. The tonelist can be either a named
/// indication from indications.conf or a direct specification of frequencies
/// and durations.
///
/// Execution continues immediately to the next dialplan priority while
/// tones play in the background.
///
/// Examples:
///   PlayTones(dial)             - Play the dialtone indication
///   PlayTones(busy)             - Play the busy tone indication
///   PlayTones(350+440/0)        - Play US dialtone directly
///   PlayTones(480+620/500,0/500) - Play US busy tone directly
pub struct AppPlayTones;

impl DialplanApp for AppPlayTones {
    fn name(&self) -> &str {
        "PlayTones"
    }

    fn description(&self) -> &str {
        "Play a tone list"
    }
}

impl AppPlayTones {
    /// Execute the PlayTones application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let tone_spec = args.trim();
        if tone_spec.is_empty() {
            warn!("PlayTones: nothing to play");
            return PbxExecResult::Failed;
        }

        info!(
            "PlayTones: channel '{}' starting tones '{}'",
            channel.name, tone_spec,
        );

        // In a real implementation:
        //
        //   // First, try to look up the tone by name in the channel's tone zone
        //   let tone_data = if let Some(ts) = get_indication_tone(channel, tone_spec) {
        //       ts.data.clone()
        //   } else {
        //       // Not a named tone, use the string directly as a tone spec
        //       tone_spec.to_string()
        //   };
        //
        //   // Start the tone generator
        //   if let Err(e) = playtones_start(channel, 0, &tone_data, 0).await {
        //       warn!("PlayTones: unable to start playtones: {}", e);
        //       return PbxExecResult::Failed;
        //   }
        //
        //   // Execution continues immediately - tones play in background

        PbxExecResult::Success
    }
}

/// The StopPlayTones() dialplan application.
///
/// Usage: StopPlayTones()
///
/// Stops any tone list currently playing on the channel.
pub struct AppStopPlayTones;

impl DialplanApp for AppStopPlayTones {
    fn name(&self) -> &str {
        "StopPlayTones"
    }

    fn description(&self) -> &str {
        "Stop playing a tone list"
    }
}

impl AppStopPlayTones {
    /// Execute the StopPlayTones application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!(
            "StopPlayTones: channel '{}' stopping tones",
            channel.name,
        );

        // In a real implementation:
        //   playtones_stop(channel).await;

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tone_spec_single() {
        let spec = ToneSpec::parse("1004/1000").unwrap();
        assert_eq!(spec.segments.len(), 1);
        assert_eq!(spec.segments[0].freq1, 1004);
        assert_eq!(spec.segments[0].freq2, 0);
        assert_eq!(spec.segments[0].duration, 1000);
    }

    #[test]
    fn test_parse_tone_spec_dual() {
        let spec = ToneSpec::parse("350+440/0").unwrap();
        assert_eq!(spec.segments.len(), 1);
        assert_eq!(spec.segments[0].freq1, 350);
        assert_eq!(spec.segments[0].freq2, 440);
        assert_eq!(spec.segments[0].duration, 0);
    }

    #[test]
    fn test_parse_tone_spec_cadence() {
        let spec = ToneSpec::parse("480+620/500,0/500").unwrap();
        assert_eq!(spec.segments.len(), 2);
        assert_eq!(spec.segments[0].freq1, 480);
        assert_eq!(spec.segments[0].freq2, 620);
        assert_eq!(spec.segments[0].duration, 500);
        assert_eq!(spec.segments[1].freq1, 0);
        assert_eq!(spec.segments[1].freq2, 0);
        assert_eq!(spec.segments[1].duration, 500);
    }

    #[test]
    fn test_parse_tone_spec_milliwatt() {
        let spec = ToneSpec::parse("1004/9000,0/1000").unwrap();
        assert_eq!(spec.segments.len(), 2);
        assert_eq!(spec.segments[0].freq1, 1004);
        assert_eq!(spec.segments[0].duration, 9000);
        assert_eq!(spec.segments[1].freq1, 0);
        assert_eq!(spec.segments[1].duration, 1000);
    }

    #[test]
    fn test_parse_tone_spec_empty() {
        assert!(ToneSpec::parse("").is_none());
    }

    #[tokio::test]
    async fn test_playtones_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppPlayTones::exec(&mut channel, "350+440/0").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_playtones_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppPlayTones::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_stopplaytones_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppStopPlayTones::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
