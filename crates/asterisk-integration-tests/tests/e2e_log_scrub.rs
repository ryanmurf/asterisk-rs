//! Red-capable log-scrub acceptance test for issue #129.
//!
//! `handle_incoming_invite` used to dump the caller number and EVERY SIP
//! header of every inbound INVITE to stderr via unconditional `eprintln!` —
//! including `Authorization` (digest credential material) and
//! `From`/`Contact`/`P-Asserted-Identity` (caller PII) — at any verbosity.
//!
//! This test drives a real INVITE carrying bogus TEST credentials and TEST
//! PII through the real `SipEventHandler` in a CHILD PROCESS and captures
//! the child's entire stderr — which sees tracing output AND anything
//! written with a raw `eprintln!` — then asserts:
//!
//! * at TRACE level the INVITE is processed and the header dump runs
//!   (header names + `<redacted>` placeholders visible), but the
//!   Authorization digest value and the PII values NEVER appear;
//! * at the DEFAULT level (info) none of the probe values appear at all —
//!   the converted `debug!`/`trace!` lines honor the log filter.
//!
//! RED-capability: restoring the raw `eprintln!` header dump that the #129
//! fix removed makes the bogus digest response and the test caller numbers
//! appear in the captured stderr, failing both tests. All values are TEST
//! values: the digest response is `deadbeef…`, the numbers are fictional.

use std::process::Command;

use asterisk_core::pbx::Dialplan;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::SipMessage;
use asterisk_sip::pjsip_config::{set_global_pjsip_config, AuthConfig, EndpointConfig, PjsipConfig};
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use std::net::SocketAddr;
use std::sync::Arc;

/// Bogus TEST digest material (never a real credential).
const TEST_DIGEST_RESPONSE: &str = "deadbeefdeadbeefdeadbeefdeadbeef";
const TEST_NONCE: &str = "scrubnonce129";
/// Fictional TEST caller PII probes.
const TEST_FROM_NUMBER: &str = "4245550777";
const TEST_PAI_NUMBER: &str = "4245550129";

/// Child-process entry point: handles one INVITE with bogus credentials and
/// PII under the log level named by `E2E_LOG_SCRUB_LEVEL`, logging to stderr.
/// A no-op unless spawned by the parent test below.
#[tokio::test]
async fn child_handle_invite_for_log_scrub() {
    if std::env::var("E2E_LOG_SCRUB_CHILD").is_err() {
        return;
    }
    let level = std::env::var("E2E_LOG_SCRUB_LEVEL").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(level))
        .with_writer(std::io::stderr)
        .init();

    // One authenticated endpoint so the INVITE takes the digest-verification
    // path; the bogus credentials fail and draw a 401. The header dump under
    // test runs BEFORE authentication, so this exercises it either way.
    let config = PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: "scrub".to_string(),
            context: "default".to_string(),
            auth: Some("authscrub".to_string()),
            ..Default::default()
        }],
        auths: vec![AuthConfig {
            name: "authscrub".to_string(),
            auth_type: "userpass".to_string(),
            username: "scrub".to_string(),
            password: "not-a-real-password".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    set_global_pjsip_config(config);

    let transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .expect("bind handler transport"),
    );
    let handler = Arc::new(SipEventHandler::new(Arc::new(Dialplan::new()), transport));

    // A real bound socket for the caller so the 401 send has a destination.
    let caller_sock = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind caller socket");
    let caller_addr = caller_sock.local_addr().expect("caller addr");

    let raw = format!(
        "INVITE sip:9001@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKscrub1\r\n\
         From: \"Scrub Test PII\" <sip:{TEST_FROM_NUMBER}@pii.test>;tag=scrubtag1\r\n\
         To: <sip:9001@127.0.0.1>\r\n\
         Call-ID: scrub-call-129\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:{TEST_FROM_NUMBER}@127.0.0.1:5062>\r\n\
         P-Asserted-Identity: <sip:{TEST_PAI_NUMBER}@pii.test>\r\n\
         Authorization: Digest username=\"scrub\", realm=\"rustisk-test\", \
         nonce=\"{TEST_NONCE}\", uri=\"sip:9001@127.0.0.1\", \
         response=\"{TEST_DIGEST_RESPONSE}\"\r\n\
         Content-Length: 0\r\n\r\n"
    );
    let invite = SipMessage::parse(raw.as_bytes()).expect("INVITE must parse");
    let sip_local: SocketAddr = "127.0.0.1:5060".parse().unwrap();
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).expect("session");

    let result = handler
        .handle_incoming_invite(&invite, caller_addr, session)
        .await;
    // Bogus credentials must be rejected on the 401 path; the header dump
    // has already run by then.
    assert!(result.is_none(), "bogus-credential INVITE must not be accepted");
    println!("CHILD_RESULT: invite_processed=1");
}

