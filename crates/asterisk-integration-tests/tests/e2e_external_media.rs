//! End-to-end acceptance tests for ARI `POST /channels/externalMedia`
//! (issue #13).
//!
//! Two tests, both against the real ARI HTTP listener over TCP:
//!
//! * `external_media_route_lifecycle` — response shape, client-supplied
//!   `channelId`, duplicate-id conflict, `UNICASTRTP_LOCAL_PORT` variable,
//!   the live RTP fork at driver level, and full teardown on
//!   `DELETE /channels/{id}` (no leaked driver entry / socket — the F23
//!   leak shape).
//!
//! * `e2e_external_media_rtp_fork_bidirectional` — a real SIP call leg
//!   (REGISTER + INVITE with digest auth, answered by the dialplan) is
//!   bridged with an externalMedia channel created via the HTTP API. RTP
//!   sent by the SIP caller is asserted to arrive at the fake external
//!   endpoint with the right payload type, fresh SSRC, and 20ms timestamp
//!   cadence; RTP sent back by the external endpoint is asserted to reach
//!   the SIP caller (bidirectional).
//!
//! NOTE on bridging: the generic bridge subsystem does not yet move driver
//! media between channels (Dial's bridge loop only monitors hangup state),
//! so the frame relay between the SIP leg and the externalMedia leg is a
//! small test-side pump. The externalMedia channel's own media plane — the
//! product code under test — is exercised end-to-end through the channel
//! driver on both directions. The missing generic bridge pump is the
//! documented remainder on issue #13.
//!
//! This lives in its own integration-test binary so its use of
//! process-global state (tech registry, channel store, pjsip config, app
//! registry) is isolated from other test binaries.

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_ari::http_listener::AriHttpListener;
use asterisk_ari::server::{AriConfig, AriServer, AriUser};
use asterisk_channels::rtp_channel::RtpChannelDriver;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::channel::{Channel, ChannelDriver};
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
use asterisk_types::Frame;
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

const USER: &str = "100";
const PASS: &str = "1234";
const EXTEN: &str = "100";
const APP: &str = "extmedia-e2e";

/// One shared UnicastRTP driver instance for this whole test binary,
/// registered exactly once in the global tech registry. Both tests must use
/// this instance so a second registration cannot orphan driver privates.
static UNICAST_DRIVER: LazyLock<Arc<RtpChannelDriver>> = LazyLock::new(|| {
    let driver = Arc::new(RtpChannelDriver::new());
    TECH_REGISTRY.register(driver.clone());
    driver
});

// ---------------------------------------------------------------------------
// Small HTTP + SIP helpers
// ---------------------------------------------------------------------------

/// Start an ARI server with routes installed and serve it on an ephemeral
/// TCP port. Returns the server handle and the bound address.
async fn start_ari_server() -> (Arc<AriServer>, SocketAddr) {
    let mut server = AriServer::new(AriConfig {
        users: vec![AriUser {
            username: "test".to_string(),
            password: "secret".to_string(),
            read_only: false,
        }],
        ..Default::default()
    });
    server.install_routes();
    let server = Arc::new(server);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ARI HTTP listener");
    let addr = listener.local_addr().expect("ARI listener local addr");
    let http = AriHttpListener::new(server.clone(), addr);
    tokio::spawn(async move {
        let _ = http.serve(listener).await;
    });
    (server, addr)
}

