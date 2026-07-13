//! End-to-end acceptance test for the inbound call path.
//!
//! Exercises, over real UDP sockets, the five fixes in the
//! `fix/inbound-media-plane` change set:
//!
//! * #10 — digest `qop=auth` verification (REGISTER **and** INVITE authenticate).
//! * #11 — inbound REGISTER routed to the registrar (401 challenge → 200 + Contact).
//! * #7  — inbound INVITE binds an RTP socket (media plane exists).
//! * #8  — the SDP answer advertises the socket's REAL port (never 10000).
//! * #9  — a live answered channel pumps media: `Echo()` reflects RTP back.
//!
//! This lives in its own integration-test binary so its use of process-global
//! state (`set_global_pjsip_config`, the tech registry, the app registry) is
//! isolated from the inline `src/lib.rs` unit tests.
//!
//! Acceptance gate: the test REGISTERs, INVITEs with `qop=auth`, sends RTP to
//! the negotiated port, and asserts echoed packets come back (count > 0, with
//! non-zero payload).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::auth::{create_digest_response, DigestChallenge, DigestCredentials};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{header_names, SipMessage};
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, AuthConfig, EndpointConfig, PjsipConfig,
};
use asterisk_sip::rtp::{build_rtp_packet, parse_rtp_header, RtpHeader};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const USER: &str = "100";
const PASS: &str = "1234";
const EXTEN: &str = "100";

/// Receive and parse one SIP datagram, bounded by `timeout`.
async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Receive SIP datagrams until one with the given status code arrives (or the
/// deadline passes). Used to skip the 100 Trying and capture the 200 OK.
async fn recv_sip_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(500)).await {
            if msg.status_code() == Some(status) {
                return Some(msg);
            }
        }
    }
    None
}

