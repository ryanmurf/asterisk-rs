//! Speech recognition utility applications.
//!
//! Port of app_speech_utils.c from Asterisk C. Provides dialplan applications
//! for speech recognition: creating speech structures, activating/deactivating
//! grammars, starting recognition, playing files while listening, and destroying
//! the speech structure.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

/// State of a speech recognition session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechState {
    /// Not yet started.
    NotReady,
    /// Ready to accept audio.
    Ready,
    /// Waiting for speech results.
    WaitingForResult,
    /// Results available.
    Done,
}

/// A speech recognition result.
#[derive(Debug, Clone)]
pub struct SpeechResult {
    /// The recognized text.
    pub text: String,
    /// Confidence score (0-1000).
    pub score: i32,
    /// The grammar that matched.
    pub grammar: String,
}

/// Options for SpeechBackground.
#[derive(Debug, Clone, Default)]
pub struct SpeechBackgroundOptions {
    /// Do not answer the channel.
    pub no_answer: bool,
    /// Return partial results on timeout.
    pub partial_results: bool,
}

impl SpeechBackgroundOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'n' => result.no_answer = true,
                'p' => result.partial_results = true,
                _ => {
                    debug!("SpeechBackground: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The SpeechCreate() dialplan application.
///
/// Usage: SpeechCreate(engine_name)
///
/// Creates a speech recognition structure on the channel using the
/// specified engine. Must be called before any other speech apps.
/// Sets ERROR channel variable to 1 if the engine cannot be used.
pub struct AppSpeechCreate;

impl DialplanApp for AppSpeechCreate {
    fn name(&self) -> &str {
        "SpeechCreate"
    }

    fn description(&self) -> &str {
        "Create a Speech Structure"
    }
}

impl AppSpeechCreate {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let engine_name = args.trim();
        if engine_name.is_empty() {
            warn!("SpeechCreate: engine name required");
            return PbxExecResult::Failed;
        }

        info!(
            "SpeechCreate: channel '{}' creating speech engine '{}'",
            channel.name, engine_name,
        );

        // In a real implementation:
        //
        //   let speech = ast_speech_new(engine_name, channel.nativeformats())?;
        //   if speech.is_none() {
        //       set_variable(channel, "ERROR", "1");
        //       return PbxExecResult::Failed;
        //   }
        //
        //   // Store speech structure in channel datastore
        //   let datastore = DataStore::new("speech", speech);
        //   channel.datastore_add(datastore);

        PbxExecResult::Success
    }
}

/// The SpeechActivateGrammar() dialplan application.
///
/// Usage: SpeechActivateGrammar(grammar_name)
///
/// Activates a grammar for the speech recognition engine to use.
/// Hangs up on failure.
pub struct AppSpeechActivateGrammar;

impl DialplanApp for AppSpeechActivateGrammar {
    fn name(&self) -> &str {
        "SpeechActivateGrammar"
    }

    fn description(&self) -> &str {
        "Activate a grammar"
    }
}

impl AppSpeechActivateGrammar {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let grammar_name = args.trim();
        if grammar_name.is_empty() {
            warn!("SpeechActivateGrammar: grammar name required");
            return PbxExecResult::Hangup;
        }

        info!(
            "SpeechActivateGrammar: channel '{}' activating grammar '{}'",
            channel.name, grammar_name,
        );

        // In a real implementation:
        //
        //   let speech = find_speech(channel)?;
        //   speech.grammar_activate(grammar_name)?;

        PbxExecResult::Success
    }
}

/// The SpeechStart() dialplan application.
///
/// Usage: SpeechStart()
///
/// Tells the speech recognition engine to start trying to get results
/// from the audio stream. Hangs up on failure.
pub struct AppSpeechStart;

impl DialplanApp for AppSpeechStart {
    fn name(&self) -> &str {
        "SpeechStart"
    }

    fn description(&self) -> &str {
        "Start recognizing voice in the audio stream"
    }
}

impl AppSpeechStart {
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!(
            "SpeechStart: channel '{}' starting recognition",
            channel.name,
        );

        // In a real implementation:
        //
        //   let speech = find_speech(channel)?;
        //   speech.start()?;

        PbxExecResult::Success
    }
}

/// The SpeechBackground() dialplan application.
///
/// Usage: SpeechBackground(sound_file[&file2...][,timeout[,options]])
///
/// Plays sound files while listening for speech. Once the caller starts
/// speaking, playback stops. When speech is detected and processed, the
/// results are available via SPEECH_TEXT() and SPEECH_SCORE() functions.
///
/// Options:
///   n - Don't answer the channel
///   p - Return partial results on timeout
pub struct AppSpeechBackground;

impl DialplanApp for AppSpeechBackground {
    fn name(&self) -> &str {
        "SpeechBackground"
    }

    fn description(&self) -> &str {
        "Play a sound file and wait for speech to be recognized"
    }
}

impl AppSpeechBackground {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();

        let sound_files = match parts.first() {
            Some(f) if !f.trim().is_empty() => f.trim(),
            _ => {
                warn!("SpeechBackground: sound file required");
                return PbxExecResult::Hangup;
            }
        };

