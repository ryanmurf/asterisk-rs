//! M6 CP5 (B4) — dynamic authenticated REGISTER + dynamic-contact routing that
//! FOLLOWS a bridge IP change, proven RECEIVER-SIDE and in-process.
//!
//! The sip-bridge advertises an EPHEMERAL address and REGISTERs (with digest);
//! a static contact is wrong because a pod restart gives it a NEW IP. This test
//! proves rustisk's registrar side of B4 with real UDP sockets:
//!   1. the bridge digest-REGISTERs from address A (401 -> digest -> 200 bound);
//!   2. an outbound Dial to the bridge endpoint routes to A — the INVITE
//!      datagram is captured on A's socket (receiver-side);
//!   3. the bridge "restarts": a NEW socket B re-REGISTERs (digest) from a new
//!      address;
//!   4. a subsequent outbound Dial routes to B — the INVITE lands on B and NOT
//!      on the stale A (receiver-side).
//!
//! This is the RUSTISK-SIDE mechanism proof. The full isolated-Docker
//! container-restart harness (bridge container restarted with a new pod IP) is
//! CP5's separate acceptance and is run out-of-band; the rustisk code exercised
//! here (`resolve_endpoint_contact` preferring the live registration, issue #33,
//! and `Registrar::best_contact` returning the most-recently-registered
//! contact) is exactly what makes that harness pass.
//!
//! RED control (captured in the PR body): make `best_contact` return the OLDEST
//! binding (or pin the contact) -> after the re-REGISTER the second INVITE still
//! goes to the stale A -> the "lands on B, not A" assertion goes RED.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::channel::ChannelDriver;
use asterisk_core::pbx::{Context, Dialplan};
use asterisk_sip::auth::{create_digest_response, DigestChallenge, DigestCredentials};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{header_names, SipMessage, SipMethod, StartLine};
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, AuthConfig, EndpointConfig, PjsipConfig, TransportConfig,
};
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const AOR: &str = "bridge";
const BRIDGE_USER: &str = "bridge";
const BRIDGE_PASS: &str = "bridgepass";

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

async fn recv_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
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

/// Wait for an inbound INVITE *request* datagram on this socket.
async fn recv_invite(sock: &UdpSocket, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Invite) {
                if let StartLine::Request(_) = &msg.start_line {
                    return true;
                }
            }
        }
    }
    false
}

