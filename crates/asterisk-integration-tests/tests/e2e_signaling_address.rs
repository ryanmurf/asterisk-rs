//! End-to-end acceptance for M6 CP2: `external_signaling_address` +
//! `external_signaling_port` (New-3) applied to the Via/Contact/From builders,
//! transport-scoped by `local_net` exactly like `advertised_media_ip` is for
//! SDP.
//!
//! Receiver-side proof: a SIP peer establishes a call and CAPTURES real
//! datagrams from rustisk. It inspects:
//!   * the INVITE 200 OK **Contact** (built by `SipSession::build_200_ok`), and
//!   * the rtptimeout **BYE**'s **Via sent-by AND From URI** (built by
//!     `SipSession::build_bye`),
//! asserting they carry the EXTERNAL address AND the EXTERNAL port for a peer
//! outside `local_net`, and the INTERNAL bind address/port for a peer inside
//! `local_net`. The BYE is still physically delivered to the peer's real
//! transport address, so the advertised (external) address is proven at the
//! receiver, independent of where the datagram was sent.
//!
//! RED control (captured in the PR body): defeat the scoping —
//! `SipSession::signaling_hostport` always returns the internal bind — and the
//! external peer sees the internal address/port -> every EXTERNAL assertion
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
use asterisk_sip::parser::{SipMessage, SipMethod, SipUri, StartLine};
use asterisk_sip::pjsip_config::{
    set_global_pjsip_config, EndpointConfig, PjsipConfig, TransportConfig,
};
use asterisk_sip::sdp::SessionDescription;
use asterisk_sip::session::SipSession;
use asterisk_sip::transport::UdpTransport;
use tokio::net::UdpSocket;

const EXTEN: &str = "100";
const EXT_ADDR: &str = "203.0.113.99";
const EXT_PORT: u16 = 6666;

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<SipMessage> {
    let mut buf = [0u8; 4096];
    let (len, _src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    SipMessage::parse(&buf[..len]).ok()
}

async fn recv_sip_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
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

async fn recv_bye(sock: &UdpSocket, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Bye) {
                if let StartLine::Request(_) = &msg.start_line {
                    return Some(msg);
                }
            }
        }
    }
    None
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

/// (host, port) from a `<sip:user@host:port>`-style header value.
fn uri_hostport(value: &str) -> (String, Option<u16>) {
    let uri = asterisk_sip::parser::extract_uri(value)
        .and_then(|u| SipUri::parse(&u).ok())
        .expect("header must carry a parseable URI");
    (uri.host, uri.port)
}

/// (host, port) from a `SIP/2.0/UDP host:port;branch=...` Via value.
fn via_hostport(value: &str) -> (String, Option<u16>) {
    let sent_by = value
        .split_whitespace()
        .nth(1)
        .expect("Via must have a sent-by")
        .split(';')
        .next()
        .unwrap();
    match sent_by.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
        None => (sent_by.to_string(), None),
    }
}

fn transport_config(local_net: Vec<String>) -> PjsipConfig {
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
            // Concrete loopback bind; the handler's 127.0.0.1 ephemeral bind is
            // matched by the exact-ip fallback in the config lookup.
            bind: "127.0.0.1:5060".parse().unwrap(),
            external_media_address: None,
            external_signaling_address: Some(EXT_ADDR.to_string()),
            external_signaling_port: Some(EXT_PORT),
            cert_file: None,
            priv_key_file: None,
            local_net,
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
    ext.add_priority(Priority {
        priority: 2,
        app: "Echo".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx.add_extension(ext);
    dp.add_context(ctx);
    dp
}

/// Establish a media-silent inbound call, capture its 200 OK and the subsequent
/// rtptimeout BYE. Returns (ok, bye).
async fn call_and_reap(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    caller_sip: &UdpSocket,
    caller_addr: SocketAddr,
    call_id: &str,
) -> (SipMessage, SipMessage) {
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );
    let invite = invite_request(call_id, caller_addr.port(), &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, caller_addr).expect("session");
    handler
        .handle_incoming_invite(&invite, caller_addr, session)
        .await;
    let ok = recv_sip_status(caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    std::mem::forget(caller_rtp);
    let bye = recv_bye(caller_sip, Duration::from_secs(6))
        .await
        .expect("rtptimeout must reap the silent call and send a BYE");
    (ok, bye)
}

#[tokio::test]
async fn external_signaling_address_and_port_scoped_by_local_net() {
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
    handler.set_rtp_timeout(Some(Duration::from_secs(2)));

    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_addr = caller_sip.local_addr().unwrap();

    // ---- EXTERNAL: caller outside local_net -> external addr + port --------
    set_global_pjsip_config(transport_config(vec![]));
    let (ok, bye) = call_and_reap(&handler, sip_local, &caller_sip, caller_addr, "sig-ext").await;

    let (c_host, c_port) = uri_hostport(ok.get_header("Contact").expect("200 OK Contact"));
    assert_eq!(c_host, EXT_ADDR, "external peer must see the external signaling ADDRESS in Contact");
    assert_eq!(c_port, Some(EXT_PORT), "external peer must see the external signaling PORT in Contact (New-3)");

    let (v_host, v_port) = via_hostport(bye.get_header("Via").expect("BYE Via"));
    assert_eq!(v_host, EXT_ADDR, "external peer must see the external address in the BYE Via sent-by");
    assert_eq!(v_port, Some(EXT_PORT), "external peer must see the external port in the BYE Via sent-by");

    let (f_host, f_port) = uri_hostport(bye.get_header("From").expect("BYE From"));
    assert_eq!(f_host, EXT_ADDR, "external peer must see the external address in the BYE From URI");
    assert_eq!(f_port, Some(EXT_PORT), "external peer must see the external port in the BYE From URI");
    println!("[E2E] external peer: Contact/Via/From carry {EXT_ADDR}:{EXT_PORT}");

    // ---- INTERNAL: caller inside local_net -> internal bind addr/port ------
    set_global_pjsip_config(transport_config(vec!["127.0.0.0/8".to_string()]));
    let (ok, bye) = call_and_reap(&handler, sip_local, &caller_sip, caller_addr, "sig-int").await;

    let (c_host, c_port) = uri_hostport(ok.get_header("Contact").expect("200 OK Contact"));
    assert_eq!(c_host, "127.0.0.1", "local_net peer must see the internal bind address, not the external one");
    assert_eq!(
        c_port,
        Some(sip_local.port()),
        "local_net peer must see the internal bind port, not the external override"
    );
    assert_ne!(c_port, Some(EXT_PORT), "the external port override must NOT reach a local_net peer");

    let (v_host, v_port) = via_hostport(bye.get_header("Via").expect("BYE Via"));
    assert_eq!(v_host, "127.0.0.1", "local_net peer must see the internal address in the BYE Via");
    assert_eq!(v_port, Some(sip_local.port()), "local_net peer must see the internal port in the BYE Via");

    let (f_host, f_port) = uri_hostport(bye.get_header("From").expect("BYE From"));
    assert_eq!(f_host, "127.0.0.1", "local_net peer must see the internal address in the BYE From URI");
    assert_eq!(f_port, Some(sip_local.port()), "local_net peer must see the internal port in the BYE From URI");
    assert_ne!(f_port, Some(EXT_PORT), "the external port override must NOT reach a local_net peer's From");
    println!("[E2E] local_net peer: Contact/Via/From carry the internal bind 127.0.0.1:{}", sip_local.port());

    set_global_pjsip_config(PjsipConfig::default());
}
