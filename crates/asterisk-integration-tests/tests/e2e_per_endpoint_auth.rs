//! End-to-end acceptance for M6 CP4: per-endpoint auth SELECTION.
//!
//! rustisk used to challenge an inbound request against the UNION of every
//! configured credential — so if ANY endpoint had auth, EVERY request was
//! challenged. That breaks the load-bearing topology where an UNAUTHENTICATED
//! trunk (a carrier, matched by source IP, no digest) and an AUTHENTICATED
//! bridge (matched by source IP, digest required) share one transport.
//!
//! CP4 matches the inbound request to an endpoint (via `type=identify`) and
//! issues a digest challenge ONLY if the MATCHED endpoint has an auth section.
//!
//! Receiver-side proofs (assert on the actual response datagram + accept/reject
//! outcome, never a log line):
//!   (a) a request matched to the unauth trunk endpoint is NOT challenged and is
//!       accepted (no 401 datagram; handler accepts it);
//!   (b) a request matched to the authed bridge endpoint IS challenged (a 401
//!       datagram) and is rejected without valid creds, and accepted with them.
//!
//! RED control (captured in the PR body): revert to the all-credentials union
//! (challenge whenever any endpoint has auth) -> the unauth trunk is challenged
//! with a 401 -> scenario (a) goes RED.

use std::net::SocketAddr;
use std::time::{Duration, Instant};
use std::sync::Arc;

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::auth::{create_digest_response, DigestChallenge, DigestCredentials};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{header_names, SipMessage};
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, AuthConfig, EndpointConfig, IdentifyConfig, PjsipConfig,
};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const EXTEN: &str = "100";
const BRIDGE_USER: &str = "bridgeuser";
const BRIDGE_PASS: &str = "bridgepass";

/// Collect every distinct status code seen within the budget.
async fn collect_status_codes(sock: &UdpSocket, budget: Duration) -> Vec<u16> {
    let deadline = Instant::now() + budget;
    let mut buf = [0u8; 4096];
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(250), sock.recv_from(&mut buf)).await
        {
            if let Ok(msg) = SipMessage::parse(&buf[..len]) {
                if let Some(code) = msg.status_code() {
                    if !seen.contains(&code) {
                        seen.push(code);
                    }
                }
            }
        }
    }
    seen
}

/// Wait for a specific status code (skipping others).
async fn recv_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(250), sock.recv_from(&mut buf)).await
        {
            if let Ok(msg) = SipMessage::parse(&buf[..len]) {
                if msg.status_code() == Some(status) {
                    return Some(msg);
                }
            }
        }
    }
    None
}

fn invite_request(call_id: &str, contact_port: u16, sdp: &str, auth: Option<&str>) -> SipMessage {
    let auth_line = auth
        .map(|a| format!("Authorization: {a}\r\n"))
        .unwrap_or_default();
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}inv\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller{call_id}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:{contact_port}>\r\n\
         {auth_line}\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// A trunk (no auth) and a bridge (digest auth) identified by distinct source
/// IPs on one transport.
fn coexist_config() -> PjsipConfig {
    PjsipConfig {
        endpoints: vec![
            EndpointConfig {
                name: "trunk".to_string(),
                context: "default".to_string(),
                auth: None,
                ..Default::default()
            },
            EndpointConfig {
                name: "bridge".to_string(),
                context: "default".to_string(),
                auth: Some("bridgeauth".to_string()),
                ..Default::default()
            },
        ],
        auths: vec![AuthConfig {
            name: "bridgeauth".to_string(),
            auth_type: "userpass".to_string(),
            username: BRIDGE_USER.to_string(),
            password: BRIDGE_PASS.to_string(),
            ..Default::default()
        }],
        identifies: vec![
            IdentifyConfig {
                name: "id-trunk".to_string(),
                endpoint: "trunk".to_string(),
                matches: vec!["127.0.0.2/32".to_string()],
                match_header: None,
            },
            IdentifyConfig {
                name: "id-bridge".to_string(),
                endpoint: "bridge".to_string(),
                matches: vec!["127.0.0.3/32".to_string()],
                match_header: None,
            },
        ],
        ..Default::default()
    }
}

