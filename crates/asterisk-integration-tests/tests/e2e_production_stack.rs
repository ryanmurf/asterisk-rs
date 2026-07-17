//! Production-stack wire contracts for re-INVITE (hold) and UPDATE (M5 review
//! MINOR-1).
//!
//! The other M5 e2e tests drive the event handler methods directly, bypassing
//! wire parse and the stack's event classification. That descope let two wire
//! defects hide (MAJOR-2 hold-answer direction, MAJOR-3 UPDATE Allow/refresher).
//! This test sends real SIP over localhost UDP into the **production
//! `SipStack`**, runs the same event loop the CLI wires (`main.rs`), and asserts
//! the wire responses a real peer would see:
//!
//!  * initial INVITE 200 advertises `UPDATE` in `Allow` (RFC 3311 §5.1);
//!  * a `sendonly` hold re-INVITE is answered `recvonly`/`inactive`, not
//!    `sendrecv` (RFC 3264 §6.1);
//!  * a no-SDP UPDATE with `refresher=uac` is answered `refresher=uac`, never
//!    `uas` (RFC 4028 §9);
//!  * an OPTIONS ping advertises `UPDATE` in `Allow` (RFC 3311 §5.2).
//!
//! The distinct-socket RTP receiver proofs remain in `e2e_reinvite`/`e2e_update`
//! (retained per the review's re-review bar); this test proves the *signaling*
//! contracts on the real wire.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::{build_options_ok, SipEventHandler};
use asterisk_sip::parser::{SipMessage, SipMethod};
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::sdp::{MediaDirection, SessionDescription};
use asterisk_sip::stack::{SipEvent, SipStack};
use asterisk_sip::transport::SipTransport;
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

