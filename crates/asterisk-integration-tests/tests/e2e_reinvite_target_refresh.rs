//! End-to-end acceptance for re-INVITE **target refresh** in BOTH directions
//! (RFC 3261 §12.2), the M5 review MAJOR-F1 / MINOR-F2 carryover into M6.
//!
//! A re-INVITE — like an UPDATE — is a target-refresh transaction. When it
//! carries a new Contact the dialog's remote target moves, and a subsequent
//! local in-dialog request (here the rtptimeout BYE) MUST be both stamped with
//! the refreshed Request-URI AND physically delivered to the refreshed transport
//! address. That must hold whichever side originated the re-INVITE:
//!
//!  * **request direction** — a REMOTE-initiated re-INVITE REQUEST carrying
//!    Contact P2 (handled by `handle_reinvite_request`). #142 implemented this.
//!  * **response direction** — a LOCALLY-initiated re-INVITE (`send_reinvite`)
//!    whose 2xx RESPONSE carries Contact P2 (handled by
//!    `handle_reinvite_response`). This was the MAJOR-F1 gap: the response
//!    Contact was validated + ACKed but never applied to the dialog target or
//!    the next hop, so a later BYE routed to the stale INVITE source port.
//!
//! ## Why this is a two-live-socket wire proof
//!
//! Both P1 (the INVITE Contact) and P2 (the refreshed Contact) are bound as
//! LIVE UDP SIP sockets. The BYE datagram is physically delivered to exactly one
//! of them and we assert on WHICH. Reading the BYE off one socket and checking
//! only its Request-URI could never catch MAJOR-3b/F1 (header moved, datagram
//! did not).
//!
//! ## RED controls (captured in the PR body), each independently load-bearing
//!
//!  * Response direction: delete the `handle_reinvite_response` next-hop refresh
//!    -> the R-URI still moves to P2 but the BYE datagram lands on P1 ->
//!    `reinvite_response_contact_refreshes_target` fails on the arrival port
//!    (this is the exact MAJOR-F1 defect).
//!  * Response direction: drop `update_remote_target` in
//!    `handle_reinvite_response` -> the dialog target stays P1 so the R-URI stays
//!    P1 -> the Request-URI assertion fails.
//!  * Request direction: delete the `handle_reinvite_request` next-hop refresh ->
//!    `reinvite_request_contact_refreshes_target` lands the BYE on P1.

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

async fn recv_sip(sock: &UdpSocket, timeout: Duration) -> Option<(SipMessage, SocketAddr)> {
    let mut buf = [0u8; 4096];
    let (len, src) = tokio::time::timeout(timeout, sock.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    Some((SipMessage::parse(&buf[..len]).ok()?, src))
}

async fn recv_sip_status(sock: &UdpSocket, status: u16, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some((msg, _)) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.status_code() == Some(status) {
                return Some(msg);
            }
        }
    }
    None
}

/// Wait for an in-dialog INVITE *request* (a re-INVITE rustisk sent us) and
/// return it. Responses (a retransmitted 200) are skipped.
async fn recv_reinvite_request(sock: &UdpSocket, budget: Duration) -> Option<SipMessage> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some((msg, _)) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Invite) {
                if let StartLine::Request(_) = &msg.start_line {
                    return Some(msg);
                }
            }
        }
    }
    None
}