/// Spawn this test binary again, running only the child entry point above,
/// and capture its full stdout/stderr.
fn run_child(level: &str) -> std::process::Output {
    let exe = std::env::current_exe().expect("current test binary");
    Command::new(exe)
        .args(["child_handle_invite_for_log_scrub", "--exact", "--nocapture"])
        .env("E2E_LOG_SCRUB_CHILD", "1")
        .env("E2E_LOG_SCRUB_LEVEL", level)
        .env_remove("RUST_LOG")
        .output()
        .expect("failed to spawn child test process")
}

#[test]
fn invite_credentials_and_pii_never_reach_logs_at_trace() {
    let out = run_child("trace");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("CHILD_RESULT: invite_processed=1"),
        "child failed to process the INVITE:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    // Positive control: the scrubbed dump DID run — the summary line and the
    // sensitive header NAMES (with redacted placeholders) are visible at
    // trace. Without this, an empty transcript would falsely pass.
    assert!(
        stderr.contains("handle_incoming_invite"),
        "INVITE summary log missing at trace level (dump never ran?):\n{stderr}"
    );
    assert!(
        stderr.contains("Authorization") && stderr.contains("P-Asserted-Identity"),
        "header dump missing at trace level (positive control):\n{stderr}"
    );
    assert!(
        stderr.contains("<redacted>"),
        "redaction placeholder missing from header dump:\n{stderr}"
    );

    // Issue #129 credential scrub: digest material must never appear, at ANY
    // level. RED when the raw `eprintln!` header dump is restored.
    assert!(
        !stderr.contains(TEST_DIGEST_RESPONSE),
        "digest response leaked into logs (issue #129):\n{stderr}"
    );
    assert!(
        !stderr.contains(TEST_NONCE),
        "digest nonce leaked into logs (issue #129):\n{stderr}"
    );
    // Issue #129 PII scrub: caller identity values must never appear.
    assert!(
        !stderr.contains(TEST_FROM_NUMBER),
        "From/Contact caller number leaked into logs (issue #129):\n{stderr}"
    );
    assert!(
        !stderr.contains(TEST_PAI_NUMBER),
        "P-Asserted-Identity leaked into logs (issue #129):\n{stderr}"
    );
}

#[test]
fn invite_debug_logging_honors_default_log_level() {
    // At the default (info) filter the converted `debug!`/`trace!` lines must
    // be silent — the pre-#129 `eprintln!` calls printed unconditionally.
    let out = run_child("info");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("CHILD_RESULT: invite_processed=1"),
        "child failed to process the INVITE:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    assert!(
        !stderr.contains("handle_incoming_invite"),
        "debug summary printed at default log level (filter not honored):\n{stderr}"
    );
    for probe in [TEST_DIGEST_RESPONSE, TEST_NONCE, TEST_FROM_NUMBER, TEST_PAI_NUMBER] {
        assert!(
            !stderr.contains(probe),
            "sensitive value '{probe}' leaked at default log level (issue #129):\n{stderr}"
        );
    }
}
