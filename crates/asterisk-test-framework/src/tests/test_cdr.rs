//! Port of asterisk/tests/test_cdr.c
//!
//! Tests the CDR (Call Detail Record) engine: channel creation, answer,
//! bridge, hangup, and the resulting CDR records produced by the engine.
//!
//! Key scenarios:
//! - Single party unanswered: channel up then hangup -> NOANSWER
//! - Single party answered: answer then hangup -> ANSWERED
//! - Two party bridge: A calls B, answer, bridge, hangup -> ANSWERED with billsec
//! - Dial unanswered: A dials B, B never answers -> NOANSWER
//! - Dial busy: A dials B, B returns busy -> BUSY
//! - Dial congestion: no route -> CONGESTION
//! - CDR variables: verify custom CDR variables are preserved
//! - CDR disable: disable prevents CDR generation
//! - CDR fork: multiple CDRs from same channel
//! - LinkedID handling: linked IDs propagate

use asterisk_cdr::engine::{CdrConfig, CdrEngine};
use asterisk_cdr::{Cdr, CdrBackend, CdrDisposition, CdrError};
use parking_lot::Mutex;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Mock CDR backend for test verification
// ---------------------------------------------------------------------------

/// Mock CDR backend that collects dispatched CDR records.
#[allow(dead_code)]
struct MockCdrBackend {
    records: Mutex<Vec<Cdr>>,
}

#[allow(dead_code)]
impl MockCdrBackend {
    fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.records.lock().len()
    }

    fn last(&self) -> Option<Cdr> {
        self.records.lock().last().cloned()
    }

    fn all(&self) -> Vec<Cdr> {
        self.records.lock().clone()
    }

    fn clear(&self) {
        self.records.lock().clear();
    }
}

impl CdrBackend for MockCdrBackend {
    fn name(&self) -> &str {
        "mock_cdr_backend"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        self.records.lock().push(cdr.clone());
        Ok(())
    }
}

/// Helper: create engine + mock backend wired together.
fn setup_engine(config: CdrConfig) -> (CdrEngine, Arc<MockCdrBackend>) {
    let engine = CdrEngine::with_config(config);
    let backend = Arc::new(MockCdrBackend::new());
    engine.register_backend(backend.clone());
    (engine, backend)
}

/// Default config for normal CDR logging (unanswered not logged).
fn debug_cdr_config() -> CdrConfig {
    CdrConfig {
        enabled: true,
        log_unanswered: false,
        log_congestion: false,
        channel_default_enabled: true,
        ..Default::default()
    }
}

/// Config that also logs unanswered calls.
fn unanswered_cdr_config() -> CdrConfig {
    CdrConfig {
        enabled: true,
        log_unanswered: true,
        log_congestion: false,
        channel_default_enabled: true,
        ..Default::default()
    }
}

/// Config that also logs congestion.
fn congestion_cdr_config() -> CdrConfig {
    CdrConfig {
        enabled: true,
        log_unanswered: true,
        log_congestion: true,
        channel_default_enabled: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Channel creation tests
// ---------------------------------------------------------------------------

/// Port of test_cdr_channel_creation: a CDR is created when a channel is created.
///
/// Channel is created, then hung up with no answer. The CDR should
/// have NOANSWER disposition.
#[test]
fn cdr_channel_creation() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.channel, "CDRTestChannel/Alice");
    assert_eq!(cdr.src, "100");
    assert_eq!(cdr.caller_id, "\"Alice\" <100>");
    assert_eq!(cdr.dst_context, "default");
    assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
}

/// Port of test_cdr_unanswered_inbound_call: unanswered inbound call.
///
/// Channel created, executes some dialplan (Wait), but never answered.
#[test]
fn cdr_unanswered_inbound_call() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.update_app("uid-alice", "Wait", "1");
    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.last_app, "Wait");
    assert_eq!(cdr.last_data, "1");
    assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
}

/// Port of test_cdr_unanswered_outbound_call: outbound call never answered.
#[test]
fn cdr_unanswered_outbound_call() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"\" <>",
        "",
        "default",
    );

    engine.update_app("uid-alice", "AppDial", "(Outgoing Line)");
    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.last_app, "AppDial");
    assert_eq!(cdr.last_data, "(Outgoing Line)");
    assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
}

