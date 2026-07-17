//! End-to-end acceptance for UPDATE **target refresh** (RFC 3261 §12.2 / RFC
//! 3311), the M5 review MAJOR-3 gap where `handle_update` advanced the remote
//! CSeq but never applied the request's Contact to the dialog, so a subsequent
//! local in-dialog request kept addressing the stale INVITE target.
//!
//! Proof, observed on the wire: establish an answered, media-silent Echo() call
//! whose INVITE Contact is port P1. Send an in-dialog UPDATE whose Contact is a
//! DIFFERENT port P2. Then let `rtptimeout` reap the silent call — rustisk
//! sends a BYE built from the dialog's remote target. Its **Request-URI** must
//! carry P2 (the refreshed target), not P1.
//!
//! RED control (PR body): drop the `update_remote_target` call in `handle_update`
//! and the BYE Request-URI stays at P1 -> this assertion fails.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{SipMessage, SipMethod, StartLine};
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
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

/// Wait for an in-dialog BYE *request* and return its Request-URI.
async fn await_bye_uri(sock: &UdpSocket, budget: Duration) -> Option<asterisk_sip::parser::SipUri> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Bye) {
                if let StartLine::Request(rl) = &msg.start_line {
                    return Some(rl.uri.clone());
                }
            }
        }
    }
    None
}

fn header_tag(msg: &SipMessage, header: &str) -> String {
    msg.get_header(header)
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("tag="))
                .map(|t| t.to_string())
        })
        .expect("header must carry a tag")
}

fn invite_request(call_id: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}inv\r\n\
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

/// A no-SDP UPDATE whose Contact is `new_contact_port` (the refreshed target).
fn update_new_contact(
    call_id: &str,
    our_tag: &str,
    caller_tag: &str,
    via_port: u16,
    new_contact_port: u16,
    cseq: u32,
) -> SipMessage {
    let raw = format!(
        "UPDATE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{via_port};branch=z9hG4bK{call_id}upd{cseq}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={caller_tag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} UPDATE\r\n\
         Contact: <sip:caller@127.0.0.1:{new_contact_port}>\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

#[tokio::test]
async fn update_refreshes_dialog_target_for_subsequent_requests() {
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
    // Arm rtptimeout so the silent call is reaped, driving rustisk to send the
    // BYE whose Request-URI we inspect.
    handler.set_rtp_timeout(Some(Duration::from_secs(2)));

    // ---- Establish an answered, media-silent call; INVITE Contact = P1 -----
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    let p1 = caller_sip_addr.port();
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );

    let call_id = "update-target-refresh";
    let caller_tag = format!("caller{call_id}");
    let invite = invite_request(call_id, p1, &offer.to_string());
    let session =
        SipSession::new_inbound(&invite, sip_local, caller_sip_addr).expect("inbound session");
    handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    let our_tag = header_tag(&ok, "To");
    // Keep the caller RTP socket bound but silent for the call's lifetime.
    std::mem::forget(caller_rtp);

    // ---- Refresh the target to a NEW Contact port P2 via UPDATE ------------
    // P2 is deliberately distinct from P1 and need not be a live socket — only
    // the BYE's Request-URI port is inspected.
    let p2: u16 = if p1 == 59999 { 59998 } else { 59999 };
    assert_ne!(p1, p2, "the refreshed target port must differ from the INVITE Contact");
    let upd = update_new_contact(call_id, &our_tag, &caller_tag, p1, p2, 2);
    handler.handle_update(&upd, caller_sip_addr).await;
    let _upd_ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(2))
        .await
        .expect("target-refresh UPDATE must be answered 200");

    // ---- rtptimeout reaps the silent call -> BYE addressed to P2 -----------
    let bye_uri = await_bye_uri(&caller_sip, Duration::from_secs(6))
        .await
        .expect("rtptimeout must reap the media-silent call and send a BYE");
    assert_eq!(
        bye_uri.port,
        Some(p2),
        "BYE Request-URI must address the REFRESHED target (Contact from the UPDATE, \
         port {p2}); got {:?}. If it is {p1} the UPDATE's target refresh was not applied.",
        bye_uri.port
    );
    println!("[E2E] UPDATE target refresh: subsequent BYE addressed the refreshed Contact (port {p2})");
}
