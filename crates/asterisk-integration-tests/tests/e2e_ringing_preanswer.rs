//! End-to-end regression for issue #57, both defects:
//!
//! 1. `Ringing()` was not a registered dialplan app — a dialplan calling it
//!    failed with "No such application".
//! 2. Worse, any pre-answer dialplan abort (failed/unknown app, early
//!    hangup) tore the channel down WITHOUT sending a final SIP response:
//!    the caller was stuck after `100 Trying` until transaction timeout.
//!
//! Scenario A drives the issue's exact dialplan (`Ringing()`, `Wait`,
//! `Answer`) and asserts the caller hears `180 Ringing` then `200 OK`.
//! Scenario B calls an extension whose only priority is an unknown app and
//! asserts a final failure response arrives (cause-mapped, default 480).
//!
//! Own integration-test binary: process-global state (pjsip config, app
//! registry, tech registry) stays isolated; scenarios run sequentially in
//! one test for the same reason.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::SipMessage;
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Receive responses until one with `status` arrives; panics on a final
/// (>=200) response that differs from the expected one.
async fn expect_status(sock: &UdpSocket, status: u16, budget: Duration, what: &str) -> SipMessage {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(500)).await {
            match msg.status_code() {
                Some(s) if s == status => return msg,
                Some(s) if s >= 200 && s != status => {
                    panic!("{what}: expected {status}, got final {s}");
                }
                _ => {}
            }
        }
    }
    panic!("{what}: expected {status}, got nothing");
}

fn invite(call_id: &str, branch: &str, exten: &str, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{exten}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=c57\r\n\
         To: <sip:{exten}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:5062>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

#[tokio::test]
async fn ringing_indicates_180_and_preanswer_abort_sends_final_response() {
    register_all_apps();
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![
            EndpointConfig {
                name: "200".to_string(),
                context: "default".to_string(),
                ..Default::default()
            },
            EndpointConfig {
                name: "300".to_string(),
                context: "default".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });

    // The issue's dialplan for 200, and an unknown-app abort for 300.
    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    let mut ext200 = Extension::new("200");
    ext200.add_priority(Priority {
        priority: 1,
        app: "Ringing".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext200.add_priority(Priority {
        priority: 2,
        app: "Wait".to_string(),
        app_data: "1".to_string(),
        label: None,
    });
    ext200.add_priority(Priority {
        priority: 3,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext200);
    let mut ext300 = Extension::new("300");
    ext300.add_priority(Priority {
        priority: 1,
        app: "NoSuchAppEver".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext300);
    dp.add_context(ctx);

    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap()).await.unwrap(),
    );
    let sip_local: SocketAddr = handler_transport.local_addr().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    // Ringing() reaches the driver through the tech registry (like Echo()).
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());

    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_addr = caller.local_addr().unwrap();
    let offer = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);

    // ---- A. Ringing() -> 180 Ringing, then Answer() -> 200 OK ------------
    let inv = invite("ring-57-1", "z9hG4bK571", "200", &offer.to_string());
    let session = SipSession::new_inbound(&inv, sip_local, caller_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, caller_addr, session).await;
    assert_eq!(accepted.as_deref(), Some("ring-57-1"));

    let _trying = expect_status(&caller, 100, Duration::from_secs(2), "scenario A").await;
    // Before the fix: "WARN No such application app=Ringing" and the channel
    // died — no 180 was ever sent.
    let _ringing = expect_status(&caller, 180, Duration::from_secs(3), "scenario A").await;
    println!("[E2E] Ringing() -> 180 Ringing received");
    let _ok = expect_status(&caller, 200, Duration::from_secs(5), "scenario A").await;
    println!("[E2E] Answer() -> 200 OK received after 180");

    // ---- B. pre-answer dialplan abort -> final failure response ----------
    let inv = invite("abort-57-1", "z9hG4bK572", "300", &offer.to_string());
    let session = SipSession::new_inbound(&inv, sip_local, caller_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, caller_addr, session).await;
    assert_eq!(accepted.as_deref(), Some("abort-57-1"));

    let _trying = expect_status(&caller, 100, Duration::from_secs(2), "scenario B").await;
    // Before the fix the channel was hung up silently: the INVITE never got
    // ANY final response and this timed out.
    let final_resp = expect_status(&caller, 480, Duration::from_secs(6), "scenario B").await;
    assert_eq!(
        final_resp.status_code(),
        Some(480),
        "a pre-answer dialplan abort must produce a final failure response"
    );
    println!("[E2E] unknown-app dialplan abort -> 480 Temporarily Unavailable received");
}
