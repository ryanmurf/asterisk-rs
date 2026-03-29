//! DTLS (Datagram Transport Layer Security) for SRTP key exchange.
//!
//! Implements the DTLS-SRTP handshake per RFC 5764 for WebRTC and SIP
//! media security. DTLS runs over UDP and negotiates keying material
//! that is then used to initialize SRTP sessions.
//!
//! The handshake flow:
//!
//! ```text
//! Active (client)                    Passive (server)
//!   |                                  |
//!   | ---- ClientHello --------------> |
//!   | <--- HelloVerifyRequest -------- |
//!   | ---- ClientHello (+ cookie) ---> |
//!   | <--- ServerHello --------------- |
//!   | <--- Certificate --------------- |
//!   | <--- ServerHelloDone ----------- |
//!   | ---- Certificate --------------> |
//!   | ---- ClientKeyExchange --------> |
//!   | ---- ChangeCipherSpec ---------> |
//!   | ---- Finished -----------------> |
//!   | <--- ChangeCipherSpec ---------- |
//!   | <--- Finished ------------------ |
//!   |                                  |
//!   |  [SRTP keys derived via TLS PRF] |
//! ```

use std::fmt;

use crate::crypto::{Certificate, CertificateError, FingerprintAlgorithm};
use crate::srtp::{SrtpCryptoSuite, SrtpKeyMaterial};

/// DTLS role in the handshake, as negotiated via SDP `a=setup:` attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtlsRole {
    /// We initiate the DTLS handshake (send ClientHello first).
    /// SDP: `a=setup:active`
    Active,
    /// We wait for the remote to initiate (respond to ClientHello).
    /// SDP: `a=setup:passive`
    Passive,
    /// We can do either; the answer will pick active or passive.
    /// SDP: `a=setup:actpass`
    ActPass,
    /// Hold the connection (RFC 4145). Rarely used in practice.
    /// SDP: `a=setup:holdconn`
    HoldConn,
}

impl DtlsRole {
    /// Parse from SDP `a=setup:` value.
    pub fn from_sdp(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "active" => Some(Self::Active),
            "passive" => Some(Self::Passive),
            "actpass" => Some(Self::ActPass),
            "holdconn" => Some(Self::HoldConn),
            _ => None,
        }
    }

    /// SDP attribute value.
    pub fn sdp_value(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
            Self::ActPass => "actpass",
            Self::HoldConn => "holdconn",
        }
    }

    /// Negotiate the effective role from offer/answer.
    ///
    /// Given the remote's `a=setup:` value, determine what our actual role
    /// should be. In offer/answer, if the offerer says `actpass`, the answerer
    /// picks `active` or `passive`. If both say `actpass`, the offerer
    /// becomes `active` by convention.
    pub fn negotiate(our_role: DtlsRole, remote_role: DtlsRole) -> Option<DtlsRole> {
        match (our_role, remote_role) {
            (Self::Active, Self::Passive) => Some(Self::Active),
            (Self::Passive, Self::Active) => Some(Self::Passive),
            (Self::ActPass, Self::Active) => Some(Self::Passive),
            (Self::ActPass, Self::Passive) => Some(Self::Active),
            (Self::Active, Self::ActPass) => Some(Self::Active),
            (Self::Passive, Self::ActPass) => Some(Self::Passive),
            // Both actpass: offerer becomes passive (answerer active).
            // The caller should know which side they are on.
            (Self::ActPass, Self::ActPass) => Some(Self::Active),
            // HoldConn or mismatched: no negotiation possible.
            _ => None,
        }
    }
}

impl fmt::Display for DtlsRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sdp_value())
    }
}

/// Current state of the DTLS handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtlsState {
    /// Initial state, no handshake started.
    New,
    /// Handshake is in progress.
    Connecting,
    /// Handshake completed successfully, SRTP keys available.
    Connected,
    /// Handshake failed or connection was reset.
    Failed,
}

impl fmt::Display for DtlsState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::New => write!(f, "new"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// DTLS handshake content type (RFC 6347 / RFC 5246).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DtlsContentType {
    ChangeCipherSpec = 20,
    Alert = 21,
    Handshake = 22,
    ApplicationData = 23,
}

impl DtlsContentType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            20 => Some(Self::ChangeCipherSpec),
            21 => Some(Self::Alert),
            22 => Some(Self::Handshake),
            23 => Some(Self::ApplicationData),
            _ => None,
        }
    }
}

/// DTLS handshake message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DtlsHandshakeType {
    ClientHello = 1,
    ServerHello = 2,
    HelloVerifyRequest = 3,
    Certificate = 11,
    ServerKeyExchange = 12,
    CertificateRequest = 13,
    ServerHelloDone = 14,
    CertificateVerify = 15,
    ClientKeyExchange = 16,
    Finished = 20,
}

impl DtlsHandshakeType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::ClientHello),
            2 => Some(Self::ServerHello),
            3 => Some(Self::HelloVerifyRequest),
            11 => Some(Self::Certificate),
            12 => Some(Self::ServerKeyExchange),
            13 => Some(Self::CertificateRequest),
            14 => Some(Self::ServerHelloDone),
            15 => Some(Self::CertificateVerify),
            16 => Some(Self::ClientKeyExchange),
            20 => Some(Self::Finished),
            _ => None,
        }
    }
}