/// Issue one HTTP request against the ARI listener and return
/// (status, body).
async fn http_request(addr: SocketAddr, method: &str, path_and_query: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect ARI listener");
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        method, path_and_query
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send HTTP request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .expect("read HTTP response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("malformed HTTP response: {}", text));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// Receive and parse one SIP datagram, bounded by `timeout`.
async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

/// Receive SIP datagrams until one with the given status code arrives (or
/// the deadline passes). Used to skip the 100 Trying and capture the 200 OK.
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
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKemreg{cseq}\r\n\
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
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKeminv1\r\n\
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

// ---------------------------------------------------------------------------
// Test 1: route lifecycle over HTTP
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_media_route_lifecycle() {
    let driver = UNICAST_DRIVER.clone();
    let (server, ari_addr) = start_ari_server().await;

    // Fake external endpoint.
    let external = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let external_addr = external.local_addr().unwrap();

    // Register the app up front so the channel lands in it.
    let app = server.app_registry.register_app("route-lc-app");

    // -- create with a client-supplied channelId over real HTTP -----------
    let (status, body) = http_request(
        ari_addr,
        "POST",
        &format!(
            "/ari/channels/externalMedia?app=route-lc-app&external_host={}&format=alaw&channelId=em-route-1",
            external_addr
        ),
    )
    .await;
    assert_eq!(status, 200, "externalMedia create failed: {}", body);
    let json: serde_json::Value = serde_json::from_str(&body).expect("channel JSON");
    assert_eq!(json["id"], "em-route-1");
    assert_eq!(json["state"], "Up");
    let em_name = json["name"].as_str().expect("channel name").to_string();
    assert!(
        em_name.starts_with("UnicastRTP/"),
        "unexpected channel name: {}",
        em_name
    );

    // Registered in the channel store and subscribed to the Stasis app.
    assert!(asterisk_core::channel_store::find_by_uniqueid("em-route-1").is_some());
    assert!(app.channel_ids.read().contains("em-route-1"));

    // Local RTP bind address exposed via channel variables.
    let (status, body) = http_request(
        ari_addr,
        "GET",
        "/ari/channels/em-route-1/variable?variable=UNICASTRTP_LOCAL_PORT",
    )
    .await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).expect("variable JSON");
    let local_port: u16 = json["value"]
        .as_str()
        .expect("variable value")
        .parse()
        .expect("UNICASTRTP_LOCAL_PORT must be numeric");
    assert_ne!(local_port, 0);

    // -- duplicate channelId -> 409 ----------------------------------------
    let (status, body) = http_request(
        ari_addr,
        "POST",
        &format!(
            "/ari/channels/externalMedia?app=route-lc-app&external_host={}&format=ulaw&channelId=em-route-1",
            external_addr
        ),
    )
    .await;
    assert_eq!(status, 409, "duplicate channelId must conflict: {}", body);

    // -- media plane is live: fork one frame to the external endpoint ------
    let mut handle = Channel::new(&em_name);
    let frame = Frame::voice(8, 160, Bytes::from_static(&[0x55u8; 160]));
    driver
        .write_frame(&mut handle, &frame)
        .await
        .expect("driver write_frame");
    let mut buf = [0u8; 2048];
    let (len, src) = tokio::time::timeout(Duration::from_secs(2), external.recv_from(&mut buf))
        .await
        .expect("timed out waiting for forked RTP")
        .unwrap();
    let (header, payload) = parse_rtp_header(&buf[..len]).unwrap();
    assert_eq!(header.payload_type, 8, "alaw fork must use payload type 8");
    assert_eq!(payload.len(), 160);
    assert_eq!(
        src.port(),
        local_port,
        "fork must originate from the advertised UNICASTRTP_LOCAL_PORT"
    );

    // -- DELETE tears everything down ---------------------------------------
    let (status, _) = http_request(ari_addr, "DELETE", "/ari/channels/em-route-1").await;
    assert_eq!(status, 204);
    assert!(
        asterisk_core::channel_store::find_by_uniqueid("em-route-1").is_none(),
        "channel must leave the store on hangup"
    );
    assert!(
        !app.channel_ids.read().contains("em-route-1"),
        "channel must leave the Stasis app on hangup"
    );
    assert!(
        driver.write_frame(&mut handle, &frame).await.is_err(),
        "driver entry (and its RTP socket) must be released on hangup"
    );
}

// ---------------------------------------------------------------------------
// Test 2: full E2E — SIP leg bridged to an externalMedia channel
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_external_media_rtp_fork_bidirectional() {
    // ---- Global wiring (isolated to this test binary) --------------------
    register_all_apps();
    let unicast_driver = UNICAST_DRIVER.clone();

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

    // Dialplan: 100 -> Answer, Wait(30). Wait never touches media, so the
    // SIP leg's RTP session is pumped exclusively by the bridge below.
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
        app: "Wait".to_string(),
        app_data: "30".to_string(),
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
    let sip_driver = Arc::new(SipChannelDriver::new(sip_local));
    sip_driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(sip_driver.clone());

    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(sip_driver.clone());

    // The caller's SIP socket and RTP socket.
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();

    // The fake external media endpoint (the "RTP sidecar").
    let external = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let external_addr = external.local_addr().unwrap();

    // ---- 1. Create the externalMedia channel via the ARI HTTP API --------
    let (server, ari_addr) = start_ari_server().await;
    let app = server.app_registry.register_app(APP);

    let (status, body) = http_request(
        ari_addr,
        "POST",
        &format!(
            "/ari/channels/externalMedia?app={}&external_host={}&format=ulaw&channelId=em-e2e-1",
            APP, external_addr
        ),
    )
    .await;
    assert_eq!(status, 200, "externalMedia create failed: {}", body);
    let json: serde_json::Value = serde_json::from_str(&body).expect("channel JSON");
    let em_name = json["name"].as_str().expect("channel name").to_string();
    assert!(app.channel_ids.read().contains("em-e2e-1"));
    println!(
        "[E2E] externalMedia channel '{}' created via HTTP, forking to {}",
        em_name, external_addr
    );

    // ---- 2. SIP leg: REGISTER (grab challenge) + authenticated INVITE ----
    let reg1 = register_request("em-reg-1", 1, None);
    handler.handle_register(&reg1, caller_sip_addr).await;
    let challenge_resp = recv_sip(&caller_sip, Duration::from_secs(2))
        .await
        .expect("expected a 401 challenge for unauthenticated REGISTER");
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
    let invite = invite_request("em-inv-1", &offer.to_string(), &inv_auth);
    let session =
        SipSession::new_inbound(&invite, sip_local, caller_sip_addr).expect("inbound session");
    let call_id = handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    assert_eq!(
        call_id.as_deref(),
        Some("em-inv-1"),
        "INVITE must be accepted"
    );

    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("expected 200 OK with SDP answer");
    let answer = SessionDescription::parse(&ok.body).expect("answer SDP must parse");
    let audio = answer
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("answer must contain an audio stream");
    assert_ne!(audio.port, 0);
    let sip_media_addr: SocketAddr = format!("127.0.0.1:{}", audio.port).parse().unwrap();
    println!(
        "[E2E] SIP leg answered; negotiated RTP port = {}",
        audio.port
    );

    // Wait for the answered SIP channel to appear in the channel store.
    let sip_chan_name = {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = None;
        while Instant::now() < deadline {
            let candidates = asterisk_core::channel_store::find_by_exten("default", EXTEN);
            if let Some(chan) = candidates.first() {
                found = Some(chan.lock().name.clone());
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        found.expect("answered SIP channel must be registered in the channel store")
    };
    println!("[E2E] SIP channel in store: {}", sip_chan_name);

    // ---- 3. Bridge the two legs (test-side pump; see module docs) --------
    // SIP leg -> externalMedia leg.
    let pump_a = {
        let sip_driver = sip_driver.clone();
        let unicast_driver = unicast_driver.clone();
        let sip_name = sip_chan_name.clone();
        let em_name = em_name.clone();
        tokio::spawn(async move {
            let mut sip_handle = Channel::new(&sip_name);
            let mut em_handle = Channel::new(&em_name);
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(500),
                    sip_driver.read_frame(&mut sip_handle),
                )
                .await
                {
                    Ok(Ok(frame @ Frame::Voice { .. })) => {
                        let _ = unicast_driver.write_frame(&mut em_handle, &frame).await;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => tokio::time::sleep(Duration::from_millis(50)).await,
                    Err(_) => {}
                }
            }
        })
    };
    // externalMedia leg -> SIP leg.
    let pump_b = {
        let sip_driver = sip_driver.clone();
        let unicast_driver = unicast_driver.clone();
        let sip_name = sip_chan_name.clone();
        let em_name = em_name.clone();
        tokio::spawn(async move {
            let mut sip_handle = Channel::new(&sip_name);
            let mut em_handle = Channel::new(&em_name);
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(500),
                    unicast_driver.read_frame(&mut em_handle),
                )
                .await
                {
                    Ok(Ok(frame @ Frame::Voice { .. })) => {
                        let _ = sip_driver.write_frame(&mut sip_handle, &frame).await;
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => tokio::time::sleep(Duration::from_millis(50)).await,
                    Err(_) => {}
                }
            }
        })
    };

    // ---- 4. Caller audio must arrive at the external endpoint as RTP -----
    let caller_payload = [0x7Fu8; 160]; // loud constant PCMU payload
    let caller_ssrc: u32 = 0x0BAD_F00D;
    let mut forked: Vec<(RtpHeader, Vec<u8>)> = Vec::new();
    let mut fork_src: Option<SocketAddr> = None;
    let mut recv_buf = [0u8; 2048];
    let mut seq: u16 = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && forked.len() < 3 {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: seq == 0,
            payload_type: 0, // PCMU
            sequence: seq,
            timestamp: (seq as u32) * 160,
            ssrc: caller_ssrc,
        };
        let packet = build_rtp_packet(&header, &caller_payload);
        let _ = caller_rtp.send_to(&packet[..], sip_media_addr).await;
        seq = seq.wrapping_add(1);

        if let Ok(Ok((len, src))) = tokio::time::timeout(
            Duration::from_millis(150),
            external.recv_from(&mut recv_buf),
        )
        .await
        {
            if let Ok((hdr, payload)) = parse_rtp_header(&recv_buf[..len]) {
                fork_src = Some(src);
                forked.push((hdr, payload.to_vec()));
            }
        }
    }

    assert!(
        !forked.is_empty(),
        "RTP from the SIP leg must be forked to the external endpoint"
    );
    let fork_ssrc = forked[0].0.ssrc;
    for (hdr, payload) in &forked {
        assert_eq!(hdr.payload_type, 0, "ulaw fork must use payload type 0");
        assert_eq!(hdr.ssrc, fork_ssrc, "forked stream must keep one SSRC");
        assert!(
            payload.contains(&0x7F),
            "forked payload must carry the caller's audio"
        );
    }
    assert_ne!(
        fork_ssrc, caller_ssrc,
        "externalMedia channel must stamp a fresh SSRC, not relay the caller's"
    );
    if forked.len() >= 2 {
        let (a, b) = (&forked[0].0, &forked[1].0);
        let seq_delta = b.sequence.wrapping_sub(a.sequence) as u32;
        let ts_delta = b.timestamp.wrapping_sub(a.timestamp);
        assert_eq!(
            ts_delta,
            160 * seq_delta,
            "timestamps must advance 160 samples (20ms of ulaw) per packet"
        );
    }
    let fork_src = fork_src.expect("fork source address");
    println!(
        "[E2E] {} RTP packets forked to external endpoint from {} (PT 0, SSRC {:#010x})",
        forked.len(),
        fork_src,
        fork_ssrc
    );

    // The fork must originate from the advertised UNICASTRTP_LOCAL_PORT.
    let (status, body) = http_request(
        ari_addr,
        "GET",
        "/ari/channels/em-e2e-1/variable?variable=UNICASTRTP_LOCAL_PORT",
    )
    .await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).expect("variable JSON");
    assert_eq!(
        json["value"].as_str().expect("variable value"),
        fork_src.port().to_string(),
        "UNICASTRTP_LOCAL_PORT must match the fork's source port"
    );

    // ---- 5. Bidirectional: external endpoint sends RTP back --------------
    // Reply to the fork's source address (symmetric RTP) and assert the
    // audio reaches the SIP caller through the externalMedia channel.
    let external_payload = [0x55u8; 160];
    let mut returned = 0usize;
    let mut ext_seq: u16 = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && returned < 3 {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: ext_seq == 0,
            payload_type: 0, // PCMU
            sequence: ext_seq,
            timestamp: (ext_seq as u32) * 160,
            ssrc: 0xFEED_FACE,
        };
        let packet = build_rtp_packet(&header, &external_payload);
        let _ = external.send_to(&packet[..], fork_src).await;
        ext_seq = ext_seq.wrapping_add(1);

        if let Ok(Ok((len, _src))) = tokio::time::timeout(
            Duration::from_millis(150),
            caller_rtp.recv_from(&mut recv_buf),
        )
        .await
        {
            if let Ok((hdr, payload)) = parse_rtp_header(&recv_buf[..len]) {
                if hdr.payload_type == 0 && payload.contains(&0x55) {
                    returned += 1;
                }
            }
        }
    }
    assert!(
        returned > 0,
        "RTP injected at the external endpoint must reach the SIP caller (bidirectional)"
    );
    println!(
        "[E2E] {} RTP packets returned from external endpoint to the SIP caller",
        returned
    );

    // ---- 6. Teardown via DELETE: no leaked sockets / driver entries ------
    let (status, _) = http_request(ari_addr, "DELETE", "/ari/channels/em-e2e-1").await;
    assert_eq!(status, 204);
    assert!(
        asterisk_core::channel_store::find_by_uniqueid("em-e2e-1").is_none(),
        "externalMedia channel must leave the store on DELETE"
    );
    assert!(
        !app.channel_ids.read().contains("em-e2e-1"),
        "externalMedia channel must leave the Stasis app on DELETE"
    );
    let mut em_handle = Channel::new(&em_name);
    let frame = Frame::voice(0, 160, Bytes::from_static(&[0x7Fu8; 160]));
    assert!(
        unicast_driver
            .write_frame(&mut em_handle, &frame)
            .await
            .is_err(),
        "externalMedia driver entry (and its RTP socket) must be gone after DELETE"
    );
    println!("[E2E] DELETE /channels/em-e2e-1 -> 204; media plane fully released");

    pump_a.abort();
    pump_b.abort();
}
