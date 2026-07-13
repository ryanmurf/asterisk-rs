//! End-to-end regression for issue #57, both defects:
//!
//! 1. `Ringing()` was not a registered dialplan app — a dialplan calling it
//!    failed with "No such application".
//! 2. Worse, any pre-answer dialplan abort (failed/unknown app, early
//!    hangup) tore the channel down WITHOUT sending a final SIP response:
//!    the caller was stuck after `100 Trying` until transaction timeout.
//!
//! Drives the full `SipStack` over real UDP with the same stack →
//! event-handler glue the rustisk binary uses (including `set_stack`, so
//! every final response is recorded in — and gated by — the transaction
//! layer). Scenarios:
//!
//! * A: the issue's exact dialplan (`Ringing()`, `Wait`, `Answer()`) yields
//!   `100 → 180 → 200` on the wire.
//! * B: an unknown-app extension yields `100 → 480` (was: silence).
//! * C: a pre-answer `Hangup(17)` yields `486 Busy Here` — the cause set by
//!   the app must survive to the SIP mapping.
//! * D: a CANCEL during ringing yields `200`-to-CANCEL + `487`, and the
//!   dialplan abort must NOT also emit a `480` (the transaction layer
//!   suppresses the second final).
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
use asterisk_sip::parser::{header_names, SipMessage};
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::stack::{SipEvent, SipStack};
use tokio::net::UdpSocket;

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Receive responses until one with `status` arrives; panics on a different
/// final (>=200) response for the same CSeq method.
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

fn add_ext(ctx: &mut Context, exten: &str, apps: &[(&str, &str)]) {
    let mut ext = Extension::new(exten);
    for (i, (app, data)) in apps.iter().enumerate() {
        ext.add_priority(Priority {
            priority: (i + 1) as i32,
            app: app.to_string(),
            app_data: data.to_string(),
            label: None,
        });
    }
    ctx.add_extension(ext);
}