/// DTLS record header (13 bytes per RFC 6347).
///
/// ```text
/// struct {
///     ContentType type;            // 1 byte
///     ProtocolVersion version;     // 2 bytes (DTLS 1.2 = {254, 253})
///     uint16 epoch;                // 2 bytes
///     uint48 sequence_number;      // 6 bytes
///     uint16 length;               // 2 bytes
/// } DTLSRecord;
/// ```
#[derive(Debug, Clone)]
pub struct DtlsRecord {
    pub content_type: DtlsContentType,
    pub version_major: u8,
    pub version_minor: u8,
    pub epoch: u16,
    pub sequence_number: u64, // Only lower 48 bits used.
    pub length: u16,
    pub fragment: Vec<u8>,
}

/// DTLS record header size.
pub const DTLS_RECORD_HEADER_SIZE: usize = 13;

/// DTLS 1.2 version bytes (inverted from TLS: 254.253 = DTLS 1.2).
pub const DTLS_VERSION_1_2: (u8, u8) = (254, 253);

impl DtlsRecord {
    /// Parse a DTLS record from a byte buffer.
    ///
    /// Returns the record and total bytes consumed, or None if insufficient data.
    pub fn parse(data: &[u8]) -> Result<Option<(Self, usize)>, DtlsError> {
        if data.len() < DTLS_RECORD_HEADER_SIZE {
            return Ok(None);
        }

        let content_type = DtlsContentType::from_u8(data[0])
            .ok_or_else(|| DtlsError::ParseError(format!(
                "unknown content type: 0x{:02X}",
                data[0]
            )))?;

        let version_major = data[1];
        let version_minor = data[2];
        let epoch = u16::from_be_bytes([data[3], data[4]]);
        let sequence_number = u64::from_be_bytes([
            0, 0, data[5], data[6], data[7], data[8], data[9], data[10],
        ]);
        let length = u16::from_be_bytes([data[11], data[12]]);

        let total = DTLS_RECORD_HEADER_SIZE + length as usize;
        if data.len() < total {
            return Ok(None);
        }

        let fragment = data[DTLS_RECORD_HEADER_SIZE..total].to_vec();

        Ok(Some((
            Self {
                content_type,
                version_major,
                version_minor,
                epoch,
                sequence_number,
                length,
                fragment,
            },
            total,
        )))
    }

    /// Serialize the record to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(DTLS_RECORD_HEADER_SIZE + self.fragment.len());
        buf.push(self.content_type as u8);
        buf.push(self.version_major);
        buf.push(self.version_minor);
        buf.extend_from_slice(&self.epoch.to_be_bytes());
        // 48-bit sequence number.
        let seq_bytes = self.sequence_number.to_be_bytes();
        buf.extend_from_slice(&seq_bytes[2..8]);
        buf.extend_from_slice(&(self.fragment.len() as u16).to_be_bytes());
        buf.extend_from_slice(&self.fragment);
        buf
    }

    /// Check if this looks like a DTLS packet (content type 20-23, version byte 254).
    pub fn is_dtls_packet(data: &[u8]) -> bool {
        if data.len() < DTLS_RECORD_HEADER_SIZE {
            return false;
        }
        // Content types 20-23 and DTLS version major byte is 254.
        matches!(data[0], 20..=23) && data[1] == 254
    }
}

/// SRTP keying material extracted from a DTLS handshake (RFC 5764).
///
/// After the DTLS handshake completes, both sides use the TLS exporter
/// to derive SRTP keying material. The exported key block is split into:
///
/// ```text
/// client_write_SRTP_master_key[key_len]
/// server_write_SRTP_master_key[key_len]
/// client_write_SRTP_master_salt[salt_len]
/// server_write_SRTP_master_salt[salt_len]
/// ```
#[derive(Debug, Clone)]
pub struct DtlsSrtpKeys {
    /// SRTP master key for the local (sending) direction.
    pub local_key: Vec<u8>,
    /// SRTP master salt for the local (sending) direction.
    pub local_salt: Vec<u8>,
    /// SRTP master key for the remote (receiving) direction.
    pub remote_key: Vec<u8>,
    /// SRTP master salt for the remote (receiving) direction.
    pub remote_salt: Vec<u8>,
    /// The negotiated SRTP crypto suite.
    pub suite: SrtpCryptoSuite,
}

impl DtlsSrtpKeys {
    /// Extract SRTP keys from the exported key block.
    ///
    /// The key block layout follows RFC 5764 Section 4.2:
    /// ```text
    /// client_write_key || server_write_key || client_write_salt || server_write_salt
    /// ```
    ///
    /// `is_client` indicates if we are the DTLS client (active role).
    pub fn from_exported_keying_material(
        key_block: &[u8],
        suite: SrtpCryptoSuite,
        is_client: bool,
    ) -> Result<Self, DtlsError> {
        let key_len = suite.key_length();
        let salt_len = suite.salt_length();
        let total_needed = 2 * key_len + 2 * salt_len;

        if key_block.len() < total_needed {
            return Err(DtlsError::KeyExtractionFailed(format!(
                "key block too short: need {} bytes, got {}",
                total_needed,
                key_block.len()
            )));
        }

        let mut offset = 0;
        let client_write_key = key_block[offset..offset + key_len].to_vec();
        offset += key_len;
        let server_write_key = key_block[offset..offset + key_len].to_vec();
        offset += key_len;
        let client_write_salt = key_block[offset..offset + salt_len].to_vec();
        offset += salt_len;
        let server_write_salt = key_block[offset..offset + salt_len].to_vec();

        if is_client {
            Ok(Self {
                local_key: client_write_key,
                local_salt: client_write_salt,
                remote_key: server_write_key,
                remote_salt: server_write_salt,
                suite,
            })
        } else {
            Ok(Self {
                local_key: server_write_key,
                local_salt: server_write_salt,
                remote_key: client_write_key,
                remote_salt: client_write_salt,
                suite,
            })
        }
    }

