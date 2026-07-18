//! SDP (Session Description Protocol) parser and generator (RFC 4566).
//!
//! Supports offer/answer model for codec negotiation, DTLS-SRTP
//! attributes (fingerprint, setup, rtcp-mux), and SRTP crypto lines.


use std::fmt;

use asterisk_codecs::Codec;

use crate::crypto::FingerprintAlgorithm;
use crate::dtls::DtlsRole;
use crate::ice::{IceCandidate, IceOptions};

/// SDP origin field (o=).
#[derive(Debug, Clone)]
pub struct Origin {
    pub username: String,
    pub session_id: String,
    pub session_version: String,
    pub net_type: String,
    pub addr_type: String,
    pub addr: String,
}

impl Default for Origin {
    fn default() -> Self {
        Self {
            username: "-".to_string(),
            session_id: "0".to_string(),
            session_version: "0".to_string(),
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            addr: "0.0.0.0".to_string(),
        }
    }
}

/// SDP connection data (c=).
#[derive(Debug, Clone)]
pub struct ConnectionData {
    pub net_type: String,
    pub addr_type: String,
    pub addr: String,
}

impl Default for ConnectionData {
    fn default() -> Self {
        Self {
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            addr: "0.0.0.0".to_string(),
        }
    }
}

/// SDP media description (m=).
#[derive(Debug, Clone)]
pub struct MediaDescription {
    pub media_type: String,
    pub port: u16,
    pub protocol: String,
    pub formats: Vec<u8>,
    pub connection: Option<ConnectionData>,
    pub attributes: Vec<(String, Option<String>)>,
    pub direction: MediaDirection,
    /// DTLS fingerprint algorithm and value from `a=fingerprint:`.
    pub fingerprint: Option<(FingerprintAlgorithm, String)>,
    /// DTLS setup role from `a=setup:`.
    pub setup: Option<DtlsRole>,
    /// Whether `a=rtcp-mux` is present.
    pub rtcp_mux: bool,
    /// ICE candidates parsed from `a=candidate:` lines.
    pub ice_candidates: Vec<IceCandidate>,
    /// Bandwidth constraints (b= lines).
    pub bandwidth: Vec<Bandwidth>,
}

/// SDP bandwidth specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bandwidth {
    /// Application Specific bandwidth in kbps (b=AS:512).
    ApplicationSpecific(u32),
    /// Transport Independent Application Specific in bps (b=TIAS:512000).
    TransportIndependent(u64),
    /// Conference Total bandwidth in kbps (b=CT:1024).
    ConferenceTotal(u32),
}

impl Bandwidth {
    /// Parse a bandwidth line value (everything after `b=`).
    pub fn parse(value: &str) -> Option<Self> {
        let (bw_type, bw_value) = value.split_once(':')?;
        match bw_type.trim() {
            "AS" => bw_value.trim().parse::<u32>().ok().map(Self::ApplicationSpecific),
            "TIAS" => bw_value.trim().parse::<u64>().ok().map(Self::TransportIndependent),
            "CT" => bw_value.trim().parse::<u32>().ok().map(Self::ConferenceTotal),
            _ => None,
        }
    }

    /// Get bandwidth in bits per second (normalized).
    pub fn as_bps(&self) -> u64 {
        match self {
            Self::ApplicationSpecific(kbps) => *kbps as u64 * 1000,
            Self::TransportIndependent(bps) => *bps,
            Self::ConferenceTotal(kbps) => *kbps as u64 * 1000,
        }
    }
}

impl std::fmt::Display for Bandwidth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplicationSpecific(kbps) => write!(f, "AS:{}", kbps),
            Self::TransportIndependent(bps) => write!(f, "TIAS:{}", bps),
            Self::ConferenceTotal(kbps) => write!(f, "CT:{}", kbps),
        }
    }
}

/// Media stream direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaDirection {
    #[default]
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl MediaDescription {
    pub fn new_audio(port: u16) -> Self {
        Self {
            media_type: "audio".to_string(),
            port,
            protocol: "RTP/AVP".to_string(),
            formats: vec![0], // Default: PCMU
            connection: None,
            attributes: Vec::new(),
            direction: MediaDirection::SendRecv,
            fingerprint: None,
            setup: None,
            rtcp_mux: false,
            ice_candidates: Vec::new(),
            bandwidth: Vec::new(),
        }
    }

    /// Create an audio media description for DTLS-SRTP (WebRTC).
    pub fn new_audio_dtls(
        port: u16,
        fingerprint_algorithm: FingerprintAlgorithm,
        fingerprint: &str,
        setup: DtlsRole,
    ) -> Self {
        let mut media = Self {
            media_type: "audio".to_string(),
            port,
            protocol: "UDP/TLS/RTP/SAVPF".to_string(),
            formats: vec![0],
            connection: None,
            attributes: Vec::new(),
            direction: MediaDirection::SendRecv,
            fingerprint: Some((fingerprint_algorithm, fingerprint.to_string())),
            setup: Some(setup),
            rtcp_mux: true,
            ice_candidates: Vec::new(),
            bandwidth: Vec::new(),
        };
        // Add DTLS attributes to the attributes list for serialization.
        media.attributes.push((
            "fingerprint".to_string(),
            Some(format!("{} {}", fingerprint_algorithm.sdp_name(), fingerprint)),
        ));
        media.attributes.push((
            "setup".to_string(),
            Some(setup.sdp_value().to_string()),
        ));
        media.attributes.push(("rtcp-mux".to_string(), None));
        media
    }

    /// Get the DTLS fingerprint from attributes.
    pub fn get_fingerprint(&self) -> Option<(FingerprintAlgorithm, &str)> {
        self.fingerprint.as_ref().map(|(alg, fp)| (*alg, fp.as_str()))
    }

    /// Get the DTLS setup role from attributes.
    pub fn get_setup(&self) -> Option<DtlsRole> {
        self.setup
    }

    /// Check if rtcp-mux is enabled.
    pub fn has_rtcp_mux(&self) -> bool {
        self.rtcp_mux
    }

    /// Get rtpmap attributes.
    pub fn get_rtpmap(&self, payload_type: u8) -> Option<String> {
        let pt_str = payload_type.to_string();
        for (name, value) in &self.attributes {
            if name == "rtpmap" {
                if let Some(val) = value {
                    if val.starts_with(&format!("{} ", pt_str)) || val.starts_with(&format!("{}/", pt_str)) {
                        return Some(val.clone());
                    }
                }
            }
        }
        None
    }

    /// Get fmtp attributes for a payload type.
    pub fn get_fmtp(&self, payload_type: u8) -> Option<String> {
        let pt_str = payload_type.to_string();
        for (name, value) in &self.attributes {
            if name == "fmtp" {
                if let Some(val) = value {
                    if val.starts_with(&format!("{} ", pt_str)) {
                        return Some(val.clone());
                    }
                }
            }
        }
        None
    }

    /// Extract codecs from this media description.
    pub fn codecs(&self) -> Vec<Codec> {
        let mut codecs = Vec::new();
        for &pt in &self.formats {
            if let Some(rtpmap) = self.get_rtpmap(pt) {
                // Parse "codec_name/sample_rate[/channels]"
                let parts: Vec<&str> = rtpmap.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let codec_info: Vec<&str> = parts[1].split('/').collect();
                    let name = codec_info[0].to_string();
                    let sample_rate = codec_info
                        .get(1)
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(8000);
                    let channels = codec_info
                        .get(2)
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(1);
                    codecs.push(Codec {
                        payload_type: pt,
                        name,
                        sample_rate,
                        channels,
                    });
                }
            } else {
                // Static payload type -- use well-known mappings
                let codec = match pt {
                    0 => Codec::new("PCMU", 0, 8000),
                    3 => Codec::new("GSM", 3, 8000),
                    4 => Codec::new("G723", 4, 8000),
                    8 => Codec::new("PCMA", 8, 8000),
                    9 => Codec::new("G722", 9, 8000),
                    18 => Codec::new("G729", 18, 8000),
                    _ => Codec::new(&format!("unknown-{}", pt), pt, 8000),
                };
                codecs.push(codec);
            }
        }
        codecs
    }
}

/// A complete SDP session description.
#[derive(Debug, Clone)]
pub struct SessionDescription {
    pub version: u32,
    pub origin: Origin,
    pub session_name: String,
    pub connection: Option<ConnectionData>,
    pub time: (u64, u64),
    pub media_descriptions: Vec<MediaDescription>,
    pub attributes: Vec<(String, Option<String>)>,
}

