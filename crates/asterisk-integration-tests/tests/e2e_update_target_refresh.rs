//! End-to-end acceptance for UPDATE **target refresh** (RFC 3261 §12.2 / RFC
//! 3311), the M5 review MAJOR-3 property: an in-dialog UPDATE that carries a new
//! Contact must refresh the dialog's remote target so that a subsequent local
//! in-dialog request (BYE) is BOTH stamped with the refreshed Request-URI AND
//! physically delivered to the refreshed transport address.
//!
//! ## Why this is a two-live-socket wire proof
//!
//! The prior version of this test bound only P1, left P2 unbound, and inspected
//! only the BYE's Request-URI. That masked MAJOR-3b: production refreshed the
//! dialog target (so the R-URI moved to P2) but still sent the BYE *datagram* to
//! the stale INVITE source tuple P1. Reading the BYE off the P1 socket and
//! checking only its header could never see that the packet went to the wrong
//! place.
//!
//! This version binds BOTH P1 and P2 as live UDP SIP sockets and races them for
//! the BYE. It asserts the BYE **datagram arrives on P2** (the refreshed target)
//! and that its **Request-URI also carries P2**. A datagram delivered to P1 is a
//! hard failure, not an ignored packet.
//!
//! ## RED controls (captured in the PR body), each independently load-bearing
//!
//! * Defeat the **next-hop refresh** (leave `CallState.next_hop` at the INVITE
//!   source in `handle_update`): the R-URI still says P2 but the datagram lands
//!   on P1 -> the arrival-port assertion fails. This is the exact defect the
//!   reviewer caught that the old test could not.
//! * Defeat the **dialog target refresh** (drop `update_remote_target`): the
//!   remote target stays P1, so both the resolved next hop and the R-URI stay
//!   P1 -> the datagram lands on P1 and the assertion fails.

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
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
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

/// Wait for an in-dialog BYE *request* on this socket and return its
/// Request-URI. Non-BYE traffic (e.g. a 200 OK) is skipped so the socket only
/// "wins" the race when a BYE datagram actually lands on it.
async fn await_bye_uri(sock: &UdpSocket, budget: Duration) -> Option<SipUri> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Bye) {
                if let StartLine::Request(rl) = &msg.start_line {
                    return Some(rl.uri.clone());
                }
            }
        }
    }
    None
}

fn header_tag(msg: &SipMessage, header: &str) -> String {
    msg.get_header(header)
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("tag="))
                .map(|t| t.to_string())
        })
        .expect("header must carry a tag")
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