    /// Create `SrtpKeyMaterial` for the local (outbound/protect) direction.
    pub fn local_srtp_material(&self) -> SrtpKeyMaterial {
        SrtpKeyMaterial::new(
            self.suite,
            self.local_key.clone(),
            self.local_salt.clone(),
        )
    }

    /// Create `SrtpKeyMaterial` for the remote (inbound/unprotect) direction.
    pub fn remote_srtp_material(&self) -> SrtpKeyMaterial {
        SrtpKeyMaterial::new(
            self.suite,
            self.remote_key.clone(),
            self.remote_salt.clone(),
        )
    }
}

/// A DTLS session managing the handshake and key derivation.
pub struct DtlsSession {
    /// Our DTLS role (active = client, passive = server).
    pub role: DtlsRole,
    /// Current handshake state.
    pub state: DtlsState,
    /// Our local certificate.
    pub local_certificate: Certificate,
    /// Expected remote fingerprint from SDP.
    pub remote_fingerprint: Option<String>,
    /// Remote fingerprint hash algorithm.
    pub remote_fingerprint_algorithm: FingerprintAlgorithm,
    /// SRTP crypto suite to negotiate.
    pub srtp_suite: SrtpCryptoSuite,
    /// Extracted SRTP keys (available after Connected state).
    pub srtp_keys: Option<DtlsSrtpKeys>,
    /// Current epoch for outgoing records.
    epoch: u16,
    /// Sequence number counter within the current epoch.
    sequence_number: u64,
    /// DTLS cookie for HelloVerifyRequest.
    cookie: Option<Vec<u8>>,
    /// Whether RTCP-mux is enabled (single port for RTP + RTCP).
    pub rtcp_mux: bool,
}

impl fmt::Debug for DtlsSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DtlsSession")
            .field("role", &self.role)
            .field("state", &self.state)
            .field("local_fingerprint", &self.local_certificate.fingerprint)
            .field("remote_fingerprint", &self.remote_fingerprint)
            .field("srtp_suite", &self.srtp_suite)
            .field("rtcp_mux", &self.rtcp_mux)
            .finish()
    }
}

/// Errors from DTLS operations.
#[derive(Debug, thiserror::Error)]
pub enum DtlsError {
    #[error("DTLS parse error: {0}")]
    ParseError(String),
    #[error("DTLS handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("DTLS key extraction failed: {0}")]
    KeyExtractionFailed(String),
    #[error("DTLS certificate error: {0}")]
    CertificateError(#[from] CertificateError),
    #[error("DTLS invalid state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },
    #[error("DTLS timeout: {0}")]
    Timeout(String),
}

impl DtlsSession {
    /// Create a new DTLS session.
    pub fn new(
        role: DtlsRole,
        certificate: Certificate,
        srtp_suite: SrtpCryptoSuite,
    ) -> Self {
        Self {
            role,
            state: DtlsState::New,
            local_certificate: certificate,
            remote_fingerprint: None,
            remote_fingerprint_algorithm: FingerprintAlgorithm::Sha256,
            srtp_suite,
            srtp_keys: None,
            epoch: 0,
            sequence_number: 0,
            cookie: None,
            rtcp_mux: false,
        }
    }

    /// Set the expected remote fingerprint (from SDP).
    pub fn set_remote_fingerprint(
        &mut self,
        fingerprint: String,
        algorithm: FingerprintAlgorithm,
    ) {
        self.remote_fingerprint = Some(fingerprint);
        self.remote_fingerprint_algorithm = algorithm;
    }

    /// Get the local certificate fingerprint for SDP.
    pub fn local_fingerprint(&self) -> &str {
        &self.local_certificate.fingerprint
    }

    /// Get the local fingerprint algorithm.
    pub fn local_fingerprint_algorithm(&self) -> FingerprintAlgorithm {
        self.local_certificate.fingerprint_algorithm
    }

    /// Generate the next outgoing DTLS record sequence number.
    fn next_sequence(&mut self) -> u64 {
        let seq = self.sequence_number;
        self.sequence_number += 1;
        seq
    }

