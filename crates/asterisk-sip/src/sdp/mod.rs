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
                        match attr_name.as_str() {
                            "fingerprint" => {
                                // Session-level fingerprint applies to all media.
                            }
                            _ => {}
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

        SessionDescription {
            version: 0,
            origin: Origin {
                username: "-".to_string(),
                session_id: session_id.clone(),
                session_version: session_id,
                net_type: "IN".to_string(),
                addr_type: "IP4".to_string(),
                addr: addr.to_string(),
            },
            session_name: "Asterisk".to_string(),
            connection: Some(ConnectionData {
                net_type: "IN".to_string(),
                addr_type: "IP4".to_string(),
                addr: addr.to_string(),
            }),
            time: (0, 0),
            media_descriptions: vec![media],
            attributes: Vec::new(),
        }
    }

    /// Create an SDP answer from an offer.
    pub fn create_answer(
        offer: &SessionDescription,
        addr: &str,
        port: u16,
        supported_codecs: &[Codec],
    ) -> Self {
        let mut answer = Self::create_offer(addr, port, &[]);

        // For each media in the offer, find common codecs
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

            if common.is_empty() {
                // No common codecs; set port to 0 (reject)
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
                attributes.push(("sendrecv".to_string(), None));

                answer.media_descriptions.push(MediaDescription {
                    media_type: offer_media.media_type.clone(),
                    port,
                    protocol: offer_media.protocol.clone(),
                    formats,
                    connection: None,
                    attributes,
                    direction: MediaDirection::SendRecv,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