fn dialplan() -> Dialplan {
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
    dp
}

#[tokio::test]
async fn unauth_trunk_and_authed_bridge_coexist() {
    register_all_apps();
    set_global_pjsip_config(coexist_config());

    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap(),
    );
    let sip_local: SocketAddr = handler_transport.local_addr().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dialplan()), handler_transport));
    handler.set_channel_driver(driver.clone());

    let offer = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);

    // ---- (a) trunk (source 127.0.0.2, no auth) -> NOT challenged ----------
    let trunk_sock = UdpSocket::bind("127.0.0.2:0").await.unwrap();
    let trunk_addr = trunk_sock.local_addr().unwrap();
    let inv = invite_request("cp4-trunk", trunk_addr.port(), &offer.to_string(), None);
    let session = SipSession::new_inbound(&inv, sip_local, trunk_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, trunk_addr, session).await;
    assert_eq!(
        accepted.as_deref(),
        Some("cp4-trunk"),
        "the unauth trunk endpoint must be accepted without a challenge"
    );
    let codes = collect_status_codes(&trunk_sock, Duration::from_secs(2)).await;
    assert!(
        !codes.contains(&401) && !codes.contains(&407),
        "the unauth trunk endpoint must NOT be challenged; saw {codes:?}"
    );
    println!("[E2E] (a) unauth trunk (127.0.0.2) accepted, NOT challenged; responses={codes:?}");

    // ---- (b) bridge (source 127.0.0.3, auth) WITHOUT creds -> 401 ---------
    let bridge_sock = UdpSocket::bind("127.0.0.3:0").await.unwrap();
    let bridge_addr = bridge_sock.local_addr().unwrap();
    let inv = invite_request("cp4-bridge-noauth", bridge_addr.port(), &offer.to_string(), None);
    let session = SipSession::new_inbound(&inv, sip_local, bridge_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, bridge_addr, session).await;
    assert_eq!(
        accepted, None,
        "the authed bridge endpoint must NOT be accepted without valid creds"
    );
    let challenge_resp = recv_status(&bridge_sock, 401, Duration::from_secs(2))
        .await
        .expect("the authed bridge endpoint must be challenged with a 401 datagram");
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("401 must carry WWW-Authenticate");
    let challenge = DigestChallenge::parse(www).expect("challenge must parse");
    println!("[E2E] (b) authed bridge (127.0.0.3) without creds -> 401 challenge");

    // ---- (c) bridge WITH valid creds -> accepted --------------------------
    let creds = DigestCredentials {
        username: BRIDGE_USER.to_string(),
        password: BRIDGE_PASS.to_string(),
        realm: challenge.realm.clone(),
    };
    let auth = create_digest_response(&challenge, &creds, "INVITE", &format!("sip:{EXTEN}@127.0.0.1"));
    let inv = invite_request(
        "cp4-bridge-auth",
        bridge_addr.port(),
        &offer.to_string(),
        Some(&auth),
    );
    let session = SipSession::new_inbound(&inv, sip_local, bridge_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, bridge_addr, session).await;
    assert_eq!(
        accepted.as_deref(),
        Some("cp4-bridge-auth"),
        "the authed bridge endpoint must be accepted WITH valid digest creds"
    );
    let codes = collect_status_codes(&bridge_sock, Duration::from_secs(2)).await;
    assert!(
        !codes.contains(&401) && !codes.contains(&407),
        "a valid-digest bridge request must NOT be re-challenged; saw {codes:?}"
    );
    println!("[E2E] (c) authed bridge WITH valid creds -> accepted, not re-challenged");

    set_global_pjsip_config(PjsipConfig::default());
}