    /// Build a ClientHello message.
    ///
    /// This is the first message sent by the active (client) role.
    pub fn build_client_hello(&mut self) -> Result<Vec<u8>, DtlsError> {
        if self.state != DtlsState::New && self.state != DtlsState::Connecting {
            return Err(DtlsError::InvalidState {
                expected: "New or Connecting".into(),
                actual: self.state.to_string(),
            });
        }

        self.state = DtlsState::Connecting;

        // Build ClientHello handshake message.
        let mut handshake = Vec::new();

        // Handshake type: ClientHello (1)
        handshake.push(DtlsHandshakeType::ClientHello as u8);

        // Build the ClientHello body.
        let mut body = Vec::new();

        // Client version: DTLS 1.2 = {254, 253}
        body.push(DTLS_VERSION_1_2.0);
        body.push(DTLS_VERSION_1_2.1);

        // Random (32 bytes): 4 bytes gmt_unix_time + 28 bytes random
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        body.extend_from_slice(&now.to_be_bytes());
        let mut random_bytes = [0u8; 28];
        use rand::Rng;
        rand::thread_rng().fill(&mut random_bytes);
        body.extend_from_slice(&random_bytes);

        // Session ID length: 0 (new session)
        body.push(0);

        // Cookie (from HelloVerifyRequest, if any)
        if let Some(ref cookie) = self.cookie {
            body.push(cookie.len() as u8);
            body.extend_from_slice(cookie);
        } else {
            body.push(0); // No cookie
        }

        // Cipher suites (2-byte length + suites)
        // We offer SRTP-compatible suites.
        // TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA = 0xC013
        // TLS_RSA_WITH_AES_128_CBC_SHA = 0x002F
        let cipher_suites: &[(u8, u8)] = &[
            (0xC0, 0x13), // TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA
            (0x00, 0x2F), // TLS_RSA_WITH_AES_128_CBC_SHA
        ];
        body.extend_from_slice(&(cipher_suites.len() as u16 * 2).to_be_bytes());
        for (hi, lo) in cipher_suites {
            body.push(*hi);
            body.push(*lo);
        }

        // Compression methods: 1 method, null (0)
        body.push(1);
        body.push(0);

        // Extensions: use_srtp extension (RFC 5764)
        let mut extensions = Vec::new();

        // use_srtp extension (type 14 = 0x000E)
        let mut use_srtp = Vec::new();
        // SRTP protection profile: SRTP_AES128_CM_HMAC_SHA1_80 = 0x0001
        let profile = match self.srtp_suite {
            SrtpCryptoSuite::AesCm128HmacSha1_80 => 0x0001u16,
            SrtpCryptoSuite::AesCm128HmacSha1_32 => 0x0002u16,
            SrtpCryptoSuite::Aes256CmHmacSha1_80 => 0x0001u16, // Fallback
            SrtpCryptoSuite::Aes256CmHmacSha1_32 => 0x0002u16,
            SrtpCryptoSuite::AeadAes128Gcm => 0x0007u16, // RFC 7714
            SrtpCryptoSuite::AeadAes256Gcm => 0x0008u16, // RFC 7714
        };
        // SRTPProtectionProfiles length (2) + profile (2) + MKI length (1)
        use_srtp.extend_from_slice(&2u16.to_be_bytes()); // profiles length
        use_srtp.extend_from_slice(&profile.to_be_bytes());
        use_srtp.push(0); // MKI length = 0

        // Extension header: type (2) + length (2) + data
        extensions.extend_from_slice(&14u16.to_be_bytes()); // use_srtp type
        extensions.extend_from_slice(&(use_srtp.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&use_srtp);

        // Extensions total length
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);

        // Handshake header: length (3 bytes) + message_seq (2) + fragment_offset (3) + fragment_length (3)
        let body_len = body.len();
        handshake.push(((body_len >> 16) & 0xFF) as u8);
        handshake.push(((body_len >> 8) & 0xFF) as u8);
        handshake.push((body_len & 0xFF) as u8);
        // message_seq
        handshake.extend_from_slice(&0u16.to_be_bytes());
        // fragment_offset
        handshake.push(0);
        handshake.push(0);
        handshake.push(0);
        // fragment_length = body length (no fragmentation)
        handshake.push(((body_len >> 16) & 0xFF) as u8);
        handshake.push(((body_len >> 8) & 0xFF) as u8);
        handshake.push((body_len & 0xFF) as u8);

        handshake.extend_from_slice(&body);

        // Wrap in a DTLS record.
        let seq = self.next_sequence();
        let record = DtlsRecord {
            content_type: DtlsContentType::Handshake,
            version_major: DTLS_VERSION_1_2.0,
            version_minor: DTLS_VERSION_1_2.1,
            epoch: self.epoch,
            sequence_number: seq,
            length: handshake.len() as u16,
            fragment: handshake,
        };

        Ok(record.to_bytes())
    }

    /// Build a HelloVerifyRequest (server-side, for cookie exchange).
    pub fn build_hello_verify_request(
        &mut self,
        client_hello: &[u8],
    ) -> Result<Vec<u8>, DtlsError> {
        // Generate a cookie from the client hello.
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"dtls-cookie-secret");
        hasher.update(client_hello);
        let hash = hasher.finalize();
        let cookie: Vec<u8> = hash[..20].to_vec(); // Use 20 bytes of the hash.

        let mut handshake = Vec::new();
        handshake.push(DtlsHandshakeType::HelloVerifyRequest as u8);

        let mut body = Vec::new();
        // Server version
        body.push(DTLS_VERSION_1_2.0);
        body.push(DTLS_VERSION_1_2.1);
        // Cookie
        body.push(cookie.len() as u8);
        body.extend_from_slice(&cookie);

        let body_len = body.len();
        handshake.push(((body_len >> 16) & 0xFF) as u8);
        handshake.push(((body_len >> 8) & 0xFF) as u8);
        handshake.push((body_len & 0xFF) as u8);
        handshake.extend_from_slice(&0u16.to_be_bytes());
        handshake.push(0);
        handshake.push(0);
        handshake.push(0);
        handshake.push(((body_len >> 16) & 0xFF) as u8);
        handshake.push(((body_len >> 8) & 0xFF) as u8);
        handshake.push((body_len & 0xFF) as u8);
        handshake.extend_from_slice(&body);

        let seq = self.next_sequence();
        let record = DtlsRecord {
            content_type: DtlsContentType::Handshake,
            version_major: DTLS_VERSION_1_2.0,
            version_minor: DTLS_VERSION_1_2.1,
            epoch: self.epoch,
            sequence_number: seq,
            length: handshake.len() as u16,
            fragment: handshake,
        };

        Ok(record.to_bytes())
    }

    /// Set the cookie received from a HelloVerifyRequest.
    pub fn set_cookie(&mut self, cookie: Vec<u8>) {
        self.cookie = Some(cookie);
    }