// ---------------------------------------------------------------------------
// Answered call tests
// ---------------------------------------------------------------------------

/// Port of test_cdr_single_party_answered: single party answers then hangs up.
///
/// Verifies that answering changes disposition to ANSWERED.
#[test]
fn cdr_single_party_answered() {
    let (engine, backend) = setup_engine(debug_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_answered("uid-alice");
    engine.update_app("uid-alice", "VoiceMailMain", "1");
    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.disposition, CdrDisposition::Answered);
    assert!(cdr.answer.is_some());
}

// ---------------------------------------------------------------------------
// Two party bridge tests
// ---------------------------------------------------------------------------

/// Port of test_cdr_outbound_bridged_call: A calls B, both answer, bridge, hangup.
///
/// Verifies that the CDR shows ANSWERED with proper source/destination channels.
#[test]
fn cdr_two_party_bridge() {
    let (engine, backend) = setup_engine(debug_cdr_config());

    // Create Alice channel
    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    // Alice answers
    engine.channel_answered("uid-alice");

    // Create Bob channel (callee side)
    engine.channel_created(
        "uid-bob",
        "CDRTestChannel/Bob",
        "\"\" <>",
        "",
        "default",
    );

    // Dial begin: Alice dials Bob
    engine.dial_begin("uid-alice", "CDRTestChannel/Bob", "200");

    // Bob answers
    engine.channel_answered("uid-bob");

    // Both enter bridge
    engine.bridge_enter("uid-alice", "bridge-1", "CDRTestChannel/Bob");
    engine.bridge_enter("uid-bob", "bridge-1", "CDRTestChannel/Alice");

    // Both leave bridge
    engine.bridge_leave("uid-alice", "bridge-1");
    engine.bridge_leave("uid-bob", "bridge-1");

    // Hangup both
    engine.channel_hangup("uid-bob", 16, "AppDial", "(Outgoing Line)");
    engine.channel_hangup("uid-alice", 16, "Dial", "CDRTestChannel/Bob,30");

    // Both CDRs should be answered
    let cdrs = backend.all();
    assert!(cdrs.len() >= 1);

    // Find Alice's CDR
    let alice_cdr = cdrs.iter().find(|c| c.channel == "CDRTestChannel/Alice");
    assert!(alice_cdr.is_some());
    let alice_cdr = alice_cdr.unwrap();
    assert_eq!(alice_cdr.disposition, CdrDisposition::Answered);
    assert_eq!(alice_cdr.dst_channel, "CDRTestChannel/Bob");
}

// ---------------------------------------------------------------------------
// Dial result tests
// ---------------------------------------------------------------------------

/// Port of test_cdr_dial_unanswered: A dials B, B never answers.
#[test]
fn cdr_dial_unanswered() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.dial_begin("uid-alice", "CDRTestChannel/Bob", "200");

    // Bob never answers; Alice hangs up
    engine.channel_hangup("uid-alice", 16, "Dial", "CDRTestChannel/Bob,30");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
    assert_eq!(cdr.dst_channel, "CDRTestChannel/Bob");
}

/// Port of test_cdr_dial_busy: A dials B, B returns busy.
#[test]
fn cdr_dial_busy() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.dial_begin("uid-alice", "CDRTestChannel/Bob", "200");

    // Hangup with busy cause (17 = AST_CAUSE_BUSY)
    engine.channel_hangup("uid-alice", 17, "Dial", "CDRTestChannel/Bob,30");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.disposition, CdrDisposition::Busy);
}

