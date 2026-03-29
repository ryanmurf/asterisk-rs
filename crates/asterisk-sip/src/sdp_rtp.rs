//! SDP/RTP integration (port of res_pjsip_sdp_rtp.c).
//!
//! Creates RTP sessions from SDP negotiation results, maps SDP media
//! descriptions to RTP parameters, handles codec negotiation from
//! SDP offer/answer, and provides hooks for DTLS-SRTP setup.

use std::net::SocketAddr;

use tracing::{debug, info};

use asterisk_codecs::Codec;

use crate::rtp::RtpSession;
use crate::sdp::{ConnectionData, MediaDescription, MediaDirection};

// ---------------------------------------------------------------------------
// RTP session parameters from SDP
// ---------------------------------------------------------------------------

/// Parameters extracted from an SDP media description for configuring
/// an RTP session.
#[derive(Debug, Clone)]
pub struct RtpParameters {
    /// Remote address to send RTP to.
    pub remote_addr: Option<SocketAddr>,
    /// Negotiated codecs in preference order.
    pub codecs: Vec<Codec>,
    /// Payload type for the primary codec.
    pub primary_payload_type: u8,
    /// Payload type for DTMF (telephone-event), if present.
    pub dtmf_payload_type: Option<u8>,
    /// Media direction.
    pub direction: MediaDirection,
    /// Whether RTCP-mux is enabled.
    pub rtcp_mux: bool,
    /// Whether DTLS-SRTP is requested.
    pub dtls_srtp: bool,
    /// Media type (audio, video).
    pub media_type: String,
}

impl RtpParameters {
    /// Extract RTP parameters from an SDP media description and session-level
    /// connection data.
    pub fn from_sdp(
        media: &MediaDescription,
        session_connection: Option<&ConnectionData>,
    ) -> Self {
        // Determine remote address from media-level or session-level c= line.
        let conn = media.connection.as_ref().or(session_connection);
        let remote_addr = conn.and_then(|c| {
            let port = media.port;
            if port == 0 {
                return None;
            }
            format!("{}:{}", c.addr, port).parse::<SocketAddr>().ok()
        });

        // Extract codecs.
        let codecs = media.codecs();

        // Find the primary (first) codec payload type.
        let primary_payload_type = codecs.first().map(|c| c.payload_type).unwrap_or(0);

        // Find telephone-event (DTMF) payload type.
        let dtmf_payload_type = codecs
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case("telephone-event"))
            .map(|c| c.payload_type);

        // Check for RTCP-mux attribute.
        let rtcp_mux = media
            .attributes
            .iter()
            .any(|(name, _)| name == "rtcp-mux");

        // Check for DTLS-SRTP (fingerprint attribute).
        let dtls_srtp = media
            .attributes
            .iter()
            .any(|(name, _)| name == "fingerprint" || name == "setup");

