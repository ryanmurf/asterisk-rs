//! T.38 fax over SIP.
//!
//! Port of `res/res_pjsip_t38.c`. Handles the T.38 fax-over-IP protocol
//! parameters, SDP attribute parsing, and re-INVITE signaling needed to
//! switch a call from audio to T.38 fax mode.

use std::fmt;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum T38Error {
    #[error("invalid T.38 parameter: {0}")]
    InvalidParameter(String),
    #[error("T.38 negotiation failed: {0}")]
    NegotiationFailed(String),
    #[error("T.38 SDP parse error: {0}")]
    SdpParseError(String),
}

pub type T38Result<T> = Result<T, T38Error>;

// ---------------------------------------------------------------------------
// Rate management
// ---------------------------------------------------------------------------

/// T.38 rate management mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T38RateManagement {
    /// transferredTCF - the TCF signal is passed through.
    TransferredTcf,
    /// localTCF - TCF is generated locally.
    LocalTcf,
}

impl T38RateManagement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TransferredTcf => "transferredTCF",
            Self::LocalTcf => "localTCF",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "transferredtcf" => Some(Self::TransferredTcf),
            "localtcf" => Some(Self::LocalTcf),
            _ => None,
        }
    }
}

impl fmt::Display for T38RateManagement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// T.38 state
// ---------------------------------------------------------------------------

/// State of T.38 negotiation on a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T38State {
    /// Not using T.38 (normal audio).
    Disabled,
    /// Local side has proposed T.38 (re-INVITE sent).
    LocalReinvite,
    /// Remote side has proposed T.38 (re-INVITE received).
    RemoteReinvite,
    /// T.38 negotiation complete, fax in progress.
    Enabled,
    /// T.38 has been rejected.
    Rejected,
}

impl T38State {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::LocalReinvite => "local-reinvite",
            Self::RemoteReinvite => "remote-reinvite",
            Self::Enabled => "enabled",
            Self::Rejected => "rejected",
        }
    }
}

impl fmt::Display for T38State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// T.38 parameters
// ---------------------------------------------------------------------------

/// T.38 session parameters negotiated in SDP.
///
/// These correspond to the SDP attributes defined in ITU-T T.38 and
/// RFC 3362 (SDP for T.38).
#[derive(Debug, Clone)]
pub struct T38Parameters {
    /// T38FaxVersion (0-based version number).
    pub version: u32,
    /// T38MaxBitRate (bits per second, typically 2400-33600).
    pub max_bitrate: u32,
    /// T38FaxRateManagement.
    pub rate_management: T38RateManagement,
    /// T38FaxMaxDatagram (maximum UDPTL datagram size).
    pub max_datagram: u32,
    /// T38FaxMaxIFP (maximum IFP packet size).
    pub max_ifp: u32,
    /// T38FaxFillBitRemoval.
    pub fill_bit_removal: bool,
    /// T38FaxTranscodingMMR.
    pub transcoding_mmr: bool,
    /// T38FaxTranscodingJBIG.
    pub transcoding_jbig: bool,
    /// T38FaxUdpEC (error correction method).
    pub udp_ec: T38UdpEc,
}

impl Default for T38Parameters {
    fn default() -> Self {
        Self {
            version: 0,
            max_bitrate: 14400,
            rate_management: T38RateManagement::TransferredTcf,
            max_datagram: 400,
            max_ifp: 400,
            fill_bit_removal: false,
            transcoding_mmr: false,
            transcoding_jbig: false,
            udp_ec: T38UdpEc::Redundancy,
        }
    }
}

impl T38Parameters {
    /// Negotiate local and remote parameters, producing the effective result.
    ///
    /// Takes the minimum/most conservative value for each parameter.
    pub fn negotiate(&self, remote: &T38Parameters) -> T38Parameters {
        T38Parameters {
            version: self.version.min(remote.version),
            max_bitrate: self.max_bitrate.min(remote.max_bitrate),
            rate_management: if self.rate_management == remote.rate_management {
                self.rate_management
            } else {
                T38RateManagement::TransferredTcf
            },
            max_datagram: self.max_datagram.min(remote.max_datagram),
            max_ifp: self.max_ifp.min(remote.max_ifp),
            fill_bit_removal: self.fill_bit_removal && remote.fill_bit_removal,
            transcoding_mmr: self.transcoding_mmr && remote.transcoding_mmr,
            transcoding_jbig: self.transcoding_jbig && remote.transcoding_jbig,
            udp_ec: if self.udp_ec == remote.udp_ec {
                self.udp_ec
            } else {
                T38UdpEc::Redundancy
            },
        }
    }

