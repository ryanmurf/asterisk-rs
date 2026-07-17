//! End-to-end acceptance for a re-INVITE that ACTUALLY renegotiates, plus hold.
//!
//! Drives a full inbound INVITE -> Answer -> Echo call over real UDP sockets,
//! then exercises the three M5 re-INVITE behaviours, each proven receiver-side
//! via Echo (which reflects only accepted media back to its accepted source):
//!
//!  * **renegotiate** — a re-INVITE moves the caller's RTP to a NEW port AND
//!    switches codec PCMU(0) -> PCMA(8). Echo reflects PCMA from the new port
//!    only if rustisk both re-pointed the remote AND re-installed the payload
//!    type (else recv_frame discards the new-source, wrong-PT datagrams).
//!  * **hold** — a `sendonly` re-INVITE pauses the media pump; the caller now
//!    gets no echo back at all.
//!  * **un-hold** — a `sendrecv` re-INVITE resumes the pump; echo returns.
//!
//! RED controls (PR body): skip `apply_inbound_offer` -> the renegotiate step's
//! echo never comes back; defeat the hold gate -> the hold step keeps echoing.

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
use asterisk_sip::rtp::{build_rtp_packet, parse_rtp_header, RtpHeader};
use asterisk_sip::sdp::{MediaDirection, SessionDescription};
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

fn header_tag(msg: &SipMessage, header: &str) -> String {
    msg.get_header(header)
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("tag="))
                .map(|t| t.to_string())
        })
        .expect("header must carry a tag")
}

fn answer_port(ok: &SipMessage) -> u16 {
    SessionDescription::parse(&ok.body)
        .unwrap()
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .unwrap()
        .port
}

fn invite_request(call_id: &str, from_tag: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}inv\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={from_tag}\r\n\
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

fn reinvite_request(
    call_id: &str,
    our_tag: &str,
    caller_tag: &str,
    contact_port: u16,
    cseq: u32,
    sdp: &str,
) -> SipMessage {
    let raw = format!(
        "INVITE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}re{cseq}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={caller_tag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:{contact_port}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Deliver a re-INVITE and capture its 200 OK.
async fn reinvite(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    caller_sip: &UdpSocket,
    caller_addr: SocketAddr,
    msg: SipMessage,
) -> SipMessage {
    let session = SipSession::new_inbound(&msg, sip_local, caller_addr).expect("session");
    handler.handle_incoming_invite(&msg, caller_addr, session).await;
    recv_sip_status(caller_sip, 200, Duration::from_secs(2))
        .await
        .expect("re-INVITE must be answered 200")
}

/// Send a burst of RTP with `pt`, report whether any non-zero echo returns.
async fn echo_ok(src: &UdpSocket, dest: SocketAddr, pt: u8, budget: Duration) -> bool {
    let payload = [0x55u8; 160];
    let mut buf = [0u8; 2048];
    let deadline = Instant::now() + budget;
    let mut seq: u16 = 100;
    while Instant::now() < deadline {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: pt,
            sequence: seq,
            timestamp: (seq as u32) * 160,
            ssrc: 0x0505_0505,
        };
        let _ = src.send_to(&build_rtp_packet(&header, &payload)[..], dest).await;
        seq = seq.wrapping_add(1);
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(120), src.recv_from(&mut buf)).await
        {
            if let Ok((_h, pl)) = parse_rtp_header(&buf[..len]) {
                if pl.iter().any(|&b| b != 0) {
                    return true;
                }
            }
        }
    }
    false
}

/// A hold offer: PCMA at `port` with `a=sendonly`.
fn hold_offer(port: u16) -> String {
    let base = SessionDescription::create_offer("127.0.0.1", port, &[codecs::pcma()]).to_string();
    format!("{base}a=sendonly\r\n")
}