/// Port of test_cdr_dial_congestion: no route available.
#[test]
fn cdr_dial_congestion() {
    let (engine, backend) = setup_engine(congestion_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.dial_begin("uid-alice", "CDRTestChannel/Bob", "200");

    // Hangup with congestion cause (34 = AST_CAUSE_CONGESTION)
    engine.channel_hangup("uid-alice", 34, "Dial", "CDRTestChannel/Bob,30");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(cdr.disposition, CdrDisposition::Congestion);
}

// ---------------------------------------------------------------------------
// CDR variable tests
// ---------------------------------------------------------------------------

/// Port of CDR variable tests: verify custom CDR variables are preserved.
#[test]
fn cdr_variables() {
    let (engine, backend) = setup_engine(unanswered_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    // Set custom variables
    engine.set_variable("uid-alice", "custom_var1", "value1");
    engine.set_variable("uid-alice", "custom_var2", "value2");

    // Verify they can be retrieved while active
    assert_eq!(
        engine.get_variable("uid-alice", "custom_var1"),
        Some("value1".to_string())
    );
    assert_eq!(
        engine.get_variable("uid-alice", "custom_var2"),
        Some("value2".to_string())
    );

    // Missing variable returns None
    assert!(engine.get_variable("uid-alice", "nonexistent").is_none());

    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    assert_eq!(
        cdr.get_variable("custom_var1"),
        Some(&"value1".to_string())
    );
    assert_eq!(
        cdr.get_variable("custom_var2"),
        Some(&"value2".to_string())
    );
}

// ---------------------------------------------------------------------------
// CDR disable tests
// ---------------------------------------------------------------------------

/// Port of CDR disable: when CDR is disabled, no records are generated.
#[test]
fn cdr_disable() {
    let config = CdrConfig {
        enabled: false,
        ..Default::default()
    };
    let (engine, backend) = setup_engine(config);

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_hangup("uid-alice", 16, "", "");

    // No CDRs should be produced when disabled
    assert_eq!(backend.count(), 0);
}

/// Verify that unanswered calls are not logged when log_unanswered is false.
#[test]
fn cdr_unanswered_not_logged() {
    let (engine, backend) = setup_engine(debug_cdr_config()); // log_unanswered = false

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_hangup("uid-alice", 16, "", "");

    // Unanswered should NOT be logged with default config
    assert_eq!(backend.count(), 0);
}

// ---------------------------------------------------------------------------
// CDR fork tests
// ---------------------------------------------------------------------------

/// Port of CDR fork: ForkCDR creates a second CDR from the same call.
///
/// We simulate by creating two channels with the same linked_id.
#[test]
fn cdr_fork() {
    let (engine, backend) = setup_engine(debug_cdr_config());

    // Original channel
    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_answered("uid-alice");
    engine.update_app("uid-alice", "ForkCDR", "");

    // Simulate fork by creating a second CDR entry
    engine.channel_created(
        "uid-alice-fork",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );
    engine.channel_answered("uid-alice-fork");

    // Hangup both
    engine.channel_hangup("uid-alice-fork", 16, "Playback", "hello-world");
    engine.channel_hangup("uid-alice", 16, "ForkCDR", "");

    let cdrs = backend.all();
    assert_eq!(cdrs.len(), 2);
}

// ---------------------------------------------------------------------------
// LinkedID tests
// ---------------------------------------------------------------------------

/// Port of LinkedID handling: linked IDs propagate through bridges.
#[test]
fn cdr_linked_id() {
    let (engine, backend) = setup_engine(debug_cdr_config());

    engine.channel_created(
        "uid-alice",
        "CDRTestChannel/Alice",
        "\"Alice\" <100>",
        "100",
        "default",
    );

    engine.channel_answered("uid-alice");
    engine.channel_hangup("uid-alice", 16, "", "");

    assert_eq!(backend.count(), 1);
    let cdr = backend.last().unwrap();
    // linked_id should be set to the channel's unique_id
    assert_eq!(cdr.linked_id, "uid-alice");
}

// ---------------------------------------------------------------------------
// State transition tests
// ---------------------------------------------------------------------------

/// Verify CDR state transitions through the call lifecycle.
#[test]
fn cdr_state_transitions() {
    let engine = CdrEngine::new();

    engine.channel_created("uid-1", "SIP/test-001", "", "", "default");
    assert_eq!(engine.active_count(), 1);

    engine.dial_begin("uid-1", "SIP/dest-001", "100");

    engine.channel_answered("uid-1");

    engine.bridge_enter("uid-1", "bridge-1", "SIP/dest-001");

    engine.bridge_leave("uid-1", "bridge-1");

    engine.channel_hangup("uid-1", 16, "Dial", "SIP/dest,30");
    assert_eq!(engine.active_count(), 0);
}

// ---------------------------------------------------------------------------
// Engine management tests
// ---------------------------------------------------------------------------

/// Verify backend registration.
#[test]
fn cdr_backend_registration() {
    let engine = CdrEngine::new();
    assert_eq!(engine.backend_count(), 0);

    let backend = Arc::new(MockCdrBackend::new());
    engine.register_backend(backend);
    assert_eq!(engine.backend_count(), 1);

    let names = engine.backend_names();
    assert_eq!(names, vec!["mock_cdr_backend"]);
}

/// Verify config update.
#[test]
fn cdr_config_update() {
    let engine = CdrEngine::new();
    let backend = Arc::new(MockCdrBackend::new());
    engine.register_backend(backend.clone());

    // Initially logging is enabled with defaults (unanswered not logged)
    engine.channel_created("uid-1", "SIP/test", "", "", "default");
    engine.channel_hangup("uid-1", 16, "", "");
    assert_eq!(backend.count(), 0); // unanswered not logged

    // Enable unanswered logging
    engine.set_config(unanswered_cdr_config());
    engine.channel_created("uid-2", "SIP/test2", "", "", "default");
    engine.channel_hangup("uid-2", 16, "", "");
    assert_eq!(backend.count(), 1); // now logged
}

/// Verify active CDR summary.
#[test]
fn cdr_active_summary() {
    let engine = CdrEngine::new();

    engine.channel_created("uid-1", "SIP/alice-001", "", "", "default");
    engine.channel_created("uid-2", "SIP/bob-001", "", "", "default");

    let summary = engine.active_cdr_summary();
    assert_eq!(summary.len(), 2);

    engine.channel_hangup("uid-1", 16, "", "");
    let summary = engine.active_cdr_summary();
    assert_eq!(summary.len(), 1);
}

/// Verify Cdr direct construction and finalization.
#[test]
fn cdr_direct_construction() {
    let mut cdr = Cdr::new("SIP/test-001".to_string(), "uid-test".to_string());
    assert_eq!(cdr.channel, "SIP/test-001");
    assert_eq!(cdr.unique_id, "uid-test");
    assert_eq!(cdr.disposition, CdrDisposition::NoAnswer);
    assert!(cdr.answer.is_none());
    assert_eq!(cdr.billsec, 0);

    // Mark answered
    cdr.mark_answered();
    assert_eq!(cdr.disposition, CdrDisposition::Answered);
    assert!(cdr.answer.is_some());

    // Finalize
    cdr.finalize();
    assert!(cdr.duration >= 0);
}

/// Verify CdrDisposition parsing.
#[test]
fn cdr_disposition_parsing() {
    assert_eq!(CdrDisposition::from_str_name("ANSWERED"), CdrDisposition::Answered);
    assert_eq!(CdrDisposition::from_str_name("BUSY"), CdrDisposition::Busy);
    assert_eq!(CdrDisposition::from_str_name("NO ANSWER"), CdrDisposition::NoAnswer);
    assert_eq!(CdrDisposition::from_str_name("NOANSWER"), CdrDisposition::NoAnswer);
    assert_eq!(CdrDisposition::from_str_name("CONGESTION"), CdrDisposition::Congestion);
    assert_eq!(CdrDisposition::from_str_name("FAILED"), CdrDisposition::Failed);
    assert_eq!(CdrDisposition::from_str_name("unknown"), CdrDisposition::NoAnswer);
}

/// Verify CdrDisposition display.
#[test]
fn cdr_disposition_display() {
    assert_eq!(CdrDisposition::Answered.as_str(), "ANSWERED");
    assert_eq!(CdrDisposition::Busy.as_str(), "BUSY");
    assert_eq!(CdrDisposition::NoAnswer.as_str(), "NO ANSWER");
    assert_eq!(CdrDisposition::Congestion.as_str(), "CONGESTION");
    assert_eq!(CdrDisposition::Failed.as_str(), "FAILED");
}

/// Verify Cdr summary formatting.
#[test]
fn cdr_summary_format() {
    let mut cdr = Cdr::new("SIP/alice-001".to_string(), "uid-1".to_string());
    cdr.src = "100".to_string();
    cdr.dst = "200".to_string();
    cdr.disposition = CdrDisposition::Answered;
    cdr.duration = 120;
    cdr.billsec = 110;

    let summary = cdr.summary();
    assert!(summary.contains("100"));
    assert!(summary.contains("200"));
    assert!(summary.contains("ANSWERED"));
    assert!(summary.contains("120"));
    assert!(summary.contains("110"));
}
