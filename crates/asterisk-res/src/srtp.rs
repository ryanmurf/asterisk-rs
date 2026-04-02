//! SRTP (Secure RTP) support.
//!
//! Port of `res/res_srtp.c`. Provides SRTP encryption/decryption for RTP
//! packets as specified in RFC 3711.
//!
//! The actual AES cryptographic operations are stubbed (would use a crate
//! like `aes` or bind to libsrtp2 in production). The interfaces, key
//! derivation structure, and SDES parsing are fully defined.

use std::fmt;

use bytes::{Bytes, BytesMut, BufMut};
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SrtpError {
    #[error("SRTP initialization failed: {0}")]
    Init(String),
    #[error("SRTP protection failed: {0}")]
    Protect(String),
    #[error("SRTP unprotection failed: {0}")]
    Unprotect(String),
    #[error("Invalid SRTP key: {0}")]
    InvalidKey(String),
    #[error("Unsupported cipher suite: {0}")]
    UnsupportedSuite(String),
    #[error("SRTP replay check failed")]
    ReplayDetected,
}

pub type SrtpResult<T> = Result<T, SrtpError>;

// ---------------------------------------------------------------------------
// Cipher suites
// ---------------------------------------------------------------------------

/// SRTP cipher suite (from RFC 4568 / SDES).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(Default)]
pub enum SrtpSuite {
    /// AES_CM_128_HMAC_SHA1_80 (default).
    #[default]
    AesCm128HmacSha1_80,
    /// AES_CM_128_HMAC_SHA1_32 (reduced auth tag).
    AesCm128HmacSha1_32,
    /// AES_256_CM_HMAC_SHA1_80.
    AesCm256HmacSha1_80,
    /// AES_256_CM_HMAC_SHA1_32.
    AesCm256HmacSha1_32,
    /// AEAD_AES_128_GCM.
    AeadAes128Gcm,
    /// AEAD_AES_256_GCM.
    AeadAes256Gcm,
}

impl SrtpSuite {
    /// Master key length in bytes for this suite.
    pub fn key_len(&self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 | Self::AesCm128HmacSha1_32 | Self::AeadAes128Gcm => 16,
            Self::AesCm256HmacSha1_80 | Self::AesCm256HmacSha1_32 | Self::AeadAes256Gcm => 32,
        }
    }

    /// Master salt length in bytes.
    pub fn salt_len(&self) -> usize {
        match self {
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => 12,
            _ => 14,
        }
    }

    /// Authentication tag length in bytes.
    pub fn auth_tag_len(&self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 | Self::AesCm256HmacSha1_80 => 10,
            Self::AesCm128HmacSha1_32 | Self::AesCm256HmacSha1_32 => 4,
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => 16,
        }
    }

    /// Parse from SDP crypto attribute suite name.
    pub fn from_sdp_name(name: &str) -> Option<Self> {
        match name {
            "AES_CM_128_HMAC_SHA1_80" => Some(Self::AesCm128HmacSha1_80),
            "AES_CM_128_HMAC_SHA1_32" => Some(Self::AesCm128HmacSha1_32),
            "AES_256_CM_HMAC_SHA1_80" => Some(Self::AesCm256HmacSha1_80),
            "AES_256_CM_HMAC_SHA1_32" => Some(Self::AesCm256HmacSha1_32),
            "AEAD_AES_128_GCM" => Some(Self::AeadAes128Gcm),
            "AEAD_AES_256_GCM" => Some(Self::AeadAes256Gcm),
            _ => None,
        }
    }

    /// Return the SDP crypto attribute suite name.
    pub fn sdp_name(&self) -> &'static str {
        match self {
            Self::AesCm128HmacSha1_80 => "AES_CM_128_HMAC_SHA1_80",
            Self::AesCm128HmacSha1_32 => "AES_CM_128_HMAC_SHA1_32",
            Self::AesCm256HmacSha1_80 => "AES_256_CM_HMAC_SHA1_80",
            Self::AesCm256HmacSha1_32 => "AES_256_CM_HMAC_SHA1_32",
            Self::AeadAes128Gcm => "AEAD_AES_128_GCM",
            Self::AeadAes256Gcm => "AEAD_AES_256_GCM",
        }
    }
}