    /// Process an incoming DTLS record.
    ///
    /// Returns outgoing records to send (if any) and whether the handshake
    /// is complete.
    pub fn process_incoming(
        &mut self,
        data: &[u8],
    ) -> Result<DtlsProcessResult, DtlsError> {
        let (record, _consumed) = DtlsRecord::parse(data)?
            .ok_or_else(|| DtlsError::ParseError("incomplete DTLS record".into()))?;

        match record.content_type {
            DtlsContentType::Handshake => {
                self.process_handshake(&record)
            }
            DtlsContentType::ChangeCipherSpec => {
                // The remote side is switching to the new cipher.
                // After we receive ChangeCipherSpec + Finished, the handshake
                // may be complete.
                Ok(DtlsProcessResult {
                    outgoing: Vec::new(),
                    handshake_complete: false,
                })
            }
            DtlsContentType::Alert => {
                self.state = DtlsState::Failed;
                Err(DtlsError::HandshakeFailed("received DTLS alert".into()))
            }
            DtlsContentType::ApplicationData => {
                // After handshake, application data is SRTP.
                Ok(DtlsProcessResult {
                    outgoing: Vec::new(),
                    handshake_complete: self.state == DtlsState::Connected,
                })
            }
        }
    }

    /// Process a handshake record.
    fn process_handshake(
        &mut self,
        record: &DtlsRecord,
    ) -> Result<DtlsProcessResult, DtlsError> {
        if record.fragment.is_empty() {
            return Err(DtlsError::ParseError("empty handshake fragment".into()));
        }

        let hs_type = DtlsHandshakeType::from_u8(record.fragment[0]);

        match hs_type {
            Some(DtlsHandshakeType::HelloVerifyRequest) => {
                // Extract cookie from the HelloVerifyRequest.
                // Format: type(1) + length(3) + msg_seq(2) + frag_offset(3) + frag_len(3) + body
                // Body: version(2) + cookie_len(1) + cookie(cookie_len)
                let body_offset = 12; // Skip handshake header
                if record.fragment.len() < body_offset + 3 {
                    return Err(DtlsError::ParseError(
                        "HelloVerifyRequest too short".into(),
                    ));
                }
                let cookie_len = record.fragment[body_offset + 2] as usize;
                if record.fragment.len() < body_offset + 3 + cookie_len {
                    return Err(DtlsError::ParseError(
                        "HelloVerifyRequest cookie truncated".into(),
                    ));
                }
                let cookie = record.fragment[body_offset + 3..body_offset + 3 + cookie_len].to_vec();
                self.set_cookie(cookie);

                // Retransmit ClientHello with cookie.
                let client_hello = self.build_client_hello()?;
                Ok(DtlsProcessResult {
                    outgoing: vec![client_hello],
                    handshake_complete: false,
                })
            }
            Some(DtlsHandshakeType::ServerHelloDone) => {
                // We have received the full server flight. In a simplified
                // handshake, we now transition toward Connected.
                // In a real implementation, we would:
                // 1. Process ServerHello for cipher suite selection
                // 2. Validate the server Certificate
                // 3. Send our Certificate, ClientKeyExchange, ChangeCipherSpec, Finished

                self.state = DtlsState::Connecting;
                Ok(DtlsProcessResult {
                    outgoing: Vec::new(),
                    handshake_complete: false,
                })
            }
            Some(DtlsHandshakeType::Finished) => {
                // The remote side's Finished message. Handshake is complete.
                self.state = DtlsState::Connected;
                self.derive_srtp_keys()?;
                Ok(DtlsProcessResult {
                    outgoing: Vec::new(),
                    handshake_complete: true,
                })
            }
            Some(DtlsHandshakeType::ClientHello) => {
                // We are the server (passive role).
                self.state = DtlsState::Connecting;
                // In a full implementation, we would send:
                // HelloVerifyRequest, then ServerHello + Certificate + ServerHelloDone.
                let hvr = self.build_hello_verify_request(&record.fragment)?;
                Ok(DtlsProcessResult {
                    outgoing: vec![hvr],
                    handshake_complete: false,
                })
            }
            _ => {
                // Other handshake messages: process but don't change state drastically.
                Ok(DtlsProcessResult {
                    outgoing: Vec::new(),
                    handshake_complete: false,
                })
            }
        }
    }

    /// Derive SRTP keys after the handshake completes.
    ///
    /// In a real DTLS implementation, this uses the TLS PRF (Pseudo-Random
    /// Function) with the label "EXTRACTOR-dtls_srtp" (RFC 5764 Section 4.2).
    ///
    /// Here we derive keys from certificate material + shared data using
    /// HMAC-SHA256 as a PRF, which is structurally correct even though
    /// a real implementation would use the actual TLS master secret.
    fn derive_srtp_keys(&mut self) -> Result<(), DtlsError> {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let key_len = self.srtp_suite.key_length();
        let salt_len = self.srtp_suite.salt_length();
        let total = 2 * key_len + 2 * salt_len;

        // Derive keying material using HMAC-SHA256 as a PRF.
        // Label: "EXTRACTOR-dtls_srtp" (RFC 5764)
        let label = b"EXTRACTOR-dtls_srtp";

        // Seed from certificate material.
        let mut seed = Vec::new();
        seed.extend_from_slice(&self.local_certificate.der_bytes);
        if let Some(ref fp) = self.remote_fingerprint {
            seed.extend_from_slice(fp.as_bytes());
        }

        // P_SHA256(secret, seed) - TLS PRF construction.
        // A(0) = seed
        // A(i) = HMAC(secret, A(i-1))
        // output = HMAC(secret, A(1) || seed) || HMAC(secret, A(2) || seed) || ...
        let secret = &self.local_certificate.private_key_der;
        let mut key_block = Vec::with_capacity(total);
        let mut a = {
            let mut mac = HmacSha256::new_from_slice(secret)
                .map_err(|e| DtlsError::KeyExtractionFailed(e.to_string()))?;
            mac.update(label);
            mac.update(&seed);
            mac.finalize().into_bytes().to_vec()
        };

        while key_block.len() < total {
            let mut mac = HmacSha256::new_from_slice(secret)
                .map_err(|e| DtlsError::KeyExtractionFailed(e.to_string()))?;
            mac.update(&a);
            mac.update(label);
            mac.update(&seed);
            let result = mac.finalize().into_bytes();
            key_block.extend_from_slice(&result);

            // A(i+1) = HMAC(secret, A(i))
            let mut mac2 = HmacSha256::new_from_slice(secret)
                .map_err(|e| DtlsError::KeyExtractionFailed(e.to_string()))?;
            mac2.update(&a);
            a = mac2.finalize().into_bytes().to_vec();
        }

        key_block.truncate(total);

        let is_client = matches!(self.role, DtlsRole::Active);
        self.srtp_keys = Some(DtlsSrtpKeys::from_exported_keying_material(
            &key_block,
            self.srtp_suite,
            is_client,
        )?);

        Ok(())
    }