impl Default for SessionDescription {
    fn default() -> Self {
        Self {
            version: 0,
            origin: Origin::default(),
            session_name: "Asterisk".to_string(),
            connection: Some(ConnectionData::default()),
            time: (0, 0),
            media_descriptions: Vec::new(),
            attributes: Vec::new(),
        }
    }
}

impl SessionDescription {
    /// Parse SDP from text.
    pub fn parse(text: &str) -> Result<Self, SdpError> {
        let mut sdp = SessionDescription::default();
        let mut current_media: Option<MediaDescription> = None;

        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.len() < 2 || line.as_bytes()[1] != b'=' {
                continue;
            }

            let field_type = line.as_bytes()[0] as char;
            let value = &line[2..];

            match field_type {
                'v' => {
                    sdp.version = value.parse().unwrap_or(0);
                }
                'o' => {
                    let parts: Vec<&str> = value.splitn(6, ' ').collect();
                    if parts.len() >= 6 {
                        sdp.origin = Origin {
                            username: parts[0].to_string(),
                            session_id: parts[1].to_string(),
                            session_version: parts[2].to_string(),
                            net_type: parts[3].to_string(),
                            addr_type: parts[4].to_string(),
                            addr: parts[5].to_string(),
                        };
                    }
                }
                's' => {
                    sdp.session_name = value.to_string();
                }
                'c' => {
                    let parts: Vec<&str> = value.splitn(3, ' ').collect();
                    if parts.len() >= 3 {
                        let conn = ConnectionData {
                            net_type: parts[0].to_string(),
                            addr_type: parts[1].to_string(),
                            addr: parts[2].to_string(),
                        };
                        if let Some(media) = current_media.as_mut() {
                            media.connection = Some(conn);
                        } else {
                            sdp.connection = Some(conn);
                        }
                    }
                }
                't' => {
                    let parts: Vec<&str> = value.splitn(2, ' ').collect();
                    let start = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let stop = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    sdp.time = (start, stop);
                }
                'm' => {
                    // Save previous media description
                    if let Some(media) = current_media.take() {
                        sdp.media_descriptions.push(media);
                    }

                    let parts: Vec<&str> = value.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let media_type = parts[0].to_string();
                        let port = parts[1].parse().unwrap_or(0);
                        let protocol = parts[2].to_string();
                        let formats: Vec<u8> = parts[3..]
                            .iter()
                            .filter_map(|s| s.parse().ok())
                            .collect();

                        current_media = Some(MediaDescription {
                            media_type,
                            port,
                            protocol,
                            formats,
                            connection: None,
                            attributes: Vec::new(),
                            direction: MediaDirection::SendRecv,
                            fingerprint: None,
                            setup: None,
                            rtcp_mux: false,
                            ice_candidates: Vec::new(),
                            bandwidth: Vec::new(),
                        });
                    }
                }
                'a' => {
                    let (attr_name, attr_value) = match value.split_once(':') {
                        Some((n, v)) => (n.to_string(), Some(v.to_string())),
                        None => (value.to_string(), None),
                    };

                    // Check for direction attributes
                    let direction = match attr_name.as_str() {
                        "sendrecv" => Some(MediaDirection::SendRecv),
                        "sendonly" => Some(MediaDirection::SendOnly),
                        "recvonly" => Some(MediaDirection::RecvOnly),
                        "inactive" => Some(MediaDirection::Inactive),
                        _ => None,
                    };

                    if let Some(media) = &mut current_media {
                        if let Some(dir) = direction {
                            media.direction = dir;
                        }

                        // Parse DTLS/security and ICE attributes.
                        match attr_name.as_str() {
                            "fingerprint" => {
                                if let Some(ref val) = attr_value {
                                    if let Some((alg_str, fp)) = val.split_once(' ') {
                                        if let Some(alg) = FingerprintAlgorithm::from_sdp_name(alg_str) {
                                            media.fingerprint = Some((alg, fp.to_string()));
                                        }
                                    }
                                }
                            }
                            "setup" => {
                                if let Some(ref val) = attr_value {
                                    media.setup = DtlsRole::from_sdp(val);
                                }
                            }
                            "rtcp-mux" => {
                                media.rtcp_mux = true;
                            }
                            "candidate" => {
                                if let Some(ref val) = attr_value {
                                    if let Some(candidate) = IceCandidate::from_sdp_attribute(val) {
                                        media.ice_candidates.push(candidate);
                                    }
                                }
                            }
                            _ => {}
                        }

                        media.attributes.push((attr_name, attr_value));
                    } else {
                        // Session-level attributes: also check for fingerprint/setup.
                        if attr_name.as_str() == "fingerprint" {
                            // Session-level fingerprint applies to all media.
                        }
                        sdp.attributes.push((attr_name, attr_value));
                    }
                }
                'b' => {
                    // Bandwidth line: b=AS:512 / b=TIAS:512000 / b=CT:1024
                    if let Some(bw) = Bandwidth::parse(value) {
                        if let Some(media) = current_media.as_mut() {
                            media.bandwidth.push(bw);
                        }
                    }
                }
                _ => {
                    // Ignore unknown field types
                }
            }
        }

        // Save last media description
        if let Some(media) = current_media {
            sdp.media_descriptions.push(media);
        }

        Ok(sdp)
    }

    /// Create an SDP offer.
    pub fn create_offer(addr: &str, port: u16, codecs: &[Codec]) -> Self {
        let session_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let formats: Vec<u8> = codecs.iter().map(|c| c.payload_type).collect();

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
        attributes.push(("sendrecv".to_string(), None));

        let media = MediaDescription {
            media_type: "audio".to_string(),
            port,
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
        };

        let addr_type = addr_type_for(addr);
        SessionDescription {
            version: 0,
            origin: Origin {
                username: "-".to_string(),
                session_id: session_id.clone(),
                session_version: session_id,
                net_type: "IN".to_string(),
                addr_type: addr_type.to_string(),
                addr: addr.to_string(),
            },
            session_name: "Asterisk".to_string(),
            connection: Some(ConnectionData {
                net_type: "IN".to_string(),
                addr_type: addr_type.to_string(),
                addr: addr.to_string(),
            }),
            time: (0, 0),
            media_descriptions: vec![media],
            attributes: Vec::new(),
        }
    }

    /// Create an SDP answer from an offer.
    ///
    /// rustisk binds a single RTP transport per call and does not negotiate
    /// BUNDLE, so it can carry exactly one media stream. Per RFC 3264 §6 the
    /// answer keeps every offered m-line, in order, but only the **first
    /// audio** m-line that shares a codec is accepted and given `port`; every
    /// other m-line (a second audio stream, any video/application stream, or
    /// one with no common codec) is rejected with port 0. This prevents the
    /// caller from sending, e.g., video RTP into the audio socket — which the
    /// receiver would mislabel as voice and echo back with the audio payload
    /// type, corrupting both streams (issue #31). Per-stream transports/ports
    /// are future work.
    ///
    /// The accepted stream's answer direction is derived from the offer per
    /// RFC 3264 §6.1 (see [`answer_direction`]); a `sendonly` hold offer is
    /// answered `recvonly`, never `sendrecv`.
    pub fn create_answer(
        offer: &SessionDescription,
        addr: &str,
        port: u16,
        supported_codecs: &[Codec],
    ) -> Self {
        let session_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let addr_type = addr_type_for(addr);
        let mut answer = SessionDescription {
            version: 0,
            origin: Origin {
                username: "-".to_string(),
                session_id: session_id.clone(),
                session_version: session_id,
                net_type: "IN".to_string(),
                addr_type: addr_type.to_string(),
                addr: addr.to_string(),
            },
            session_name: "Asterisk".to_string(),
            connection: Some(ConnectionData {
                net_type: "IN".to_string(),
                addr_type: addr_type.to_string(),
                addr: addr.to_string(),
            }),
            time: (0, 0),
            media_descriptions: Vec::new(),
            attributes: Vec::new(),
        };

        // For each media in the offer, find common codecs. Only the first
        // audio stream with a common codec is accepted; a single transport is
        // bound per call, so every other stream must be rejected (port 0).
        let mut audio_accepted = false;
        for offer_media in &offer.media_descriptions {
            let offer_codecs = offer_media.codecs();
            let mut common: Vec<Codec> = Vec::new();
            for oc in &offer_codecs {
                for sc in supported_codecs {
                    if oc.name.eq_ignore_ascii_case(&sc.name) && oc.sample_rate == sc.sample_rate {
                        common.push(Codec {
                            payload_type: oc.payload_type,
                            name: oc.name.clone(),
                            sample_rate: oc.sample_rate,
                            channels: oc.channels,
                        });
                        break;
                    }
                }
            }

            let is_audio = offer_media.media_type.eq_ignore_ascii_case("audio");
            let accept = is_audio && !audio_accepted && !common.is_empty();

            if !accept {
                // Reject: no common codec, a non-audio stream, or an
                // additional audio stream we have no transport for. Echo the
                // media line with port 0 (RFC 3264 §6) so the m-line count and
                // order still match the offer.
                answer.media_descriptions.push(MediaDescription {
                    media_type: offer_media.media_type.clone(),
                    port: 0,
                    protocol: offer_media.protocol.clone(),
                    formats: offer_media.formats.clone(),
                    connection: None,
                    attributes: Vec::new(),
                    direction: MediaDirection::Inactive,
                    fingerprint: None,
                    setup: None,
                    rtcp_mux: false,
                    ice_candidates: Vec::new(),
                    bandwidth: Vec::new(),
                });
            } else {
                audio_accepted = true;
                let formats: Vec<u8> = common.iter().map(|c| c.payload_type).collect();
                let mut attributes = Vec::new();
                for codec in &common {
                    attributes.push((
                        "rtpmap".to_string(),
                        Some(format!(
                            "{} {}/{}",
                            codec.payload_type, codec.name, codec.sample_rate
                        )),
                    ));
                }
                // RFC 3264 §6.1: the answer's direction is derived from the
                // offer's — a directional/hold offer must NOT be answered
                // `sendrecv`. (M5 hold-answer defect: a `sendonly` hold offer
                // was answered `sendrecv`, claiming to send into a stream the
                // peer will not receive.)
                let answer_dir = answer_direction(offer_media, offer.connection.as_ref());
                attributes.push((direction_attr(answer_dir).to_string(), None));

                answer.media_descriptions.push(MediaDescription {
                    media_type: offer_media.media_type.clone(),
                    port,
                    protocol: offer_media.protocol.clone(),
                    formats,
                    connection: None,
                    attributes,
                    direction: answer_dir,
                    fingerprint: offer_media.fingerprint.clone(),
                    setup: offer_media.setup.map(|s| {
                        // Answer flips the setup role.
                        match s {
                            DtlsRole::Active => DtlsRole::Passive,
                            DtlsRole::Passive => DtlsRole::Active,
                            DtlsRole::ActPass => DtlsRole::Active,
                            other => other,
                        }
                    }),
                    rtcp_mux: offer_media.rtcp_mux,
                    ice_candidates: Vec::new(),
                    bandwidth: Vec::new(),
                });
            }
        }

        answer
    }

    // ----- ICE SDP methods -----

    /// Get the ICE ufrag from session-level attributes.
    pub fn ice_ufrag(&self) -> Option<&str> {
        for (name, value) in &self.attributes {
            if name == "ice-ufrag" {
                return value.as_deref();
            }
        }
        None
    }

    /// Get the ICE password from session-level attributes.
    pub fn ice_pwd(&self) -> Option<&str> {
        for (name, value) in &self.attributes {
            if name == "ice-pwd" {
                return value.as_deref();
            }
        }
        None
    }

    /// Get ICE options from session-level attributes.
    pub fn ice_options(&self) -> Option<IceOptions> {
        for (name, value) in &self.attributes {
            if name == "ice-options" {
                if let Some(v) = value {
                    return Some(IceOptions::parse(v));
                }
            }
        }
        None
    }

    /// Check if `a=ice-lite` is present at session level.
    pub fn is_ice_lite(&self) -> bool {
        self.attributes
            .iter()
            .any(|(name, _)| name == "ice-lite")
    }

    /// Set ICE credentials at session level.
    pub fn set_ice_credentials(&mut self, ufrag: &str, pwd: &str) {
        // Remove existing
        self.attributes.retain(|(n, _)| n != "ice-ufrag" && n != "ice-pwd");
        self.attributes
            .push(("ice-ufrag".to_string(), Some(ufrag.to_string())));
        self.attributes
            .push(("ice-pwd".to_string(), Some(pwd.to_string())));
    }

    /// Set ICE options at session level.
    pub fn set_ice_options(&mut self, options: &IceOptions) {
        self.attributes.retain(|(n, _)| n != "ice-options");
        if !options.tokens.is_empty() {
            self.attributes.push((
                "ice-options".to_string(),
                Some(options.to_sdp_value()),
            ));
        }
    }

    /// Set ice-lite at session level.
    pub fn set_ice_lite(&mut self) {
        if !self.is_ice_lite() {
            self.attributes
                .push(("ice-lite".to_string(), None));
        }
    }

    /// Add ICE candidates to a media description's attributes.
    ///
    /// This both stores them in the `ice_candidates` vec and adds
    /// `a=candidate:` lines to the attributes for serialization.
    pub fn add_ice_candidates_to_media(
        &mut self,
        media_idx: usize,
        candidates: &[IceCandidate],
    ) {
        if media_idx >= self.media_descriptions.len() {
            return;
        }
        let media = &mut self.media_descriptions[media_idx];
        for candidate in candidates {
            media.ice_candidates.push(candidate.clone());
            media.attributes.push((
                "candidate".to_string(),
                Some(candidate.to_sdp_attribute()),
            ));
        }
    }

    /// Get ICE ufrag for a specific media description (falls back to session level).
    pub fn media_ice_ufrag(&self, media_idx: usize) -> Option<&str> {
        if let Some(media) = self.media_descriptions.get(media_idx) {
            for (name, value) in &media.attributes {
                if name == "ice-ufrag" {
                    return value.as_deref();
                }
            }
        }
        self.ice_ufrag()
    }

    /// Get ICE password for a specific media description (falls back to session level).
    pub fn media_ice_pwd(&self, media_idx: usize) -> Option<&str> {
        if let Some(media) = self.media_descriptions.get(media_idx) {
            for (name, value) in &media.attributes {
                if name == "ice-pwd" {
                    return value.as_deref();
                }
            }
        }
        self.ice_pwd()
    }
}

