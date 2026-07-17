//! Cross-endpoint credential rejection (M6 review, AUTH MINOR-2).
//!
//! Per-endpoint auth SELECTION (M6 CP4) challenges an inbound request against
//! the credential of the endpoint the SOURCE was matched to (via
//! `type=identify`) — never the union of every configured credential. A
//! consequence that holds by construction today but was previously unguarded:
//! a VALID credential for endpoint X presented from endpoint Y's source IP
//! must be REJECTED, because Y's credential list simply does not contain X's.
//!
//! This is the regression fence for that property. If endpoint selection is
//! ever reverted to the pre-CP4 all-credentials union (verify against every
//! configured credential regardless of the matched endpoint), the
//! cross-endpoint INVITE below authenticates successfully and the test goes
//! RED at the `accepted == None` assertion (captured in the PR body).
//!
//! Receiver-side proofs (assert on the handler outcome + actual response
//! datagrams, never a log line):
//!   (a) control: alpha's credential from ALPHA's source IP is accepted —
//!       proving the digest itself is well-formed, so (b) cannot pass because
//!       of a broken client digest;
//!   (b) alpha's (valid) credential from BETA's source IP is rejected with a
//!       fresh 401 challenge and never accepted.
//!
//! TEST credentials only — nothing here is a real secret.

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
    set_global_pjsip_config, AuthConfig, EndpointConfig, IdentifyConfig, PjsipConfig,
};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const EXTEN: &str = "100";
const ALPHA_USER: &str = "alphauser";
const ALPHA_PASS: &str = "alphapass";
const BETA_USER: &str = "betauser";
const BETA_PASS: &str = "betapass";

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

/// TWO authed endpoints with DISTINCT credentials on DISTINCT source IPs,
/// sharing one transport.
fn two_authed_endpoints_config() -> PjsipConfig {
    PjsipConfig {
        endpoints: vec![
            EndpointConfig {
                name: "alpha".to_string(),
                context: "default".to_string(),
                auth: Some("alpha-auth".to_string()),
                ..Default::default()
            },
            EndpointConfig {
                name: "beta".to_string(),
                context: "default".to_string(),
                auth: Some("beta-auth".to_string()),
                ..Default::default()
            },
        ],
        auths: vec![
            AuthConfig {
                name: "alpha-auth".to_string(),
                auth_type: "userpass".to_string(),
                username: ALPHA_USER.to_string(),
                password: ALPHA_PASS.to_string(),
                ..Default::default()
            },
            AuthConfig {
                name: "beta-auth".to_string(),
                auth_type: "userpass".to_string(),
                username: BETA_USER.to_string(),
                password: BETA_PASS.to_string(),
                ..Default::default()
            },
        ],
        identifies: vec![
            IdentifyConfig {
                name: "id-alpha".to_string(),
                endpoint: "alpha".to_string(),
                matches: vec!["127.0.0.6/32".to_string()],
                match_header: None,
            },
            IdentifyConfig {
                name: "id-beta".to_string(),
                endpoint: "beta".to_string(),
                matches: vec!["127.0.0.7/32".to_string()],
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

/// Obtain a digest challenge by sending a credential-less INVITE from `sock`,
/// asserting the handler rejects it with a 401.
async fn obtain_challenge(
    handler: &Arc<SipEventHandler>,
    sock: &UdpSocket,
    sip_local: SocketAddr,
    call_id: &str,
    offer: &SessionDescription,
) -> DigestChallenge {
    let addr = sock.local_addr().unwrap();
    let inv = invite_request(call_id, addr.port(), &offer.to_string(), None);
    let session = SipSession::new_inbound(&inv, sip_local, addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, addr, session).await;
    assert_eq!(
        accepted, None,
        "an authed endpoint must not be accepted without credentials"
    );
    let challenge_resp = recv_status(sock, 401, Duration::from_secs(2))
        .await
        .expect("authed endpoint must be challenged with a 401 datagram");
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("401 must carry WWW-Authenticate");
    DigestChallenge::parse(www).expect("challenge must parse")
}

#[tokio::test]
async fn valid_credential_from_wrong_endpoint_ip_is_rejected() {
    register_all_apps();
    set_global_pjsip_config(two_authed_endpoints_config());

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

    // ---- (a) CONTROL: alpha's credential from ALPHA's IP -> accepted ------
    // Load-bearing: proves the digest we build is valid, so scenario (b)
    // cannot "pass" merely because the client digest is malformed.
    let alpha_sock = UdpSocket::bind("127.0.0.6:0").await.unwrap();
    let alpha_addr = alpha_sock.local_addr().unwrap();
    let challenge =
        obtain_challenge(&handler, &alpha_sock, sip_local, "xep-alpha-ctl", &offer).await;
    let alpha_creds = DigestCredentials {
        username: ALPHA_USER.to_string(),
        password: ALPHA_PASS.to_string(),
        realm: challenge.realm.clone(),
    };
    let auth = create_digest_response(
        &challenge,
        &alpha_creds,
        "INVITE",
        &format!("sip:{EXTEN}@127.0.0.1"),
    );
    let inv = invite_request(
        "xep-alpha-ok",
        alpha_addr.port(),
        &offer.to_string(),
        Some(&auth),
    );
    let session = SipSession::new_inbound(&inv, sip_local, alpha_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, alpha_addr, session).await;
    assert_eq!(
        accepted.as_deref(),
        Some("xep-alpha-ok"),
        "control: alpha's credential from alpha's own IP must be accepted"
    );
    println!("[E2E] (a) control: alpha cred from alpha IP (127.0.0.6) accepted");

    // ---- (b) alpha's VALID credential from BETA's IP -> rejected ----------
    // Per-endpoint selection: the source matches endpoint `beta`, whose
    // credential list is [betauser] only — alphauser must not be in it.
    // RED control (captured in the PR body): revert selection to the
    // all-credentials union and this INVITE authenticates -> `accepted` is
    // Some(..) -> this test FAILS.
    let beta_sock = UdpSocket::bind("127.0.0.7:0").await.unwrap();
    let beta_addr = beta_sock.local_addr().unwrap();
    let challenge =
        obtain_challenge(&handler, &beta_sock, sip_local, "xep-cross-chal", &offer).await;
    let stolen_alpha_creds = DigestCredentials {
        username: ALPHA_USER.to_string(),
        password: ALPHA_PASS.to_string(),
        realm: challenge.realm.clone(),
    };
    let auth = create_digest_response(
        &challenge,
        &stolen_alpha_creds,
        "INVITE",
        &format!("sip:{EXTEN}@127.0.0.1"),
    );
    let inv = invite_request(
        "xep-cross",
        beta_addr.port(),
        &offer.to_string(),
        Some(&auth),
    );
    let session = SipSession::new_inbound(&inv, sip_local, beta_addr).expect("session");
    let accepted = handler.handle_incoming_invite(&inv, beta_addr, session).await;
    assert_eq!(
        accepted, None,
        "a valid credential for endpoint alpha presented from endpoint beta's \
         source IP must be REJECTED (per-endpoint selection, not a credential union)"
    );
    let codes = collect_status_codes(&beta_sock, Duration::from_secs(2)).await;
    assert!(
        codes.contains(&401),
        "the cross-endpoint attempt must be re-challenged (401); saw {codes:?}"
    );
    assert!(
        !codes.contains(&200),
        "the cross-endpoint attempt must never see a 200; saw {codes:?}"
    );
    println!("[E2E] (b) alpha cred from beta IP (127.0.0.7) rejected with 401, never accepted");

    set_global_pjsip_config(PjsipConfig::default());
}
