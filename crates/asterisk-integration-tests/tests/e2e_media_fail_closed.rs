//! End-to-end acceptance for M6 CP3: `external_media_address` FQDN-vs-literal in
//! SDP, **fail-closed on DNS failure**.
//!
//! `external_media_address` was an uninterpreted string emitted verbatim into
//! `c=`/`o=`. CP3 interprets it: an IP literal is emitted; an FQDN is resolved;
//! and if the FQDN does NOT resolve, rustisk must FAIL CLOSED — reject the call
//! setup rather than advertise an unresolved FQDN or fall back to a leaky
//! internal address.
//!
//! Receiver-side proof: a SIP peer sends an INVITE and observes what rustisk
//! sends back.
//!   * **positive control** — an IP-literal external resolves and the call is
//!     ANSWERED 200 with that address in `c=`.
//!   * **fail-closed** — an unresolvable FQDN external -> rustisk REJECTS with a
//!     4xx and NEVER sends a 200 (so no bogus/internal media address reaches the
//!     peer).
//!
//! RED control (captured in the PR body): make the resolution fall open (emit
//! the internal address instead of failing closed) -> the peer receives a 200
//! for the unresolvable-FQDN case -> the "no 200 / must be rejected" assertion
//! goes RED.

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
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, EndpointConfig, PjsipConfig, TransportConfig,
};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const EXTEN: &str = "100";

/// Wait for a specific SIP status code, skipping provisional 1xx.
async fn recv_sip_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await
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

/// Collect every distinct status code seen within the budget.
async fn collect_status_codes(sock: &UdpSocket, budget: Duration) -> Vec<u16> {
    let deadline = Instant::now() + budget;
    let mut buf = [0u8; 4096];
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await
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

fn invite_request(call_id: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}inv\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag=caller{call_id}\r\n\
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

fn config_with_external_media(external: &str) -> PjsipConfig {
    PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            auth: None,
            ..Default::default()
        }],
        transports: vec![TransportConfig {
            name: "transport-udp".to_string(),
            protocol: "udp".to_string(),
            bind: "127.0.0.1:5060".parse().unwrap(),
            external_media_address: Some(external.to_string()),
            external_signaling_address: None,
            external_signaling_port: None,
            cert_file: None,
            priv_key_file: None,
            // Empty local_net: the loopback caller is treated as EXTERNAL, so
            // the external_media_address path (and CP3 resolution) is exercised.
            local_net: vec![],
        }],
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

async fn send_invite(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    caller: &UdpSocket,
    call_id: &str,
) -> Option<String> {
    let caller_addr = caller.local_addr().unwrap();
    let offer = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);
    let invite = invite_request(call_id, caller_addr.port(), &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).expect("session");
    handler
        .handle_incoming_invite(&invite, caller_addr, session)
        .await
}

#[tokio::test]
async fn external_media_fqdn_fails_closed() {
    register_all_apps();

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

    let caller = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // ---- POSITIVE CONTROL: IP-literal external resolves -> 200 answered ----
    set_global_pjsip_config(config_with_external_media("203.0.113.99"));
    let accepted = send_invite(&handler, sip_local, &caller, "media-fc-ok").await;
    assert_eq!(
        accepted.as_deref(),
        Some("media-fc-ok"),
        "positive control: an IP-literal external_media_address must be accepted"
    );
    let ok = recv_sip_status(&caller, 200, Duration::from_secs(5))
        .await
        .expect("positive control: IP-literal external must be answered 200");
    let answer = SessionDescription::parse(&ok.body).expect("200 must carry SDP");
    assert_eq!(
        answer.connection.as_ref().unwrap().addr,
        "203.0.113.99",
        "positive control: the resolved IP literal is advertised in c="
    );
    println!("[E2E] positive control: IP-literal external_media_address -> 200 with c=203.0.113.99");

    // ---- FAIL-CLOSED: unresolvable FQDN external -> rejected, NO 200 -------
    set_global_pjsip_config(config_with_external_media("no-such-host.invalid"));
    let accepted = send_invite(&handler, sip_local, &caller, "media-fc-fail").await;
    assert_eq!(
        accepted, None,
        "fail-closed: an unresolvable external_media_address FQDN must NOT be accepted"
    );
    let codes = collect_status_codes(&caller, Duration::from_secs(3)).await;
    assert!(
        !codes.contains(&200),
        "fail-closed: rustisk must NEVER send a 200 (with a bogus/internal c=) for an \
         unresolvable external_media_address FQDN; saw {codes:?}"
    );
    assert!(
        codes.iter().any(|c| (400..600).contains(c)),
        "fail-closed: rustisk must reject the INVITE with a 4xx/5xx; saw {codes:?}"
    );
    println!("[E2E] fail-closed: unresolvable external_media_address FQDN -> rejected {codes:?}, no 200");

    set_global_pjsip_config(PjsipConfig::default());
}