    /// Generate SDP attributes for these T.38 parameters.
    pub fn to_sdp_attributes(&self) -> Vec<String> {
        let mut attrs = Vec::new();
        attrs.push(format!("T38FaxVersion:{}", self.version));
        attrs.push(format!("T38MaxBitRate:{}", self.max_bitrate));
        attrs.push(format!(
            "T38FaxRateManagement:{}",
            self.rate_management.as_str()
        ));
        attrs.push(format!("T38FaxMaxDatagram:{}", self.max_datagram));
        attrs.push(format!("T38FaxMaxIFP:{}", self.max_ifp));
        if self.fill_bit_removal {
            attrs.push("T38FaxFillBitRemoval".to_string());
        }
        if self.transcoding_mmr {
            attrs.push("T38FaxTranscodingMMR".to_string());
        }
        if self.transcoding_jbig {
            attrs.push("T38FaxTranscodingJBIG".to_string());
        }
        attrs.push(format!("T38FaxUdpEC:{}", self.udp_ec.as_str()));
        attrs
    }

    /// Parse a single SDP attribute line and update parameters.
    pub fn parse_sdp_attribute(&mut self, attr: &str) -> T38Result<()> {
        let attr = attr.trim();

        if let Some(val) = attr.strip_prefix("T38FaxVersion:") {
            self.version = val.trim().parse().map_err(|_| {
                T38Error::SdpParseError(format!("invalid T38FaxVersion: {}", val))
            })?;
        } else if let Some(val) = attr.strip_prefix("T38MaxBitRate:") {
            self.max_bitrate = val.trim().parse().map_err(|_| {
                T38Error::SdpParseError(format!("invalid T38MaxBitRate: {}", val))
            })?;
        } else if let Some(val) = attr.strip_prefix("T38FaxRateManagement:") {
            self.rate_management =
                T38RateManagement::from_str_value(val.trim()).ok_or_else(|| {
                    T38Error::SdpParseError(format!("invalid T38FaxRateManagement: {}", val))
                })?;
        } else if let Some(val) = attr.strip_prefix("T38FaxMaxDatagram:") {
            self.max_datagram = val.trim().parse().map_err(|_| {
                T38Error::SdpParseError(format!("invalid T38FaxMaxDatagram: {}", val))
            })?;
        } else if let Some(val) = attr.strip_prefix("T38FaxMaxIFP:") {
            self.max_ifp = val.trim().parse().map_err(|_| {
                T38Error::SdpParseError(format!("invalid T38FaxMaxIFP: {}", val))
            })?;
        } else if attr == "T38FaxFillBitRemoval" {
            self.fill_bit_removal = true;
        } else if attr == "T38FaxTranscodingMMR" {
            self.transcoding_mmr = true;
        } else if attr == "T38FaxTranscodingJBIG" {
            self.transcoding_jbig = true;
        } else if let Some(val) = attr.strip_prefix("T38FaxUdpEC:") {
            self.udp_ec = T38UdpEc::from_str_value(val.trim()).ok_or_else(|| {
                T38Error::SdpParseError(format!("invalid T38FaxUdpEC: {}", val))
            })?;
        }
        // Unknown attributes are silently ignored.

        Ok(())
    }

    /// Parse a complete set of T.38 SDP attribute lines.
    pub fn from_sdp_attributes(attrs: &[&str]) -> T38Result<Self> {
        let mut params = T38Parameters::default();
        for attr in attrs {
            params.parse_sdp_attribute(attr)?;
        }
        Ok(params)
    }
}

// ---------------------------------------------------------------------------
// UDP error correction
// ---------------------------------------------------------------------------

/// T.38 UDP error correction method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T38UdpEc {
    /// t38UDPRedundancy.
    Redundancy,
    /// t38UDPFEC.
    Fec,
    /// No error correction.
    None,
}