/// RFC 3264 §6.1: derive the answer's direction for a unicast stream from the
/// offer's direction. We reflect the offer so we never advertise a direction
/// the peer did not agree to receive:
///
/// | offer      | answer     |
/// |------------|------------|
/// | `sendrecv` | `sendrecv` |
/// | `sendonly` | `recvonly` | (peer put us on hold; we only receive)
/// | `recvonly` | `sendonly` |
/// | `inactive` | `inactive` |
///
/// A stream whose (media- or, absent that, session-level) connection address is
/// zeroed — `c=…0.0.0.0` / `c=…::`, the RFC 2543 legacy hold — is treated as a
/// `sendonly` hold, so the answer is `recvonly`. Emitting `a=sendrecv` for a
/// directional or held offer violates RFC 3264 §6.1.
fn answer_direction(
    offer_media: &MediaDescription,
    session_connection: Option<&ConnectionData>,
) -> MediaDirection {
    let is_zeroed = |c: &ConnectionData| c.addr == "0.0.0.0" || c.addr == "::";
    let held_by_zeroed_connection = match offer_media.connection.as_ref() {
        Some(conn) => is_zeroed(conn),
        None => session_connection.map(is_zeroed).unwrap_or(false),
    };
    let effective = if held_by_zeroed_connection {
        MediaDirection::SendOnly
    } else {
        offer_media.direction
    };
    match effective {
        MediaDirection::SendRecv => MediaDirection::SendRecv,
        MediaDirection::SendOnly => MediaDirection::RecvOnly,
        MediaDirection::RecvOnly => MediaDirection::SendOnly,
        MediaDirection::Inactive => MediaDirection::Inactive,
    }
}

/// The SDP `a=` attribute name for a media direction.
fn direction_attr(dir: MediaDirection) -> &'static str {
    match dir {
        MediaDirection::SendRecv => "sendrecv",
        MediaDirection::SendOnly => "sendonly",
        MediaDirection::RecvOnly => "recvonly",
        MediaDirection::Inactive => "inactive",
    }
}