        RtpParameters {
            remote_addr,
            codecs,
            primary_payload_type,
            dtmf_payload_type,
            direction: media.direction,
            rtcp_mux,
            dtls_srtp,
            media_type: media.media_type.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// RTP session creation from SDP
// ---------------------------------------------------------------------------

/// Create an RTP session configured according to negotiated SDP parameters.
///
/// Binds to `local_addr` (port 0 for automatic selection) and configures
/// the session with the negotiated codec and DTMF settings.
pub async fn create_rtp_from_sdp(
    local_addr: SocketAddr,
    params: &RtpParameters,
) -> Result<RtpSession, std::io::Error> {
    let mut session = RtpSession::bind(local_addr)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    // Set primary payload type.
    session.payload_type = params.primary_payload_type;

    // Set DTMF payload type.
    if let Some(dtmf_pt) = params.dtmf_payload_type {
        session.dtmf_payload_type = dtmf_pt;
    }

    // Determine samples per packet from the primary codec.
    let sample_rate = params
        .codecs
        .first()
        .map(|c| c.sample_rate)
        .unwrap_or(8000);
    // Standard 20ms packetization.
    session.samples_per_packet = sample_rate / 50;

    // Set remote address.
    if let Some(remote) = params.remote_addr {
        session.set_remote_addr(remote);
    }

    info!(
        payload_type = params.primary_payload_type,
        codecs = params.codecs.len(),
        dtmf = ?params.dtmf_payload_type,
        remote = ?params.remote_addr,
        "RTP session created from SDP"
    );

    Ok(session)
}

// ---------------------------------------------------------------------------
// Codec negotiation
// ---------------------------------------------------------------------------

/// Negotiate codecs between a local set and an SDP offer/answer.
///
/// Returns the list of common codecs in the order presented by the offer
/// (per RFC 3264 -- offerer's preference wins for answer).
pub fn negotiate_codecs(
    local_codecs: &[Codec],
    remote_media: &MediaDescription,
) -> Vec<Codec> {
    let remote_codecs = remote_media.codecs();
    let mut result = Vec::new();

    for rc in &remote_codecs {
        for lc in local_codecs {
            if rc.name.eq_ignore_ascii_case(&lc.name) && rc.sample_rate == lc.sample_rate {
                result.push(Codec {
                    payload_type: rc.payload_type,
                    name: rc.name.clone(),
                    sample_rate: rc.sample_rate,
                    channels: rc.channels,
                });
                break;
            }
        }
    }

    result
}

/// Apply negotiated RTP parameters to an existing RTP session.
pub fn apply_sdp_to_rtp(session: &mut RtpSession, params: &RtpParameters) {
    session.payload_type = params.primary_payload_type;

    if let Some(dtmf_pt) = params.dtmf_payload_type {
        session.dtmf_payload_type = dtmf_pt;
    }

    if let Some(remote) = params.remote_addr {
        session.set_remote_addr(remote);
    }

    let sample_rate = params
        .codecs
        .first()
        .map(|c| c.sample_rate)
        .unwrap_or(8000);
    session.samples_per_packet = sample_rate / 50;

    debug!(
        payload = params.primary_payload_type,
        remote = ?params.remote_addr,
        "Applied SDP parameters to RTP session"
    );
}

// ---------------------------------------------------------------------------
// SDP media description builder for RTP
// ---------------------------------------------------------------------------

/// Build an SDP media description for an RTP session.
pub fn build_media_for_rtp(
    rtp_port: u16,
    codecs: &[Codec],
    include_dtmf: bool,
) -> MediaDescription {
    let mut formats: Vec<u8> = codecs.iter().map(|c| c.payload_type).collect();

    let mut attributes = Vec::new();
    for codec in codecs {
        attributes.push((
            "rtpmap".to_string(),
            Some(format!(
                "{} {}/{}",
                codec.payload_type, codec.name, codec.sample_rate
            )),
        ));
    }

    // Add telephone-event if requested and not already present.
    if include_dtmf && !codecs.iter().any(|c| c.name.eq_ignore_ascii_case("telephone-event")) {
        let dtmf_pt = 101u8;
        formats.push(dtmf_pt);
        attributes.push((
            "rtpmap".to_string(),
            Some(format!("{} telephone-event/8000", dtmf_pt)),
        ));
        attributes.push((
            "fmtp".to_string(),
            Some(format!("{} 0-16", dtmf_pt)),
        ));
    }

    attributes.push(("sendrecv".to_string(), None));

    // Add ptime attribute (20ms default).
    attributes.push(("ptime".to_string(), Some("20".to_string())));

    MediaDescription {
        media_type: "audio".to_string(),
        port: rtp_port,
        protocol: "RTP/AVP".to_string(),
        formats,
        connection: None,
        attributes,
        direction: MediaDirection::SendRecv,
        fingerprint: None,
        setup: None,
        rtcp_mux: false,
        ice_candidates: Vec::new(),
        bandwidth: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// DTLS-SRTP (stub)
// ---------------------------------------------------------------------------

/// DTLS-SRTP configuration (placeholder for future implementation).
#[derive(Debug, Clone)]
pub struct DtlsSrtpConfig {
    /// Fingerprint hash algorithm (e.g. "sha-256").
    pub hash: String,
    /// Certificate fingerprint.
    pub fingerprint: String,
    /// DTLS setup role.
    pub setup: DtlsSetup,
}

/// DTLS setup role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtlsSetup {
    Active,
    Passive,
    ActPass,
    HoldConn,
}

impl DtlsSetup {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
            Self::ActPass => "actpass",
            Self::HoldConn => "holdconn",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "active" => Self::Active,
            "passive" => Self::Passive,
            "actpass" => Self::ActPass,
            "holdconn" => Self::HoldConn,
            _ => Self::ActPass,
        }
    }
}

/// Add DTLS-SRTP attributes to an SDP media description.
pub fn add_dtls_attributes(media: &mut MediaDescription, config: &DtlsSrtpConfig) {
    media.attributes.push((
        "fingerprint".to_string(),
        Some(format!("{} {}", config.hash, config.fingerprint)),
    ));
    media.attributes.push((
        "setup".to_string(),
        Some(config.setup.as_str().to_string()),
    ));

    // Change protocol to DTLS-SRTP.
    media.protocol = "UDP/TLS/RTP/SAVPF".to_string();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdp::SessionDescription;

    #[test]
    fn test_rtp_params_from_sdp() {
        let sdp_text = "v=0\r\n\
            o=- 12345 12345 IN IP4 10.0.0.2\r\n\
            s=Test\r\n\
            c=IN IP4 10.0.0.2\r\n\
            t=0 0\r\n\
            m=audio 20000 RTP/AVP 0 8 101\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            a=rtpmap:8 PCMA/8000\r\n\
            a=rtpmap:101 telephone-event/8000\r\n\
            a=fmtp:101 0-16\r\n\
            a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        let media = &sdp.media_descriptions[0];
        let params = RtpParameters::from_sdp(media, sdp.connection.as_ref());

        assert_eq!(params.remote_addr, Some("10.0.0.2:20000".parse().unwrap()));
        assert_eq!(params.codecs.len(), 3);
        assert_eq!(params.primary_payload_type, 0);
        assert_eq!(params.dtmf_payload_type, Some(101));
        assert_eq!(params.direction, MediaDirection::SendRecv);
    }

    #[test]
    fn test_negotiate_codecs() {
        let local = vec![
            Codec::new("PCMU", 0, 8000),
            Codec::new("G722", 9, 8000),
        ];

        let remote_media = MediaDescription {
            media_type: "audio".to_string(),
            port: 20000,
            protocol: "RTP/AVP".to_string(),
            formats: vec![8, 0],
            connection: None,
            attributes: vec![
                ("rtpmap".to_string(), Some("8 PCMA/8000".to_string())),
                ("rtpmap".to_string(), Some("0 PCMU/8000".to_string())),
            ],
            direction: MediaDirection::SendRecv,
            fingerprint: None,
            setup: None,
            rtcp_mux: false,
            ice_candidates: Vec::new(),
            bandwidth: Vec::new(),
        };

        let result = negotiate_codecs(&local, &remote_media);
        // Only PCMU should match (PCMA is not in local).
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "PCMU");
    }

    #[test]
    fn test_build_media() {
        let codecs = vec![
            Codec::new("PCMU", 0, 8000),
            Codec::new("PCMA", 8, 8000),
        ];

        let media = build_media_for_rtp(20000, &codecs, true);
        assert_eq!(media.port, 20000);
        assert_eq!(media.formats.len(), 3); // PCMU + PCMA + telephone-event
        assert!(media.attributes.iter().any(|(n, _)| n == "ptime"));
    }
}