impl T38UdpEc {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Redundancy => "t38UDPRedundancy",
            Self::Fec => "t38UDPFEC",
            Self::None => "t38UDPNoEC",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "t38udpredundancy" => Some(Self::Redundancy),
            "t38udpfec" => Some(Self::Fec),
            "t38udpnoec" | "none" => Some(Self::None),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Re-INVITE builder
// ---------------------------------------------------------------------------

/// Describes a T.38 re-INVITE to switch from audio to fax mode.
#[derive(Debug, Clone)]
pub struct T38ReinviteRequest {
    /// The T.38 parameters for the offer.
    pub params: T38Parameters,
    /// The SDP media line port.
    pub port: u16,
    /// Whether this is a switchover (true) or reversion to audio (false).
    pub switch_to_t38: bool,
}

impl T38ReinviteRequest {
    /// Build a T.38 switchover re-INVITE.
    pub fn switchover(params: T38Parameters, port: u16) -> Self {
        Self {
            params,
            port,
            switch_to_t38: true,
        }
    }

    /// Build a revert-to-audio re-INVITE.
    pub fn revert_to_audio(port: u16) -> Self {
        Self {
            params: T38Parameters::default(),
            port,
            switch_to_t38: false,
        }
    }

    /// Generate the SDP media description for this re-INVITE.
    pub fn to_sdp_media(&self) -> String {
        if self.switch_to_t38 {
            let mut lines = vec![format!("m=image {} udptl t38", self.port)];
            for attr in self.params.to_sdp_attributes() {
                lines.push(format!("a={}", attr));
            }
            lines.join("\r\n")
        } else {
            format!("m=audio {} RTP/AVP 0", self.port)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_parameters() {
        let params = T38Parameters::default();
        assert_eq!(params.version, 0);
        assert_eq!(params.max_bitrate, 14400);
        assert_eq!(params.rate_management, T38RateManagement::TransferredTcf);
        assert!(!params.fill_bit_removal);
    }

    #[test]
    fn test_parse_sdp_attributes() {
        let attrs = vec![
            "T38FaxVersion:0",
            "T38MaxBitRate:14400",
            "T38FaxRateManagement:transferredTCF",
            "T38FaxMaxDatagram:400",
            "T38FaxMaxIFP:400",
            "T38FaxFillBitRemoval",
            "T38FaxUdpEC:t38UDPRedundancy",
        ];
        let params = T38Parameters::from_sdp_attributes(&attrs).unwrap();
        assert_eq!(params.version, 0);
        assert_eq!(params.max_bitrate, 14400);
        assert!(params.fill_bit_removal);
        assert_eq!(params.udp_ec, T38UdpEc::Redundancy);
    }

    #[test]
    fn test_to_sdp_attributes_roundtrip() {
        let mut params = T38Parameters::default();
        params.fill_bit_removal = true;
        params.transcoding_mmr = true;

        let attrs = params.to_sdp_attributes();
        assert!(attrs.iter().any(|a| a == "T38FaxFillBitRemoval"));
        assert!(attrs.iter().any(|a| a == "T38FaxTranscodingMMR"));
        assert!(attrs.iter().any(|a| a.starts_with("T38FaxVersion:")));
    }

    #[test]
    fn test_negotiate() {
        let local = T38Parameters {
            max_bitrate: 14400,
            max_datagram: 400,
            fill_bit_removal: true,
            ..Default::default()
        };
        let remote = T38Parameters {
            max_bitrate: 9600,
            max_datagram: 200,
            fill_bit_removal: false,
            ..Default::default()
        };

        let result = local.negotiate(&remote);
        assert_eq!(result.max_bitrate, 9600);
        assert_eq!(result.max_datagram, 200);
        assert!(!result.fill_bit_removal); // AND logic
    }

    #[test]
    fn test_reinvite_sdp() {
        let req = T38ReinviteRequest::switchover(T38Parameters::default(), 5004);
        let sdp = req.to_sdp_media();
        assert!(sdp.contains("m=image 5004 udptl t38"));
        assert!(sdp.contains("a=T38FaxVersion:0"));
    }

    #[test]
    fn test_revert_to_audio() {
        let req = T38ReinviteRequest::revert_to_audio(5004);
        let sdp = req.to_sdp_media();
        assert!(sdp.contains("m=audio 5004 RTP/AVP 0"));
    }

    #[test]
    fn test_rate_management_parse() {
        assert_eq!(
            T38RateManagement::from_str_value("transferredTCF"),
            Some(T38RateManagement::TransferredTcf)
        );
        assert_eq!(
            T38RateManagement::from_str_value("localTCF"),
            Some(T38RateManagement::LocalTcf)
        );
        assert_eq!(T38RateManagement::from_str_value("invalid"), None);
    }

    #[test]
    fn test_t38_state() {
        assert_eq!(T38State::Disabled.as_str(), "disabled");
        assert_eq!(T38State::Enabled.as_str(), "enabled");
    }
}