/// Read datagrams until a final (>= 200) response with the given CSeq method
/// arrives, skipping 100 Trying and any retransmissions of earlier finals.
async fn recv_final(sock: &UdpSocket, cseq_method: &str, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            let is_response = msg.status_code().is_some();
            let cseq_ok = msg
                .cseq()
                .map(|c| c.to_ascii_uppercase().contains(&cseq_method.to_ascii_uppercase()))
                .unwrap_or(false);
            if is_response && cseq_ok && msg.status_code().map(|s| s >= 200).unwrap_or(false) {
                return Some(msg);
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

async fn send(sock: &UdpSocket, dst: SocketAddr, raw: String) {
    sock.send_to(raw.as_bytes(), dst).await.unwrap();
}

fn audio_direction(msg: &SipMessage) -> MediaDirection {
    SessionDescription::parse(&msg.body)
        .expect("200 must carry SDP")
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("audio stream in answer")
        .direction
}

#[tokio::test]
async fn production_stack_hold_and_update_wire_contracts() {
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

    // Dialplan: 100 -> Answer, Echo (keeps the call up for the exchange).
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

    // ---- Bring up the PRODUCTION stack + the CLI's event loop -------------
    let mut stack = SipStack::new("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind stack");
    let stack_local = stack.local_addr();
    let transport = stack.transport();
    let event_rx = stack.take_event_rx().expect("event rx");
    let stack = Arc::new(stack);

    let driver = Arc::new(SipChannelDriver::new(stack_local));
    driver.set_transport(transport.clone());
    driver.set_stack(stack.clone());
    TECH_REGISTRY.register(driver.clone());

    let handler = Arc::new(SipEventHandler::new(
        Arc::new(dp),
        transport.clone() as Arc<dyn SipTransport>,
    ));
    handler.set_channel_driver(driver.clone());
    handler.set_stack(stack.clone());

    // Drive the stack's UDP receive loop.
    {
        let stack = stack.clone();
        tokio::spawn(async move { stack.run().await });
    }
    // Consume stack events exactly like the CLI (`main.rs`): INVITE/ACK/BYE to
    // the handler, OPTIONS via the shared builder, UPDATE to handle_update.
    {
        let handler = handler.clone();
        let stack = stack.clone();
        let mut rx = event_rx;
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    SipEvent::IncomingInvite { session, request, remote_addr } => {
                        handler.handle_incoming_invite(&request, remote_addr, session).await;
                    }
                    SipEvent::IncomingAck { request, remote_addr, .. } => {
                        handler.handle_ack(&request, remote_addr).await;
                    }
                    SipEvent::IncomingBye { request, remote_addr, .. } => {
                        handler.handle_bye(&request, remote_addr).await;
                    }
                    SipEvent::Response { response, remote_addr } => {
                        handler.handle_response(&response, remote_addr).await;
                        handler.handle_reinvite_response(&response, remote_addr).await;
                    }
                    SipEvent::IncomingRequest { request, remote_addr } => {
                        match request.method() {
                            Some(SipMethod::Options) => {
                                if let Some(ok) = build_options_ok(&request) {
                                    let _ = stack.send_response(ok, remote_addr).await;
                                }
                            }
                            Some(SipMethod::Update) => {
                                handler.handle_update(&request, remote_addr).await;
                            }
                            Some(SipMethod::Register) => {
                                handler.handle_register(&request, remote_addr).await;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    // ---- The far-end peer: a plain UDP socket speaking raw SIP ------------
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_addr = peer.local_addr().unwrap();
    let pport = peer_addr.port();
    let peer_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_rtp_port = peer_rtp.local_addr().unwrap().port();
    let call_id = "prodstack-call";
    let ctag = "peerA";

    // 1) INVITE with a sendrecv offer.
    let offer = SessionDescription::create_offer("127.0.0.1", peer_rtp_port, &[codecs::pcmu()]);
    send(&peer, stack_local, format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}inv\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag={ctag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:peer@127.0.0.1:{pport}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\r\n{body}",
        len = offer.to_string().len(),
        body = offer,
    )).await;

    let ok = recv_final(&peer, "INVITE", Duration::from_secs(6))
        .await
        .expect("stack must answer the INVITE 200");
    assert_eq!(ok.status_code(), Some(200), "INVITE must be answered 200");
    let our_tag = header_tag(&ok, "To");

    // RFC 3311 §5.1: the initial 2xx advertises UPDATE in Allow.
    let allow = ok.get_header("Allow").unwrap_or("");
    assert!(
        allow.to_ascii_uppercase().contains("UPDATE"),
        "initial INVITE 200 Allow must advertise UPDATE (wire), got {allow:?}"
    );

    // ACK the 200.
    send(&peer, stack_local, format!(
        "ACK sip:asterisk@{stack_local} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}ack1\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag={ctag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n"
    )).await;

    // 2) Hold re-INVITE (sendonly) -> answer must be recvonly/inactive.
    let hold = format!(
        "{base}a=sendonly\r\n",
        base = SessionDescription::create_offer("127.0.0.1", peer_rtp_port, &[codecs::pcmu()])
    );
    send(&peer, stack_local, format!(
        "INVITE sip:asterisk@{stack_local} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}re2\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag={ctag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 2 INVITE\r\n\
         Contact: <sip:peer@127.0.0.1:{pport}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\r\n{body}",
        len = hold.len(),
        body = hold,
    )).await;

    let hold_ok = recv_final(&peer, "INVITE", Duration::from_secs(4))
        .await
        .expect("stack must answer the hold re-INVITE 200");
    assert_eq!(hold_ok.status_code(), Some(200), "hold re-INVITE must be 200");
    let hold_dir = audio_direction(&hold_ok);
    assert!(
        matches!(hold_dir, MediaDirection::RecvOnly | MediaDirection::Inactive),
        "RFC 3264 §6.1 (wire): sendonly hold offer must be answered recvonly/inactive, got {hold_dir:?}"
    );

    send(&peer, stack_local, format!(
        "ACK sip:asterisk@{stack_local} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}ack2\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag={ctag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 2 ACK\r\n\
         Content-Length: 0\r\n\r\n"
    )).await;

    // 3) No-SDP UPDATE, refresher=uac -> answered refresher=uac (never uas).
    send(&peer, stack_local, format!(
        "UPDATE sip:asterisk@{stack_local} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}upd3\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag={ctag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 3 UPDATE\r\n\
         Contact: <sip:peer@127.0.0.1:{pport}>\r\n\
         Supported: timer\r\n\
         Session-Expires: 1800;refresher=uac\r\n\
         Content-Length: 0\r\n\r\n"
    )).await;

    let upd_ok = recv_final(&peer, "UPDATE", Duration::from_secs(4))
        .await
        .expect("stack must answer the UPDATE 200");
    assert_eq!(upd_ok.status_code(), Some(200), "UPDATE must be 200");
    let se = upd_ok.get_header("Session-Expires").unwrap_or("");
    assert!(
        se.contains("refresher=uac"),
        "RFC 4028 §9 (wire): explicit refresher=uac must be honored, got {se:?}"
    );
    assert!(
        !se.to_ascii_lowercase().contains("uas"),
        "must never claim refresher=uas on the wire, got {se:?}"
    );

    // 4) OPTIONS ping (out-of-dialog) -> Allow advertises UPDATE.
    send(&peer, stack_local, format!(
        "OPTIONS sip:asterisk@{stack_local} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pport};branch=z9hG4bK{call_id}opt9\r\n\
         Max-Forwards: 70\r\n\
         From: \"Peer\" <sip:peer@127.0.0.1>;tag=opt{ctag}\r\n\
         To: <sip:asterisk@127.0.0.1>\r\n\
         Call-ID: {call_id}-options\r\n\
         CSeq: 9 OPTIONS\r\n\
         Contact: <sip:peer@127.0.0.1:{pport}>\r\n\
         Content-Length: 0\r\n\r\n"
    )).await;

    let opt_ok = recv_final(&peer, "OPTIONS", Duration::from_secs(4))
        .await
        .expect("stack must answer the OPTIONS 200");
    assert_eq!(opt_ok.status_code(), Some(200), "OPTIONS must be 200");
    let opt_allow = opt_ok.get_header("Allow").unwrap_or("");
    assert!(
        opt_allow.to_ascii_uppercase().contains("UPDATE"),
        "RFC 3311 §5.2 (wire): OPTIONS Allow must advertise UPDATE, got {opt_allow:?}"
    );

    println!(
        "[E2E] production stack: INVITE-2xx Allow+UPDATE, hold answered {hold_dir:?}, \
         UPDATE refresher=uac, OPTIONS Allow+UPDATE — all proven on the real wire"
    );
}
