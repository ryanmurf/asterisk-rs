//! End-to-end acceptance for the answered-channel **state transition** in the
//! GLOBAL CHANNEL STORE (M5, `store.rs` baseline).
//!
//! `pbx_run` executes the dialplan against a *detached* copy of the channel, so
//! `Answer` moving that copy to `Up` never touches the store's copy — the one
//! every status consumer observes (`core show channels`, AMI `CoreShowChannels`
//! / `Status`). The event handler must reflect the answered state back into the
//! store copy after the 200 OK goes out (`event_handler.rs`, the "Reflect the
//! answered state in the GLOBAL STORE copy" block).
//!
//! This test is the load-bearing observation the M5 review said was missing:
//! while an answered call is held open, it reads the **store copy's state**
//! (the product-facing channel view, matched to this call by its SIP Call-ID)
//! and asserts it is `Up`. A count-only soak or a BYE-timing check cannot see a
//! mid-call state transition.
//!
//! RED control (for the PR body): delete the store-`Up` reflection block in a
//! scratch clone. The detached-copy `Answer` still fires its own `Newstate`, so
//! the 200 OK and media still work — but the *store copy* stays at its
//! pre-answer `Ring` state, and this test's `Up` assertion fails. That is
//! exactly the defeat the previous harness could not detect.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::store;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::SipMessage;
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use asterisk_types::ChannelState;
use tokio::net::UdpSocket;

const EXTEN: &str = "100";

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Read SIP datagrams until one with `status` arrives (skips 100 Trying).
async fn recv_sip_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.status_code() == Some(status) {
                return Some(msg);
            }
        }
    }
    None
}

fn invite_request(call_id: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller{call_id}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:{contact_port}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// The state of the STORE copy of the channel carrying this SIP Call-ID, or
/// `None` if no such channel is registered. Matches by the `__SIP_CALL_ID`
/// channel variable the inbound handler stamps on the store copy, so a stray
/// channel from another call can never satisfy the assertion.
fn store_state_for_call(call_id: &str) -> Option<ChannelState> {
    for chan in store::all_channels() {
        let ch = chan.lock();
        if ch.variables.get("__SIP_CALL_ID").map(String::as_str) == Some(call_id) {
            return Some(ch.state);
        }
    }
    None
}

/// Poll the store copy for up to `budget`, returning its state the moment it
/// becomes `Up`. Returns the last-seen state if it never reaches `Up` (so the
/// failure message shows what it was stuck at — `Ring` when the reflection
/// block is defeated).
fn await_store_up(call_id: &str, budget: Duration) -> Option<ChannelState> {
    let deadline = Instant::now() + budget;
    let mut last = None;
    while Instant::now() < deadline {
        last = store_state_for_call(call_id);
        if last == Some(ChannelState::Up) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    last
}

#[tokio::test]
async fn answered_call_store_copy_transitions_to_up() {
    register_all_apps();
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            auth: None,
            ..Default::default()
        }],
        ..Default::default()
    });

    // Dialplan: 100 -> Answer, Echo. Echo blocks (reading silent media) so the
    // answered call stays open while we observe the store copy's state.
    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    let mut ext = Extension::new(EXTEN);
    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "Echo".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext);
    dp.add_context(ctx);

    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap(),
    );
    let sip_local: SocketAddr = "127.0.0.1:5060".parse().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());
    // No RTP inactivity timeout: keep the answered call open for the whole
    // observation window (this test is about state, not teardown).
    handler.set_rtp_timeout(None);

    let call_id = "chanstate-up";
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );

    // Before the answer, the store copy is registered in its pre-answer `Ring`
    // state (the inbound handler sets `Ring` at Newchannel). It must NOT already
    // be `Up` — otherwise the later `Up` assertion would be vacuous.
    let invite = invite_request(call_id, caller_sip_addr.port(), &offer.to_string());
    let session =
        SipSession::new_inbound(&invite, sip_local, caller_sip_addr).expect("inbound session");
    let accepted = handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    assert_eq!(accepted.as_deref(), Some(call_id), "INVITE must be accepted");

    // Capture the 200 OK: the answer is now on the wire. The store-`Up`
    // reflection runs on the handler task right after; poll for it.
    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("expected 200 OK with SDP answer");
    assert_eq!(ok.status_code(), Some(200), "call must be answered");

    let observed = await_store_up(call_id, Duration::from_secs(3));
    assert_eq!(
        observed,
        Some(ChannelState::Up),
        "the GLOBAL STORE copy of an answered call must read `Up` (product-facing \
         channel state). Observed {observed:?} — if this is `Ring`, the store-`Up` \
         reflection after the 200 OK was defeated; the detached pbx copy's `Answer` \
         does not update the store."
    );
    println!("[E2E] answered call: store copy observed in state Up (product-facing view)");

    // Keep the caller RTP socket bound for the call's lifetime (silent).
    std::mem::forget(caller_rtp);
}
