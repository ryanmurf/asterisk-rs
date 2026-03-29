//! Port of asterisk/tests/test_aeap_speech.c
//!
//! Tests the AEAP speech engine interface:
//!
//! - Speech engine creation with codecs
//! - DTMF input
//! - Setting/getting speech parameters
//! - Changing results type
//! - Retrieving speech recognition results
//!
//! Since we do not have a live WebSocket speech server, we model the
//! speech engine and its request/response protocol locally.

use serde_json::{json, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Speech engine model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeechResultsType {
    Normal,
    NBest,
}

#[derive(Debug, Clone)]
struct SpeechResult {
    text: String,
    score: i32,
    grammar: String,
    nbest_num: i32,
}

struct SpeechEngine {
    name: String,
    started: bool,
    results_type: SpeechResultsType,
    settings: HashMap<String, String>,
    results: Vec<SpeechResult>,
    codecs: Vec<String>,
}

impl SpeechEngine {
    fn new(name: &str, codecs: Vec<String>) -> Option<Self> {
        if codecs.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            started: false,
            results_type: SpeechResultsType::Normal,
            settings: HashMap::new(),
            results: Vec::new(),
            codecs,
        })
    }

    fn start(&mut self) {
        self.started = true;
    }

    fn dtmf(&mut self, digit: &str) -> Result<(), String> {
        if digit.is_empty() {
            return Err("Empty DTMF digit".to_string());
        }
        // Just validate the DTMF is accepted
        Ok(())
    }

    fn change(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.settings.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn change_results_type(&mut self, rtype: SpeechResultsType) -> Result<(), String> {
        self.results_type = rtype;
        Ok(())
    }

    fn get_setting(&self, key: &str) -> Option<String> {
        // Simulate the server returning "bar" for any setting request
        // (matching the C test's speech_test_server_get behavior)
        if self.settings.contains_key(key) {
            Some(self.settings.get(key).unwrap().clone())
        } else {
            Some("bar".to_string())
        }
    }

    fn get_results(&self) -> Vec<SpeechResult> {
        // Simulate the server returning fixed results matching
        // TEST_SPEECH_RESULTS_* from the C test
        vec![SpeechResult {
            text: "foo".to_string(),
            score: 7,
            grammar: "bar".to_string(),
            nbest_num: 1,
        }]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(res_speech_aeap_test).
///
/// Test the full speech AEAP lifecycle: create, start, DTMF, settings, results.
#[test]
fn test_speech_aeap_lifecycle() {
    let codecs = vec!["ulaw".to_string()];
    let mut speech = SpeechEngine::new("_aeap_test_speech_", codecs).unwrap();

    // Start the engine
    speech.start();
    assert!(speech.started);

    // Send DTMF
    assert!(speech.dtmf("1").is_ok());

    // Change a setting
    assert!(speech.change("foo", "bar").is_ok());

    // Change results type
    assert!(speech.change_results_type(SpeechResultsType::NBest).is_ok());
    assert_eq!(speech.results_type, SpeechResultsType::NBest);

    // Get a setting
    let setting = speech.get_setting("foo").unwrap();
    assert_eq!(setting, "bar");

    // Get results
    let results = speech.get_results();
    assert!(!results.is_empty());

    let result = &results[0];
    assert_eq!(result.text, "foo");
    assert_eq!(result.score, 7);
    assert_eq!(result.grammar, "bar");
    assert_eq!(result.nbest_num, 1);
}
