//! End-to-end acceptance test for ConfBridge audio mixing (issue #12).
//!
//! Three SIP legs join `ConfBridge(mixroom)` over real UDP sockets and
//! exchange RTP with the conference mixer:
//!
//! * leg A sends a constant **+6000** µ-law tone,
//! * leg B sends a constant **-2000** µ-law tone,
//! * leg C sends **nothing at all** (a listen-only participant — its mix
//!   must still arrive at full cadence even though the leg never sends a
//!   packet).
//!
//! With genuine per-participant mix-minus each leg must hear the sum of the
//! *other* legs, so the received signal separates cleanly by sign/level:
//!
//! * A hears B + C = **-2000** — strictly negative. If A's own +6000 leaked
//!   into its mix the received mean would flip positive (full-sum leak
//!   reads +4000), so this asserts "does not hear its own signal echoed".
//! * B hears A + C = **+6000**. A self-leak would read +4000, so the level
//!   asserts B's own -2000 is absent.
//! * C hears A + B = **+4000** — strictly *between* A's +6000 and B's
//!   -2000, which proves audio energy from BOTH other legs is present.
//!
//! Then A hangs up (BYE): C's received mean must flip to **-2000** (B's
//! tone, with A's contribution gone) — the remaining two still pass audio
//! and the departed leg's audio is fully removed. Finally B and C hang up
//! and the conference (and its mixer) must be destroyed.
//!
//! This lives in its own integration-test binary because it uses
//! process-global state (global PJSIP config, tech registry, app registry),
//! mirroring `e2e_inbound_call.rs`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_apps::confbridge::AppConfBridge;
use asterisk_apps::confbridge_mix;
use asterisk_codecs::codecs;
use asterisk_codecs::ulaw_table::{linear_to_mulaw_fast, mulaw_to_linear};
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
const CONF_EXTEN: &str = "300";
const ROOM: &str = "mixroom";

/// Received-sample magnitude above which a sample counts as "voiced"
/// (well above µ-law idle noise, well below the test tones).
const VOICED: i16 = 500;

/// Receive and parse one SIP datagram, bounded by `timeout`.
async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Receive SIP datagrams until one with the given status code arrives.
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
         From: <sip:{USER}@127.0.0.1>;tag=reg\r\n\
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

/// Build a raw INVITE to the conference extension for one leg.
fn invite_request(leg: &str, call_id: &str, sdp: &str, auth: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{CONF_EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKinv{leg}\r\n\
         From: \"Leg {leg}\" <sip:{USER}@127.0.0.1>;tag=leg-{leg}\r\n\
         To: <sip:{CONF_EXTEN}@127.0.0.1>\r\n\
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

/// Build a raw BYE for one leg's dialog (matched by Call-ID).
fn bye_request(leg: &str, call_id: &str, local_tag: &str) -> SipMessage {
    let raw = format!(
        "BYE sip:{CONF_EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKbye{leg}\r\n\
         From: <sip:{USER}@127.0.0.1>;tag=leg-{leg}\r\n\
         To: <sip:{CONF_EXTEN}@127.0.0.1>;tag={local_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 2 BYE\r\n\
         Content-Length: 0\r\n\r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Spawn a task sending a constant-amplitude 20 ms PCMU stream to `dest`.
fn spawn_sender(
    rtp: Arc<UdpSocket>,
    dest: SocketAddr,
    amplitude: i16,
    ssrc: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let payload = [linear_to_mulaw_fast(amplitude); 160];
        let mut seq: u16 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        loop {
            interval.tick().await;
            let header = RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                csrc_count: 0,
                marker: seq == 0,
                payload_type: 0, // PCMU
                sequence: seq,
                timestamp: (seq as u32) * 160,
                ssrc,
            };
            let packet = build_rtp_packet(&header, &payload);
            let _ = rtp.send_to(&packet[..], dest).await;
            seq = seq.wrapping_add(1);
        }
    })
}

/// Collect RTP received on `rtp` for `window`: (frame count, voiced samples).
async fn collect(rtp: &UdpSocket, window: Duration) -> (usize, Vec<i16>) {
    let deadline = Instant::now() + window;
    let mut frames = 0usize;
    let mut voiced = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        if let Ok(Ok((len, _src))) =
            tokio::time::timeout(Duration::from_millis(100), rtp.recv_from(&mut buf)).await
        {
            if let Ok((_hdr, payload)) = parse_rtp_header(&buf[..len]) {
                frames += 1;
                for &b in payload {
                    let s = mulaw_to_linear(b);
                    if s.abs() > VOICED {
                        voiced.push(s);
                    }
                }
            }
        }
    }
    (frames, voiced)
}

/// Drain anything queued on a socket (phase separator).
async fn drain(rtp: &UdpSocket, window: Duration) {
    let deadline = Instant::now() + window;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(50), rtp.recv_from(&mut buf)).await;
    }
}

fn mean(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|&s| s as f64).sum::<f64>() / samples.len() as f64
}