// ---------------------------------------------------------------------------
// SRTP policy trait
// ---------------------------------------------------------------------------

/// Encryption policy configuration for an SRTP session.
pub trait SrtpPolicy: fmt::Debug + Send + Sync {
    /// Get the cipher suite.
    fn suite(&self) -> SrtpSuite;

    /// Get the master key.
    fn master_key(&self) -> &[u8];

    /// Get the master salt.
    fn master_salt(&self) -> &[u8];

    /// SSRC for this policy (0 = any).
    fn ssrc(&self) -> u32;

    /// Whether this is for inbound or outbound.
    fn is_inbound(&self) -> bool;
}

/// Default SRTP policy implementation.
#[derive(Debug, Clone)]
pub struct DefaultSrtpPolicy {
    pub suite: SrtpSuite,
    pub key: Vec<u8>,
    pub salt: Vec<u8>,
    pub ssrc: u32,
    pub inbound: bool,
}

impl DefaultSrtpPolicy {
    pub fn new(suite: SrtpSuite, key: Vec<u8>, salt: Vec<u8>) -> SrtpResult<Self> {
        if key.len() != suite.key_len() {
            return Err(SrtpError::InvalidKey(format!(
                "Key length {} != expected {} for {:?}",
                key.len(),
                suite.key_len(),
                suite
            )));
        }
        if salt.len() != suite.salt_len() {
            return Err(SrtpError::InvalidKey(format!(
                "Salt length {} != expected {} for {:?}",
                salt.len(),
                suite.salt_len(),
                suite
            )));
        }
        Ok(Self {
            suite,
            key,
            salt,
            ssrc: 0,
            inbound: false,
        })
    }

    pub fn with_ssrc(mut self, ssrc: u32) -> Self {
        self.ssrc = ssrc;
        self
    }

    pub fn with_direction(mut self, inbound: bool) -> Self {
        self.inbound = inbound;
        self
    }
}

impl SrtpPolicy for DefaultSrtpPolicy {
    fn suite(&self) -> SrtpSuite {
        self.suite
    }

    fn master_key(&self) -> &[u8] {
        &self.key
    }

    fn master_salt(&self) -> &[u8] {
        &self.salt
    }

    fn ssrc(&self) -> u32 {
        self.ssrc
    }

    fn is_inbound(&self) -> bool {
        self.inbound
    }
}

// ---------------------------------------------------------------------------
// SDES key exchange
// ---------------------------------------------------------------------------

/// Parsed SDES crypto attribute from SDP (`a=crypto:...`).
#[derive(Debug, Clone)]
pub struct SdesCrypto {
    /// Crypto tag number.
    pub tag: u32,
    /// Cipher suite.
    pub suite: SrtpSuite,
    /// Base64-encoded key material (key + salt concatenated).
    pub key_params: String,
    /// Optional session parameters.
    pub session_params: Vec<String>,
}

impl SdesCrypto {
    /// Parse an SDP `a=crypto:` line.
    ///
    /// Format: `<tag> <suite> inline:<key_material>[|...] [session_params]`
    pub fn parse(line: &str) -> SrtpResult<Self> {
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() < 3 {
            return Err(SrtpError::InvalidKey(format!(
                "Invalid SDES crypto line: {}",
                line
            )));
        }

        let tag: u32 = parts[0]
            .parse()
            .map_err(|_| SrtpError::InvalidKey("Invalid crypto tag".into()))?;

        let suite = SrtpSuite::from_sdp_name(parts[1]).ok_or_else(|| {
            SrtpError::UnsupportedSuite(parts[1].to_string())
        })?;

        let key_param = parts[2];
        let key_material = key_param
            .strip_prefix("inline:")
            .ok_or_else(|| SrtpError::InvalidKey("Missing 'inline:' prefix".into()))?;

        // Strip any lifetime/MKI suffix after '|'.
        let key_base64 = key_material.split('|').next().unwrap_or(key_material);

        let session_params = if parts.len() > 3 {
            parts[3].split(' ').map(|s| s.to_string()).collect()
        } else {
            Vec::new()
        };

        Ok(Self {
            tag,
            suite,
            key_params: key_base64.to_string(),
            session_params,
        })
    }