/// Wait for an in-dialog BYE *request* on this socket and return its
/// Request-URI. Non-BYE traffic is skipped so a socket only "wins" the race when
/// a BYE datagram actually lands on it.
async fn await_bye_uri(sock: &UdpSocket, budget: Duration) -> Option<SipUri> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some((msg, _)) = recv_sip(sock, Duration::from_millis(300)).await {
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

/// A remote-initiated re-INVITE REQUEST whose Contact is `new_contact_port` (the
/// refreshed target). Via stays at P1 (symmetric source).
fn reinvite_request_new_contact(
    call_id: &str,
    our_tag: &str,
    caller_tag: &str,
    via_port: u16,
    new_contact_port: u16,
    cseq: u32,
    sdp: &str,
) -> SipMessage {
    let raw = format!(
        "INVITE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{via_port};branch=z9hG4bK{call_id}re{cseq}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={caller_tag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:{new_contact_port}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Build a 200 OK to a re-INVITE rustisk sent us. Copies the request's
/// Via/From/To/Call-ID/CSeq verbatim (so it validates as an in-dialog response)
/// and advertises `contact_port` as the refreshed Contact target.
fn reinvite_ok_with_contact(reinvite: &SipMessage, contact_port: u16, sdp: &str) -> SipMessage {
    let via = reinvite.get_header("Via").expect("re-INVITE Via");
    let from = reinvite.get_header("From").expect("re-INVITE From");
    let to = reinvite.get_header("To").expect("re-INVITE To");
    let call_id = reinvite.call_id().expect("re-INVITE Call-ID");
    let cseq = reinvite.cseq().expect("re-INVITE CSeq");
    let raw = format!(
        "SIP/2.0 200 OK\r\n\
         Via: {via}\r\n\
         From: {from}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         Contact: <sip:caller@127.0.0.1:{contact_port}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         \r\n\
         {sdp}",
        len = sdp.len()
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Which live socket the BYE datagram was physically delivered to.
enum ByeArrival {
    /// Delivered to the refreshed Contact target P2 (correct), with its R-URI.
    OnP2(SipUri),
    /// Delivered to the stale INVITE source P1 (the defect), with its R-URI so
    /// the failure message can show the header moved but the packet did not.
    OnP1(SipUri),
    /// No BYE landed on either socket within the budget.
    Neither,
}

/// Assemble a driver + handler wired over a fresh ephemeral UDP transport.
async fn make_handler() -> (Arc<SipEventHandler>, SocketAddr) {
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
    (handler, sip_local)
}

/// Establish an answered, media-silent inbound call whose INVITE Contact is P1.
/// Returns the negotiated `our_tag`.
async fn establish_call(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    p1_sock: &UdpSocket,
    p1_addr: SocketAddr,
    call_id: &str,
) -> String {
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );
    let invite = invite_request(call_id, p1_addr.port(), &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, p1_addr).expect("inbound session");
    handler
        .handle_incoming_invite(&invite, p1_addr, session)
        .await;
    let ok = recv_sip_status(p1_sock, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    // Keep the caller RTP socket bound but silent for the call's lifetime.
    std::mem::forget(caller_rtp);
    header_tag(&ok, "To")
}

async fn race_bye(p1_sock: &UdpSocket, p2_sock: &UdpSocket, budget: Duration) -> ByeArrival {
    tokio::select! {
        uri = await_bye_uri(p2_sock, budget) => match uri {
            Some(u) => ByeArrival::OnP2(u),
            None => ByeArrival::Neither,
        },
        uri = await_bye_uri(p1_sock, budget) => match uri {
            Some(u) => ByeArrival::OnP1(u),
            None => ByeArrival::Neither,
        },
    }
}

fn assert_on_p2(arrival: ByeArrival, p1: u16, p2: u16, label: &str) {
    match arrival {
        ByeArrival::OnP2(uri) => {
            assert_eq!(
                uri.port,
                Some(p2),
                "{label}: BYE datagram reached P2 but its Request-URI addressed {:?}, not the \
                 refreshed target port {p2}",
                uri.port
            );
            println!(
                "[E2E] {label}: BYE datagram delivered to the refreshed Contact port P2={p2} \
                 and its Request-URI carries P2 (INVITE port P1={p1})"
            );
        }
        ByeArrival::OnP1(uri) => panic!(
            "{label}: BYE datagram was delivered to the STALE INVITE source port P1={p1}, not the \
             re-INVITE-refreshed Contact port P2={p2}. Its Request-URI was {:?} — proof the header \
             moved but the routing did not.",
            uri.port
        ),
        ByeArrival::Neither => {
            panic!("{label}: rtptimeout must reap the media-silent call and send a BYE, but none arrived on P1 or P2")
        }
    }
}

/// Response direction (MAJOR-F1): a 2xx to a locally initiated `send_reinvite`
/// whose Contact is P2 must move the dialog target + next hop to P2.
#[tokio::test]
async fn reinvite_response_contact_refreshes_target() {
    let (handler, sip_local) = make_handler().await;

    let p1_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p1_addr = p1_sock.local_addr().unwrap();
    let p1 = p1_addr.port();
    let p2_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p2_addr = p2_sock.local_addr().unwrap();
    let p2 = p2_addr.port();
    assert_ne!(p1, p2, "the refreshed target port must differ from the INVITE Contact");

    let call_id = "reinvite-response-refresh";
    let _our_tag = establish_call(&handler, sip_local, &p1_sock, p1_addr, call_id).await;

    // rustisk sends a re-INVITE to its current next hop (P1). Capture it.
    let reinvite_sdp = SessionDescription::create_offer("127.0.0.1", 40000, &[codecs::pcmu()]);
    assert!(
        handler.send_reinvite(call_id, reinvite_sdp).await,
        "send_reinvite must succeed for the established call"
    );
    let reinvite = recv_reinvite_request(&p1_sock, Duration::from_secs(3))
        .await
        .expect("rustisk must send a re-INVITE to P1");

    // Answer that re-INVITE 200 OK with Contact P2 (the refreshed target),
    // sourced from the symmetric P1 tuple. This is the UAC-side target refresh.
    let answer = SessionDescription::create_offer("127.0.0.1", 41000, &[codecs::pcmu()]);
    let ok = reinvite_ok_with_contact(&reinvite, p2, &answer.to_string());
    handler.handle_reinvite_response(&ok, p1_addr).await;

    // rtptimeout reaps the silent call -> BYE MUST land on P2.
    let arrival = race_bye(&p1_sock, &p2_sock, Duration::from_secs(6)).await;
    assert_on_p2(arrival, p1, p2, "response-Contact refresh");
}

/// Request direction (#142): a remote-initiated re-INVITE REQUEST whose Contact
/// is P2 must move the dialog target + next hop to P2.
#[tokio::test]
async fn reinvite_request_contact_refreshes_target() {
    let (handler, sip_local) = make_handler().await;

    let p1_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p1_addr = p1_sock.local_addr().unwrap();
    let p1 = p1_addr.port();
    let p2_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let p2_addr = p2_sock.local_addr().unwrap();
    let p2 = p2_addr.port();
    assert_ne!(p1, p2, "the refreshed target port must differ from the INVITE Contact");

    let call_id = "reinvite-request-refresh";
    let caller_tag = format!("caller{call_id}");
    let our_tag = establish_call(&handler, sip_local, &p1_sock, p1_addr, call_id).await;

    // A remote-initiated re-INVITE REQUEST from the symmetric P1 source, Contact
    // P2. Delivered via handle_incoming_invite -> handle_reinvite_request.
    let reneg = SessionDescription::create_offer("127.0.0.1", 42000, &[codecs::pcmu()]);
    let reinvite = reinvite_request_new_contact(
        call_id,
        &our_tag,
        &caller_tag,
        p1,
        p2,
        2,
        &reneg.to_string(),
    );
    let session = SipSession::new_inbound(&reinvite, sip_local, p1_addr).expect("session");
    handler
        .handle_incoming_invite(&reinvite, p1_addr, session)
        .await;
    // Drain the re-INVITE 200 OK (sent back to the P1 source).
    let _ = recv_sip_status(&p1_sock, 200, Duration::from_secs(2)).await;

    // rtptimeout reaps the silent call -> BYE MUST land on P2.
    let arrival = race_bye(&p1_sock, &p2_sock, Duration::from_secs(6)).await;
    assert_on_p2(arrival, p1, p2, "request-Contact refresh");
}
