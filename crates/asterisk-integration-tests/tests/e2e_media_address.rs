//! End-to-end regression for issue #56: the SDP answer must advertise a
//! concrete, routable connection address — never `c=IN IP4 0.0.0.0` — and
//! the parsed-but-unused `external_media_address` transport option must
//! actually be applied (with `local_net` peers exempted).
//!
//! Before the fix, a stack bound to INADDR_ANY advertised the bind address
//! verbatim: peers that honor the c-line (no symmetric RTP) sent audio to
//! 0.0.0.0 and got silence.
//!
//! Own integration-test binary so its use of process-global state (pjsip
//! config, app registry) is isolated from the other e2e tests. The three
//! scenarios run sequentially inside one test for the same reason.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::SipMessage;
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, EndpointConfig, PjsipConfig, TransportConfig,
};
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

/// Receive SIP datagrams until a response with `status` arrives.
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

fn invite_request(call_id: &str, branch: &str, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch={branch}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=c56\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
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

fn endpoint_only_config(external: Option<&str>, local_net: Vec<String>) -> PjsipConfig {
    let mut cfg = PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    if external.is_some() || !local_net.is_empty() {
        cfg.transports.push(TransportConfig {
            name: "transport-udp".to_string(),
            protocol: "udp".to_string(),
            bind: "0.0.0.0:5060".parse().unwrap(),
            external_media_address: external.map(|s| s.to_string()),
            external_signaling_address: None,
            external_signaling_port: None,
            cert_file: None,
            priv_key_file: None,
            local_net,
        });
    }
    cfg
}

/// Run one INVITE → 200 OK exchange and return the answer's session-level
/// connection address.
async fn answer_connection_addr(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    caller: &UdpSocket,
    call_id: &str,
    branch: &str,
    offer_media_addr: &str,
) -> String {
    let caller_addr = caller.local_addr().unwrap();
    let offer = SessionDescription::create_offer(offer_media_addr, 40000, &[codecs::pcmu()]);
    let invite = invite_request(call_id, branch, &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).expect("session");

    let accepted = handler
        .handle_incoming_invite(&invite, caller_addr, session)
        .await;
    assert_eq!(accepted.as_deref(), Some(call_id), "INVITE must be accepted");

    let ok = recv_sip_status(caller, 200, Duration::from_secs(5))
        .await
        .expect("expected 200 OK with SDP answer");
    let answer = SessionDescription::parse(&ok.body).expect("answer SDP must parse");
    answer
        .connection
        .as_ref()
        .expect("answer must carry a c= line")
        .addr
        .clone()
}

#[tokio::test]
async fn sdp_answer_never_advertises_inaddr_any() {
    register_all_apps();

    // Dialplan: 100 -> Answer (the 200 OK carries the SDP answer under test).
    let mut dp = Dialplan::new();
    let mut ctx = Context::new("default");
    let mut ext = Extension::new(EXTEN);
    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext);
    dp.add_context(ctx);

    // The handler's transport is bound to INADDR_ANY — the exact setup that
    // used to leak `c=IN IP4 0.0.0.0` into answers.
    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("0.0.0.0:0".parse().unwrap()).await.unwrap(),
    );
    let sip_local: SocketAddr = handler_transport.local_addr().unwrap();
    assert!(sip_local.ip().is_unspecified(), "test premise: bound to 0.0.0.0");
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());

    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // ---- 1. No external_media_address: the routed interface is used ------
    set_global_pjsip_config(endpoint_only_config(None, vec![]));
    let addr =
        answer_connection_addr(&handler, sip_local, &caller, "media-56-1", "z9hG4bK561", "127.0.0.1")
            .await;
    assert_ne!(addr, "0.0.0.0", "answer must never advertise INADDR_ANY (issue #56)");
    assert_eq!(addr, "127.0.0.1", "loopback caller must be answered via loopback");
    println!("[E2E] INADDR_ANY bind, no external_media_address -> c={addr}");

    // ---- 2. external_media_address applies to a non-local peer -----------
    set_global_pjsip_config(endpoint_only_config(Some("203.0.113.99"), vec![]));
    let addr =
        answer_connection_addr(&handler, sip_local, &caller, "media-56-2", "z9hG4bK562", "127.0.0.1")
            .await;
    assert_eq!(
        addr, "203.0.113.99",
        "configured external_media_address must be applied to the answer (issue #56)"
    );
    println!("[E2E] external_media_address configured -> c={addr}");

    // ---- 3. a local_net peer bypasses the external address ---------------
    set_global_pjsip_config(endpoint_only_config(
        Some("203.0.113.99"),
        vec!["127.0.0.0/8".to_string()],
    ));
    let addr =
        answer_connection_addr(&handler, sip_local, &caller, "media-56-3", "z9hG4bK563", "127.0.0.1")
            .await;
    assert_eq!(
        addr, "127.0.0.1",
        "a peer inside local_net must get the real local address, not the external one"
    );
    println!("[E2E] local_net peer -> c={addr}");

    // ---- 4. NAT decisions target the MEDIA peer, not the SIP source ------
    // Signaling still arrives from loopback (inside local_net), but the
    // offer's c-line — where RTP will actually flow — is non-local. The
    // external address must be applied (review finding: routing toward the
    // signaling source exempted this case wrongly).
    let addr = answer_connection_addr(
        &handler, sip_local, &caller, "media-56-4", "z9hG4bK564", "198.51.100.7",
    )
    .await;
    assert_eq!(
        addr, "203.0.113.99",
        "NAT selection must follow the offer's media endpoint, not the signaling source"
    );
    println!("[E2E] non-local media peer behind local signaling -> c={addr}");
}