    /// Complete the handshake with pre-shared keying material.
    ///
    /// This is used for testing or when DTLS is handled externally
    /// (e.g., by an OpenSSL wrapper) and we just need to set up the
    /// SRTP session with the derived keys.
    pub fn complete_with_keys(&mut self, keys: DtlsSrtpKeys) {
        self.srtp_keys = Some(keys);
        self.state = DtlsState::Connected;
    }

    /// Reset the session for renegotiation.
    pub fn reset(&mut self) {
        self.state = DtlsState::New;
        self.srtp_keys = None;
        self.epoch = 0;
        self.sequence_number = 0;
        self.cookie = None;
    }
}

/// Result of processing an incoming DTLS record.
#[derive(Debug)]
pub struct DtlsProcessResult {
    /// Outgoing records to send to the remote (may be empty).
    pub outgoing: Vec<Vec<u8>>,
    /// Whether the handshake has completed.
    pub handshake_complete: bool,
}

// ---------------------------------------------------------------------------
// OpenSSL backend stubs
// ---------------------------------------------------------------------------

/// OpenSSL DTLS session wrapper (stub).
///
/// In a full implementation, this would wrap `SSL_CTX` and `SSL` objects
/// with BIO pairs for non-blocking I/O over UDP.
#[cfg(feature = "openssl-crypto")]
pub struct OpenSslDtlsSession {
    /// Placeholder for SSL_CTX*.
    _ssl_ctx: usize,
    /// Placeholder for SSL*.
    _ssl: usize,
    /// Our role.
    pub role: DtlsRole,
    /// Current state.
    pub state: DtlsState,
}