/// Poll the conference registry until it has `count` participants.
async fn wait_for_participants(count: usize, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let n = AppConfBridge::show_conference(ROOM)
            .map(|info| info.participant_count)
            .unwrap_or(0);
        if n == count {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

struct Leg {
    name: &'static str,
    sip: UdpSocket,
    sip_addr: SocketAddr,
    rtp: Arc<UdpSocket>,
    call_id: String,
    local_tag: String,
    /// The conference-side RTP address this leg sends to (from the answer).
    media_addr: SocketAddr,
}

#[tokio::test]
async fn e2e_three_leg_confbridge_mixing() {
    // ---- Global wiring (isolated to this test binary) -------------------
    register_all_apps();

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

    // Dialplan: 300 -> Answer, ConfBridge(mixroom).
    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    let mut ext = Extension::new(CONF_EXTEN);
    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "ConfBridge".to_string(),
        app_data: ROOM.to_string(),
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

    // ---- Digest challenge (REGISTER once, reuse the nonce) --------------
    let reg_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let reg_addr = reg_sock.local_addr().unwrap();
    handler
        .handle_register(&register_request("conf-reg-1", 1, None), reg_addr)
        .await;
    let challenge_resp = recv_sip(&reg_sock, Duration::from_secs(2))
        .await
        .expect("expected 401 challenge for unauthenticated REGISTER");
    assert_eq!(challenge_resp.status_code(), Some(401));
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("401 must carry WWW-Authenticate");
    let challenge = DigestChallenge::parse(www).expect("challenge must parse");
    let creds = DigestCredentials {
        username: USER.to_string(),
        password: PASS.to_string(),
        realm: challenge.realm.clone(),
    };

    // ---- Three legs INVITE the conference extension ----------------------
    let mut legs: Vec<Leg> = Vec::new();
    for name in ["a", "b", "c"] {
        let sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sip_addr = sip.local_addr().unwrap();
        let rtp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let rtp_addr = rtp.local_addr().unwrap();
        let call_id = format!("conf-call-{name}");

        let offer = SessionDescription::create_offer(
            &rtp_addr.ip().to_string(),
            rtp_addr.port(),
            &[codecs::pcmu()],
        );
        let auth = create_digest_response(
            &challenge,
            &creds,
            "INVITE",
            &format!("sip:{CONF_EXTEN}@127.0.0.1"),
        );
        let invite = invite_request(name, &call_id, &offer.to_string(), &auth);
        let session =
            SipSession::new_inbound(&invite, sip_local, sip_addr).expect("inbound session");
        let accepted = handler.handle_incoming_invite(&invite, sip_addr, session).await;
        assert_eq!(
            accepted.as_deref(),
            Some(call_id.as_str()),
            "leg {name}: authenticated INVITE must be accepted"
        );

        let ok = recv_sip_status(&sip, 200, Duration::from_secs(5))
            .await
            .unwrap_or_else(|| panic!("leg {name}: expected 200 OK with SDP answer"));
        let answer = SessionDescription::parse(&ok.body).expect("answer SDP must parse");
        let local_tag = ok
            .to_header()
            .and_then(asterisk_sip::parser::extract_tag)
            .expect("200 OK must establish a local dialog tag");
        let audio = answer
            .media_descriptions
            .iter()
            .find(|m| m.media_type == "audio")
            .expect("answer must contain an audio stream");
        assert_ne!(audio.port, 0, "leg {name}: answer must advertise a live RTP port");
        let media_addr: SocketAddr = format!("127.0.0.1:{}", audio.port).parse().unwrap();

        println!("[E2E] leg {name}: joined, conference RTP at {media_addr}");
        legs.push(Leg {
            name,
            sip,
            sip_addr,
            rtp,
            call_id,
            local_tag,
            media_addr,
        });
    }

    assert!(
        wait_for_participants(3, Duration::from_secs(10)).await,
        "all three legs must join conference '{ROOM}'"
    );
    let mixer = confbridge_mix::get_mixer(ROOM).expect("conference must have a live mixer");
    assert!(mixer.is_running());
    println!("[E2E] conference '{ROOM}' has 3 participants; mixer running");

    // ---- Phase 1: A=+6000, B=-2000, C=listen-only ------------------------
    let send_a = spawn_sender(legs[0].rtp.clone(), legs[0].media_addr, 6000, 0xAAAA);
    let send_b = spawn_sender(legs[1].rtp.clone(), legs[1].media_addr, -2000, 0xBBBB);
    // Leg C deliberately sends NOTHING: a listen-only participant must
    // still receive the conference mix at a steady cadence.

    let window = Duration::from_millis(2500);
    let ((fa, va), (fb, vb), (fc, vc)) = tokio::join!(
        collect(&legs[0].rtp, window),
        collect(&legs[1].rtp, window),
        collect(&legs[2].rtp, window),
    );
    let (ma, mb, mc) = (mean(&va), mean(&vb), mean(&vc));
    println!(
        "[E2E] phase 1: A {fa} frames, {} voiced, mean {ma:.0} | B {fb} frames, {} voiced, mean {mb:.0} | C {fc} frames, {} voiced, mean {mc:.0}",
        va.len(), vb.len(), vc.len()
    );

    // Every leg receives a steady stream and real audio energy.
    for (leg, frames, voiced) in [(&legs[0], fa, &va), (&legs[1], fb, &vb), (&legs[2], fc, &vc)] {
        assert!(
            frames > 50,
            "leg {}: expected a steady ~50 pps mix stream, got {frames} frames",
            leg.name
        );
        assert!(
            voiced.len() > 800,
            "leg {}: expected audio energy from the other legs, got {} voiced samples",
            leg.name,
            voiced.len()
        );
    }
    // A hears B+C = -2000: strictly negative. Self-leak would read +4000.
    assert!(
        ma < -1000.0,
        "leg a: mix must be B+C (~-2000) with A's own +6000 absent, got mean {ma:.0}"
    );
    // B hears A+C = +6000. With B's own -2000 leaked it would read +4000.
    assert!(
        mb > 4800.0,
        "leg b: mix must be A+C (~+6000) with B's own -2000 absent, got mean {mb:.0}"
    );
    // C hears A+B = +4000: strictly between A's +6000 and B's -2000, which
    // requires energy from BOTH other legs in the mix.
    assert!(
        mc > 1000.0 && mc < 5500.0,
        "leg c: mix must contain both A and B (~+4000), got mean {mc:.0}"
    );
    println!("[E2E] phase 1 OK: mix-minus verified on all three legs");

    // ---- Phase 2: A hangs up; B and C must keep passing audio -----------
    send_a.abort();
    handler
        .handle_bye(
            &bye_request("a", &legs[0].call_id, &legs[0].local_tag),
            legs[0].sip_addr,
        )
        .await;
    let bye_ok = recv_sip_status(&legs[0].sip, 200, Duration::from_secs(2)).await;
    assert!(bye_ok.is_some(), "leg a: BYE must be answered with 200 OK");
    assert!(
        wait_for_participants(2, Duration::from_secs(5)).await,
        "conference must drop to 2 participants after A's BYE"
    );
    println!("[E2E] leg a left (BYE); 2 participants remain");

    // Let in-flight frames flush, then measure the new steady state.
    tokio::join!(
        drain(&legs[1].rtp, Duration::from_millis(400)),
        drain(&legs[2].rtp, Duration::from_millis(400)),
    );
    let window2 = Duration::from_millis(1500);
    let ((fb2, vb2), (fc2, vc2)) = tokio::join!(
        collect(&legs[1].rtp, window2),
        collect(&legs[2].rtp, window2),
    );
    let (mb2, mc2) = (mean(&vb2), mean(&vc2));
    println!(
        "[E2E] phase 2: B {fb2} frames, {} voiced, mean {mb2:.0} | C {fc2} frames, {} voiced, mean {mc2:.0}",
        vb2.len(), vc2.len()
    );

    // Audio still flows to the remaining legs at cadence.
    assert!(fb2 > 30, "leg b: mix stream must continue after A left, got {fb2} frames");
    assert!(fc2 > 30, "leg c: mix stream must continue after A left, got {fc2} frames");
    // C now hears only B: the mean flips negative (~-2000), proving A's
    // +6000 is fully gone AND B's audio still reaches C.
    assert!(
        vc2.len() > 400 && mc2 < -1000.0,
        "leg c: after A left the mix must be B alone (~-2000), got {} voiced samples, mean {mc2:.0}",
        vc2.len()
    );
    // B hears only C (silence): essentially no voiced samples.
    assert!(
        vb2.len() < 480,
        "leg b: only silent C remains, expected near-silence, got {} voiced samples",
        vb2.len()
    );
    println!("[E2E] phase 2 OK: remaining legs still pass audio; A's signal fully removed");

    // ---- Phase 3: everyone leaves; conference + mixer torn down ---------
    send_b.abort();
    handler
        .handle_bye(
            &bye_request("b", &legs[1].call_id, &legs[1].local_tag),
            legs[1].sip_addr,
        )
        .await;
    handler
        .handle_bye(
            &bye_request("c", &legs[2].call_id, &legs[2].local_tag),
            legs[2].sip_addr,
        )
        .await;
    for leg in &legs[1..] {
        let ok = recv_sip_status(&leg.sip, 200, Duration::from_secs(2)).await;
        assert!(ok.is_some(), "leg {}: BYE must be answered with 200 OK", leg.name);
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && AppConfBridge::show_conference(ROOM).is_some() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        AppConfBridge::show_conference(ROOM).is_none(),
        "conference must be destroyed after the last leg leaves"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && confbridge_mix::get_mixer(ROOM).is_some() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        confbridge_mix::get_mixer(ROOM).is_none(),
        "the conference's mixer must be unregistered on destroy"
    );
    assert!(!mixer.is_running(), "the mixer task must be stopped");
    println!("[E2E] phase 3 OK: conference and mixer torn down after last leave");
}
