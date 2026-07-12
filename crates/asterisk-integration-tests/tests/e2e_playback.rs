//! End-to-end regression for issue #29: Playback() must actually stream the
//! file's audio to the channel's media plane (RTP), not silently return
//! Success having sent nothing.
//!
//! Own integration-test binary so its use of the process-global tech registry
//! is isolated from the other e2e tests.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use asterisk_apps::playback::AppPlayback;
use asterisk_apps::PbxExecResult;
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::channel::Channel;
use asterisk_sip::channel_driver::SipChannelDriver;
use asterisk_sip::rtp::{parse_rtp_header, RtpSession};
use asterisk_sip::transport::UdpTransport;
use asterisk_types::ChannelState;
use tokio::net::UdpSocket;

const FRAMES: usize = 5;
const FRAME_BYTES: usize = 160; // µ-law, 20 ms @ 8 kHz

/// Write a raw µ-law sounds file of `FRAMES` frames to a unique temp path.
fn write_temp_ulaw() -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("rustisk-playback-{}.ulaw", std::process::id()));
    let data = vec![0x55u8; FRAME_BYTES * FRAMES];
    std::fs::write(&path, &data).expect("write temp ulaw");
    path
}

#[tokio::test]
async fn playback_streams_file_audio_to_the_media_plane() {
    let file = write_temp_ulaw();

    // Media plane: an RTP session pointed at a peer socket, attached to a
    // driver under a PJSIP channel name so Playback's tech lookup finds it.
    let transport: Arc<dyn asterisk_sip::transport::SipTransport> = Arc::new(
        UdpTransport::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap(),
    );
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_addr = peer.local_addr().unwrap();

    let rtp = RtpSession::bind(local).await.unwrap();
    rtp.set_remote_addr(peer_addr);

    let driver = Arc::new(SipChannelDriver::new(local));
    driver.set_transport(transport.clone());
    let chan_name = "PJSIP/play-1";
    // Media-only test: the session is not exercised by the pump, so a plain
    // session is sufficient here.
    let session = asterisk_sip::session::SipSession::new_outbound(local, peer_addr);
    driver.attach_inbound_media(chan_name, session, transport, rtp);
    TECH_REGISTRY.register(driver.clone());

    // Drain the peer socket in the background, counting non-empty RTP payloads.
    let peer = Arc::new(peer);
    let reader = {
        let peer = peer.clone();
        tokio::spawn(async move {
            let mut got = 0usize;
            let mut buf = [0u8; 2048];
            loop {
                match tokio::time::timeout(Duration::from_millis(500), peer.recv_from(&mut buf))
                    .await
                {
                    Ok(Ok((n, _))) => {
                        if let Ok((_h, pl)) = parse_rtp_header(&buf[..n]) {
                            if pl.iter().all(|&b| b == 0x55) && !pl.is_empty() {
                                got += 1;
                            }
                        }
                    }
                    // Timed out — playback is done.
                    _ => break,
                }
            }
            got
        })
    };

    let mut channel = Channel::new(chan_name);
    channel.state = ChannelState::Up;
    let result = AppPlayback::exec(&mut channel, file.to_str().unwrap()).await;
    assert_eq!(result, PbxExecResult::Success, "playback of a real file succeeds");

    let received = reader.await.unwrap();
    let _ = std::fs::remove_file(&file);

    assert_eq!(
        received, FRAMES,
        "every 20 ms frame of the file must reach the RTP peer (got {received}/{FRAMES})"
    );
}
