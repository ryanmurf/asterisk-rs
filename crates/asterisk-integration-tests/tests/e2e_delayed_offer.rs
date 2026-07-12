//! End-to-end regression for issue #30: a delayed-offer INVITE (no SDP body)
//! must get a clean 488 Not Acceptable Here, not a 200 OK with no SDP answer
//! (which brought the call up with no media and discarded the ACK's SDP).
//!
//! Own integration-test binary so its use of the process-global pjsip config
//! is isolated from the other e2e tests.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::SipMessage;
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
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

/// INVITE with no SDP body and no auth (endpoint has no credentials).
fn delayed_offer_invite(call_id: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bKdo1\r\n\
         From: \"Trunk\" <sip:gw@127.0.0.1>;tag=trunk\r\n\
         To: <sip:{EXTEN}@127.0.0.1>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:gw@127.0.0.1:5062>\r\n\
         Content-Length: 0\r\n\r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

#[tokio::test]
async fn delayed_offer_invite_is_rejected_with_488() {
    // Endpoint "100" in context "default", no auth configured (so the auth
    // step is skipped and we reach the offer check).
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    });

    // Dialplan: extension 100 exists (so the reject is the no-SDP 488, not a
    // 404 for an unknown extension).
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

    let handler_transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap(),
    );
    let sip_local: SocketAddr = handler_transport.local_addr().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());

    // A caller socket that captures the handler's response.
    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_addr = caller.local_addr().unwrap();

    let invite = delayed_offer_invite("delayed-offer-1");
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).expect("session");
    // Sanity: this really is a no-SDP (delayed) offer.
    assert!(session.remote_sdp.is_none(), "test INVITE must carry no SDP");

    let result = handler
        .handle_incoming_invite(&invite, caller_addr, session)
        .await;

    // No call is set up.
    assert_eq!(result, None, "delayed-offer INVITE must not create a call");
    assert_eq!(driver.active_channel_count(), 0, "no RTP socket must be bound");

    // The response is a 488, and it carries no SDP body.
    let resp = recv_sip(&caller, Duration::from_secs(2))
        .await
        .expect("expected a final response to the delayed-offer INVITE");
    assert_eq!(
        resp.status_code(),
        Some(488),
        "delayed-offer INVITE must be rejected with 488 Not Acceptable Here"
    );
    assert!(resp.body.is_empty(), "488 must not carry an SDP body");
}