/// A REGISTER for AoR `bridge` whose Contact is `contact_addr`, optionally
/// carrying an Authorization header. Sent from `contact_addr` (the bridge's
/// current ephemeral address).
fn register_request(call_id: &str, cseq: u32, contact_addr: SocketAddr, auth: Option<&str>) -> SipMessage {
    let auth_line = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    let raw = format!(
        "REGISTER sip:127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP {contact_addr};branch=z9hG4bK{call_id}{cseq}\r\n\
         From: <sip:{AOR}@127.0.0.1>;tag=reg{call_id}\r\n\
         To: <sip:{AOR}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} REGISTER\r\n\
         Contact: <sip:{AOR}@{contact_addr}>\r\n\
         {auth_line}\
         Expires: 3600\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Perform a full digest REGISTER round-trip from `bridge_sock` (401 -> digest
/// -> 200) and return once the contact is bound. Panics on failure.
async fn digest_register(
    handler: &Arc<SipEventHandler>,
    bridge_sock: &UdpSocket,
    bridge_addr: SocketAddr,
    call_id: &str,
) {
    // 1. REGISTER without auth -> 401 challenge.
    let reg1 = register_request(call_id, 1, bridge_addr, None);
    handler.handle_register(&reg1, bridge_addr).await;
    let challenge_resp = recv_status(bridge_sock, 401, Duration::from_secs(2))
        .await
        .expect("unauthenticated REGISTER must be challenged 401");
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("401 must carry WWW-Authenticate");
    let challenge = DigestChallenge::parse(www).expect("challenge must parse");

    // 2. REGISTER with a valid digest -> 200 bound.
    let creds = DigestCredentials {
        username: BRIDGE_USER.to_string(),
        password: BRIDGE_PASS.to_string(),
        realm: challenge.realm.clone(),
    };
    let auth = create_digest_response(&challenge, &creds, "REGISTER", "sip:127.0.0.1");
    let reg2 = register_request(call_id, 2, bridge_addr, Some(&auth));
    handler.handle_register(&reg2, bridge_addr).await;
    let ok = recv_status(bridge_sock, 200, Duration::from_secs(2))
        .await
        .expect("authenticated REGISTER must be answered 200");
    assert_eq!(ok.status_code(), Some(200));
}

/// Drive an outbound Dial to the `bridge` endpoint and return true if the INVITE
/// datagram lands on `expect_sock` within the budget (racing it against the
/// `other_sock` to detect mis-routing to the stale address).
async fn dial_lands_on(
    driver: &Arc<SipChannelDriver>,
    expect_sock: &UdpSocket,
    other_sock: &UdpSocket,
) -> (bool, bool) {
    // request() resolves the bridge's current registered contact and allocates
    // the outbound leg; call() sends the INVITE to it (direct transport send,
    // no stack configured).
    let mut channel = driver
        .request(AOR, None)
        .await
        .expect("request(bridge) must resolve the registered contact");
    driver
        .call(&mut channel, AOR, 5)
        .await
        .expect("call() must send the INVITE to the resolved contact");

    let budget = Duration::from_secs(2);
    tokio::join!(recv_invite(expect_sock, budget), recv_invite(other_sock, budget))
}

#[tokio::test]
async fn re_register_from_new_ip_reroutes_the_call() {
    register_all_apps();

    // Endpoint "bridge": digest auth, AoR "bridge" (bound dynamically, no static
    // contact). No type=identify -> source ACL passes (dynamic pod IP).
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: AOR.to_string(),
            context: "default".to_string(),
            aors: Some(AOR.to_string()),
            auth: Some("bridgeauth".to_string()),
            ..Default::default()
        }],
        auths: vec![AuthConfig {
            name: "bridgeauth".to_string(),
            auth_type: "userpass".to_string(),
            username: BRIDGE_USER.to_string(),
            password: BRIDGE_PASS.to_string(),
            ..Default::default()
        }],
        transports: vec![TransportConfig {
            name: "transport-udp".to_string(),
            protocol: "udp".to_string(),
            bind: "127.0.0.1:5060".parse().unwrap(),
            external_media_address: None,
            external_signaling_address: None,
            external_signaling_port: None,
            cert_file: None,
            priv_key_file: None,
            local_net: vec![],
        }],
        ..Default::default()
    });

    let mut dp = Dialplan::new();
    dp.add_context(Context::new("default"));

    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap(),
    );
    let sip_local: SocketAddr = handler_transport.local_addr().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());
    // The driver resolves the bridge's live contact via the shared registrar.
    driver.set_registrar(handler.registrar());

    // ---- Bridge address A: digest-REGISTER, then a Dial must land on A ------
    let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_a = sock_a.local_addr().unwrap();
    digest_register(&handler, &sock_a, addr_a, "reg-a").await;
    assert_eq!(
        handler.registrar().best_contact(AOR),
        Some(format!("sip:{AOR}@{addr_a}")),
        "the bridge's contact must be bound to address A"
    );

    // Second live socket B, bound now so both are live for the race.
    let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr_b = sock_b.local_addr().unwrap();
    assert_ne!(addr_a, addr_b);

    let (on_a, on_b) = dial_lands_on(&driver, &sock_a, &sock_b).await;
    assert!(on_a, "before restart: the outbound INVITE must land on the registered address A");
    assert!(!on_b, "before restart: nothing must reach the not-yet-registered address B");
    println!("[E2E] bridge registered at A={addr_a}: outbound INVITE routed to A (receiver-side)");

    // ---- Bridge "restarts" with a NEW IP: re-REGISTER from B ----------------
    digest_register(&handler, &sock_b, addr_b, "reg-b").await;
    assert_eq!(
        handler.registrar().best_contact(AOR),
        Some(format!("sip:{AOR}@{addr_b}")),
        "after the re-REGISTER the live contact must move to the new address B"
    );

    // A subsequent Dial must FOLLOW to B (receiver-side), not the stale A.
    let (on_b2, on_a2) = dial_lands_on(&driver, &sock_b, &sock_a).await;
    assert!(
        on_b2,
        "after restart: the outbound INVITE must FOLLOW to the re-REGISTERed address B"
    );
    assert!(
        !on_a2,
        "after restart: the outbound INVITE must NOT go to the stale address A"
    );
    println!("[E2E] bridge re-registered at B={addr_b}: outbound INVITE followed to B, not stale A");

    set_global_pjsip_config(PjsipConfig::default());
}