    /// Decode the base64 key material into (key, salt).
    pub fn decode_key_material(&self) -> SrtpResult<(Vec<u8>, Vec<u8>)> {
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&self.key_params)
            .map_err(|e| SrtpError::InvalidKey(format!("Base64 decode failed: {}", e)))?;

        let key_len = self.suite.key_len();
        let salt_len = self.suite.salt_len();
        let total = key_len + salt_len;

        if decoded.len() < total {
            return Err(SrtpError::InvalidKey(format!(
                "Key material too short: {} < {}",
                decoded.len(),
                total
            )));
        }

        let key = decoded[..key_len].to_vec();
        let salt = decoded[key_len..total].to_vec();
        Ok((key, salt))
    }

    /// Format as an SDP `a=crypto:` attribute value.
    pub fn to_sdp(&self) -> String {
        let mut s = format!("{} {} inline:{}", self.tag, self.suite.sdp_name(), self.key_params);
        for param in &self.session_params {
            s.push(' ');
            s.push_str(param);
        }
        s
    }

    /// Generate a new SDES crypto line with a random key.
    pub fn generate(tag: u32, suite: SrtpSuite) -> SrtpResult<Self> {
        use rand::RngCore;

        let key_len = suite.key_len();
        let salt_len = suite.salt_len();
        let mut material = vec![0u8; key_len + salt_len];
        rand::thread_rng().fill_bytes(&mut material);

        use base64::Engine;
        let key_params = base64::engine::general_purpose::STANDARD.encode(&material);

        Ok(Self {
            tag,
            suite,
            key_params,
            session_params: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// SRTP session
// ---------------------------------------------------------------------------

/// An SRTP session that can protect (encrypt) and unprotect (decrypt) RTP
/// packets.
///
/// The actual AES-CM and HMAC-SHA1 operations are stubbed -- in production
/// this would use `aes`, `hmac`, and `sha1` crates, or bind to libsrtp2.
/// The interface and key management are fully implemented.
#[derive(Debug)]
pub struct SrtpSession {
    /// Cipher suite in use.
    suite: SrtpSuite,
    /// Master key.
    master_key: Vec<u8>,
    /// Master salt.
    master_salt: Vec<u8>,
    /// SSRC of the stream.
    ssrc: u32,
    /// Whether this session is for inbound packets.
    inbound: bool,
    /// Rollover counter for replay protection.
    roc: u32,
    /// Highest sequence number seen.
    highest_seq: u16,
    /// Replay window bitmask (64-bit sliding window).
    replay_window: u64,
}

impl SrtpSession {
    /// Create a new SRTP session from a policy.
    pub fn new(policy: &dyn SrtpPolicy) -> SrtpResult<Self> {
        Ok(Self {
            suite: policy.suite(),
            master_key: policy.master_key().to_vec(),
            master_salt: policy.master_salt().to_vec(),
            ssrc: policy.ssrc(),
            inbound: policy.is_inbound(),
            roc: 0,
            highest_seq: 0,
            replay_window: 0,
        })
    }

    /// Create from SDES key material.
    pub fn from_sdes(crypto: &SdesCrypto, ssrc: u32, inbound: bool) -> SrtpResult<Self> {
        let (key, salt) = crypto.decode_key_material()?;
        let policy = DefaultSrtpPolicy::new(crypto.suite, key, salt)?
            .with_ssrc(ssrc)
            .with_direction(inbound);
        Self::new(&policy)
    }

    /// Protect (encrypt) an RTP packet.
    ///
    /// Takes a raw RTP packet and returns the SRTP packet with encrypted
    /// payload and appended authentication tag.
    ///
    /// NOTE: Actual encryption is stubbed. The packet is returned with the
    /// auth tag appended but payload is NOT actually encrypted.
    pub fn protect(&mut self, rtp_packet: &[u8]) -> SrtpResult<Bytes> {
        if rtp_packet.len() < 12 {
            return Err(SrtpError::Protect("RTP packet too short".into()));
        }

        // In a real implementation:
        // 1. Derive session keys from master key + salt using KDF
        // 2. Compute IV from SSRC, sequence number, and ROC
        // 3. Encrypt payload with AES-CM
        // 4. Compute HMAC-SHA1 authentication tag

        let auth_tag_len = self.suite.auth_tag_len();
        let mut output = BytesMut::with_capacity(rtp_packet.len() + auth_tag_len);
        output.put_slice(rtp_packet);

        // Stub: append zero auth tag.
        output.put_slice(&vec![0u8; auth_tag_len]);

        debug!(
            suite = ?self.suite,
            packet_len = rtp_packet.len(),
            "SRTP protect (stub)"
        );

        Ok(output.freeze())
    }

    /// Unprotect (decrypt) an SRTP packet.
    ///
    /// Takes an SRTP packet and returns the decrypted RTP packet.
    ///
    /// NOTE: Actual decryption is stubbed. The authentication tag is
    /// stripped but payload is NOT actually decrypted.
    pub fn unprotect(&mut self, srtp_packet: &[u8]) -> SrtpResult<Bytes> {
        let auth_tag_len = self.suite.auth_tag_len();

        if srtp_packet.len() < 12 + auth_tag_len {
            return Err(SrtpError::Unprotect("SRTP packet too short".into()));
        }

        // In a real implementation:
        // 1. Verify HMAC-SHA1 authentication tag
        // 2. Check replay window
        // 3. Derive session keys
        // 4. Decrypt payload with AES-CM

        let rtp_len = srtp_packet.len() - auth_tag_len;
        let rtp_packet = Bytes::copy_from_slice(&srtp_packet[..rtp_len]);

        // Update replay tracking.
        let seq = u16::from_be_bytes([srtp_packet[2], srtp_packet[3]]);
        if seq > self.highest_seq {
            let shift = (seq - self.highest_seq) as u32;
            if shift < 64 {
                self.replay_window <<= shift;
            } else {
                self.replay_window = 0;
            }
            self.replay_window |= 1;
            self.highest_seq = seq;
        } else {
            let diff = (self.highest_seq - seq) as u32;
            if diff >= 64 {
                return Err(SrtpError::ReplayDetected);
            }
            let bit = 1u64 << diff;
            if self.replay_window & bit != 0 {
                return Err(SrtpError::ReplayDetected);
            }
            self.replay_window |= bit;
        }

        debug!(
            suite = ?self.suite,
            packet_len = srtp_packet.len(),
            "SRTP unprotect (stub)"
        );

        Ok(rtp_packet)
    }

    /// Get random bytes for key generation.
    pub fn get_random(buf: &mut [u8]) -> SrtpResult<()> {
        use rand::RngCore;
        rand::thread_rng().fill_bytes(buf);
        Ok(())
    }

    /// Change the SSRC for an existing stream.
    pub fn change_ssrc(&mut self, new_ssrc: u32) {
        self.ssrc = new_ssrc;
        // Reset replay tracking for new SSRC.
        self.highest_seq = 0;
        self.replay_window = 0;
        self.roc = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suite_properties() {
        let suite = SrtpSuite::AesCm128HmacSha1_80;
        assert_eq!(suite.key_len(), 16);
        assert_eq!(suite.salt_len(), 14);
        assert_eq!(suite.auth_tag_len(), 10);

        let suite256 = SrtpSuite::AesCm256HmacSha1_80;
        assert_eq!(suite256.key_len(), 32);
    }

    #[test]
    fn test_suite_sdp_roundtrip() {
        for suite in &[
            SrtpSuite::AesCm128HmacSha1_80,
            SrtpSuite::AesCm128HmacSha1_32,
            SrtpSuite::AesCm256HmacSha1_80,
            SrtpSuite::AeadAes128Gcm,
        ] {
            let name = suite.sdp_name();
            let parsed = SrtpSuite::from_sdp_name(name).unwrap();
            assert_eq!(*suite, parsed);
        }
    }

    #[test]
    fn test_sdes_parse() {
        let line = "1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj";
        let crypto = SdesCrypto::parse(line).unwrap();
        assert_eq!(crypto.tag, 1);
        assert_eq!(crypto.suite, SrtpSuite::AesCm128HmacSha1_80);
        assert!(crypto.key_params.starts_with("d0RmdmcmVCsp"));
    }

    #[test]
    fn test_sdes_generate_and_decode() {
        let crypto = SdesCrypto::generate(1, SrtpSuite::AesCm128HmacSha1_80).unwrap();
        let (key, salt) = crypto.decode_key_material().unwrap();
        assert_eq!(key.len(), 16);
        assert_eq!(salt.len(), 14);

        // Verify the SDP output can be re-parsed.
        let sdp = crypto.to_sdp();
        let reparsed = SdesCrypto::parse(&sdp).unwrap();
        assert_eq!(reparsed.tag, 1);
        assert_eq!(reparsed.suite, SrtpSuite::AesCm128HmacSha1_80);
    }

    #[test]
    fn test_policy_creation() {
        let key = vec![0u8; 16];
        let salt = vec![0u8; 14];
        let policy =
            DefaultSrtpPolicy::new(SrtpSuite::AesCm128HmacSha1_80, key.clone(), salt.clone())
                .unwrap();
        assert_eq!(policy.suite(), SrtpSuite::AesCm128HmacSha1_80);
        assert_eq!(policy.master_key(), &key[..]);
    }

    #[test]
    fn test_policy_invalid_key_len() {
        let key = vec![0u8; 8]; // too short
        let salt = vec![0u8; 14];
        let result = DefaultSrtpPolicy::new(SrtpSuite::AesCm128HmacSha1_80, key, salt);
        assert!(result.is_err());
    }

    #[test]
    fn test_srtp_session_protect_unprotect() {
        let key = vec![0xAB; 16];
        let salt = vec![0xCD; 14];
        let policy =
            DefaultSrtpPolicy::new(SrtpSuite::AesCm128HmacSha1_80, key, salt).unwrap();

        let mut session = SrtpSession::new(&policy).unwrap();

        // Minimal RTP packet (12-byte header + 160 bytes payload).
        let mut rtp = vec![0u8; 172];
        rtp[0] = 0x80; // V=2
        rtp[1] = 0x00; // PT=0
        rtp[2] = 0x00;
        rtp[3] = 0x01; // seq=1

        let srtp = session.protect(&rtp).unwrap();
        assert_eq!(srtp.len(), rtp.len() + 10); // + auth tag

        // Unprotect.
        let mut rx_session = SrtpSession::new(&policy).unwrap();
        let decrypted = rx_session.unprotect(&srtp).unwrap();
        assert_eq!(decrypted.len(), rtp.len());
    }

    #[test]
    fn test_srtp_replay_detection() {
        let key = vec![0xAB; 16];
        let salt = vec![0xCD; 14];
        let policy =
            DefaultSrtpPolicy::new(SrtpSuite::AesCm128HmacSha1_80, key, salt).unwrap();

        let mut session = SrtpSession::new(&policy).unwrap();

        let mut rtp = vec![0u8; 172];
        rtp[0] = 0x80;
        rtp[1] = 0x00;
        rtp[2] = 0x00;
        rtp[3] = 0x01; // seq=1

        let mut protect_session = SrtpSession::new(&policy).unwrap();
        let srtp = protect_session.protect(&rtp).unwrap();

        // First unprotect succeeds.
        session.unprotect(&srtp).unwrap();

        // Second unprotect (replay) should fail.
        let result = session.unprotect(&srtp);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_random() {
        let mut buf = [0u8; 32];
        SrtpSession::get_random(&mut buf).unwrap();
        // Very unlikely all zeros.
        assert!(buf.iter().any(|&b| b != 0));
    }
}