#[tokio::test]
async fn reinvite_renegotiates_media_and_holds_receiver_side() {
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

    // ---- Establish an answered Echo() call (PCMU at port A) ---------------
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_addr = caller_sip.local_addr().unwrap();
    let rtp_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_a_addr = rtp_a.local_addr().unwrap();

    let call_id = "reinv-call-1";
    let caller_tag = "callerR";
    let offer =
        SessionDescription::create_offer("127.0.0.1", rtp_a_addr.port(), &[codecs::pcmu()]);
    let invite = invite_request(call_id, caller_tag, caller_addr.port(), &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).unwrap();
    handler.handle_incoming_invite(&invite, caller_addr, session).await;
    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    let our_tag = header_tag(&ok, "To");
    let rustisk_rtp: SocketAddr = format!("127.0.0.1:{}", answer_port(&ok)).parse().unwrap();
    assert!(
        echo_ok(&rtp_a, rustisk_rtp, 0, Duration::from_secs(3)).await,
        "baseline: PCMU echo from port A must return"
    );

    // ---- Renegotiate: NEW port B + codec switch PCMU->PCMA ----------------
    let rtp_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_b_addr = rtp_b.local_addr().unwrap();
    // Before the re-INVITE, PCMA from port B is neither the latched source nor
    // the negotiated PT -> discarded, no echo.
    assert!(
        !echo_ok(&rtp_b, rustisk_rtp, 8, Duration::from_millis(700)).await,
        "pre-renegotiation: PCMA from the new port must not be echoed"
    );
    let reneg_offer =
        SessionDescription::create_offer("127.0.0.1", rtp_b_addr.port(), &[codecs::pcma()]);
    let ok2 = reinvite(
        &handler,
        sip_local,
        &caller_sip,
        caller_addr,
        reinvite_request(call_id, &our_tag, caller_tag, caller_addr.port(), 2, &reneg_offer.to_string()),
    )
    .await;
    let rustisk_rtp2: SocketAddr = format!("127.0.0.1:{}", answer_port(&ok2)).parse().unwrap();
    assert!(
        echo_ok(&rtp_b, rustisk_rtp2, 8, Duration::from_secs(3)).await,
        "renegotiate: after the re-INVITE, PCMA from the new port must be echoed \
         (remote re-pointed AND payload type re-installed)"
    );
    println!("[E2E] re-INVITE renegotiated: new RTP port + PCMU->PCMA proven at the receiver");

    // ---- Hold: sendonly re-INVITE pauses the pump -------------------------
    let ok3 = reinvite(
        &handler,
        sip_local,
        &caller_sip,
        caller_addr,
        reinvite_request(call_id, &our_tag, caller_tag, caller_addr.port(), 3, &hold_offer(rtp_b_addr.port())),
    )
    .await;
    assert_eq!(ok3.status_code(), Some(200), "hold re-INVITE must be answered 200");
    // RFC 3264 §6.1 (M5 review MAJOR-2): a `sendonly` hold offer MUST be
    // answered `recvonly` or `inactive`, never `sendrecv`. Assert the wire SDP
    // direction of the hold 200 — the receiver-side pump-pause proof below is
    // necessary but not sufficient; the negotiated wire contract must be valid.
    let hold_answer = SessionDescription::parse(&ok3.body).expect("hold 200 must carry SDP");
    let hold_dir = hold_answer
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("audio stream in hold answer")
        .direction;
    assert!(
        matches!(hold_dir, MediaDirection::RecvOnly | MediaDirection::Inactive),
        "RFC 3264 §6.1: a sendonly hold offer must be answered recvonly or inactive, \
         got {hold_dir:?}"
    );
    assert!(
        !echo_ok(&rtp_b, rustisk_rtp2, 8, Duration::from_secs(1)).await,
        "hold: the media pump must be paused (no echo returns while on hold)"
    );
    println!("[E2E] hold: 200 answered recvonly/inactive (RFC 3264 §6.1); media pump paused");

    // ---- Un-hold: sendrecv re-INVITE resumes the pump ---------------------
    let unhold_offer =
        SessionDescription::create_offer("127.0.0.1", rtp_b_addr.port(), &[codecs::pcma()]);
    let ok4 = reinvite(
        &handler,
        sip_local,
        &caller_sip,
        caller_addr,
        reinvite_request(call_id, &our_tag, caller_tag, caller_addr.port(), 4, &unhold_offer.to_string()),
    )
    .await;
    assert_eq!(ok4.status_code(), Some(200), "un-hold re-INVITE must be answered 200");
    assert!(
        echo_ok(&rtp_b, rustisk_rtp2, 8, Duration::from_secs(3)).await,
        "un-hold: the media pump must resume (echo returns)"
    );
    println!("[E2E] un-hold: media resumed at the receiver");
}
