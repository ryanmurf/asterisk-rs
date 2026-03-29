//! Generic Speech Recognition API.
//!
//! Port of `res/res_speech.c` and `include/asterisk/speech.h`. Provides a
//! pluggable speech recognition framework that allows different speech
//! engines to be registered and used through a common API.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::info;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SpeechError {
    #[error("speech engine not found: {0}")]
    EngineNotFound(String),
    #[error("speech engine already registered: {0}")]
    EngineAlreadyRegistered(String),
    #[error("speech not ready (state: {0:?})")]
    NotReady(SpeechState),
    #[error("speech engine error: {0}")]
    EngineError(String),
    #[error("speech grammar error: {0}")]
    GrammarError(String),
}

pub type SpeechResult<T> = Result<T, SpeechError>;

// ---------------------------------------------------------------------------
// Speech state (from ast_speech_states)
// ---------------------------------------------------------------------------

/// State of a speech recognition session.
///
/// Mirrors `enum ast_speech_states` from the C header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpeechState {
    /// Not ready to accept audio.
    NotReady,
    /// Ready and accepting audio.
    Ready,
    /// Waiting for results (audio was received, engine heard speech).
    Wait,
    /// Processing is done, results are available.
    Done,
}

impl Default for SpeechState {
    fn default() -> Self {
        Self::NotReady
    }
}

// ---------------------------------------------------------------------------
// Speech flags (from ast_speech_flags)
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Flags on a speech recognition session.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SpeechFlags: u32 {
        /// Output is quiet (speaker is talking).
        const QUIET        = 1 << 0;
        /// Speaker has spoken.
        const SPOKE        = 1 << 1;
        /// Results are available.
        const HAVE_RESULTS = 1 << 2;
    }
}

// ---------------------------------------------------------------------------
// Results type
// ---------------------------------------------------------------------------

/// Type of results requested from the speech engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechResultsType {
    /// Normal (single best result).
    Normal,
    /// N-Best (multiple alternative results).
    NBest,
}

impl SpeechResultsType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::NBest => "nbest",
        }
    }
}

impl Default for SpeechResultsType {
    fn default() -> Self {
        Self::Normal
    }
}

// ---------------------------------------------------------------------------
// Speech result
// ---------------------------------------------------------------------------

/// A single speech recognition result.
///
/// Mirrors `struct ast_speech_result` from the C header.
#[derive(Debug, Clone)]
pub struct SpeechRecognitionResult {
    /// Recognized text.
    pub text: String,
    /// Confidence score (0-100, higher is better).
    pub score: i32,
    /// N-Best alternative number (0 for primary result).
    pub nbest_num: i32,
    /// Grammar that matched.
    pub grammar: String,
}