/// Build a raw REGISTER, optionally carrying an Authorization header.
fn register_request(call_id: &str, cseq: u32, auth: Option<&str>) -> SipMessage {
    let mut raw = format!(
        "REGISTER sip:127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKreg{cseq}\r\n\
         From: <sip:{USER}@127.0.0.1>;tag=caller\r\n\
         To: <sip:{USER}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} REGISTER\r\n\
         Contact: <sip:{USER}@127.0.0.1:5062>\r\n\
         Expires: 3600\r\n"
    );
    if let Some(a) = auth {
        raw.push_str(&format!("Authorization: {a}\r\n"));
    }
    raw.push_str("Content-Length: 0\r\n\r\n");
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Build a raw INVITE carrying an SDP offer and an Authorization header.
fn invite_request(call_id: &str, sdp: &str, auth: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKinv1\r\n\
         From: \"Caller\" <sip:{USER}@127.0.0.1>;tag=caller\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:{USER}@127.0.0.1:5062>\r\n\
         Authorization: {auth}\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

#[tokio::test]
async fn e2e_inbound_register_auth_and_media_echo() {
    // ---- Global wiring (isolated to this test binary) -------------------
    register_all_apps();

    // One endpoint "100" in context "default", authenticated as 100/1234.
    let config = PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: USER.to_string(),
            context: "default".to_string(),
            auth: Some("auth100".to_string()),
            ..Default::default()
        }],
        auths: vec![AuthConfig {
            name: "auth100".to_string(),
            auth_type: "userpass".to_string(),
            username: USER.to_string(),
            password: PASS.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    set_global_pjsip_config(config);

    // Dialplan: 100 -> Answer, Echo.
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

    // The handler's SEND transport, and the SIP channel driver that will hold
    // the inbound RTP session. Register the driver in the tech registry so
    // Echo() can find it by the "PJSIP" tech.
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

    // The caller's SIP socket (captures the handler's responses) and RTP socket.
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();

    // ---- 1. REGISTER without auth -> 401 challenge (issue #11) ----------
    let reg1 = register_request("reg-call-1", 1, None);
    handler.handle_register(&reg1, caller_sip_addr).await;
    let challenge_resp = recv_sip(&caller_sip, Duration::from_secs(2))
        .await
        .expect("expected a 401 challenge for unauthenticated REGISTER");
    assert_eq!(
        challenge_resp.status_code(),
        Some(401),
        "unauthenticated REGISTER must be challenged"
    );
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("401 must carry WWW-Authenticate");
    let challenge = DigestChallenge::parse(www).expect("challenge must parse");
    assert_eq!(challenge.qop.as_deref(), Some("auth"), "challenge offers qop=auth");
    println!("[E2E] REGISTER (no auth) -> 401 Unauthorized, qop=auth challenge received");

    // ---- 2. REGISTER with qop=auth -> 200 + Contact (issues #10, #11) ---
    let creds = DigestCredentials {
        username: USER.to_string(),
        password: PASS.to_string(),
        realm: challenge.realm.clone(),
    };
    let reg_auth = create_digest_response(&challenge, &creds, "REGISTER", "sip:127.0.0.1");
    let reg2 = register_request("reg-call-1", 2, Some(&reg_auth));
    handler.handle_register(&reg2, caller_sip_addr).await;
    let reg_ok = recv_sip(&caller_sip, Duration::from_secs(2))
        .await
        .expect("expected 200 OK for authenticated REGISTER");
    assert_eq!(
        reg_ok.status_code(),
        Some(200),
        "qop=auth REGISTER must authenticate (issue #10)"
    );
    assert!(
        reg_ok.get_header(header_names::CONTACT).is_some(),
        "200 OK to REGISTER must list the bound Contact"
    );
    assert_eq!(
        handler.registrar().get_contacts(USER).len(),
        1,
        "the contact must be bound in the registrar"
    );
    println!("[E2E] REGISTER (qop=auth) -> 200 OK, contact bound in registrar");

    // ---- 3. INVITE with qop=auth + SDP offer (issues #10, #7, #8) -------
    // Offer advertises the caller's real RTP port so the handler echoes there.
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );
    let inv_auth = create_digest_response(
        &challenge,
        &creds,
        "INVITE",
        &format!("sip:{EXTEN}@127.0.0.1"),
    );
    let invite = invite_request("inv-call-1", &offer.to_string(), &inv_auth);
    let session = SipSession::new_inbound(&invite, sip_local, caller_sip_addr)
        .expect("inbound session");
    let call_id = handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    assert_eq!(
        call_id.as_deref(),
        Some("inv-call-1"),
        "authenticated INVITE must be accepted (issue #10 on the INVITE path)"
    );

    // Capture the 200 OK (skipping 100 Trying) and read the negotiated port.
    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("expected 200 OK with SDP answer");
    let answer = SessionDescription::parse(&ok.body).expect("answer SDP must parse");
    let audio = answer
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("answer must contain an audio stream");
    assert_ne!(audio.port, 0, "answer must advertise a live RTP port");
    assert!(
        (asterisk_sip::rtp::DEFAULT_RTP_PORT_START
            ..=asterisk_sip::rtp::DEFAULT_RTP_PORT_END)
            .contains(&audio.port),
        "answer port must be inside the configured default RTP range"
    );
    let media_addr: SocketAddr = format!("127.0.0.1:{}", audio.port).parse().unwrap();
    println!(
        "[E2E] INVITE (qop=auth) -> 100 Trying -> 200 OK; SDP answer RTP port = {} (bounded socket)",
        audio.port
    );

    // ---- 4. Media pump: send RTP, assert echoes come back (issue #9) ----
    // Send a burst of PCMU frames and count non-zero echoed payloads. Echo
    // starts once pbx_run reaches priority 2 (just after Answer sends 200), so
    // interleave send/recv over a budget to avoid a startup race.
    let payload = [0x7Fu8; 160]; // constant non-silent PCMU payload
    let mut echoed = 0usize;
    let mut recv_buf = [0u8; 2048];
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut seq: u16 = 0;
    while Instant::now() < deadline && echoed < 3 {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: seq == 0,
            payload_type: 0, // PCMU
            sequence: seq,
            timestamp: (seq as u32) * 160,
            ssrc: 0x0BADF00D,
        };
        let packet = build_rtp_packet(&header, &payload);
        let _ = caller_rtp.send_to(&packet[..], media_addr).await;
        seq = seq.wrapping_add(1);

        if let Ok(Ok((len, _src))) =
            tokio::time::timeout(Duration::from_millis(150), caller_rtp.recv_from(&mut recv_buf))
                .await
        {
            if let Ok((_hdr, echoed_payload)) = parse_rtp_header(&recv_buf[..len]) {
                if echoed_payload.iter().any(|&b| b != 0) {
                    echoed += 1;
                }
            }
        }
    }

    assert!(
        echoed > 0,
        "Echo() must reflect RTP frames back to the caller (issue #9): got {echoed} echoed frames"
    );
    println!(
        "[E2E] Sent {} RTP frames to port {}; Echo() reflected {} non-zero payload frames back",
        seq, audio.port, echoed
    );
}
