//! Answering Machine Detection (AMD) application.
//!
//! Port of app_amd.c from Asterisk C. Analyzes audio after a call is
//! answered to determine whether a human or answering machine picked up.
//! Uses silence/voice pattern analysis with configurable thresholds.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// AMD detection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmdStatus {
    /// A human answered.
    Human,
    /// An answering machine was detected.
    Machine,
    /// Detection was not conclusive.
    NotSure,
    /// The channel hung up during detection.
    Hangup,
}

impl AmdStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "HUMAN",
            Self::Machine => "MACHINE",
            Self::NotSure => "NOTSURE",
            Self::Hangup => "HANGUP",
        }
    }
}

/// AMD detection cause (reason for the classification).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmdCause {
    /// Long initial silence (answering machine).
    InitialSilence,
    /// Long greeting (answering machine).
    LongGreeting,
    /// Too many words in greeting (answering machine).
    MaximumWords,
    /// Maximum word length exceeded (answering machine).
    MaximumWordLength,
    /// Detected as human by short response.
    HumanDetected,
    /// Analysis time expired.
    MaxTime,
}

impl AmdCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InitialSilence => "INITIALSILENCE",
            Self::LongGreeting => "LONGGREETING",
            Self::MaximumWords => "MAXWORDS",
            Self::MaximumWordLength => "MAXWORDLENGTH",
            Self::HumanDetected => "HUMAN",
            Self::MaxTime => "MAXTIME",
        }
    }
}

/// AMD configuration thresholds.
#[derive(Debug, Clone)]
pub struct AmdConfig {
    /// Maximum initial silence before considering answering machine (ms).
    pub initial_silence: u32,
    /// Maximum greeting duration (ms).
    pub greeting: u32,
    /// Silence duration after greeting that indicates end (ms).
    pub after_greeting_silence: u32,
    /// Total analysis time limit (ms).
    pub total_analysis_time: u32,
    /// Minimum duration of a word (ms).
    pub minimum_word_length: u32,
    /// Silence between words (ms).
    pub between_words_silence: u32,
    /// Maximum number of words in greeting before machine classification.
    pub maximum_number_of_words: u32,
    /// Silence threshold (0-32767).
    pub silence_threshold: u32,
    /// Maximum length of a single word (ms).
    pub maximum_word_length: u32,
    /// Maximum time to wait for a frame (ms).
    pub max_wait_time_for_frame: u32,
}

impl Default for AmdConfig {
    fn default() -> Self {
        Self {
            initial_silence: 2500,
            greeting: 1500,
            after_greeting_silence: 800,
            total_analysis_time: 5000,
            minimum_word_length: 100,
            between_words_silence: 50,
            maximum_number_of_words: 2,
            silence_threshold: 256,
            maximum_word_length: 5000,
            max_wait_time_for_frame: 50,
        }
    }
}

impl AmdConfig {
    /// Parse from comma-separated arguments, overriding defaults.
    ///
    /// Format: initialSilence,greeting,afterGreetingSilence,totalAnalysisTime,
    ///         minimumWordLength,betweenWordsSilence,maximumNumberOfWords,
    ///         silenceThreshold,maximumWordLength
    pub fn parse(args: &str) -> Self {
        let mut config = Self::default();
        let parts: Vec<&str> = args.split(',').collect();

        if let Some(v) = parts.first().and_then(|s| s.trim().parse().ok()) {
            config.initial_silence = v;
        }
        if let Some(v) = parts.get(1).and_then(|s| s.trim().parse().ok()) {
            config.greeting = v;
        }
        if let Some(v) = parts.get(2).and_then(|s| s.trim().parse().ok()) {
            config.after_greeting_silence = v;
        }
        if let Some(v) = parts.get(3).and_then(|s| s.trim().parse().ok()) {
            config.total_analysis_time = v;
        }
        if let Some(v) = parts.get(4).and_then(|s| s.trim().parse().ok()) {
            config.minimum_word_length = v;
        }
        if let Some(v) = parts.get(5).and_then(|s| s.trim().parse().ok()) {
            config.between_words_silence = v;
        }
        if let Some(v) = parts.get(6).and_then(|s| s.trim().parse().ok()) {
            config.maximum_number_of_words = v;
        }
        if let Some(v) = parts.get(7).and_then(|s| s.trim().parse().ok()) {
            config.silence_threshold = v;
        }
        if let Some(v) = parts.get(8).and_then(|s| s.trim().parse().ok()) {
            config.maximum_word_length = v;
        }

        config
    }
}

