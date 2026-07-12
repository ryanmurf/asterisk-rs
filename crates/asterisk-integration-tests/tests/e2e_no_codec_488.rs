//! Regression test: an inbound INVITE whose SDP offer shares no codec with our
//! supported set must be rejected with `488 Not Acceptable Here` — not answered
//! with a `200 OK` whose media is entirely rejected (which would bring the call
//! "up" with guaranteed silence and a leaked RTP socket). See FINDINGS.md F27.
//!
//! Lives in its own integration-test binary because it touches process-global
//! state (`set_global_pjsip_config`, the tech registry).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use asterisk_apps::adapter::register_all_apps;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::auth::{create_digest_response, DigestChallenge, DigestCredentials};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{header_names, SipMessage};
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, AuthConfig, EndpointConfig, PjsipConfig,
};
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const USER: &str = "100";
const PASS: &str = "1234";
const EXTEN: &str = "100";

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

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
async fn e2e_inbound_invite_no_common_codec_gets_488() {
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

    // Dialplan: extension 100 exists (Answer, Echo) — so a 488 can only come
    // from codec negotiation, never from a missing extension.
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
    let sip_local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(driver.clone());

    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());

    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();

    // Authenticate (REGISTER 401 -> obtain challenge), reused for the INVITE.
    let reg1 = register_request("reg-1", 1, None);
    handler.handle_register(&reg1, caller_sip_addr).await;
    let challenge_resp = recv_sip(&caller_sip, Duration::from_secs(2))
        .await
        .expect("401 challenge");
    let www = challenge_resp
        .get_header(header_names::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate");
    let challenge = DigestChallenge::parse(www).expect("challenge parses");
    let creds = DigestCredentials {
        username: USER.to_string(),
        password: PASS.to_string(),
        realm: challenge.realm.clone(),
    };

    // Offer only G729 (PT 18) — not in our supported set (PCMU/PCMA/DTMF/video).
    let offer = "v=0\r\n\
                 o=caller 0 0 IN IP4 127.0.0.1\r\n\
                 s=-\r\n\
                 c=IN IP4 127.0.0.1\r\n\
                 t=0 0\r\n\
                 m=audio 40000 RTP/AVP 18\r\n\
                 a=rtpmap:18 G729/8000\r\n";
    let inv_auth =
        create_digest_response(&challenge, &creds, "INVITE", &format!("sip:{EXTEN}@127.0.0.1"));
    let invite = invite_request("inv-no-codec", offer, &inv_auth);
    let session =
        SipSession::new_inbound(&invite, caller_sip_addr, caller_sip_addr).expect("session");

    let call_id = handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    assert!(
        call_id.is_none(),
        "an INVITE with no common codec must be rejected, not accepted"
    );

    let resp = recv_sip(&caller_sip, Duration::from_secs(2))
        .await
        .expect("expected a final response to the un-answerable INVITE");
    assert_eq!(
        resp.status_code(),
        Some(488),
        "no-common-codec offer must be answered 488 Not Acceptable Here, got {:?}",
        resp.status_code()
    );
}
