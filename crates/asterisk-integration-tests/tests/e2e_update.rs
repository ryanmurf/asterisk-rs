//! End-to-end acceptance for answering in-dialog **UPDATE** (RFC 3311).
//!
//! Drives a full inbound INVITE -> Answer -> Echo call over real UDP sockets,
//! then exercises both UPDATE shapes against the live dialog:
//!
//!  * **no-SDP** (session-timer refresh) -> a 200 OK is returned (previously a
//!    silent 501 that dropped the refresh).
//!  * **with-SDP** (mid-dialog media renegotiation) -> the caller moves its RTP
//!    to a NEW port in the UPDATE offer; rustisk answers 200 + SDP and
//!    *re-points its media plane*. Proven **receiver-side**: Echo() only
//!    reflects packets back to the caller's NEW port after the UPDATE, because
//!    symmetric-RTP will not re-latch a new source on its own (packets from an
//!    unexpected source are discarded). If the offer were not applied, the new
//!    port would stay silent — which is exactly the RED control.
//!
//! RED control (PR body): revert `handle_update` to the old `501`/drop, or skip
//! `apply_inbound_offer`, and the corresponding assertion fails.

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
use asterisk_sip::pjsip_config::{set_global_pjsip_config, EndpointConfig, PjsipConfig};
use asterisk_sip::rtp::{build_rtp_packet, parse_rtp_header, RtpHeader};
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

fn header_tag(msg: &SipMessage, header: &str) -> String {
    msg.get_header(header)
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("tag="))
                .map(|t| t.to_string())
        })
        .expect("header must carry a tag")
}

fn invite_request(call_id: &str, from_tag: &str, contact_port: u16, sdp: &str) -> SipMessage {
    let raw = format!(
        "INVITE sip:{EXTEN}@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}inv\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={from_tag}\r\n\
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

#[allow(clippy::too_many_arguments)]
fn update_request(
    call_id: &str,
    our_tag: &str,
    caller_tag: &str,
    contact_port: u16,
    cseq: u32,
    sdp: Option<&str>,
) -> SipMessage {
    let (ctype, clen, body) = match sdp {
        Some(s) => (
            "Content-Type: application/sdp\r\n".to_string(),
            s.len(),
            s.to_string(),
        ),
        None => (String::new(), 0, String::new()),
    };
    let raw = format!(
        "UPDATE sip:asterisk@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{contact_port};branch=z9hG4bK{call_id}upd{cseq}\r\n\
         From: \"Caller\" <sip:caller@127.0.0.1>;tag={caller_tag}\r\n\
         To: <sip:{EXTEN}@127.0.0.1>;tag={our_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} UPDATE\r\n\
         Contact: <sip:caller@127.0.0.1:{contact_port}>\r\n\
         Session-Expires: 1800\r\n\
         {ctype}Content-Length: {clen}\r\n\
         \r\n\
         {body}"
    );
    SipMessage::parse(raw.as_bytes()).unwrap()
}

/// Send a burst of PCMU frames from `src` to `dest` and report whether any
/// non-zero echoed payload comes back on `src` inside the budget.
async fn echo_roundtrip(src: &UdpSocket, dest: SocketAddr, budget: Duration) -> bool {
    let payload = [0x7Fu8; 160];
    let mut buf = [0u8; 2048];
    let deadline = Instant::now() + budget;
    let mut seq: u16 = 0;
    while Instant::now() < deadline {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: seq == 0,
            payload_type: 0,
            sequence: seq,
            timestamp: (seq as u32) * 160,
            ssrc: 0x0BAD_F00D,
        };
        let _ = src.send_to(&build_rtp_packet(&header, &payload)[..], dest).await;
        seq = seq.wrapping_add(1);
        if let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(120), src.recv_from(&mut buf)).await
        {
            if let Ok((_h, pl)) = parse_rtp_header(&buf[..len]) {
                if pl.iter().any(|&b| b != 0) {
                    return true;
                }
            }
        }
    }
    false
}