        let timeout: u32 = parts
            .get(1)
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0);

        let options = parts
            .get(2)
            .map(|o| SpeechBackgroundOptions::parse(o.trim()))
            .unwrap_or_default();

        let file_list: Vec<&str> = sound_files.split('&').collect();

        info!(
            "SpeechBackground: channel '{}' playing {} file(s), timeout={}s",
            channel.name,
            file_list.len(),
            timeout,
        );

        // Answer if needed
        if !options.no_answer && channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   let speech = find_speech(channel)?;
        //
        //   // Start speech recognition
        //   speech.start()?;
        //
        //   // Play files while listening
        //   for file in &file_list {
        //       let result = play_file_with_speech(channel, file, speech).await;
        //       match result {
        //           PlayResult::SpeechDetected => break,
        //           PlayResult::Hangup => return PbxExecResult::Hangup,
        //           PlayResult::Complete => continue,
        //       }
        //   }
        //
        //   // Wait for results (with timeout)
        //   let results = speech.wait_for_results(timeout).await;
        //
        //   // Set channel variables with results
        //   for (i, result) in results.iter().enumerate() {
        //       set_variable(channel, &format!("SPEECH_TEXT({})", i), &result.text);
        //       set_variable(channel, &format!("SPEECH_SCORE({})", i), &result.score.to_string());
        //   }
        //   set_variable(channel, "SPEECH(results)", &results.len().to_string());

        PbxExecResult::Success
    }
}

/// The SpeechDeactivateGrammar() dialplan application.
///
/// Usage: SpeechDeactivateGrammar(grammar_name)
///
/// Deactivates a previously activated grammar.
pub struct AppSpeechDeactivateGrammar;

impl DialplanApp for AppSpeechDeactivateGrammar {
    fn name(&self) -> &str {
        "SpeechDeactivateGrammar"
    }

    fn description(&self) -> &str {
        "Deactivate a grammar"
    }
}

impl AppSpeechDeactivateGrammar {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let grammar_name = args.trim();
        if grammar_name.is_empty() {
            warn!("SpeechDeactivateGrammar: grammar name required");
            return PbxExecResult::Hangup;
        }

        info!(
            "SpeechDeactivateGrammar: channel '{}' deactivating grammar '{}'",
            channel.name, grammar_name,
        );

        PbxExecResult::Success
    }
}

/// The SpeechProcessingSound() dialplan application.
///
/// Usage: SpeechProcessingSound(sound_file)
///
/// Changes the processing sound that SpeechBackground plays while the
/// engine is working on results.
pub struct AppSpeechProcessingSound;

impl DialplanApp for AppSpeechProcessingSound {
    fn name(&self) -> &str {
        "SpeechProcessingSound"
    }

    fn description(&self) -> &str {
        "Change background processing sound"
    }
}

impl AppSpeechProcessingSound {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let sound_file = args.trim();
        if sound_file.is_empty() {
            warn!("SpeechProcessingSound: sound file required");
            return PbxExecResult::Hangup;
        }

        info!(
            "SpeechProcessingSound: channel '{}' set processing sound to '{}'",
            channel.name, sound_file,
        );

        PbxExecResult::Success
    }
}

/// The SpeechDestroy() dialplan application.
///
/// Usage: SpeechDestroy()
///
/// Destroys the speech recognition structure on the channel, freeing
/// all resources. Must call SpeechCreate() again to use speech after this.
pub struct AppSpeechDestroy;

impl DialplanApp for AppSpeechDestroy {
    fn name(&self) -> &str {
        "SpeechDestroy"
    }

    fn description(&self) -> &str {
        "End speech recognition"
    }
}

impl AppSpeechDestroy {
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!(
            "SpeechDestroy: channel '{}' destroying speech structure",
            channel.name,
        );

        // In a real implementation:
        //
        //   speech_datastore_destroy(channel);

        PbxExecResult::Success
    }
}

/// The SpeechLoadGrammar() dialplan application.
///
/// Usage: SpeechLoadGrammar(grammar_name,path)
///
/// Loads a grammar on the channel (not globally).
pub struct AppSpeechLoadGrammar;

impl DialplanApp for AppSpeechLoadGrammar {
    fn name(&self) -> &str {
        "SpeechLoadGrammar"
    }

    fn description(&self) -> &str {
        "Load a grammar"
    }
}

impl AppSpeechLoadGrammar {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            warn!("SpeechLoadGrammar: requires grammar_name and path");
            return PbxExecResult::Hangup;
        }

        let grammar_name = parts[0].trim();
        let path = parts[1].trim();

        info!(
            "SpeechLoadGrammar: channel '{}' loading grammar '{}' from '{}'",
            channel.name, grammar_name, path,
        );

        PbxExecResult::Success
    }
}

/// The SpeechUnloadGrammar() dialplan application.
///
/// Usage: SpeechUnloadGrammar(grammar_name)
///
/// Unloads a grammar from the channel.
pub struct AppSpeechUnloadGrammar;

impl DialplanApp for AppSpeechUnloadGrammar {
    fn name(&self) -> &str {
        "SpeechUnloadGrammar"
    }

    fn description(&self) -> &str {
        "Unload a grammar"
    }
}

impl AppSpeechUnloadGrammar {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let grammar_name = args.trim();
        if grammar_name.is_empty() {
            warn!("SpeechUnloadGrammar: grammar name required");
            return PbxExecResult::Hangup;
        }

        info!(
            "SpeechUnloadGrammar: channel '{}' unloading grammar '{}'",
            channel.name, grammar_name,
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speech_background_options() {
        let opts = SpeechBackgroundOptions::parse("np");
        assert!(opts.no_answer);
        assert!(opts.partial_results);
    }

    #[test]
    fn test_speech_state() {
        assert_ne!(SpeechState::NotReady, SpeechState::Ready);
    }

    #[tokio::test]
    async fn test_speech_create() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSpeechCreate::exec(&mut channel, "default").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_speech_create_no_engine() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSpeechCreate::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_speech_activate_grammar() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSpeechActivateGrammar::exec(&mut channel, "digits").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_speech_background() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSpeechBackground::exec(&mut channel, "beep,5,n").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_speech_destroy() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSpeechDestroy::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
