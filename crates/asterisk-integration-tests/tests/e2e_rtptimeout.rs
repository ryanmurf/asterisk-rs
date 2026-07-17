//! End-to-end acceptance test for the RTP inactivity teardown (`rtptimeout`).
//!
//! Drives a full inbound INVITE -> Answer -> Echo call over real UDP sockets,
//! then goes media-silent (sends no RTP). Two phases, sharing one handler:
//!
//!  * **armed** — with `rtptimeout` set, the established, media-silent call is
//!    reaped and the caller receives a BYE at ~`rtptimeout` (not instantly, not
//!    never). This is the *receiver-observable* proof that the timer fired.
//!  * **disabled** — with `rtptimeout = None`, the SAME media-silent call is
//!    NOT reaped inside a generous window. Because nothing else bounds a silent
//!    established Echo() call, this proves the RTP-inactivity timer is the
//!    *load-bearing* reaper (the M4 Timer-B-masking lesson): remove it and the
//!    call lives on.
//!
//! RED control (for the PR body): neutralise the watchdog in a scratch clone
//! (e.g. never `softhangup` on timeout, or force `effective < timeout`) and the
//! `armed` phase gets no BYE -> the test fails. Its companion `disabled` phase
//! guarantees the BYE in `armed` cannot come from any other teardown path.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asterisk_apps::adapter::register_all_apps;
use asterisk_codecs::codecs;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::event_handler::SipEventHandler;
use asterisk_sip::parser::{SipMessage, SipMethod};
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

/// Read SIP datagrams until one with `status` arrives (skips 100 Trying).
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

/// Wait up to `budget` for an in-dialog BYE *request* from the handler,
/// returning how long it took to arrive (or `None` if none arrived).
async fn await_bye(sock: &UdpSocket, budget: Duration) -> Option<Duration> {
    let start = Instant::now();
    let deadline = start + budget;
    while Instant::now() < deadline {
        if let Some(msg) = recv_sip(sock, Duration::from_millis(300)).await {
            if msg.method() == Some(SipMethod::Bye) {
                return Some(start.elapsed());
            }
        }
    }
    None
}

fn invite_request(call_id: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}\r\n\
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

/// Establish an inbound, answered, media-silent call. Returns the caller's SIP
/// socket (source of the INVITE, sink for the handler's BYE) after capturing
/// the 200 OK. No RTP is ever sent, so the call is media-silent from answer.
async fn establish_silent_call(
    handler: &Arc<SipEventHandler>,
    sip_local: SocketAddr,
    call_id: &str,
) -> UdpSocket {
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    // Offer a real (but silent) RTP endpoint so the media plane is bound.
    let caller_rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_rtp_addr = caller_rtp.local_addr().unwrap();
    let offer = SessionDescription::create_offer(
        &caller_rtp_addr.ip().to_string(),
        caller_rtp_addr.port(),
        &[codecs::pcmu()],
    );
    let invite = invite_request(call_id, caller_sip_addr.port(), &offer.to_string());
    let session = SipSession::new_inbound(&invite, sip_local, caller_sip_addr)
        .expect("inbound session");
    let accepted = handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    assert_eq!(accepted.as_deref(), Some(call_id), "INVITE must be accepted");

    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("expected 200 OK with SDP answer");
    let answer = SessionDescription::parse(&ok.body).expect("answer SDP");
    let audio = answer
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("audio stream in answer");
    assert_ne!(audio.port, 0, "answer must advertise a live RTP port");
    // Deliberately keep `caller_rtp` bound but SILENT for the call's lifetime.
    std::mem::forget(caller_rtp);
    caller_sip
}

#[tokio::test]
async fn rtptimeout_reaps_media_silent_call_and_is_load_bearing() {
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

    // Dialplan: 100 -> Answer, Echo. Echo blocks reading (silent) media and is
    // hangup-aware (polls the store copy), so ONLY rtptimeout can end it.
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

    // ---- Phase 1: ARMED — the silent call is reaped at ~rtptimeout ---------
    let rtptimeout = Duration::from_secs(2);
    handler.set_rtp_timeout(Some(rtptimeout));
    let caller_sip = establish_silent_call(&handler, sip_local, "rtptimeout-armed").await;

    let elapsed = await_bye(&caller_sip, Duration::from_secs(6))
        .await
        .expect("ARMED: rtptimeout must reap the media-silent call (received BYE)");
    // Attributable to the ~2 s inactivity timer: not an instant teardown, and
    // comfortably inside the window. (Lower bound guards against some other
    // path tearing the call down immediately.)
    assert!(
        elapsed >= Duration::from_millis(1200),
        "ARMED: BYE at {elapsed:?} is too early to be the {rtptimeout:?} RTP timer"
    );
    assert!(
        elapsed <= Duration::from_secs(5),
        "ARMED: BYE at {elapsed:?} is later than the RTP timer window"
    );
    // The handler drove teardown to a BYE on the wire; the caller here does not
    // ACK it, so the dialog map lingers until dialog-timeout (exact drain with a
    // real, BYE-ACKing peer is the soak's job). Phase 2 works off a relative
    // baseline so this lingering state does not confuse the load-bearing check.
    println!("[E2E] ARMED: media-silent call reaped via rtptimeout after {elapsed:?}");

    // ---- Phase 2: DISABLED — nothing else reaps the silent call ------------
    // This is the load-bearing control: if any backstop (a Dial deadline, a
    // transaction timer, a stray answer timeout) could reap a silent
    // established Echo() call, this phase would see a BYE and fail.
    handler.set_rtp_timeout(None);
    let baseline = handler.active_calls();
    let caller_sip2 = establish_silent_call(&handler, sip_local, "rtptimeout-disabled").await;
    assert_eq!(
        handler.active_calls(),
        baseline + 1,
        "DISABLED: the new silent call must register a dialog"
    );
    let reaped = await_bye(&caller_sip2, Duration::from_secs(4)).await;
    assert!(
        reaped.is_none(),
        "DISABLED: a media-silent established call was reaped in {reaped:?} with \
         rtptimeout off — the RTP timer is being masked by another backstop"
    );
    assert_eq!(
        handler.active_calls(),
        baseline + 1,
        "DISABLED: the silent call must still be up with the reaper off"
    );
    println!("[E2E] DISABLED: media-silent call survived the window (rtptimeout is load-bearing)");
}