#[tokio::test]
async fn in_dialog_update_is_answered_and_renegotiates_media() {
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

    // ---- Establish an answered Echo() call --------------------------------
    let caller_sip = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let caller_sip_addr = caller_sip.local_addr().unwrap();
    let rtp_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_a_addr = rtp_a.local_addr().unwrap();

    let call_id = "update-call-1";
    let caller_tag = "callerA";
    let offer = SessionDescription::create_offer(
        &rtp_a_addr.ip().to_string(),
        rtp_a_addr.port(),
        &[codecs::pcmu()],
    );
    let invite = invite_request(call_id, caller_tag, caller_sip_addr.port(), &offer.to_string());
    let session =
        SipSession::new_inbound(&invite, sip_local, caller_sip_addr).expect("inbound session");
    handler
        .handle_incoming_invite(&invite, caller_sip_addr, session)
        .await;
    let ok = recv_sip_status(&caller_sip, 200, Duration::from_secs(5))
        .await
        .expect("200 OK for INVITE");
    let our_tag = header_tag(&ok, "To");

    // Media flows to the original port A.
    assert!(
        echo_roundtrip(&rtp_a, {
            let answer = SessionDescription::parse(&ok.body).unwrap();
            let port = answer
                .media_descriptions
                .iter()
                .find(|m| m.media_type == "audio")
                .unwrap()
                .port;
            format!("127.0.0.1:{port}").parse().unwrap()
        }, Duration::from_secs(3))
        .await,
        "Echo must reflect media to the original RTP port"
    );

    // ---- Phase 1: UPDATE with no SDP (session refresh) -> 200 --------------
    let refresh = update_request(call_id, &our_tag, caller_tag, caller_sip_addr.port(), 2, None);
    handler.handle_update(&refresh, caller_sip_addr).await;
    let resp = recv_sip_status(&caller_sip, 200, Duration::from_secs(2))
        .await
        .expect("session-refresh UPDATE must be answered 200 (was 501/drop)");
    assert!(
        resp.body.trim().is_empty(),
        "a no-SDP UPDATE answer must not carry an SDP body"
    );
    println!("[E2E] UPDATE (no SDP) answered 200 OK");

    // ---- Phase 2: UPDATE with SDP moving RTP to a NEW port ----------------
    let rtp_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_b_addr = rtp_b.local_addr().unwrap();
    // Sanity: before the UPDATE, packets from the new port are NOT accepted
    // (symmetric RTP will not re-latch), so no echo comes back to rtp_b.
    assert!(
        !echo_roundtrip(&rtp_b, {
            let answer = SessionDescription::parse(&ok.body).unwrap();
            let port = answer
                .media_descriptions
                .iter()
                .find(|m| m.media_type == "audio")
                .unwrap()
                .port;
            format!("127.0.0.1:{port}").parse().unwrap()
        }, Duration::from_millis(700))
        .await,
        "pre-UPDATE: media from the new source port must not be echoed"
    );

    let new_offer = SessionDescription::create_offer(
        &rtp_b_addr.ip().to_string(),
        rtp_b_addr.port(),
        &[codecs::pcmu()],
    );
    let reneg = update_request(
        call_id,
        &our_tag,
        caller_tag,
        caller_sip_addr.port(),
        3,
        Some(&new_offer.to_string()),
    );
    handler.handle_update(&reneg, caller_sip_addr).await;
    let ok2 = recv_sip_status(&caller_sip, 200, Duration::from_secs(2))
        .await
        .expect("SDP UPDATE must be answered 200 + SDP");
    let answer2 = SessionDescription::parse(&ok2.body).expect("UPDATE answer must carry SDP");
    let ans_port = answer2
        .media_descriptions
        .iter()
        .find(|m| m.media_type == "audio")
        .expect("audio in UPDATE answer")
        .port;
    let ans_addr: SocketAddr = format!("127.0.0.1:{ans_port}").parse().unwrap();

    // Receiver-side proof: after the UPDATE, Echo now reflects media sent from
    // the NEW port back to the NEW port — the media plane was re-pointed.
    assert!(
        echo_roundtrip(&rtp_b, ans_addr, Duration::from_secs(3)).await,
        "post-UPDATE: media from the renegotiated port must be echoed back \
         (media plane must have re-pointed to the new remote)"
    );
    println!("[E2E] UPDATE (SDP) answered 200 + SDP; media re-pointed to the new port");
}
