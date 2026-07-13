//! End-to-end regression for issue #55: an inbound CANCEL during a pending
//! INVITE must get `200 OK`, the INVITE must get `487 Request Terminated`,
//! and the call must NOT proceed to answer (RFC 3261 §9.2).
//!
//! Before the fix the CANCEL was silently swallowed by branch-only server
//! transaction matching (a CANCEL reuses the INVITE's Via branch, §9.1): no
//! response to the CANCEL, no 487, and the dialplan kept running until
//! Answer() sent a 200 OK for a call the caller had already abandoned.
//!
//! This drives the full `SipStack` over a real UDP socket — reproducing the
//! wire evidence in the issue (INVITE → 100 Trying → CANCEL mid-`Wait()`) —
//! with the same stack → event-handler glue the rustisk binary uses.
//!
//! Own integration-test binary so its use of process-global state (pjsip
//! config, app registry) is isolated from the other e2e tests.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{header_names, SipMessage};
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::stack::{SipEvent, SipStack};
use tokio::net::UdpSocket;

const EXTEN: &str = "200";

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

fn invite_request(call_id: &str, branch: &str, target: SocketAddr, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller55\r\n\
         To: <sip:{EXTEN}@{target}>\r\n\
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

/// CANCEL matching the INVITE: same Via branch and CSeq number (RFC 3261 §9.1).
fn cancel_request(call_id: &str, branch: &str, target: SocketAddr) -> SipMessage {
    let raw = format!(
        "CANCEL sip:{EXTEN}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller55\r\n\
         To: <sip:{EXTEN}@{target}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 CANCEL\r\n\
         Content-Length: 0\r\n\r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

fn ack_request(call_id: &str, branch: &str, target: SocketAddr) -> SipMessage {
    let raw = format!(
        "ACK sip:{EXTEN}@{target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller55\r\n\
         To: <sip:{EXTEN}@{target}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

#[tokio::test]
async fn cancel_during_pending_invite_gets_200_and_487_and_no_answer() {
    // ---- Global wiring (isolated to this test binary) -------------------
    register_all_apps();
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    });

    // Dialplan reproducing the issue: hold the call pre-answer, then Answer.
    // Wait(2) keeps the test fast while leaving a wide CANCEL window.
    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    let mut ext = Extension::new(EXTEN);
    ext.add_priority(Priority {
        priority: 1,
        app: "Wait".to_string(),
        app_data: "2".to_string(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext);
    dp.add_context(ctx);

    // ---- Full stack + the same glue loop the rustisk binary runs --------
    let mut stack = SipStack::new("127.0.0.1:0".parse().unwrap())
        .await
        .expect("stack");
    let sip_local = stack.local_addr();
    let mut rx = stack.take_event_rx().expect("event rx");

    let transport: Arc<dyn asterisk_sip::transport::SipTransport> = stack.transport();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(transport.clone());
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
                SipEvent::IncomingInvite {
                    session,
                    request,
                    remote_addr,
                } => {
                    handler_for_glue
                        .handle_incoming_invite(&request, remote_addr, session)
                        .await;
                }
                SipEvent::IncomingCancel {
                    call_id: _,
                    request,
                    remote_addr,
                } => {
                    handler_for_glue.handle_cancel(&request, remote_addr).await;
                }
                _ => {}
            }
        }
    });

    // ---- The caller: INVITE, get 100 Trying, CANCEL mid-Wait ------------
    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let offer = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);

    let call_id = "e2e-cancel-55";
    let branch = "z9hG4bKe2e55";
    let invite = invite_request(call_id, branch, sip_local, &offer.to_string());
    caller
        .send_to(invite.to_string().as_bytes(), sip_local)
        .await
        .unwrap();

    let trying = recv_sip(&caller, Duration::from_secs(2))
        .await
        .expect("expected 100 Trying for the INVITE");
    assert_eq!(trying.status_code(), Some(100), "first response is 100 Trying");

    // The call is now held in Wait(2). Cancel it.
    let cancel = cancel_request(call_id, branch, sip_local);
    caller
        .send_to(cancel.to_string().as_bytes(), sip_local)
        .await
        .unwrap();

    // Expect BOTH: 200 OK to the CANCEL and 487 to the INVITE (order not
    // significant). Before the fix, neither ever arrived.
    let mut got_200_cancel = false;
    let mut got_487_invite = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(got_200_cancel && got_487_invite) {
        if let Some(resp) = recv_sip(&caller, Duration::from_millis(500)).await {
            let cseq = resp.get_header(header_names::CSEQ).unwrap_or("");
            match (resp.status_code(), cseq) {
                (Some(200), "1 CANCEL") => got_200_cancel = true,
                (Some(487), "1 INVITE") => got_487_invite = true,
                _ => {}
            }
        }
    }
    assert!(got_200_cancel, "the CANCEL must be answered with 200 OK (RFC 3261 §9.2)");
    assert!(got_487_invite, "the INVITE must be terminated with 487 Request Terminated");
    println!("[E2E] CANCEL -> 200 OK; INVITE -> 487 Request Terminated");

    // Complete the INVITE transaction like a real UAC.
    let ack = ack_request(call_id, branch, sip_local);
    caller.send_to(ack.to_string().as_bytes(), sip_local).await.unwrap();

    // The dialplan must NOT proceed to answer: no 200 OK to the INVITE may
    // arrive after the 487, even once Wait(2) elapses. Before the fix the
    // call answered anyway ("ghost call").
    let observe_until = Instant::now() + Duration::from_secs(4);
    while Instant::now() < observe_until {
        if let Some(msg) = recv_sip(&caller, Duration::from_millis(500)).await {
            if msg.is_response() {
                let cseq = msg.get_header(header_names::CSEQ).unwrap_or("");
                assert!(
                    !(msg.status_code() == Some(200) && cseq.ends_with("INVITE")),
                    "cancelled call must not answer: got 200 OK to the INVITE after 487"
                );
            }
        }
    }

    // The aborted call's channel and media plane are released.
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while driver.active_channel_count() != 0 && Instant::now() < cleanup_deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        driver.active_channel_count(),
        0,
        "cancelled call must release its channel / RTP socket"
    );
    assert_eq!(handler.active_calls(), 0, "no call state may survive the CANCEL");
    println!("[E2E] cancelled call released its channel and call state");
}