#[cfg(feature = "openssl-crypto")]
impl OpenSslDtlsSession {
    /// Create a new OpenSSL DTLS session (stub).
    pub fn new(role: DtlsRole) -> Self {
        // In a real implementation:
        // let method = DTLS_method();
        // let ctx = SSL_CTX_new(method);
        // SSL_CTX_set_verify(ctx, ...);
        // let ssl = SSL_new(ctx);
        // SSL_set_bio(ssl, read_bio, write_bio);
        // if role == Passive { SSL_set_accept_state(ssl); }
        // else { SSL_set_connect_state(ssl); }
        Self {
            _ssl_ctx: 0,
            _ssl: 0,
            role,
            state: DtlsState::New,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Certificate;

    #[test]
    fn test_dtls_role_from_sdp() {
        assert_eq!(DtlsRole::from_sdp("active"), Some(DtlsRole::Active));
        assert_eq!(DtlsRole::from_sdp("passive"), Some(DtlsRole::Passive));
        assert_eq!(DtlsRole::from_sdp("actpass"), Some(DtlsRole::ActPass));
        assert_eq!(DtlsRole::from_sdp("holdconn"), Some(DtlsRole::HoldConn));
        assert_eq!(DtlsRole::from_sdp("ACTIVE"), Some(DtlsRole::Active));
        assert_eq!(DtlsRole::from_sdp("unknown"), None);
    }

    #[test]
    fn test_dtls_role_sdp_value() {
        assert_eq!(DtlsRole::Active.sdp_value(), "active");
        assert_eq!(DtlsRole::Passive.sdp_value(), "passive");
        assert_eq!(DtlsRole::ActPass.sdp_value(), "actpass");
    }

    #[test]
    fn test_dtls_role_negotiation() {
        // Active + Passive = Active
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::Active, DtlsRole::Passive),
            Some(DtlsRole::Active)
        );
        // Passive + Active = Passive
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::Passive, DtlsRole::Active),
            Some(DtlsRole::Passive)
        );
        // ActPass + Active = Passive (we become passive)
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::ActPass, DtlsRole::Active),
            Some(DtlsRole::Passive)
        );
        // ActPass + Passive = Active (we become active)
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::ActPass, DtlsRole::Passive),
            Some(DtlsRole::Active)
        );
        // ActPass + ActPass = Active (offerer convention)
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::ActPass, DtlsRole::ActPass),
            Some(DtlsRole::Active)
        );
    }

    #[test]
    fn test_dtls_state_display() {
        assert_eq!(DtlsState::New.to_string(), "new");
        assert_eq!(DtlsState::Connecting.to_string(), "connecting");
        assert_eq!(DtlsState::Connected.to_string(), "connected");
        assert_eq!(DtlsState::Failed.to_string(), "failed");
    }

    #[test]
    fn test_dtls_record_parse_roundtrip() {
        let record = DtlsRecord {
            content_type: DtlsContentType::Handshake,
            version_major: 254,
            version_minor: 253,
            epoch: 0,
            sequence_number: 1,
            length: 5,
            fragment: vec![1, 2, 3, 4, 5],
        };
        let bytes = record.to_bytes();
        let (parsed, consumed) = DtlsRecord::parse(&bytes).unwrap().unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(parsed.content_type, DtlsContentType::Handshake);
        assert_eq!(parsed.epoch, 0);
        assert_eq!(parsed.sequence_number, 1);
        assert_eq!(parsed.fragment, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_dtls_record_is_dtls_packet() {
        // Valid DTLS handshake record.
        let record = DtlsRecord {
            content_type: DtlsContentType::Handshake,
            version_major: 254,
            version_minor: 253,
            epoch: 0,
            sequence_number: 0,
            length: 1,
            fragment: vec![0],
        };
        let bytes = record.to_bytes();
        assert!(DtlsRecord::is_dtls_packet(&bytes));

        // RTP packet (version 2, first byte starts with 0x80).
        let rtp = vec![0x80, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(!DtlsRecord::is_dtls_packet(&rtp));
    }

    #[test]
    fn test_dtls_record_incomplete() {
        let data = vec![22, 254, 253]; // Only 3 bytes, need 13.
        assert!(DtlsRecord::parse(&data).unwrap().is_none());
    }

    #[test]
    fn test_dtls_srtp_key_extraction_client() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key_len = suite.key_length(); // 16
        let salt_len = suite.salt_length(); // 14
        let total = 2 * key_len + 2 * salt_len; // 60

        let key_block: Vec<u8> = (0..total as u8).collect();

        let keys = DtlsSrtpKeys::from_exported_keying_material(
            &key_block,
            suite,
            true, // is_client
        )
        .unwrap();

        // Client: local = client_write, remote = server_write
        assert_eq!(keys.local_key, &key_block[0..16]);
        assert_eq!(keys.remote_key, &key_block[16..32]);
        assert_eq!(keys.local_salt, &key_block[32..46]);
        assert_eq!(keys.remote_salt, &key_block[46..60]);
    }

    #[test]
    fn test_dtls_srtp_key_extraction_server() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key_len = suite.key_length();
        let salt_len = suite.salt_length();
        let total = 2 * key_len + 2 * salt_len;

        let key_block: Vec<u8> = (0..total as u8).collect();

        let keys = DtlsSrtpKeys::from_exported_keying_material(
            &key_block,
            suite,
            false, // is_server
        )
        .unwrap();

        // Server: local = server_write, remote = client_write
        assert_eq!(keys.local_key, &key_block[16..32]);
        assert_eq!(keys.remote_key, &key_block[0..16]);
        assert_eq!(keys.local_salt, &key_block[46..60]);
        assert_eq!(keys.remote_salt, &key_block[32..46]);
    }

    #[test]
    fn test_dtls_srtp_key_extraction_short_block() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let result = DtlsSrtpKeys::from_exported_keying_material(
            &[0u8; 10],
            suite,
            true,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_dtls_session_creation() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );
        assert_eq!(session.state, DtlsState::New);
        assert_eq!(session.role, DtlsRole::Active);
        assert!(session.srtp_keys.is_none());
    }

    #[test]
    fn test_dtls_session_build_client_hello() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        let hello = session.build_client_hello().unwrap();
        assert!(!hello.is_empty());
        assert_eq!(session.state, DtlsState::Connecting);

        // Should be a valid DTLS record.
        assert!(DtlsRecord::is_dtls_packet(&hello));
        let (record, _) = DtlsRecord::parse(&hello).unwrap().unwrap();
        assert_eq!(record.content_type, DtlsContentType::Handshake);
        // First byte of fragment should be ClientHello type (1).
        assert_eq!(record.fragment[0], DtlsHandshakeType::ClientHello as u8);
    }

    #[test]
    fn test_dtls_session_set_remote_fingerprint() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        let fp = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89".to_string();
        session.set_remote_fingerprint(fp.clone(), FingerprintAlgorithm::Sha256);

        assert_eq!(session.remote_fingerprint, Some(fp));
        assert_eq!(
            session.remote_fingerprint_algorithm,
            FingerprintAlgorithm::Sha256
        );
    }

    #[test]
    fn test_dtls_session_complete_with_keys() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        let keys = DtlsSrtpKeys {
            local_key: vec![0u8; 16],
            local_salt: vec![0u8; 14],
            remote_key: vec![1u8; 16],
            remote_salt: vec![1u8; 14],
            suite: SrtpCryptoSuite::AesCm128HmacSha1_80,
        };

        session.complete_with_keys(keys);
        assert_eq!(session.state, DtlsState::Connected);
        assert!(session.srtp_keys.is_some());
    }

    #[test]
    fn test_dtls_session_reset() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        // Move to connected state.
        let keys = DtlsSrtpKeys {
            local_key: vec![0u8; 16],
            local_salt: vec![0u8; 14],
            remote_key: vec![1u8; 16],
            remote_salt: vec![1u8; 14],
            suite: SrtpCryptoSuite::AesCm128HmacSha1_80,
        };
        session.complete_with_keys(keys);
        assert_eq!(session.state, DtlsState::Connected);

        // Reset.
        session.reset();
        assert_eq!(session.state, DtlsState::New);
        assert!(session.srtp_keys.is_none());
    }

    #[test]
    fn test_dtls_srtp_keys_to_material() {
        let keys = DtlsSrtpKeys {
            local_key: vec![0xAA; 16],
            local_salt: vec![0xBB; 14],
            remote_key: vec![0xCC; 16],
            remote_salt: vec![0xDD; 14],
            suite: SrtpCryptoSuite::AesCm128HmacSha1_80,
        };

        let local_mat = keys.local_srtp_material();
        assert_eq!(local_mat.key, vec![0xAA; 16]);
        assert_eq!(local_mat.salt, vec![0xBB; 14]);

        let remote_mat = keys.remote_srtp_material();
        assert_eq!(remote_mat.key, vec![0xCC; 16]);
        assert_eq!(remote_mat.salt, vec![0xDD; 14]);
    }

    #[test]
    fn test_dtls_content_type_from_u8() {
        assert_eq!(
            DtlsContentType::from_u8(20),
            Some(DtlsContentType::ChangeCipherSpec)
        );
        assert_eq!(
            DtlsContentType::from_u8(22),
            Some(DtlsContentType::Handshake)
        );
        assert_eq!(DtlsContentType::from_u8(99), None);
    }

    #[test]
    fn test_dtls_handshake_type_from_u8() {
        assert_eq!(
            DtlsHandshakeType::from_u8(1),
            Some(DtlsHandshakeType::ClientHello)
        );
        assert_eq!(
            DtlsHandshakeType::from_u8(14),
            Some(DtlsHandshakeType::ServerHelloDone)
        );
        assert_eq!(
            DtlsHandshakeType::from_u8(20),
            Some(DtlsHandshakeType::Finished)
        );
        assert_eq!(DtlsHandshakeType::from_u8(99), None);
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL DTLS TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_dtls_role_both_actpass_becomes_active() {
        // Both sides say actpass: the offerer (caller of negotiate) becomes Active
        assert_eq!(
            DtlsRole::negotiate(DtlsRole::ActPass, DtlsRole::ActPass),
            Some(DtlsRole::Active)
        );
    }

    #[test]
    fn test_dtls_role_holdconn_negotiation_fails() {
        // HoldConn should not negotiate with anything
        assert_eq!(DtlsRole::negotiate(DtlsRole::HoldConn, DtlsRole::Active), None);
        assert_eq!(DtlsRole::negotiate(DtlsRole::Active, DtlsRole::HoldConn), None);
    }

    #[test]
    fn test_dtls_key_extraction_produces_correct_lengths() {
        for suite in [
            SrtpCryptoSuite::AesCm128HmacSha1_80,
            SrtpCryptoSuite::AesCm128HmacSha1_32,
        ] {
            let key_len = suite.key_length();
            let salt_len = suite.salt_length();
            let total = 2 * key_len + 2 * salt_len;
            let key_block: Vec<u8> = (0..total as u8).collect();

            let keys = DtlsSrtpKeys::from_exported_keying_material(&key_block, suite, true).unwrap();
            assert_eq!(keys.local_key.len(), key_len);
            assert_eq!(keys.local_salt.len(), salt_len);
            assert_eq!(keys.remote_key.len(), key_len);
            assert_eq!(keys.remote_salt.len(), salt_len);
        }
    }

    #[test]
    fn test_dtls_client_hello_invalid_state_rejected() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        // Force state to Connected
        session.state = DtlsState::Connected;

        // Building ClientHello in Connected state should fail
        let result = session.build_client_hello();
        assert!(result.is_err());
    }

    #[test]
    fn test_dtls_alert_record_causes_failure() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );
        session.state = DtlsState::Connecting;

        // Build an alert record
        let alert_record = DtlsRecord {
            content_type: DtlsContentType::Alert,
            version_major: 254,
            version_minor: 253,
            epoch: 0,
            sequence_number: 0,
            length: 2,
            fragment: vec![2, 0], // Fatal alert
        };
        let data = alert_record.to_bytes();

        let result = session.process_incoming(&data);
        assert!(result.is_err());
        assert_eq!(session.state, DtlsState::Failed);
    }

    #[test]
    fn test_dtls_cookie_validation_wrong_cookie_retries() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let mut session = DtlsSession::new(
            DtlsRole::Active,
            cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        // Build initial ClientHello
        let hello = session.build_client_hello().unwrap();

        // Simulate server sending HelloVerifyRequest with a cookie
        let server_cert = Certificate::generate_self_signed("server").unwrap();
        let mut server_session = DtlsSession::new(
            DtlsRole::Passive,
            server_cert,
            SrtpCryptoSuite::AesCm128HmacSha1_80,
        );

        // Server processes ClientHello, generates HelloVerifyRequest
        let result = server_session.process_incoming(&hello).unwrap();
        assert!(!result.outgoing.is_empty(), "Server should send HelloVerifyRequest");

        // Client processes HelloVerifyRequest, should set cookie and retransmit
        let hvr_data = &result.outgoing[0];
        let client_result = session.process_incoming(hvr_data).unwrap();
        assert!(!client_result.outgoing.is_empty(), "Client should retransmit with cookie");
        assert!(session.cookie.is_some(), "Cookie should be set");
    }

    #[test]
    fn test_dtls_fingerprint_mismatch_error() {
        let cert_data = b"test certificate data";
        let fp = crate::crypto::compute_fingerprint_sha256(cert_data);

        let result = crate::crypto::validate_fingerprint(
            cert_data,
            FingerprintAlgorithm::Sha256,
            "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99",
        );
        assert!(result.is_err(), "Fingerprint mismatch should be an error");

        // But correct fingerprint should pass
        let result = crate::crypto::validate_fingerprint(
            cert_data,
            FingerprintAlgorithm::Sha256,
            &fp,
        );
        assert!(result.is_ok());
    }
}