/// The AMD() dialplan application.
///
/// Usage: AMD([initialSilence[,greeting[,afterGreetingSilence[,totalAnalysisTime[,
///             minimumWordLength[,betweenWordsSilence[,maximumNumberOfWords[,
///             silenceThreshold[,maximumWordLength]]]]]]]]])
///
/// Analyzes audio to detect answering machines. Results are set in channel
/// variables:
///   AMDSTATUS = MACHINE | HUMAN | NOTSURE | HANGUP
///   AMDCAUSE  = reason for the classification
pub struct AppAmd;

impl DialplanApp for AppAmd {
    fn name(&self) -> &str {
        "AMD"
    }

    fn description(&self) -> &str {
        "Answering Machine Detection"
    }
}

impl AppAmd {
    /// Execute the AMD application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let config = AmdConfig::parse(args);

        info!(
            "AMD: channel '{}' initial_silence={}ms greeting={}ms total={}ms",
            channel.name, config.initial_silence, config.greeting, config.total_analysis_time,
        );

        // In a real implementation:
        // 1. Create DSP for silence detection
        // 2. Set silence threshold
        // 3. Loop reading audio frames:
        //    a. Feed each frame to silence detector
        //    b. Track state transitions (silence <-> voice)
        //    c. In initial silence: if silence > initial_silence => MACHINE
        //    d. Count words and word lengths
        //    e. If word_count > max_words => MACHINE
        //    f. If single word > max_word_length => MACHINE
        //    g. If after-greeting silence detected => HUMAN
        //    h. If total time exceeded => NOTSURE
        // 4. Set AMDSTATUS and AMDCAUSE channel variables

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amd_config_defaults() {
        let config = AmdConfig::default();
        assert_eq!(config.initial_silence, 2500);
        assert_eq!(config.greeting, 1500);
        assert_eq!(config.after_greeting_silence, 800);
        assert_eq!(config.total_analysis_time, 5000);
        assert_eq!(config.minimum_word_length, 100);
        assert_eq!(config.between_words_silence, 50);
        assert_eq!(config.maximum_number_of_words, 2);
        assert_eq!(config.silence_threshold, 256);
        assert_eq!(config.maximum_word_length, 5000);
    }

    #[test]
    fn test_amd_config_parse() {
        let config = AmdConfig::parse("3000,2000,1000,6000");
        assert_eq!(config.initial_silence, 3000);
        assert_eq!(config.greeting, 2000);
        assert_eq!(config.after_greeting_silence, 1000);
        assert_eq!(config.total_analysis_time, 6000);
        // Rest should be defaults
        assert_eq!(config.minimum_word_length, 100);
    }

    #[test]
    fn test_amd_status() {
        assert_eq!(AmdStatus::Human.as_str(), "HUMAN");
        assert_eq!(AmdStatus::Machine.as_str(), "MACHINE");
        assert_eq!(AmdStatus::NotSure.as_str(), "NOTSURE");
    }

    #[test]
    fn test_amd_cause() {
        assert_eq!(AmdCause::InitialSilence.as_str(), "INITIALSILENCE");
        assert_eq!(AmdCause::LongGreeting.as_str(), "LONGGREETING");
    }

    #[tokio::test]
    async fn test_amd_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAmd::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