/// A no-SDP UPDATE whose Contact is `new_contact_port` (the refreshed target).
fn update_new_contact(
    call_id: &str,
    our_tag: &str,
    caller_tag: &str,
    via_port: u16,
    new_contact_port: u16,
    cseq: u32,
) -> SipMessage {
    let raw = format!(
        "UPDATE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{via_port};branch=z9hG4bK{call_id}upd{cseq}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={caller_tag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} UPDATE\r\n\
         Contact: <sip:caller@127.0.0.1:{new_contact_port}>\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Which live socket the BYE datagram was physically delivered to.
enum ByeArrival {
    /// Delivered to the refreshed Contact target P2 (correct), with its R-URI.
    OnP2(SipUri),
    /// Delivered to the stale INVITE source P1 (the MAJOR-3b defect), with its
    /// R-URI so the failure message can show the header moved but the packet
    /// did not.
    OnP1(SipUri),
    /// No BYE landed on either socket within the budget.
    Neither,
}

#[tokio::test]
async fn update_refreshes_dialog_target_for_subsequent_requests() {
    register_all_apps();
    set_global_pjsip_config(PjsipConfig {
        endpoints: vec![EndpointConfig {
            name: EXTEN.to_string(),
            context: "default".to_string(),
            auth: None,
            ..Default::default()
        }],
        ..Default::default()
    });

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
    let sip_local: SocketAddr = "127.0.0.1:5060".parse().unwrap();
    let driver = Arc::new(SipChannelDriver::new(sip_local));
    driver.set_transport(handler_transport.clone());
    TECH_REGISTRY.register(driver.clone());
    let handler = Arc::new(SipEventHandler::new(Arc::new(dp), handler_transport));
    handler.set_channel_driver(driver.clone());
    // Arm rtptimeout so the silent call is reaped, driving rustisk to send the
    // BYE whose destination + Request-URI we inspect.
    handler.set_rtp_timeout(Some(Duration::from_secs(2)));

    // ---- Two LIVE SIP sockets: P1 (INVITE Contact) and P2 (refreshed) ------
    // Binding BOTH is the whole point: the BYE datagram is physically delivered
    // to exactly one of them, and we assert on which.
    let p1_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p1_addr = p1_sock.local_addr().unwrap();
    let p1 = p1_addr.port();
    let p2_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p2_addr = p2_sock.local_addr().unwrap();
    let p2 = p2_addr.port();
    assert_ne!(p1, p2, "the refreshed target port must differ from the INVITE Contact");

    // ---- Establish an answered, media-silent call; INVITE Contact = P1 -----
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );

    let call_id = "update-target-refresh";
    let caller_tag = format!("caller{call_id}");
    let invite = invite_request(call_id, p1, &offer.to_string());
    let session =
        SipSession::new_inbound(&invite, sip_local, p1_addr).expect("inbound session");
    handler
        .handle_incoming_invite(&invite, p1_addr, session)
        .await;
    let ok = recv_sip_status(&p1_sock, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    let our_tag = header_tag(&ok, "To");
    // Keep the caller RTP socket bound but silent for the call's lifetime.
    std::mem::forget(caller_rtp);

    // ---- Refresh the target to the LIVE P2 Contact via UPDATE --------------
    // The UPDATE arrives from P1's source tuple (symmetric), but its Contact is
    // P2. The 200 OK for the UPDATE is a response and returns to the P1 source;
    // only the subsequent local BYE should move to P2.
    let upd = update_new_contact(call_id, &our_tag, &caller_tag, p1, p2, 2);
    handler.handle_update(&upd, p1_addr).await;
    let _upd_ok = recv_sip_status(&p1_sock, 200, Duration::from_secs(2))
        .await
        .expect("target-refresh UPDATE must be answered 200");

    // ---- rtptimeout reaps the silent call -> BYE MUST land on P2 -----------
    // Race both live sockets. Whichever receives the BYE first decides the
    // verdict; a BYE on P1 is the MAJOR-3b defect.
    let budget = Duration::from_secs(6);
    let arrival = tokio::select! {
        uri = await_bye_uri(&p2_sock, budget) => match uri {
            Some(u) => ByeArrival::OnP2(u),
            None => ByeArrival::Neither,
        },
        uri = await_bye_uri(&p1_sock, budget) => match uri {
            Some(u) => ByeArrival::OnP1(u),
            None => ByeArrival::Neither,
        },
    };

    match arrival {
        ByeArrival::OnP2(uri) => {
            // Datagram routing proven. Also require the Request-URI to carry the
            // refreshed target, so a regression in EITHER the dialog target or
            // the next-hop resolution is caught.
            assert_eq!(
                uri.port,
                Some(p2),
                "BYE datagram reached P2 but its Request-URI addressed {:?}, not the refreshed \
                 target port {p2}",
                uri.port
            );
            println!(
                "[E2E] UPDATE target refresh OPERATIONAL: BYE datagram delivered to the refreshed \
                 Contact port P2={p2} and its Request-URI carries P2 (INVITE port P1={p1})"
            );
        }
        ByeArrival::OnP1(uri) => panic!(
            "MAJOR-3b: BYE datagram was delivered to the STALE INVITE source port P1={p1}, not the \
             UPDATE-refreshed Contact port P2={p2}. Its Request-URI was {:?} — proof the header \
             moved but the routing did not.",
            uri.port
        ),
        ByeArrival::Neither => {
            panic!("rtptimeout must reap the media-silent call and send a BYE, but none arrived on P1 or P2")
        }
    }
}