impl SpeechRecognitionResult {
    pub fn new(text: &str, score: i32, grammar: &str) -> Self {
        Self {
            text: text.to_string(),
            score,
            nbest_num: 0,
            grammar: grammar.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Speech engine trait
// ---------------------------------------------------------------------------

/// A pluggable speech recognition engine.
///
/// Mirrors `struct ast_speech_engine` from the C header. Each method
/// corresponds to a function pointer in the C struct.
///
/// Required methods: `create`, `write`, `destroy`.
/// Optional methods have default implementations that return errors.
pub trait SpeechEngine: Send + Sync + fmt::Debug {
    /// Engine name.
    fn name(&self) -> &str;

    /// Create/initialize engine-specific data for a new speech session.
    fn create(&self, speech: &mut Speech) -> SpeechResult<()>;

    /// Destroy engine-specific data for a speech session.
    fn destroy(&self, speech: &mut Speech) -> SpeechResult<()>;

    /// Write audio data to the engine for recognition.
    fn write(&self, speech: &mut Speech, data: &[u8]) -> SpeechResult<()>;

    /// Signal that a DTMF digit was received.
    fn dtmf(&self, _speech: &mut Speech, _dtmf: &str) -> SpeechResult<()> {
        Ok(())
    }

    /// Prepare the engine to start accepting audio.
    fn start(&self, _speech: &mut Speech) -> SpeechResult<()> {
        Ok(())
    }

    /// Load a grammar into the speech session.
    fn load_grammar(
        &self,
        _speech: &mut Speech,
        _grammar_name: &str,
        _grammar_data: &str,
    ) -> SpeechResult<()> {
        Err(SpeechError::GrammarError("load not supported".into()))
    }

    /// Unload a grammar from the speech session.
    fn unload_grammar(&self, _speech: &mut Speech, _grammar_name: &str) -> SpeechResult<()> {
        Err(SpeechError::GrammarError("unload not supported".into()))
    }

    /// Activate a loaded grammar.
    fn activate_grammar(&self, _speech: &mut Speech, _grammar_name: &str) -> SpeechResult<()> {
        Err(SpeechError::GrammarError("activate not supported".into()))
    }

    /// Deactivate a loaded grammar.
    fn deactivate_grammar(&self, _speech: &mut Speech, _grammar_name: &str) -> SpeechResult<()> {
        Err(SpeechError::GrammarError("deactivate not supported".into()))
    }

    /// Change an engine-specific setting.
    fn change(&self, _speech: &mut Speech, _name: &str, _value: &str) -> SpeechResult<()> {
        Ok(())
    }

    /// Get an engine-specific setting.
    fn get_setting(&self, _speech: &Speech, _name: &str) -> SpeechResult<String> {
        Err(SpeechError::EngineError("get_setting not supported".into()))
    }

    /// Get recognition results.
    fn get_results(&self, _speech: &Speech) -> SpeechResult<Vec<SpeechRecognitionResult>> {
        Ok(Vec::new())
    }

    /// Change the type of results wanted.
    fn change_results_type(
        &self,
        _speech: &mut Speech,
        _results_type: SpeechResultsType,
    ) -> SpeechResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Speech session
// ---------------------------------------------------------------------------

/// A speech recognition session.
///
/// Mirrors `struct ast_speech` from the C header.
pub struct Speech {
    /// Current state.
    pub state: SpeechState,
    /// Session flags.
    pub flags: SpeechFlags,
    /// Audio format name (e.g., "slin").
    pub format: String,
    /// Processing sound file to play while engine processes.
    pub processing_sound: Option<String>,
    /// Cached results.
    pub results: Vec<SpeechRecognitionResult>,
    /// Desired results type.
    pub results_type: SpeechResultsType,
    /// Engine name.
    pub engine_name: String,
    /// Engine-specific opaque data.
    pub engine_data: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl Speech {
    /// Create a new speech session (not yet bound to an engine).
    fn new(engine_name: &str, format: &str) -> Self {
        Self {
            state: SpeechState::NotReady,
            flags: SpeechFlags::empty(),
            format: format.to_string(),
            processing_sound: None,
            results: Vec::new(),
            results_type: SpeechResultsType::Normal,
            engine_name: engine_name.to_string(),
            engine_data: None,
        }
    }

    /// Change the state of the speech session.
    ///
    /// Mirrors `ast_speech_change_state()`.
    pub fn change_state(&mut self, state: SpeechState) {
        if state == SpeechState::Wait {
            self.flags |= SpeechFlags::SPOKE;
        }
        self.state = state;
    }

    /// Start recognition: clear flags and results, transition to Ready.
    pub fn start_recognition(&mut self) {
        self.flags.remove(SpeechFlags::SPOKE);
        self.flags.remove(SpeechFlags::QUIET);
        self.flags.remove(SpeechFlags::HAVE_RESULTS);
        self.results.clear();
    }
}

impl fmt::Debug for Speech {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Speech")
            .field("engine", &self.engine_name)
            .field("state", &self.state)
            .field("flags", &self.flags)
            .field("format", &self.format)
            .field("results", &self.results.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Speech engine registry
// ---------------------------------------------------------------------------

/// Registry of speech recognition engines.
///
/// Mirrors the `engines` linked list and `ast_speech_register` /
/// `ast_speech_unregister` functions from the C source.
pub struct SpeechEngineRegistry {
    /// Registered engines keyed by name.
    engines: RwLock<HashMap<String, Arc<dyn SpeechEngine>>>,
    /// Default engine name.
    default_engine: RwLock<Option<String>>,
}

impl SpeechEngineRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            engines: RwLock::new(HashMap::new()),
            default_engine: RwLock::new(None),
        }
    }

    /// Register a speech recognition engine.
    ///
    /// The first engine registered becomes the default.
    pub fn register(&self, engine: Arc<dyn SpeechEngine>) -> SpeechResult<()> {
        let name = engine.name().to_string();

        let mut engines = self.engines.write();
        if engines.contains_key(&name) {
            return Err(SpeechError::EngineAlreadyRegistered(name));
        }

        info!(engine = %name, "Registered speech recognition engine");
        engines.insert(name.clone(), engine);

        // First engine becomes default.
        let mut default = self.default_engine.write();
        if default.is_none() {
            *default = Some(name.clone());
            info!(engine = %name, "Set as default speech recognition engine");
        }

        Ok(())
    }

    /// Unregister a speech recognition engine.
    pub fn unregister(&self, name: &str) -> SpeechResult<Arc<dyn SpeechEngine>> {
        let mut engines = self.engines.write();
        let engine = engines.remove(name).ok_or_else(|| {
            SpeechError::EngineNotFound(name.to_string())
        })?;

        // Update default if needed.
        let mut default = self.default_engine.write();
        if default.as_deref() == Some(name) {
            *default = engines.keys().next().cloned();
            if let Some(ref new_default) = *default {
                info!(engine = %new_default, "New default speech recognition engine");
            }
        }

        info!(engine = %name, "Unregistered speech recognition engine");
        Ok(engine)
    }

    /// Find an engine by name, or return the default engine if name is empty.
    pub fn find(&self, name: &str) -> Option<Arc<dyn SpeechEngine>> {
        let engines = self.engines.read();
        if name.is_empty() {
            let default_name = self.default_engine.read();
            default_name.as_ref().and_then(|n| engines.get(n).cloned())
        } else {
            engines.get(name).cloned()
        }
    }

    /// Create a new speech session using the named engine.
    ///
    /// Mirrors `ast_speech_new()`.
    pub fn create_speech(&self, engine_name: &str) -> SpeechResult<(Speech, Arc<dyn SpeechEngine>)> {
        let engine = self.find(engine_name).ok_or_else(|| {
            SpeechError::EngineNotFound(
                if engine_name.is_empty() { "default".to_string() } else { engine_name.to_string() }
            )
        })?;

        let mut speech = Speech::new(engine.name(), "slin");
        engine.create(&mut speech)?;
        speech.change_state(SpeechState::NotReady);

        Ok((speech, engine))
    }

    /// List all registered engine names.
    pub fn engine_names(&self) -> Vec<String> {
        self.engines.read().keys().cloned().collect()
    }

    /// Get the default engine name.
    pub fn default_engine_name(&self) -> Option<String> {
        self.default_engine.read().clone()
    }
}

impl Default for SpeechEngineRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Convenience functions (mirror C API)
// ---------------------------------------------------------------------------

/// Write audio data to a speech session.
///
/// Mirrors `ast_speech_write()`.
pub fn speech_write(
    speech: &mut Speech,
    engine: &dyn SpeechEngine,
    data: &[u8],
) -> SpeechResult<()> {
    if speech.state != SpeechState::Ready {
        return Err(SpeechError::NotReady(speech.state));
    }
    engine.write(speech, data)
}

/// Signal DTMF to a speech session.
///
/// Mirrors `ast_speech_dtmf()`.
pub fn speech_dtmf(
    speech: &mut Speech,
    engine: &dyn SpeechEngine,
    dtmf: &str,
) -> SpeechResult<()> {
    if speech.state != SpeechState::Ready {
        return Err(SpeechError::NotReady(speech.state));
    }
    engine.dtmf(speech, dtmf)
}

/// Start speech recognition on a session.
///
/// Mirrors `ast_speech_start()`.
pub fn speech_start(speech: &mut Speech, engine: &dyn SpeechEngine) -> SpeechResult<()> {
    speech.start_recognition();
    engine.start(speech)?;
    Ok(())
}

/// Get results from a speech session.
///
/// Mirrors `ast_speech_results_get()`.
pub fn speech_get_results(
    speech: &Speech,
    engine: &dyn SpeechEngine,
) -> SpeechResult<Vec<SpeechRecognitionResult>> {
    engine.get_results(speech)
}

/// Load a grammar.
///
/// Mirrors `ast_speech_grammar_load()`.
pub fn speech_grammar_load(
    speech: &mut Speech,
    engine: &dyn SpeechEngine,
    grammar_name: &str,
    grammar_data: &str,
) -> SpeechResult<()> {
    engine.load_grammar(speech, grammar_name, grammar_data)
}

/// Activate a grammar.
///
/// Mirrors `ast_speech_grammar_activate()`.
pub fn speech_grammar_activate(
    speech: &mut Speech,
    engine: &dyn SpeechEngine,
    grammar_name: &str,
) -> SpeechResult<()> {
    engine.activate_grammar(speech, grammar_name)
}

/// Deactivate a grammar.
///
/// Mirrors `ast_speech_grammar_deactivate()`.
pub fn speech_grammar_deactivate(
    speech: &mut Speech,
    engine: &dyn SpeechEngine,
    grammar_name: &str,
) -> SpeechResult<()> {
    engine.deactivate_grammar(speech, grammar_name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A test/mock speech engine.
    #[derive(Debug)]
    struct MockSpeechEngine {
        name: String,
    }

    impl SpeechEngine for MockSpeechEngine {
        fn name(&self) -> &str {
            &self.name
        }

        fn create(&self, speech: &mut Speech) -> SpeechResult<()> {
            speech.change_state(SpeechState::Ready);
            Ok(())
        }

        fn destroy(&self, _speech: &mut Speech) -> SpeechResult<()> {
            Ok(())
        }

        fn write(&self, _speech: &mut Speech, _data: &[u8]) -> SpeechResult<()> {
            Ok(())
        }

        fn get_results(&self, _speech: &Speech) -> SpeechResult<Vec<SpeechRecognitionResult>> {
            Ok(vec![SpeechRecognitionResult::new("hello world", 95, "default")])
        }
    }

    #[test]
    fn test_speech_state_default() {
        assert_eq!(SpeechState::default(), SpeechState::NotReady);
    }

    #[test]
    fn test_speech_result() {
        let result = SpeechRecognitionResult::new("test", 85, "my_grammar");
        assert_eq!(result.text, "test");
        assert_eq!(result.score, 85);
        assert_eq!(result.grammar, "my_grammar");
    }

    #[test]
    fn test_speech_state_transition() {
        let mut speech = Speech::new("test", "slin");
        assert_eq!(speech.state, SpeechState::NotReady);

        speech.change_state(SpeechState::Ready);
        assert_eq!(speech.state, SpeechState::Ready);

        speech.change_state(SpeechState::Wait);
        assert_eq!(speech.state, SpeechState::Wait);
        assert!(speech.flags.contains(SpeechFlags::SPOKE));
    }

    #[test]
    fn test_speech_start_recognition() {
        let mut speech = Speech::new("test", "slin");
        speech.flags = SpeechFlags::SPOKE | SpeechFlags::HAVE_RESULTS;
        speech.results.push(SpeechRecognitionResult::new("old", 50, "g"));

        speech.start_recognition();
        assert!(!speech.flags.contains(SpeechFlags::SPOKE));
        assert!(!speech.flags.contains(SpeechFlags::HAVE_RESULTS));
        assert!(speech.results.is_empty());
    }

    #[test]
    fn test_engine_registry_register() {
        let registry = SpeechEngineRegistry::new();
        let engine = Arc::new(MockSpeechEngine {
            name: "mock".to_string(),
        });

        registry.register(engine.clone()).unwrap();
        assert!(registry.find("mock").is_some());
        assert_eq!(registry.default_engine_name(), Some("mock".to_string()));
    }

    #[test]
    fn test_engine_registry_duplicate() {
        let registry = SpeechEngineRegistry::new();
        let engine = Arc::new(MockSpeechEngine {
            name: "mock".to_string(),
        });

        registry.register(engine.clone()).unwrap();
        let result = registry.register(engine);
        assert!(matches!(result, Err(SpeechError::EngineAlreadyRegistered(_))));
    }

    #[test]
    fn test_engine_registry_unregister() {
        let registry = SpeechEngineRegistry::new();
        let engine = Arc::new(MockSpeechEngine {
            name: "mock".to_string(),
        });

        registry.register(engine).unwrap();
        let removed = registry.unregister("mock").unwrap();
        assert_eq!(removed.name(), "mock");
        assert!(registry.find("mock").is_none());
    }

    #[test]
    fn test_engine_registry_default_fallback() {
        let registry = SpeechEngineRegistry::new();
        let engine1 = Arc::new(MockSpeechEngine {
            name: "engine1".to_string(),
        });
        let engine2 = Arc::new(MockSpeechEngine {
            name: "engine2".to_string(),
        });

        registry.register(engine1).unwrap();
        registry.register(engine2).unwrap();

        // Default should be engine1 (first registered).
        assert_eq!(registry.default_engine_name(), Some("engine1".to_string()));

        // Empty name should return default.
        assert_eq!(registry.find("").unwrap().name(), "engine1");

        // Unregister default; new default should be assigned.
        registry.unregister("engine1").unwrap();
        assert!(registry.default_engine_name().is_some());
    }

    #[test]
    fn test_create_speech_session() {
        let registry = SpeechEngineRegistry::new();
        let engine = Arc::new(MockSpeechEngine {
            name: "mock".to_string(),
        });
        registry.register(engine).unwrap();

        let (speech, engine) = registry.create_speech("mock").unwrap();
        assert_eq!(speech.engine_name, "mock");
        // Engine create() sets state to Ready, but create_speech then resets to NotReady.
        // (matching C behavior)
        assert_eq!(speech.state, SpeechState::NotReady);
    }

    #[test]
    fn test_speech_write_requires_ready() {
        let engine = MockSpeechEngine {
            name: "mock".to_string(),
        };
        let mut speech = Speech::new("mock", "slin");

        // Not ready: should fail.
        let result = speech_write(&mut speech, &engine, &[0u8; 320]);
        assert!(matches!(result, Err(SpeechError::NotReady(_))));

        // Set to ready: should succeed.
        speech.change_state(SpeechState::Ready);
        speech_write(&mut speech, &engine, &[0u8; 320]).unwrap();
    }

    #[test]
    fn test_speech_get_results() {
        let engine = MockSpeechEngine {
            name: "mock".to_string(),
        };
        let speech = Speech::new("mock", "slin");

        let results = speech_get_results(&speech, &engine).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "hello world");
        assert_eq!(results[0].score, 95);
    }

    #[test]
    fn test_speech_results_type() {
        assert_eq!(SpeechResultsType::Normal.as_str(), "normal");
        assert_eq!(SpeechResultsType::NBest.as_str(), "nbest");
    }
}