impl fmt::Display for SessionDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v={}\r\n", self.version)?;
        write!(
            f,
            "o={} {} {} {} {} {}\r\n",
            self.origin.username,
            self.origin.session_id,
            self.origin.session_version,
            self.origin.net_type,
            self.origin.addr_type,
            self.origin.addr
        )?;
        write!(f, "s={}\r\n", self.session_name)?;

        if let Some(ref conn) = self.connection {
            write!(f, "c={} {} {}\r\n", conn.net_type, conn.addr_type, conn.addr)?;
        }

        write!(f, "t={} {}\r\n", self.time.0, self.time.1)?;

        for (name, value) in &self.attributes {
            match value {
                Some(v) => write!(f, "a={}:{}\r\n", name, v)?,
                None => write!(f, "a={}\r\n", name)?,
            }
        }

        for media in &self.media_descriptions {
            let fmts: Vec<String> = media.formats.iter().map(|pt| pt.to_string()).collect();
            write!(
                f,
                "m={} {} {} {}\r\n",
                media.media_type,
                media.port,
                media.protocol,
                fmts.join(" ")
            )?;

            if let Some(ref conn) = media.connection {
                write!(f, "c={} {} {}\r\n", conn.net_type, conn.addr_type, conn.addr)?;
            }

            // Bandwidth lines.
            for bw in &media.bandwidth {
                write!(f, "b={}\r\n", bw)?;
            }

            for (name, value) in &media.attributes {
                match value {
                    Some(v) => write!(f, "a={}:{}\r\n", name, v)?,
                    None => write!(f, "a={}\r\n", name)?,
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SdpError {
    #[error("SDP parse error: {0}")]
    Parse(String),
}

/// RFC 4566 address type for an address literal: `IP6` for IPv6, else `IP4`.
fn addr_type_for(addr: &str) -> &'static str {
    if addr.contains(':') { "IP6" } else { "IP4" }
}

/// Pick the concrete IP address to advertise in SDP `c=`/`o=` lines toward
/// `remote` (issue #56).
///
/// Never returns the unspecified address for a routable peer: `c=IN IP4
/// 0.0.0.0` is not a valid media destination in an active session
/// (RFC 3264 §5.1 reserves it for hold/black-hole semantics), so
/// advertising the raw INADDR_ANY bind blackholes audio for any peer that
/// honors the answer's c-line and lacks symmetric-RTP latching.
///
/// Selection order, mirroring pjsip's NAT handling:
/// 1. a transport's configured `external_media_address`, unless the peer
///    falls inside that transport's `local_net` CIDRs;
/// 2. the concrete local bind address, when there is one;
/// 3. for an unspecified bind (`0.0.0.0`/`::`), the local interface the
///    kernel routes toward the peer.
///
/// **Fail-closed (CP3):** returns `None` when a configured
/// `external_media_address` is an FQDN that does NOT resolve. In that case the
/// caller MUST reject the call setup rather than advertise an unresolved FQDN
/// or fall back to a leaky internal address in `c=`/`o=`. An IP-literal external
/// address, a resolvable FQDN, or any non-external path always returns `Some`.
pub fn advertised_media_ip(
    local: std::net::SocketAddr,
    remote: std::net::SocketAddr,
) -> Option<String> {
    let (external, local_net) = match crate::pjsip_config::get_global_pjsip_config() {
        Some(cfg) => {
            // Select the transport whose bind COVERS `local` first — exact
            // ip+port, then exact ip, then a wildcard bind — and only THEN read
            // its NAT config. Selecting by bind coverage (not by "has an
            // external address") stops a same-ip/wildcard transport that
            // happens to set an external address from donating it to a covering
            // transport that set none — which, with fail-closed resolution,
            // would otherwise REJECT a normal call on the transport that has no
            // external address (codex CP3 F2). A transport bound to a DIFFERENT
            // concrete address never covers `local`. (Dialogs don't yet carry
            // their transport name; when they do, that binding should replace
            // this bind-coverage lookup.)
            let transport = cfg
                .transports
                .iter()
                .find(|t| t.bind == local)
                .or_else(|| cfg.transports.iter().find(|t| t.bind.ip() == local.ip()))
                .or_else(|| cfg.transports.iter().find(|t| t.bind.ip().is_unspecified()));
            match transport {
                Some(t) => (t.external_media_address.clone(), t.local_net.clone()),
                None => (None, Vec::new()),
            }
        }
        None => (None, Vec::new()),
    };
    advertised_media_ip_with(external.as_deref(), &local_net, local, remote)
}

/// Pick the `host:port` string to advertise in SIP Via/Contact/From toward
/// `remote` (New-3), transport-scoped by `local_net` exactly like
/// [`advertised_media_ip`].
///
/// Selection order:
/// 1. a transport's configured `external_signaling_address` (with its optional
///    `external_signaling_port`, else the bind port), unless the peer falls
///    inside that transport's `local_net` CIDRs;
/// 2. otherwise the concrete local bind `host:port`, unchanged.
///
/// A peer inside `local_net` therefore sees the internal bind address/port; an
/// external peer sees the external address AND the external port. This lets a
/// NAT/forward map an external port to a different internal bind port without
/// breaking the first in-dialog request (which is targeted by the Contact this
/// produces).
pub fn advertised_signaling_hostport(
    local: std::net::SocketAddr,
    remote: std::net::SocketAddr,
) -> String {
    let (external, external_port, local_net) = match crate::pjsip_config::get_global_pjsip_config()
    {
        Some(cfg) => {
            // Select the transport whose bind COVERS `local` first — exact
            // ip+port, then exact ip, then a wildcard bind — and only THEN read
            // its NAT config. Selecting by bind coverage (not by "has an
            // external address") stops a same-ip/wildcard transport that
            // happens to set an external address from donating it to a covering
            // transport that deliberately set none (codex CP2 F1). A transport
            // bound to a DIFFERENT concrete address never covers `local`.
            // (Transport identity is not yet carried on the dialog; when it is,
            // that binding should replace this bind-coverage lookup — protocol
            // is likewise not distinguished here because only UDP is bound.)
            let transport = cfg
                .transports
                .iter()
                .find(|t| t.bind == local)
                .or_else(|| cfg.transports.iter().find(|t| t.bind.ip() == local.ip()))
                .or_else(|| cfg.transports.iter().find(|t| t.bind.ip().is_unspecified()));
            match transport {
                Some(t) => (
                    t.external_signaling_address.clone(),
                    t.external_signaling_port,
                    t.local_net.clone(),
                ),
                None => (None, None, Vec::new()),
            }
        }
        None => (None, None, Vec::new()),
    };
    advertised_signaling_hostport_with(
        external.as_deref(),
        external_port,
        &local_net,
        local,
        remote,
    )
}

/// Testable core of [`advertised_signaling_hostport`] (config plumbed in
/// explicitly).
fn advertised_signaling_hostport_with(
    external: Option<&str>,
    external_port: Option<u16>,
    local_net: &[String],
    local: std::net::SocketAddr,
    remote: std::net::SocketAddr,
) -> String {
    // A configured external signaling address wins for peers outside local_net.
    if let Some(ext) = external.filter(|e| !e.is_empty()) {
        let peer_is_local = local_net.iter().any(|cidr| {
            crate::acl::AclRule::permit(cidr)
                .map(|rule| rule.matches(&remote.ip()))
                .unwrap_or(false)
        });
        if !peer_is_local {
            let port = external_port.unwrap_or_else(|| local.port());
            return format_hostport(ext, port);
        }
    }
    // Otherwise advertise the concrete bind host:port, unchanged.
    local.to_string()
}

/// Format `host:port` for a SIP sent-by / URI host, bracketing a bare IPv6
/// literal (`::1` -> `[::1]:5060`). An FQDN or IPv4 literal is emitted as-is.
fn format_hostport(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Testable core of [`advertised_media_ip`] (config plumbed in explicitly).
fn advertised_media_ip_with(
    external: Option<&str>,
    local_net: &[String],
    local: std::net::SocketAddr,
    remote: std::net::SocketAddr,
) -> Option<String> {
    // 1. A configured NAT address wins for peers outside local_net.
    if let Some(ext) = external.filter(|e| !e.is_empty()) {
        let peer_is_local = local_net.iter().any(|cidr| {
            crate::acl::AclRule::permit(cidr)
                .map(|rule| rule.matches(&remote.ip()))
                .unwrap_or(false)
        });
        if !peer_is_local {
            // CP3: interpret the external address. An IP literal is emitted as
            // is; an FQDN is resolved to an IP. On DNS failure we FAIL CLOSED
            // (return None) — never emit an unresolved FQDN into c=/o= and never
            // fall through to the internal/routed address below, which would
            // leak the internal topology or advertise a bogus media address.
            return resolve_external_media_addr(ext, remote.ip());
        }
    }

    // 2. A concrete bound address is advertised as-is.
    if !local.ip().is_unspecified() {
        return Some(local.ip().to_string());
    }

    // 3. Bound to INADDR_ANY: let the kernel pick the interface it would
    //    route to the peer from. connect() on a UDP socket performs route
    //    selection without sending any packet.
    let wildcard = if remote.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    if let Ok(sock) = std::net::UdpSocket::bind(wildcard) {
        if sock.connect(remote).is_ok() {
            if let Ok(resolved) = sock.local_addr() {
                if !resolved.ip().is_unspecified() {
                    return Some(resolved.ip().to_string());
                }
            }
        }
    }

    // 4. No route to the peer: keep the bind address (previous behaviour)
    //    rather than inventing one.
    Some(local.ip().to_string())
}

/// Interpret a configured `external_media_address` for SDP `c=`/`o=` (CP3).
///
/// * An IP literal (v4 or v6) is emitted verbatim.
/// * An FQDN is resolved to an IP, preferring the peer's address family so an
///   IPv4 peer is not handed an IPv6 media address (and vice-versa), falling
///   back to any resolved address.
/// * **Fail closed:** returns `None` on DNS failure (or an empty resolution set)
///   so the caller rejects the offer/answer rather than advertising an
///   unresolved FQDN or a leaky internal address.
///
/// The FQDN lookup is bounded by a hard timeout so a slow NSS/DNS resolver
/// cannot stall the serialized SIP control path indefinitely — on timeout it
/// fails closed (`None`), the safe posture. Full async resolution with TTL
/// caching is the separate M1/M-l DNS hardening; this only bounds the blocking
/// call and preserves fail-closed.
fn resolve_external_media_addr(ext: &str, remote: std::net::IpAddr) -> Option<String> {
    if let Ok(ip) = ext.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    // Resolve on a scratch thread with a bounded receive: a slow lookup times
    // out to None rather than blocking every other call on this stack.
    let host = ext.to_string();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<std::net::SocketAddr>>(1);
    std::thread::spawn(move || {
        use std::net::ToSocketAddrs;
        let resolved = (host.as_str(), 0u16)
            .to_socket_addrs()
            .map(|it| it.collect())
            .unwrap_or_default();
        let _ = tx.send(resolved);
    });
    let resolved = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .ok()?;
    let pick = resolved
        .iter()
        .find(|sa| sa.is_ipv4() == remote.is_ipv4())
        .or_else(|| resolved.first())?;
    Some(pick.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- advertised_media_ip (issue #56) --------------------------------

    fn sa(s: &str) -> std::net::SocketAddr {
        s.parse().unwrap()
    }

    /// A concrete bound address is advertised unchanged.
    #[test]
    fn test_media_ip_concrete_bind_passes_through() {
        assert_eq!(
            advertised_media_ip_with(None, &[], sa("192.0.2.10:5060"), sa("198.51.100.7:5060")),
            Some("192.0.2.10".to_string())
        );
    }

    /// INADDR_ANY must never be advertised: the routed interface toward the
    /// peer is used instead (loopback peer -> loopback source).
    #[test]
    fn test_media_ip_unspecified_resolves_routable_source() {
        let ip = advertised_media_ip_with(None, &[], sa("0.0.0.0:5060"), sa("127.0.0.1:5062"))
            .expect("non-external path always resolves Some");
        assert_ne!(ip, "0.0.0.0", "must never advertise INADDR_ANY (issue #56)");
        assert_eq!(ip, "127.0.0.1", "loopback peer routes via loopback");
    }

    /// A configured external_media_address wins for a peer outside local_net.
    #[test]
    fn test_media_ip_external_applies_to_nonlocal_peer() {
        assert_eq!(
            advertised_media_ip_with(
                Some("203.0.113.99"),
                &["10.0.0.0/8".to_string()],
                sa("0.0.0.0:5060"),
                sa("198.51.100.7:5060"),
            ),
            Some("203.0.113.99".to_string())
        );
    }

    /// A peer inside local_net bypasses the external address and gets the
    /// real local/routed one.
    #[test]
    fn test_media_ip_local_net_peer_bypasses_external() {
        assert_eq!(
            advertised_media_ip_with(
                Some("203.0.113.99"),
                &["127.0.0.0/8".to_string()],
                sa("127.0.0.1:5060"),
                sa("127.0.0.1:5062"),
            ),
            Some("127.0.0.1".to_string())
        );
    }

    /// IPv6 addresses must be typed IP6 in o=/c= lines (RFC 4566); a
    /// half-IPv6 SDP like `c=IN IP4 2001:db8::1` is invalid.
    #[test]
    fn test_sdp_addr_type_follows_family() {
        let v6 = SessionDescription::create_offer("2001:db8::1", 40000, &[codecs_pcmu()]);
        assert_eq!(v6.origin.addr_type, "IP6");
        assert_eq!(v6.connection.as_ref().unwrap().addr_type, "IP6");

        let v4 = SessionDescription::create_offer("192.0.2.1", 40000, &[codecs_pcmu()]);
        assert_eq!(v4.origin.addr_type, "IP4");
        assert_eq!(v4.connection.as_ref().unwrap().addr_type, "IP4");

        let answer =
            SessionDescription::create_answer(&v6, "2001:db8::2", 40002, &[codecs_pcmu()]);
        assert_eq!(answer.connection.as_ref().unwrap().addr_type, "IP6");
    }

    fn codecs_pcmu() -> Codec {
        asterisk_codecs::codecs::pcmu()
    }

    /// A transport bound to a DIFFERENT concrete address must not donate its
    /// external_media_address; only a covering bind (same ip, or wildcard)
    /// may.
    #[test]
    fn test_media_ip_foreign_transport_does_not_donate_external() {
        use crate::pjsip_config::{set_global_pjsip_config, PjsipConfig, TransportConfig};
        let cfg = PjsipConfig {
            transports: vec![TransportConfig {
                name: "other".to_string(),
                protocol: "udp".to_string(),
                bind: "192.0.2.50:5060".parse().unwrap(),
                external_media_address: Some("203.0.113.99".to_string()),
                external_signaling_address: None,
                external_signaling_port: None,
                cert_file: None,
                priv_key_file: None,
                local_net: vec![],
            }],
            ..Default::default()
        };
        set_global_pjsip_config(cfg);
        // Local bind 127.0.0.1 is NOT covered by the 192.0.2.50 transport:
        // its external address must not leak into this dialog's SDP.
        let ip = advertised_media_ip(sa("127.0.0.1:5060"), sa("127.0.0.1:5062"));
        assert_eq!(ip, Some("127.0.0.1".to_string()));
        // Reset the process-global config for other tests in this binary.
        set_global_pjsip_config(PjsipConfig::default());
    }

    /// codex CP3 F2: a call on a transport with NO external_media_address must
    /// get the normal (internal/routed) address — a same-ip transport whose
    /// external FQDN is unresolvable must NOT be selected and fail the call
    /// closed. Covering-transport-first selection prevents that cross-donation.
    #[test]
    fn test_media_ip_covering_transport_without_external_not_rejected() {
        use crate::pjsip_config::{set_global_pjsip_config, PjsipConfig, TransportConfig};
        let base = |bind: &str, ext: Option<&str>| TransportConfig {
            name: format!("t-{bind}"),
            protocol: "udp".to_string(),
            bind: bind.parse().unwrap(),
            external_media_address: ext.map(|s| s.to_string()),
            external_signaling_address: None,
            external_signaling_port: None,
            cert_file: None,
            priv_key_file: None,
            local_net: vec![],
        };
        let cfg = PjsipConfig {
            transports: vec![
                // The covering transport (exact ip 127.0.0.1) has NO external.
                base("127.0.0.1:5060", None),
                // A same-ip transport has an UNRESOLVABLE external FQDN.
                base("127.0.0.1:5070", Some("no-such-host.invalid")),
            ],
            ..Default::default()
        };
        set_global_pjsip_config(cfg);
        // The call is on 127.0.0.1:5060 (no external) -> must resolve to the
        // internal address, NOT fail closed by inheriting the :5070 FQDN.
        assert_eq!(
            advertised_media_ip(sa("127.0.0.1:5060"), sa("127.0.0.1:5062")),
            Some("127.0.0.1".to_string()),
            "a call on the no-external transport must not be rejected via a sibling's unresolvable FQDN"
        );
        set_global_pjsip_config(PjsipConfig::default());
    }

    // ---- external_media_address FQDN-vs-literal, fail-closed (CP3) -------

    /// An IP-literal external_media_address is emitted verbatim.
    #[test]
    fn test_media_external_ip_literal_passes_through() {
        assert_eq!(
            resolve_external_media_addr("203.0.113.99", sa("198.51.100.7:5060").ip()),
            Some("203.0.113.99".to_string())
        );
        assert_eq!(
            resolve_external_media_addr("2001:db8::1", sa("[2001:db8::9]:5060").ip()),
            Some("2001:db8::1".to_string())
        );
    }

    /// A resolvable FQDN external_media_address resolves to an IP literal
    /// (localhost is guaranteed to resolve on any host).
    #[test]
    fn test_media_external_fqdn_resolves_to_ip() {
        let resolved = resolve_external_media_addr("localhost", sa("127.0.0.1:5060").ip())
            .expect("localhost must resolve");
        let ip: std::net::IpAddr = resolved.parse().expect("resolved value must be an IP literal");
        assert!(ip.is_loopback(), "localhost must resolve to a loopback IP, got {ip}");
    }

    /// **Fail closed:** an unresolvable FQDN yields None so the caller rejects
    /// the offer/answer instead of advertising a bogus/internal address. Uses
    /// the RFC 6761 `.invalid` TLD, guaranteed never to resolve.
    #[test]
    fn test_media_external_fqdn_unresolvable_fails_closed() {
        assert_eq!(
            resolve_external_media_addr("no-such-host.invalid", sa("198.51.100.7:5060").ip()),
            None,
            "an unresolvable external_media_address FQDN must fail closed (None)"
        );
    }

    /// End-to-end through advertised_media_ip_with: an external FQDN that will
    /// not resolve, for a non-local peer, fails closed rather than falling
    /// through to the internal bind address.
    #[test]
    fn test_media_ip_with_unresolvable_external_fails_closed() {
        assert_eq!(
            advertised_media_ip_with(
                Some("no-such-host.invalid"),
                &[],
                sa("10.1.2.3:5060"),
                sa("198.51.100.7:5060"),
            ),
            None,
            "fail closed: must NOT fall back to the internal bind 10.1.2.3"
        );
    }

    // ---- advertised_signaling_hostport (New-3) --------------------------

    /// No external signaling address: the bind host:port is advertised as-is.
    #[test]
    fn test_signaling_no_external_passes_through_bind() {
        assert_eq!(
            advertised_signaling_hostport_with(None, None, &[], sa("192.0.2.10:5060"), sa("198.51.100.7:5062")),
            "192.0.2.10:5060"
        );
    }

    /// A configured external signaling address wins for a peer outside
    /// local_net; with no port override the BIND port is advertised.
    #[test]
    fn test_signaling_external_applies_to_nonlocal_peer_default_port() {
        assert_eq!(
            advertised_signaling_hostport_with(
                Some("203.0.113.99"),
                None,
                &[],
                sa("10.1.2.3:5060"),
                sa("198.51.100.7:5062"),
            ),
            "203.0.113.99:5060"
        );
    }

    /// The external signaling PORT override (New-3) replaces the bind port in
    /// the advertised host:port for an external peer — independent of the bind.
    #[test]
    fn test_signaling_external_port_override_applies() {
        assert_eq!(
            advertised_signaling_hostport_with(
                Some("203.0.113.99"),
                Some(6666),
                &[],
                sa("10.1.2.3:5060"),
                sa("198.51.100.7:5062"),
            ),
            "203.0.113.99:6666"
        );
    }

    /// A peer inside local_net bypasses the external address/port and gets the
    /// internal bind host:port — even when an external port override is set.
    #[test]
    fn test_signaling_local_net_peer_bypasses_external() {
        assert_eq!(
            advertised_signaling_hostport_with(
                Some("203.0.113.99"),
                Some(6666),
                &["10.0.0.0/8".to_string()],
                sa("10.1.2.3:5060"),
                sa("10.9.9.9:5062"),
            ),
            "10.1.2.3:5060"
        );
    }

    /// A bare IPv6 external literal is bracketed in the advertised host:port.
    #[test]
    fn test_signaling_external_ipv6_is_bracketed() {
        assert_eq!(
            advertised_signaling_hostport_with(
                Some("2001:db8::1"),
                Some(5080),
                &[],
                sa("192.0.2.10:5060"),
                sa("198.51.100.7:5062"),
            ),
            "[2001:db8::1]:5080"
        );
    }

    /// An FQDN external signaling address is emitted as-is (legal in a SIP
    /// sent-by / URI host); resolution is the peer's job.
    #[test]
    fn test_signaling_external_fqdn_passes_through() {
        assert_eq!(
            advertised_signaling_hostport_with(
                Some("pbx.example.com"),
                Some(5090),
                &[],
                sa("192.0.2.10:5060"),
                sa("198.51.100.7:5062"),
            ),
            "pbx.example.com:5090"
        );
    }

    /// End-to-end through the config lookup: a transport bound to a DIFFERENT
    /// concrete address must not donate its external signaling address.
    #[test]
    fn test_signaling_foreign_transport_does_not_donate_external() {
        use crate::pjsip_config::{set_global_pjsip_config, PjsipConfig, TransportConfig};
        let cfg = PjsipConfig {
            transports: vec![TransportConfig {
                name: "other".to_string(),
                protocol: "udp".to_string(),
                bind: "192.0.2.50:5060".parse().unwrap(),
                external_media_address: None,
                external_signaling_address: Some("203.0.113.99".to_string()),
                external_signaling_port: Some(6666),
                cert_file: None,
                priv_key_file: None,
                local_net: vec![],
            }],
            ..Default::default()
        };
        set_global_pjsip_config(cfg);
        // Local bind 127.0.0.1 is NOT covered by the 192.0.2.50 transport.
        let hp = advertised_signaling_hostport(sa("127.0.0.1:5060"), sa("198.51.100.7:5062"));
        assert_eq!(hp, "127.0.0.1:5060");
        set_global_pjsip_config(PjsipConfig::default());
    }

    /// codex CP2 F1: the transport that COVERS `local` is selected first, even
    /// when it sets no external address. A same-ip / wildcard transport that
    /// happens to configure an external address must NOT donate it to the
    /// covering transport that deliberately set none.
    #[test]
    fn test_signaling_covering_transport_without_external_wins() {
        use crate::pjsip_config::{set_global_pjsip_config, PjsipConfig, TransportConfig};
        let base = |bind: &str, ext: Option<&str>| TransportConfig {
            name: format!("t-{bind}"),
            protocol: "udp".to_string(),
            bind: bind.parse().unwrap(),
            external_media_address: None,
            external_signaling_address: ext.map(|s| s.to_string()),
            external_signaling_port: ext.map(|_| 6666),
            cert_file: None,
            priv_key_file: None,
            local_net: vec![],
        };
        let cfg = PjsipConfig {
            transports: vec![
                // The covering transport (exact ip 192.0.2.10) sets NO external.
                base("192.0.2.10:5060", None),
                // A wildcard transport DOES set an external address.
                base("0.0.0.0:5062", Some("203.0.113.99")),
            ],
            ..Default::default()
        };
        set_global_pjsip_config(cfg);
        // local is covered by the exact-ip transport (no external) -> internal
        // bind must be advertised, NOT the wildcard transport's external.
        let hp = advertised_signaling_hostport(sa("192.0.2.10:5060"), sa("198.51.100.7:5062"));
        assert_eq!(hp, "192.0.2.10:5060", "covering transport (no external) must not inherit the wildcard's external");
        set_global_pjsip_config(PjsipConfig::default());
    }

    #[test]
    fn test_parse_sdp() {
        let sdp_text = "v=0\r\n\
o=- 12345 12345 IN IP4 10.0.0.1\r\n\
s=Test\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 10000 RTP/AVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n\
a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        assert_eq!(sdp.version, 0);
        assert_eq!(sdp.origin.addr, "10.0.0.1");
        assert_eq!(sdp.media_descriptions.len(), 1);

        let media = &sdp.media_descriptions[0];
        assert_eq!(media.media_type, "audio");
        assert_eq!(media.port, 10000);
        assert_eq!(media.formats, vec![0, 8, 101]);

        let codecs = media.codecs();
        assert_eq!(codecs.len(), 3);
        assert_eq!(codecs[0].name, "PCMU");
        assert_eq!(codecs[1].name, "PCMA");
    }

    #[test]
    fn test_sdp_roundtrip() {
        let codecs = vec![
            Codec::new("PCMU", 0, 8000),
            Codec::new("PCMA", 8, 8000),
        ];
        let sdp = SessionDescription::create_offer("10.0.0.1", 20000, &codecs);
        let text = sdp.to_string();
        let parsed = SessionDescription::parse(&text).unwrap();
        assert_eq!(parsed.media_descriptions.len(), 1);
        assert_eq!(parsed.media_descriptions[0].port, 20000);
    }

    #[test]
    fn chime_rtcp_attributes_do_not_reject_offer_or_answer() {
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=Chime\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=rtcp:40001 IN IP4 10.0.0.1\r\n\
             a=rtcp-mux\r\n\
             a=sendrecv\r\n",
        )
        .expect("Chime-style SDP offer must parse");

        let offered_audio = &offer.media_descriptions[0];
        assert!(offered_audio.has_rtcp_mux());
        assert!(offered_audio.attributes.iter().any(|(name, value)| {
            name == "rtcp" && value.as_deref() == Some("40001 IN IP4 10.0.0.1")
        }));

        let answer =
            SessionDescription::create_answer(&offer, "127.0.0.1", 55555, &[codecs_pcmu()]);
        assert_eq!(answer.media_descriptions.len(), 1);
        assert_eq!(answer.media_descriptions[0].port, 55555);

        let reparsed_answer = SessionDescription::parse(&answer.to_string())
            .expect("answer to Chime-style offer must serialize and parse");
        assert_eq!(reparsed_answer.media_descriptions[0].port, 55555);
    }

    #[test]
    fn test_sdp_parse_dtls_attributes() {
        let sdp_text = "v=0\r\n\
            o=- 12345 12345 IN IP4 10.0.0.1\r\n\
            s=Test\r\n\
            c=IN IP4 10.0.0.1\r\n\
            t=0 0\r\n\
            m=audio 10000 UDP/TLS/RTP/SAVPF 111\r\n\
            a=rtpmap:111 opus/48000/2\r\n\
            a=fingerprint:sha-256 AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89\r\n\
            a=setup:actpass\r\n\
            a=rtcp-mux\r\n\
            a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        assert_eq!(sdp.media_descriptions.len(), 1);

        let media = &sdp.media_descriptions[0];
        assert_eq!(media.protocol, "UDP/TLS/RTP/SAVPF");

        // Fingerprint.
        let (alg, fp) = media.get_fingerprint().unwrap();
        assert_eq!(alg, FingerprintAlgorithm::Sha256);
        assert_eq!(
            fp,
            "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89"
        );

        // Setup role.
        assert_eq!(media.get_setup(), Some(DtlsRole::ActPass));

        // rtcp-mux.
        assert!(media.has_rtcp_mux());
    }

    #[test]
    fn test_sdp_dtls_roundtrip() {
        let fp = "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";
        let media = MediaDescription::new_audio_dtls(
            12000,
            FingerprintAlgorithm::Sha256,
            fp,
            DtlsRole::ActPass,
        );

        assert_eq!(media.protocol, "UDP/TLS/RTP/SAVPF");
        assert!(media.rtcp_mux);
        assert_eq!(media.setup, Some(DtlsRole::ActPass));

        // Build an SDP with this media.
        let sdp = SessionDescription {
            version: 0,
            origin: Origin::default(),
            session_name: "Test".to_string(),
            connection: Some(ConnectionData::default()),
            time: (0, 0),
            media_descriptions: vec![media],
            attributes: Vec::new(),
        };

        let text = sdp.to_string();
        let parsed = SessionDescription::parse(&text).unwrap();
        let pm = &parsed.media_descriptions[0];

        let (alg, parsed_fp) = pm.get_fingerprint().unwrap();
        assert_eq!(alg, FingerprintAlgorithm::Sha256);
        assert_eq!(parsed_fp, fp);
        assert_eq!(pm.get_setup(), Some(DtlsRole::ActPass));
        assert!(pm.has_rtcp_mux());
    }

    #[test]
    fn test_sdp_parse_no_dtls() {
        let sdp_text = "v=0\r\n\
            o=- 1 1 IN IP4 10.0.0.1\r\n\
            s=-\r\n\
            t=0 0\r\n\
            m=audio 5000 RTP/AVP 0\r\n\
            a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        let media = &sdp.media_descriptions[0];

        assert!(media.get_fingerprint().is_none());
        assert!(media.get_setup().is_none());
        assert!(!media.has_rtcp_mux());
    }

    #[test]
    fn test_sdp_parse_fingerprint_sha1() {
        let sdp_text = "v=0\r\n\
            o=- 1 1 IN IP4 10.0.0.1\r\n\
            s=-\r\n\
            t=0 0\r\n\
            m=audio 5000 RTP/AVP 0\r\n\
            a=fingerprint:sha-1 AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        let media = &sdp.media_descriptions[0];
        let (alg, _fp) = media.get_fingerprint().unwrap();
        assert_eq!(alg, FingerprintAlgorithm::Sha1);
    }

    // -----------------------------------------------------------------------
    // Bandwidth tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_bandwidth_parse_as() {
        let bw = Bandwidth::parse("AS:512").unwrap();
        assert_eq!(bw, Bandwidth::ApplicationSpecific(512));
        assert_eq!(bw.as_bps(), 512_000);
    }

    #[test]
    fn test_bandwidth_parse_tias() {
        let bw = Bandwidth::parse("TIAS:512000").unwrap();
        assert_eq!(bw, Bandwidth::TransportIndependent(512_000));
        assert_eq!(bw.as_bps(), 512_000);
    }

    #[test]
    fn test_bandwidth_parse_ct() {
        let bw = Bandwidth::parse("CT:1024").unwrap();
        assert_eq!(bw, Bandwidth::ConferenceTotal(1024));
        assert_eq!(bw.as_bps(), 1_024_000);
    }

    #[test]
    fn test_bandwidth_parse_unknown() {
        assert!(Bandwidth::parse("XX:100").is_none());
    }

    #[test]
    fn test_bandwidth_display() {
        assert_eq!(Bandwidth::ApplicationSpecific(512).to_string(), "AS:512");
        assert_eq!(Bandwidth::TransportIndependent(512000).to_string(), "TIAS:512000");
        assert_eq!(Bandwidth::ConferenceTotal(1024).to_string(), "CT:1024");
    }

    #[test]
    fn test_sdp_parse_bandwidth() {
        let sdp_text = "v=0\r\n\
            o=- 1 1 IN IP4 10.0.0.1\r\n\
            s=Test\r\n\
            c=IN IP4 10.0.0.1\r\n\
            t=0 0\r\n\
            m=audio 10000 RTP/AVP 0\r\n\
            b=AS:512\r\n\
            b=TIAS:512000\r\n\
            a=rtpmap:0 PCMU/8000\r\n\
            a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        let media = &sdp.media_descriptions[0];
        assert_eq!(media.bandwidth.len(), 2);
        assert_eq!(media.bandwidth[0], Bandwidth::ApplicationSpecific(512));
        assert_eq!(media.bandwidth[1], Bandwidth::TransportIndependent(512000));
    }

    #[test]
    fn test_sdp_bandwidth_roundtrip() {
        let sdp_text = "v=0\r\n\
            o=- 1 1 IN IP4 10.0.0.1\r\n\
            s=Test\r\n\
            c=IN IP4 10.0.0.1\r\n\
            t=0 0\r\n\
            m=audio 10000 RTP/AVP 0\r\n\
            b=AS:256\r\n\
            a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        let text = sdp.to_string();
        assert!(text.contains("b=AS:256"));

        let reparsed = SessionDescription::parse(&text).unwrap();
        assert_eq!(reparsed.media_descriptions[0].bandwidth.len(), 1);
        assert_eq!(
            reparsed.media_descriptions[0].bandwidth[0],
            Bandwidth::ApplicationSpecific(256)
        );
    }

    // -----------------------------------------------------------------------
    // ICE SDP tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sdp_parse_ice_attributes() {
        let sdp_text = "v=0\r\n\
o=- 12345 12345 IN IP4 10.0.0.1\r\n\
s=Test\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
a=ice-ufrag:abcd\r\n\
a=ice-pwd:aabbccddeeffgghhiijjkk\r\n\
a=ice-options:trickle\r\n\
m=audio 10000 RTP/AVP 0\r\n\
a=candidate:H192.168.1.11 1 UDP 2130706431 192.168.1.1 5000 typ host\r\n\
a=candidate:S203.0.113.501 1 UDP 1694498815 203.0.113.50 12345 typ srflx raddr 192.168.1.1 rport 5000\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=sendrecv\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();

        // Session-level ICE attributes
        assert_eq!(sdp.ice_ufrag(), Some("abcd"));
        assert_eq!(sdp.ice_pwd(), Some("aabbccddeeffgghhiijjkk"));

        let options = sdp.ice_options().unwrap();
        assert!(options.trickle);
        assert!(!options.renomination);

        // Media-level candidates
        let media = &sdp.media_descriptions[0];
        assert_eq!(media.ice_candidates.len(), 2);
        assert_eq!(media.ice_candidates[0].candidate_type, crate::ice::CandidateType::Host);
        assert_eq!(media.ice_candidates[1].candidate_type, crate::ice::CandidateType::ServerReflexive);
        assert_eq!(
            media.ice_candidates[1].related_address,
            Some("192.168.1.1".parse().unwrap())
        );
        assert_eq!(media.ice_candidates[1].related_port, Some(5000));
    }

    #[test]
    fn test_sdp_ice_lite() {
        let sdp_text = "v=0\r\n\
o=- 1 1 IN IP4 10.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-lite\r\n\
a=ice-ufrag:xyz\r\n\
a=ice-pwd:longpasswordstringhere!!\r\n\
m=audio 5000 RTP/AVP 0\r\n\
a=candidate:H10.0.0.11 1 UDP 2130706431 10.0.0.1 5000 typ host\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();
        assert!(sdp.is_ice_lite());
        assert_eq!(sdp.ice_ufrag(), Some("xyz"));
    }

    #[test]
    fn test_sdp_set_ice_credentials() {
        let mut sdp = SessionDescription::default();
        sdp.set_ice_credentials("myufrag", "myverylongpassword1234");

        assert_eq!(sdp.ice_ufrag(), Some("myufrag"));
        assert_eq!(sdp.ice_pwd(), Some("myverylongpassword1234"));

        // Setting again should replace
        sdp.set_ice_credentials("newufrag", "newpassword123456789012");
        assert_eq!(sdp.ice_ufrag(), Some("newufrag"));
    }

    #[test]
    fn test_sdp_add_ice_candidates() {
        let codecs = vec![Codec::new("PCMU", 0, 8000)];
        let mut sdp = SessionDescription::create_offer("10.0.0.1", 5000, &codecs);
        sdp.set_ice_credentials("uf1", "pw12345678901234567890");

        let candidates = vec![
            IceCandidate::new_host("10.0.0.1:5000".parse().unwrap(), 1, 65535),
        ];
        sdp.add_ice_candidates_to_media(0, &candidates);

        // Verify the candidate is in both places
        assert_eq!(sdp.media_descriptions[0].ice_candidates.len(), 1);

        // Roundtrip through text
        let text = sdp.to_string();
        assert!(text.contains("a=ice-ufrag:uf1"));
        assert!(text.contains("a=ice-pwd:pw12345678901234567890"));
        assert!(text.contains("a=candidate:"));
        assert!(text.contains("typ host"));

        let parsed = SessionDescription::parse(&text).unwrap();
        assert_eq!(parsed.ice_ufrag(), Some("uf1"));
        assert_eq!(parsed.media_descriptions[0].ice_candidates.len(), 1);
        assert_eq!(
            parsed.media_descriptions[0].ice_candidates[0].candidate_type,
            crate::ice::CandidateType::Host
        );
    }

    #[test]
    fn test_sdp_media_ice_credentials_fallback() {
        let sdp_text = "v=0\r\n\
o=- 1 1 IN IP4 10.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-ufrag:session_ufrag\r\n\
a=ice-pwd:session_password_long_enough\r\n\
m=audio 5000 RTP/AVP 0\r\n\
a=ice-ufrag:media_ufrag\r\n";

        let sdp = SessionDescription::parse(sdp_text).unwrap();

        // Media has its own ufrag
        assert_eq!(sdp.media_ice_ufrag(0), Some("media_ufrag"));
        // But no media-level pwd, so falls back to session
        assert_eq!(sdp.media_ice_pwd(0), Some("session_password_long_enough"));
    }

    // --- issue #31: multi-m-line answers must not share the audio port -----

    fn supported_audio_and_video() -> Vec<Codec> {
        vec![Codec::new("PCMU", 0, 8000), Codec::new("VP8", 96, 90000)]
    }

    #[test]
    fn answer_gives_only_first_audio_the_port_and_rejects_video() {
        // A video-capable phone offers audio + video; both share a codec with
        // us, but we bind one transport. The answer must put the real port on
        // audio and reject video with port 0, so the caller never sends video
        // RTP into the audio socket (issue #31).
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             m=video 40002 RTP/AVP 96\r\n\
             a=rtpmap:96 VP8/90000\r\n",
        )
        .unwrap();

        let answer =
            SessionDescription::create_answer(&offer, "127.0.0.1", 55555, &supported_audio_and_video());

        // Same number of m-lines, same order (RFC 3264 §6).
        assert_eq!(answer.media_descriptions.len(), 2);
        assert_eq!(answer.media_descriptions[0].media_type, "audio");
        assert_eq!(answer.media_descriptions[1].media_type, "video");
        // Audio accepted on the real port; video rejected with port 0.
        assert_eq!(answer.media_descriptions[0].port, 55555);
        assert_eq!(answer.media_descriptions[1].port, 0);
    }

    #[test]
    fn answer_rejects_a_second_audio_stream() {
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             m=audio 40002 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .unwrap();

        let answer =
            SessionDescription::create_answer(&offer, "127.0.0.1", 55555, &supported_audio_and_video());

        assert_eq!(answer.media_descriptions.len(), 2);
        assert_eq!(answer.media_descriptions[0].port, 55555, "first audio accepted");
        assert_eq!(answer.media_descriptions[1].port, 0, "second audio rejected");
    }

    #[test]
    fn answer_puts_port_on_audio_even_when_video_is_first() {
        // The bound transport is for audio, so the real port must land on the
        // audio m-line regardless of offer order; the leading video is
        // rejected.
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=video 40002 RTP/AVP 96\r\n\
             a=rtpmap:96 VP8/90000\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .unwrap();

        let answer =
            SessionDescription::create_answer(&offer, "127.0.0.1", 55555, &supported_audio_and_video());

        assert_eq!(answer.media_descriptions[0].media_type, "video");
        assert_eq!(answer.media_descriptions[0].port, 0, "leading video rejected");
        assert_eq!(answer.media_descriptions[1].media_type, "audio");
        assert_eq!(answer.media_descriptions[1].port, 55555, "audio gets the port");
    }

    #[test]
    fn answer_still_accepts_a_single_audio_stream() {
        // Regression guard: the common single-audio case is unchanged.
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0 8\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=rtpmap:8 PCMA/8000\r\n",
        )
        .unwrap();

        let answer =
            SessionDescription::create_answer(&offer, "127.0.0.1", 55555, &supported_audio_and_video());

        assert_eq!(answer.media_descriptions.len(), 1);
        assert_eq!(answer.media_descriptions[0].port, 55555);
    }

    /// Build a single-audio (PCMU) offer with the given media-level direction
    /// attribute (`None` = omit it), connection `10.0.0.1`.
    fn audio_offer_with_direction(dir: Option<&str>) -> SessionDescription {
        let mut sdp = String::from(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 10.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        if let Some(d) = dir {
            sdp.push_str(&format!("a={d}\r\n"));
        }
        SessionDescription::parse(&sdp).unwrap()
    }

    fn answered_audio_direction(offer: &SessionDescription) -> MediaDirection {
        let answer =
            SessionDescription::create_answer(offer, "127.0.0.1", 55555, &[codecs_pcmu()]);
        let audio = answer
            .media_descriptions
            .iter()
            .find(|m| m.media_type == "audio")
            .expect("audio accepted");
        assert_eq!(audio.port, 55555, "audio stream must be accepted");
        // The wire form must also carry the matching a= attribute (Display
        // renders from `attributes`, not the `direction` field), and it must
        // round-trip back to the same direction on re-parse.
        let reparsed = SessionDescription::parse(&answer.to_string()).unwrap();
        let reparsed_dir = reparsed
            .media_descriptions
            .iter()
            .find(|m| m.media_type == "audio")
            .unwrap()
            .direction;
        assert_eq!(
            reparsed_dir, audio.direction,
            "answer direction must survive serialization round-trip"
        );
        audio.direction
    }

    #[test]
    fn answer_direction_follows_offer_rfc3264_6_1() {
        // RFC 3264 §6.1: the answer direction is derived from the offer.
        assert_eq!(
            answered_audio_direction(&audio_offer_with_direction(Some("sendrecv"))),
            MediaDirection::SendRecv,
        );
        assert_eq!(
            answered_audio_direction(&audio_offer_with_direction(None)),
            MediaDirection::SendRecv,
            "no direction attribute defaults to sendrecv -> sendrecv"
        );
        // The hold case the M5 review flagged: sendonly MUST NOT be sendrecv.
        assert_eq!(
            answered_audio_direction(&audio_offer_with_direction(Some("sendonly"))),
            MediaDirection::RecvOnly,
        );
        assert_eq!(
            answered_audio_direction(&audio_offer_with_direction(Some("recvonly"))),
            MediaDirection::SendOnly,
        );
        assert_eq!(
            answered_audio_direction(&audio_offer_with_direction(Some("inactive"))),
            MediaDirection::Inactive,
        );
    }

    #[test]
    fn answer_direction_treats_zeroed_connection_as_hold() {
        // RFC 2543 legacy hold: c=0.0.0.0 with no direction attribute is a
        // sendonly hold, so the answer must be recvonly (never sendrecv).
        let offer = SessionDescription::parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 10.0.0.1\r\n\
             s=x\r\n\
             c=IN IP4 0.0.0.0\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .unwrap();
        assert_eq!(answered_audio_direction(&offer), MediaDirection::RecvOnly);
    }
}