fn invite(call_id: &str, branch: &str, exten: &str, target: SocketAddr, sdp: &str) -> String {
    format!(
        "INVITE sip:{exten}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=c57\r\n\
         To: <sip:{exten}@{target}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:5062>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    )
}

fn cancel(call_id: &str, branch: &str, exten: &str, target: SocketAddr) -> String {
    format!(
        "CANCEL sip:{exten}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=c57\r\n\
         To: <sip:{exten}@{target}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 CANCEL\r\n\
         Content-Length: 0\r\n\r\n"
    )
}

fn ack(call_id: &str, branch: &str, exten: &str, target: SocketAddr) -> String {
    format!(
        "ACK sip:{exten}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=c57\r\n\
         To: <sip:{exten}@{target}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n"
    )
}

#[tokio::test]
async fn ringing_indicates_180_and_preanswer_abort_sends_final_response() {
    register_all_apps();
    set_global_pjsip_config(PjsipConfig {
        endpoints: ["200", "300", "301", "202"]
            .iter()
            .map(|e| EndpointConfig {
                name: e.to_string(),
                context: "default".to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });

    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    // A: the issue's exact dialplan.
    add_ext(&mut ctx, "200", &[("Ringing", ""), ("Wait", "1"), ("Answer", "")]);
    // B: unknown-app abort.
    add_ext(&mut ctx, "300", &[("NoSuchAppEver", "")]);
    // C: explicit pre-answer busy.
    add_ext(&mut ctx, "301", &[("Hangup", "17")]);
    // D: held ringing long enough to CANCEL.
    add_ext(&mut ctx, "202", &[("Ringing", ""), ("Wait", "2"), ("Answer", "")]);
    dp.add_context(ctx);

    // ---- Full stack + the same glue loop the rustisk binary runs --------
    let mut stack = SipStack::new("127.0.0.1:0".parse().unwrap()).await.expect("stack");
    let sip_local = stack.local_addr();
    let mut rx = stack.take_event_rx().expect("event rx");

    let transport: Arc<dyn asterisk_sip::transport::SipTransport> = stack.transport();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(transport.clone());
    // Ringing() reaches the driver through the tech registry (like Echo()).
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), transport));
    handler.set_channel_driver(driver.clone());

    let stack = Arc::new(stack);
    handler.set_stack(stack.clone());
    let stack_run = stack.clone();
    tokio::spawn(async move { stack_run.run().await });

    let handler_for_glue = handler.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                SipEvent::IncomingInvite { session, request, remote_addr } => {
                    handler_for_glue
                        .handle_incoming_invite(&request, remote_addr, session)
                        .await;
                }
                SipEvent::IncomingCancel { call_id: _, request, remote_addr } => {
                    handler_for_glue.handle_cancel(&request, remote_addr).await;
                }
                _ => {}
            }
        }
    });

    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let offer = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);

    // ---- A. Ringing() -> 180 Ringing, then Answer() -> 200 OK ------------
    let inv = invite("ring-57-1", "z9hG4bK571", "200", sip_local, &offer.to_string());
    caller.send_to(inv.as_bytes(), sip_local).await.unwrap();
    expect_status(&caller, 100, Duration::from_secs(2), "scenario A").await;
    // Before the fix: "WARN No such application app=Ringing" and no 180 ever.
    expect_status(&caller, 180, Duration::from_secs(3), "scenario A").await;
    println!("[E2E] Ringing() -> 180 Ringing received");
    expect_status(&caller, 200, Duration::from_secs(5), "scenario A").await;
    println!("[E2E] Answer() -> 200 OK received after 180");

    // ---- B. unknown-app abort -> 480 --------------------------------------
    let inv = invite("abort-57-1", "z9hG4bK572", "300", sip_local, &offer.to_string());
    caller.send_to(inv.as_bytes(), sip_local).await.unwrap();
    expect_status(&caller, 100, Duration::from_secs(2), "scenario B").await;
    // Before the fix the channel died silently and this timed out.
    expect_status(&caller, 480, Duration::from_secs(6), "scenario B").await;
    let a = ack("abort-57-1", "z9hG4bK572", "300", sip_local);
    caller.send_to(a.as_bytes(), sip_local).await.unwrap();
    println!("[E2E] unknown-app abort -> 480 Temporarily Unavailable");

    // ---- C. pre-answer Hangup(17) -> 486 Busy Here ------------------------
    let inv = invite("busy-57-1", "z9hG4bK573", "301", sip_local, &offer.to_string());
    caller.send_to(inv.as_bytes(), sip_local).await.unwrap();
    expect_status(&caller, 100, Duration::from_secs(2), "scenario C").await;
    // The app-set cause must survive pbx_run's final hangup (it was
    // overwritten with NormalClearing, collapsing every cause to 480).
    expect_status(&caller, 486, Duration::from_secs(6), "scenario C").await;
    let a = ack("busy-57-1", "z9hG4bK573", "301", sip_local);
    caller.send_to(a.as_bytes(), sip_local).await.unwrap();
    println!("[E2E] Hangup(17) -> 486 Busy Here");

    // ---- D. CANCEL during ringing -> 487, and no 480 afterwards -----------
    let inv = invite("cxl-57-1", "z9hG4bK574", "202", sip_local, &offer.to_string());
    caller.send_to(inv.as_bytes(), sip_local).await.unwrap();
    expect_status(&caller, 100, Duration::from_secs(2), "scenario D").await;
    expect_status(&caller, 180, Duration::from_secs(3), "scenario D").await;
    let cxl = cancel("cxl-57-1", "z9hG4bK574", "202", sip_local);
    caller.send_to(cxl.as_bytes(), sip_local).await.unwrap();

    let mut got_200_cancel = false;
    let mut got_487 = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(got_200_cancel && got_487) {
        if let Some(resp) = recv_sip(&caller, Duration::from_millis(500)).await {
            let cseq = resp.get_header(header_names::CSEQ).unwrap_or("");
            match (resp.status_code(), cseq) {
                (Some(200), "1 CANCEL") => got_200_cancel = true,
                (Some(487), "1 INVITE") => got_487 = true,
                _ => {}
            }
        }
    }
    assert!(got_200_cancel && got_487, "CANCEL during ringing must yield 200 + 487");
    let a = ack("cxl-57-1", "z9hG4bK574", "202", sip_local);
    caller.send_to(a.as_bytes(), sip_local).await.unwrap();

    // The aborted dialplan's failure final must be suppressed by the
    // transaction layer: after the 487 no other final may follow (Timer G
    // retransmits of the 487 itself are fine).
    let observe_until = Instant::now() + Duration::from_secs(4);
    while Instant::now() < observe_until {
        if let Some(msg) = recv_sip(&caller, Duration::from_millis(500)).await {
            if let Some(s) = msg.status_code() {
                assert!(
                    s < 200,
                    "scenario D: no final may follow the ACKed 487, got {s}"
                );
            }
        }
    }
    println!("[E2E] CANCEL during ringing -> 200 + 487, no 480 followed");
}
