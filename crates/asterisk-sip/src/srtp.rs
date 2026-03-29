//! SRTP (Secure RTP) support with feature-gated crypto backends.
//!
//! This module provides SRTP encryption/decryption for RTP media streams
//! per RFC 3711. The crypto backend is selected at compile time:
//!
//! - `pure-rust-crypto` (default): Uses pure-Rust AES + HMAC-SHA1.
//! - `openssl-crypto`: Uses OpenSSL bindings (stubs for now).
//!
//! ## Cryptographic Operations (RFC 3711)
//!
//! **AES-128-CM (Counter Mode) encryption:**
//! - IV = (SSRC XOR salt) || packet_index
//! - Keystream = AES_ECB(session_key, IV || counter)
//! - Ciphertext = Plaintext XOR Keystream
//!
//! **HMAC-SHA1 authentication:**
//! - Computed over: RTP header || encrypted payload || ROC
//! - Tag truncated to 80 bits (10 bytes) or 32 bits (4 bytes)
//!
//! **Replay protection:**
//! - 128-bit sliding window tracks received packet indices

use std::fmt;

/// SRTP crypto suite identifiers (RFC 4568, RFC 7714).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrtpCryptoSuite {
    /// AES_CM_128_HMAC_SHA1_80 (default, most common)
    AesCm128HmacSha1_80,
    /// AES_CM_128_HMAC_SHA1_32
    AesCm128HmacSha1_32,
    /// AES_256_CM_HMAC_SHA1_80
    Aes256CmHmacSha1_80,
    /// AES_256_CM_HMAC_SHA1_32
    Aes256CmHmacSha1_32,
    /// AEAD_AES_128_GCM (RFC 7714)
    AeadAes128Gcm,
    /// AEAD_AES_256_GCM (RFC 7714)
    AeadAes256Gcm,
}

impl SrtpCryptoSuite {
    /// Key length in bytes for this suite.
    pub fn key_length(&self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 | Self::AesCm128HmacSha1_32 | Self::AeadAes128Gcm => 16,
            Self::Aes256CmHmacSha1_80 | Self::Aes256CmHmacSha1_32 | Self::AeadAes256Gcm => 32,
        }
    }

    /// Salt length in bytes for this suite.
    pub fn salt_length(&self) -> usize {
        match self {
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => 12, // RFC 7714: 12-byte IV
            _ => 14,
        }
    }

    /// Total keying material length (key + salt).
    pub fn master_key_length(&self) -> usize {
        self.key_length() + self.salt_length()
    }

    /// Authentication tag length in bytes.
    pub fn auth_tag_length(&self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 | Self::Aes256CmHmacSha1_80 => 10,
            Self::AesCm128HmacSha1_32 | Self::Aes256CmHmacSha1_32 => 4,
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => 16, // GCM 128-bit tag
        }
    }

    /// Whether this suite uses AEAD (GCM).
    pub fn is_aead(&self) -> bool {
        matches!(self, Self::AeadAes128Gcm | Self::AeadAes256Gcm)
    }

    /// Parse a crypto suite from its SDP name.
    pub fn from_sdp_name(name: &str) -> Option<Self> {
        match name {
            "AES_CM_128_HMAC_SHA1_80" => Some(Self::AesCm128HmacSha1_80),
            "AES_CM_128_HMAC_SHA1_32" => Some(Self::AesCm128HmacSha1_32),
            "AES_256_CM_HMAC_SHA1_80" => Some(Self::Aes256CmHmacSha1_80),
            "AES_256_CM_HMAC_SHA1_32" => Some(Self::Aes256CmHmacSha1_32),
            "AEAD_AES_128_GCM" => Some(Self::AeadAes128Gcm),
            "AEAD_AES_256_GCM" => Some(Self::AeadAes256Gcm),
            _ => None,
        }
    }

    /// SDP name for this suite.
    pub fn sdp_name(&self) -> &'static str {
        match self {
            Self::AesCm128HmacSha1_80 => "AES_CM_128_HMAC_SHA1_80",
            Self::AesCm128HmacSha1_32 => "AES_CM_128_HMAC_SHA1_32",
            Self::Aes256CmHmacSha1_80 => "AES_256_CM_HMAC_SHA1_80",
            Self::Aes256CmHmacSha1_32 => "AES_256_CM_HMAC_SHA1_32",
            Self::AeadAes128Gcm => "AEAD_AES_128_GCM",
            Self::AeadAes256Gcm => "AEAD_AES_256_GCM",
        }
    }
}

impl fmt::Display for SrtpCryptoSuite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sdp_name())
    }
}

/// SRTP keying material.
#[derive(Clone)]
pub struct SrtpKeyMaterial {
    /// Crypto suite.
    pub suite: SrtpCryptoSuite,
    /// Master key.
    pub key: Vec<u8>,
    /// Master salt.
    pub salt: Vec<u8>,
}

impl SrtpKeyMaterial {
    /// Create new keying material.
    pub fn new(suite: SrtpCryptoSuite, key: Vec<u8>, salt: Vec<u8>) -> Self {
        Self { suite, key, salt }
    }

    /// Validate that key and salt lengths match the suite.
    pub fn validate(&self) -> Result<(), SrtpError> {
        if self.key.len() != self.suite.key_length() {
            return Err(SrtpError::InvalidKeyLength {
                expected: self.suite.key_length(),
                actual: self.key.len(),
            });
        }
        if self.salt.len() != self.suite.salt_length() {
            return Err(SrtpError::InvalidSaltLength {
                expected: self.suite.salt_length(),
                actual: self.salt.len(),
            });
        }
        Ok(())
    }
}

impl fmt::Debug for SrtpKeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrtpKeyMaterial")
            .field("suite", &self.suite)
            .field("key_len", &self.key.len())
            .field("salt_len", &self.salt.len())
            .finish()
    }
}

/// Errors from SRTP operations.
#[derive(Debug, thiserror::Error)]
pub enum SrtpError {
    #[error("invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    #[error("invalid salt length: expected {expected}, got {actual}")]
    InvalidSaltLength { expected: usize, actual: usize },
    #[error("SRTP protection failed: {0}")]
    ProtectFailed(String),
    #[error("SRTP unprotection failed: {0}")]
    UnprotectFailed(String),
    #[error("crypto backend not available: {0}")]
    BackendUnavailable(String),
    #[error("SRTP replay detected: packet index {0} already received")]
    ReplayDetected(u64),
    #[error("SRTP authentication failed")]
    AuthenticationFailed,
    #[error("SRTP packet too short: need at least {min} bytes, got {actual}")]
    PacketTooShort { min: usize, actual: usize },
}

/// Trait for SRTP crypto operations.
///
/// Both the pure-Rust and OpenSSL backends implement this trait.
pub trait SrtpCrypto: Send + Sync {
    /// Name of the crypto backend.
    fn backend_name(&self) -> &str;

    /// Protect (encrypt + authenticate) an RTP packet in place.
    fn protect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError>;

    /// Unprotect (verify + decrypt) an SRTP packet in place.
    fn unprotect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError>;

    /// Protect an RTCP packet.
    fn protect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError>;

    /// Unprotect an SRTCP packet.
    fn unprotect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError>;
}

// ---------------------------------------------------------------------------
// Minimum RTP header size.
// ---------------------------------------------------------------------------

/// Minimum RTP header: V(2)+P+X+CC(1) + M+PT(1) + Seq(2) + TS(4) + SSRC(4) = 12 bytes.
const RTP_HEADER_MIN: usize = 12;

/// Minimum RTCP header: V(2)+P+RC(1) + PT(1) + Length(2) + SSRC(4) = 8 bytes.
const RTCP_HEADER_MIN: usize = 8;

// ---------------------------------------------------------------------------
// Replay protection: 128-bit sliding window (RFC 3711 Section 3.3.2)
// ---------------------------------------------------------------------------

/// Replay protection using a 128-bit sliding window.
///
/// The window tracks the highest received packet index and a bitmap
/// of recently received indices. Duplicate or very old packets are rejected.
#[derive(Debug, Clone)]
struct ReplayWindow {
    /// Highest received packet index.
    highest: u64,
    /// Bitmap: bit i (counting from LSB) = 1 if (highest - i) was received.
    /// Covers indices [highest - 127, highest].
    bitmap: u128,
    /// Whether any packet has been received yet.
    initialized: bool,
}

impl ReplayWindow {
    fn new() -> Self {
        Self {
            highest: 0,
            bitmap: 0,
            initialized: false,
        }
    }

    /// Check if a packet index is acceptable (not a replay).
    /// Returns true if the packet should be accepted.
    #[inline(always)]
    fn check(&self, index: u64) -> bool {
        if !self.initialized {
            return true;
        }
        if index > self.highest {
            return true; // New packet beyond the window.
        }
        let delta = self.highest - index;
        if delta >= 128 {
            return false; // Too old.
        }
        // Check if already received.
        (self.bitmap & (1u128 << delta)) == 0
    }

    /// Update the window after accepting a packet.
    #[inline(always)]
    fn update(&mut self, index: u64) {
        if !self.initialized {
            self.highest = index;
            self.bitmap = 1; // bit 0 = self.highest received.
            self.initialized = true;
            return;
        }

        if index > self.highest {
            let shift = index - self.highest;
            if shift >= 128 {
                self.bitmap = 1;
            } else {
                self.bitmap <<= shift;
                self.bitmap |= 1; // Mark the new highest as received.
            }
            self.highest = index;
        } else {
            let delta = self.highest - index;
            if delta < 128 {
                self.bitmap |= 1u128 << delta;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AES-128-CM key derivation (RFC 3711 Section 4.3)
// ---------------------------------------------------------------------------

/// SRTP key derivation labels (RFC 3711 Section 4.3.1).
const LABEL_CIPHER_KEY: u8 = 0x00;
const LABEL_AUTH_KEY: u8 = 0x01;
const LABEL_SALT: u8 = 0x02;
const LABEL_SRTCP_CIPHER_KEY: u8 = 0x03;
const LABEL_SRTCP_AUTH_KEY: u8 = 0x04;
const LABEL_SRTCP_SALT: u8 = 0x05;

/// Derive a session key from master key and master salt using AES-CM PRF.
///
/// Per RFC 3711 Section 4.3.1:
///   r = key_derivation_rate (0 for SRTP default)
///   key_id = label || r (48 bits)
///   x = key_id XOR master_salt (112 bits)
///   session_key = AES_CM(master_key, x || 0..0, needed_len)
fn derive_session_key(
    master_key: &[u8],
    master_salt: &[u8],
    label: u8,
    needed_len: usize,
) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt, KeyInit};
    use aes::Aes128;

    // Build x = label XOR salt, padded to 14 bytes.
    // The label goes in byte position 7 (per RFC 3711 Section 4.3.1):
    //   key_id = label || r   where r = 0 (48-bit index DIV key_derivation_rate)
    //   key_id is 7 bytes total (label is 1 byte, r is 6 bytes of zeros for kdr=0)
    //   x = key_id XOR salt  (both 14 bytes, with key_id zero-padded on the left to 14 bytes)
    let mut x = [0u8; 14];
    // Copy the salt.
    let salt_len = master_salt.len().min(14);
    x[..salt_len].copy_from_slice(&master_salt[..salt_len]);
    // XOR the label into byte 7 (0-indexed) of the x value.
    x[7] ^= label;

    // Generate keystream using AES-CM (Counter Mode).
    // IV = x || 0x0000 (padded to 16 bytes)
    // Keystream block i = AES(master_key, IV + i)
    let cipher = Aes128::new(master_key.into());

    let mut output = Vec::with_capacity(needed_len);
    let mut counter = 0u16;

    while output.len() < needed_len {
        let mut block = [0u8; 16];
        block[..14].copy_from_slice(&x);
        block[14] = (counter >> 8) as u8;
        block[15] = counter as u8;

        let block_ref: &mut aes::Block = block.as_mut().into();
        cipher.encrypt_block(block_ref);

        let remaining = needed_len - output.len();
        let take = remaining.min(16);
        output.extend_from_slice(&block[..take]);
        counter += 1;
    }

    output
}

/// Derive all session keys for SRTP from master key material.
///
/// Pre-computes AES cipher instances for RTP and RTCP to avoid
/// per-packet key schedule computation.
struct DerivedKeys {
    /// Pre-computed AES cipher for RTP encryption/decryption.
    rtp_cipher: aes::Aes128,
    /// Session authentication key for RTP (160 bits for HMAC-SHA1).
    auth_key: Vec<u8>,
    /// Session salt for RTP (112 bits).
    salt: Vec<u8>,
    /// Pre-computed AES cipher for RTCP encryption/decryption.
    rtcp_cipher: aes::Aes128,
    /// Session authentication key for RTCP.
    rtcp_auth_key: Vec<u8>,
    /// Session salt for RTCP.
    rtcp_salt: Vec<u8>,
}

impl DerivedKeys {
    fn derive(master_key: &[u8], master_salt: &[u8], key_len: usize) -> Self {
        use aes::cipher::KeyInit;

        let cipher_key = derive_session_key(master_key, master_salt, LABEL_CIPHER_KEY, key_len);
        let rtcp_cipher_key = derive_session_key(
            master_key,
            master_salt,
            LABEL_SRTCP_CIPHER_KEY,
            key_len,
        );

        // Pre-compute AES key schedules once per session.
        let rtp_cipher = aes::Aes128::new(cipher_key.as_slice().into());
        let rtcp_cipher = aes::Aes128::new(rtcp_cipher_key.as_slice().into());

        Self {
            rtp_cipher,
            auth_key: derive_session_key(master_key, master_salt, LABEL_AUTH_KEY, 20), // 160 bits
            salt: derive_session_key(master_key, master_salt, LABEL_SALT, 14), // 112 bits
            rtcp_cipher,
            rtcp_auth_key: derive_session_key(
                master_key,
                master_salt,
                LABEL_SRTCP_AUTH_KEY,
                20,
            ),
            rtcp_salt: derive_session_key(master_key, master_salt, LABEL_SRTCP_SALT, 14),
        }
    }
}

// ---------------------------------------------------------------------------
// AES-128-CM encryption/decryption (RFC 3711 Section 4.1)
// ---------------------------------------------------------------------------

/// Apply AES-CM keystream to payload (in-place XOR) using a pre-computed cipher.
///
/// Per RFC 3711 Section 4.1:
///   IV = (SSRC XOR salt[4..8]) || (packet_index XOR salt[8..14])
///   Keystream block i = AES(session_key, IV || i)
///   Encrypted = Payload XOR Keystream
#[inline(always)]
fn aes_cm_encrypt_decrypt_with_cipher(
    cipher: &aes::Aes128,
    session_salt: &[u8],
    ssrc: u32,
    packet_index: u64,
    payload: &mut [u8],
) {
    use aes::cipher::BlockEncrypt;

    // Build IV (128 bits = 16 bytes):
    //
    // RFC 3711 Section 4.1.1:
    //   IV = (k_s * 2^16) XOR (SSRC * 2^64) XOR (i * 2^16)
    //
    // Where k_s = session salt (14 bytes = 112 bits), laid out as:
    //   bytes [0..4]:  salt[0..4]
    //   bytes [4..8]:  salt[4..8] XOR SSRC
    //   bytes [8..14]: salt[8..14] XOR packet_index (top 48 bits)
    //   bytes [14..16]: 0x0000 (block counter)
    let mut iv = [0u8; 16];
    iv[..14].copy_from_slice(&session_salt[..14]);

    // XOR SSRC into bytes 4..8
    let ssrc_bytes = ssrc.to_be_bytes();
    iv[4] ^= ssrc_bytes[0];
    iv[5] ^= ssrc_bytes[1];
    iv[6] ^= ssrc_bytes[2];
    iv[7] ^= ssrc_bytes[3];

    // XOR packet_index (48 bits) into bytes 8..14
    let pi_bytes = packet_index.to_be_bytes(); // 8 bytes, we use lower 6
    iv[8] ^= pi_bytes[2];
    iv[9] ^= pi_bytes[3];
    iv[10] ^= pi_bytes[4];
    iv[11] ^= pi_bytes[5];
    iv[12] ^= pi_bytes[6];
    iv[13] ^= pi_bytes[7];

    // bytes 14..16 are the block counter, starting at 0.

    // Generate keystream blocks and XOR with payload.
    let mut offset = 0;
    let mut counter = 0u16;

    while offset < payload.len() {
        let mut block = iv;
        block[14] = (counter >> 8) as u8;
        block[15] = counter as u8;

        let block_ref: &mut aes::Block = block.as_mut().into();
        cipher.encrypt_block(block_ref);

        let end = (offset + 16).min(payload.len());
        for i in offset..end {
            payload[i] ^= block[i - offset];
        }

        offset += 16;
        counter += 1;
    }
}

/// Legacy wrapper that creates a cipher on the fly (used by key derivation).
fn aes_cm_encrypt_decrypt(
    session_key: &[u8],
    session_salt: &[u8],
    ssrc: u32,
    packet_index: u64,
    payload: &mut [u8],
) {
    use aes::cipher::KeyInit;
    let cipher = aes::Aes128::new(session_key.into());
    aes_cm_encrypt_decrypt_with_cipher(&cipher, session_salt, ssrc, packet_index, payload);
}

/// Compute HMAC-SHA1 over the authenticated portion of an SRTP packet.
///
/// Per RFC 3711 Section 4.2:
///   auth_portion = RTP_header || encrypted_payload
///   M = HMAC-SHA1(auth_key, auth_portion || ROC)
///   auth_tag = M[0..tag_len]
///
/// Returns raw 20-byte HMAC output to avoid allocation -- callers
/// truncate to the required tag length.
#[inline(always)]
fn compute_hmac_sha1(auth_key: &[u8], data: &[u8], roc: u32) -> [u8; 20] {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    type HmacSha1 = Hmac<Sha1>;

    let mut mac = HmacSha1::new_from_slice(auth_key)
        .expect("HMAC-SHA1 accepts any key length");
    mac.update(data);
    mac.update(&roc.to_be_bytes());
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 20];
    out.copy_from_slice(&result);
    out
}

// ---------------------------------------------------------------------------
// Pure-Rust crypto backend (feature = "pure-rust-crypto")
// ---------------------------------------------------------------------------

#[cfg(feature = "pure-rust-crypto")]
mod pure_rust_backend {
    use super::*;

    /// Pure-Rust SRTP crypto backend.
    ///
    /// Implements AES-128-CM encryption and HMAC-SHA1 authentication
    /// per RFC 3711 using pure-Rust crates (`aes`, `hmac`, `sha1`).
    pub struct PureRustSrtp {
        /// Original master key material.
        key_material: SrtpKeyMaterial,
        /// Derived session keys.
        derived: DerivedKeys,
        /// Rollover counter for RTP (upper 32 bits of 48-bit packet index).
        roc: u32,
        /// Last received RTP sequence number (for ROC estimation).
        last_seq: u16,
        /// Whether we have received any RTP packet yet.
        seq_initialized: bool,
        /// Replay protection window.
        replay_window: ReplayWindow,
        /// SRTCP index counter (31 bits + E flag).
        srtcp_index: u32,
        /// SRTCP replay window.
        srtcp_replay_window: ReplayWindow,
    }

    impl PureRustSrtp {
        pub fn new(key_material: SrtpKeyMaterial) -> Result<Self, SrtpError> {
            key_material.validate()?;

            let derived = DerivedKeys::derive(
                &key_material.key,
                &key_material.salt,
                key_material.suite.key_length(),
            );

            Ok(Self {
                key_material,
                derived,
                roc: 0,
                last_seq: 0,
                seq_initialized: false,
                replay_window: ReplayWindow::new(),
                srtcp_index: 0,
                srtcp_replay_window: ReplayWindow::new(),
            })
        }

        /// Estimate the packet index from the RTP sequence number.
        ///
        /// Per RFC 3711 Section 3.3.1, we estimate the ROC from the
        /// received sequence number to compute the full 48-bit index.
        ///
        /// The algorithm uses signed 16-bit distance to determine if
        /// the sequence number has wrapped forward (ROC+1), backward
        /// (ROC-1), or is in the same epoch (ROC).
        fn estimate_packet_index(&self, seq: u16) -> (u64, u32) {
            if !self.seq_initialized {
                return (seq as u64, 0);
            }

            // Compute the signed distance from last_seq to seq.
            // Positive = seq is ahead, negative = seq is behind.
            let delta = seq.wrapping_sub(self.last_seq) as i16;

            let v = if delta > 0 {
                // seq is ahead of last_seq (normal forward case).
                // Check if we wrapped: if last_seq is high and seq is
                // low, wrapping_sub gives a small positive number.
                self.roc
            } else if delta == 0 {
                // Same sequence number (replay).
                self.roc
            } else {
                // delta < 0: seq is behind last_seq.
                // If it's close behind, it's just reordering (same ROC).
                // If it's far behind (wrapped around), ROC changed.
                if (delta as i32).unsigned_abs() > 0x8000 {
                    // Very far behind in signed terms means seq is actually
                    // ahead with a wrap: seq crossed 0xFFFF -> 0x0000.
                    // But wait: negative delta > 0x8000 in magnitude means
                    // wrapping_sub gave a large negative. In u16 terms,
                    // that means seq - last_seq > 0x8000, which indicates
                    // seq is actually far ahead with wrap. Increment ROC.
                    self.roc.wrapping_add(1)
                } else {
                    // Close behind: just reordering, same ROC.
                    self.roc
                }
            };

            // Special case: if last_seq >= 0x8000 and seq is small (near 0),
            // the signed delta will be negative with magnitude > 0x8000.
            // This is the forward-wrap case: last_seq = 0xFFFE, seq = 0x0001.
            // delta = 0x0001 - 0xFFFE = 0x0003 (as wrapping u16), which as
            // i16 is +3. So it hits the delta > 0 branch and returns self.roc.
            // BUT the ROC should be incremented here!
            //
            // Re-examine: wrapping_sub(0xFFFE) for seq=0x0001:
            // 0x0001_u16.wrapping_sub(0xFFFE) = 0x0003 (u16), as i16 = 3 > 0.
            // So delta=3, we return self.roc. But we need self.roc+1!
            //
            // The update_roc function will handle this: after the packet is
            // verified, update_roc checks estimated_roc > self.roc || seq > last_seq.
            // Since estimated_roc == self.roc and seq < last_seq, it won't update.
            // We need a different approach.

            // Actually, we need to detect the wrap differently. The standard
            // approach from libsrtp:
            // if (seq < last_seq) and the distance wraps, it's ROC+1.
            // if (seq > last_seq) and distance is huge, it's ROC-1 (late packet
            // from before a wrap).

            // Let's implement the standard algorithm correctly:
            let v = self.estimate_roc_standard(seq);
            let index = ((v as u64) << 16) | (seq as u64);
            (index, v)
        }

        /// Standard ROC estimation per RFC 3711 Section 3.3.1,
        /// corrected for the signed distance interpretation.
        fn estimate_roc_standard(&self, seq: u16) -> u32 {
            // Compute the distance as signed: positive means forward.
            // seq.wrapping_sub(last_seq) reinterpret as i16 gives:
            //   positive if seq is 1..32767 ahead of last_seq (modular)
            //   negative if seq is 1..32768 behind last_seq (modular)
            let diff = seq.wrapping_sub(self.last_seq) as i16;

            if diff > 0 {
                // seq is ahead of last_seq.
                // If last_seq was near the top and seq near the bottom,
                // wrapping_sub gives a small positive number, meaning
                // seq crossed the wrap boundary forward. But did the
                // ROC actually need to increment? Only if last_seq was
                // very high and seq is very low (the wrap happened).
                //
                // The key insight: if last_seq >= 0x8000 and seq < last_seq
                // (in unsigned sense), then the wrap happened. But in this
                // case, diff would be: e.g., last_seq=0xFFFF, seq=0x0001,
                // diff = 0x0001 - 0xFFFF (wrapping) = 0x0002, i16 = 2.
                // seq < last_seq in unsigned, so the wrap happened.
                if seq < self.last_seq {
                    // Forward wrap: seq crossed 0xFFFF -> 0x0000.
                    self.roc.wrapping_add(1)
                } else {
                    // Normal forward, no wrap.
                    self.roc
                }
            } else if diff < 0 {
                // seq is behind last_seq.
                if seq > self.last_seq {
                    // Backward wrap: seq is numerically larger but the signed
                    // distance says it's behind. This means last_seq was near 0
                    // and seq is near 65535 (late packet from before the wrap).
                    self.roc.wrapping_sub(1)
                } else {
                    // Normal reordering, same ROC.
                    self.roc
                }
            } else {
                // Same sequence number (duplicate/replay).
                self.roc
            }
        }

        /// Update ROC after successfully processing a packet.
        fn update_roc(&mut self, seq: u16, estimated_roc: u32) {
            if !self.seq_initialized {
                self.last_seq = seq;
                self.roc = 0;
                self.seq_initialized = true;
                return;
            }

            if estimated_roc > self.roc
                || (estimated_roc == self.roc && seq > self.last_seq)
            {
                self.last_seq = seq;
                self.roc = estimated_roc;
            }
        }
    }

    impl SrtpCrypto for PureRustSrtp {
        fn backend_name(&self) -> &str {
            "pure-rust"
        }

        fn protect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            if packet.len() < RTP_HEADER_MIN {
                return Err(SrtpError::PacketTooShort {
                    min: RTP_HEADER_MIN,
                    actual: packet.len(),
                });
            }

            // 1. Parse RTP header to get SSRC and sequence number.
            let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
            let seq = u16::from_be_bytes([packet[2], packet[3]]);
            let cc = (packet[0] & 0x0F) as usize;
            let header_len = RTP_HEADER_MIN + cc * 4;

            if packet.len() < header_len {
                return Err(SrtpError::PacketTooShort {
                    min: header_len,
                    actual: packet.len(),
                });
            }

            // Handle RTP header extensions.
            let mut payload_offset = header_len;
            if (packet[0] & 0x10) != 0 && packet.len() >= header_len + 4 {
                let ext_len = u16::from_be_bytes([
                    packet[header_len + 2],
                    packet[header_len + 3],
                ]) as usize;
                payload_offset = header_len + 4 + ext_len * 4;
            }

            if payload_offset > packet.len() {
                return Err(SrtpError::PacketTooShort {
                    min: payload_offset,
                    actual: packet.len(),
                });
            }

            // 2. Compute packet index = (ROC << 16) | seq
            let (packet_index, estimated_roc) = self.estimate_packet_index(seq);

            // 3-5. Encrypt payload in-place using AES-CM with pre-computed cipher.
            aes_cm_encrypt_decrypt_with_cipher(
                &self.derived.rtp_cipher,
                &self.derived.salt,
                ssrc,
                packet_index,
                &mut packet[payload_offset..],
            );

            // 6. Compute HMAC-SHA1 over the entire packet (header + encrypted payload).
            //    Use the estimated ROC (not self.roc) so that the HMAC is
            //    consistent with what the receiver will estimate for the same seq.
            let hmac = compute_hmac_sha1(
                &self.derived.auth_key,
                packet,
                estimated_roc,
            );

            // 7. Append authentication tag (truncated to tag_len).
            let tag_len = self.key_material.suite.auth_tag_length();
            packet.extend_from_slice(&hmac[..tag_len]);

            // 8. Update ROC for sequence tracking.
            self.update_roc(seq, estimated_roc);

            Ok(())
        }

        fn unprotect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            let tag_len = self.key_material.suite.auth_tag_length();

            if packet.len() < RTP_HEADER_MIN + tag_len {
                return Err(SrtpError::PacketTooShort {
                    min: RTP_HEADER_MIN + tag_len,
                    actual: packet.len(),
                });
            }

            // 1. Parse RTP header.
            let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
            let seq = u16::from_be_bytes([packet[2], packet[3]]);
            let cc = (packet[0] & 0x0F) as usize;
            let header_len = RTP_HEADER_MIN + cc * 4;

            // Handle RTP header extensions.
            let mut payload_offset = header_len;
            if (packet[0] & 0x10) != 0 && packet.len() >= header_len + 4 {
                let ext_len = u16::from_be_bytes([
                    packet[header_len + 2],
                    packet[header_len + 3],
                ]) as usize;
                payload_offset = header_len + 4 + ext_len * 4;
            }

            // 2. Estimate packet index and ROC.
            let (packet_index, estimated_roc) = self.estimate_packet_index(seq);

            // 3. Check replay window.
            if !self.replay_window.check(packet_index) {
                return Err(SrtpError::ReplayDetected(packet_index));
            }

            // 4. Verify authentication tag (avoid heap allocation).
            let auth_portion_len = packet.len() - tag_len;
            // Copy tag to stack buffer (max 10 bytes) to avoid Vec allocation.
            let mut received_tag = [0u8; 10];
            received_tag[..tag_len].copy_from_slice(&packet[auth_portion_len..]);
            let auth_portion = &packet[..auth_portion_len];

            let computed_hmac = compute_hmac_sha1(
                &self.derived.auth_key,
                auth_portion,
                estimated_roc,
            );

            // Constant-time comparison of tag.
            let computed_tag = &computed_hmac[..tag_len];
            let mut diff = 0u8;
            for i in 0..tag_len {
                diff |= received_tag[i] ^ computed_tag[i];
            }
            if diff != 0 {
                return Err(SrtpError::AuthenticationFailed);
            }

            // 5. Remove auth tag.
            packet.truncate(auth_portion_len);

            // 6. Decrypt payload using pre-computed cipher.
            if payload_offset < packet.len() {
                aes_cm_encrypt_decrypt_with_cipher(
                    &self.derived.rtp_cipher,
                    &self.derived.salt,
                    ssrc,
                    packet_index,
                    &mut packet[payload_offset..],
                );
            }

            // 7. Update replay window and ROC.
            self.replay_window.update(packet_index);
            self.update_roc(seq, estimated_roc);

            Ok(())
        }

        fn protect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            if packet.len() < RTCP_HEADER_MIN {
                return Err(SrtpError::PacketTooShort {
                    min: RTCP_HEADER_MIN,
                    actual: packet.len(),
                });
            }

            let ssrc = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);

            // SRTCP uses a 31-bit index with an E (encryption) flag.
            let srtcp_index = self.srtcp_index;
            self.srtcp_index = self.srtcp_index.wrapping_add(1) & 0x7FFFFFFF;

            // Encrypt payload (everything after the first 8 bytes).
            if packet.len() > RTCP_HEADER_MIN {
                aes_cm_encrypt_decrypt_with_cipher(
                    &self.derived.rtcp_cipher,
                    &self.derived.rtcp_salt,
                    ssrc,
                    srtcp_index as u64,
                    &mut packet[RTCP_HEADER_MIN..],
                );
            }

            // Append E flag (1 = encrypted) || SRTCP index (31 bits).
            let e_index = 0x80000000u32 | srtcp_index;
            packet.extend_from_slice(&e_index.to_be_bytes());

            // Compute HMAC-SHA1 over the entire packet (including E||index).
            let hmac = compute_hmac_sha1(
                &self.derived.rtcp_auth_key,
                packet,
                0, // No ROC for RTCP; the index is appended.
            );

            let tag_len = self.key_material.suite.auth_tag_length();
            packet.extend_from_slice(&hmac[..tag_len]);

            Ok(())
        }

        fn unprotect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            let tag_len = self.key_material.suite.auth_tag_length();
            // Need at least: header(8) + E||index(4) + auth_tag
            let min_len = RTCP_HEADER_MIN + 4 + tag_len;

            if packet.len() < min_len {
                return Err(SrtpError::PacketTooShort {
                    min: min_len,
                    actual: packet.len(),
                });
            }

            // Extract auth tag (stack buffer, no heap allocation).
            let auth_tag_start = packet.len() - tag_len;
            let mut received_tag = [0u8; 10];
            received_tag[..tag_len].copy_from_slice(&packet[auth_tag_start..]);

            // Extract E||index (4 bytes before auth tag).
            let e_index_start = auth_tag_start - 4;
            let e_index = u32::from_be_bytes([
                packet[e_index_start],
                packet[e_index_start + 1],
                packet[e_index_start + 2],
                packet[e_index_start + 3],
            ]);
            let is_encrypted = (e_index & 0x80000000) != 0;
            let srtcp_index = e_index & 0x7FFFFFFF;

            // Verify auth tag over packet up to (but not including) auth tag.
            let auth_portion = &packet[..auth_tag_start];
            let computed_hmac = compute_hmac_sha1(
                &self.derived.rtcp_auth_key,
                auth_portion,
                0,
            );

            let computed_tag = &computed_hmac[..tag_len];
            let mut diff = 0u8;
            for i in 0..tag_len {
                diff |= received_tag[i] ^ computed_tag[i];
            }
            if diff != 0 {
                return Err(SrtpError::AuthenticationFailed);
            }

            // Check replay.
            if !self.srtcp_replay_window.check(srtcp_index as u64) {
                return Err(SrtpError::ReplayDetected(srtcp_index as u64));
            }
            self.srtcp_replay_window.update(srtcp_index as u64);

            // Remove auth tag and E||index.
            packet.truncate(e_index_start);

            // Decrypt if E flag is set, using pre-computed cipher.
            if is_encrypted && packet.len() > RTCP_HEADER_MIN {
                let ssrc = u32::from_be_bytes([
                    packet[4], packet[5], packet[6], packet[7],
                ]);
                aes_cm_encrypt_decrypt_with_cipher(
                    &self.derived.rtcp_cipher,
                    &self.derived.rtcp_salt,
                    ssrc,
                    srtcp_index as u64,
                    &mut packet[RTCP_HEADER_MIN..],
                );
            }

            Ok(())
        }
    }
}

#[cfg(feature = "pure-rust-crypto")]
pub use pure_rust_backend::PureRustSrtp;

// ---------------------------------------------------------------------------
// OpenSSL crypto backend (feature = "openssl-crypto")
// ---------------------------------------------------------------------------

#[cfg(feature = "openssl-crypto")]
mod openssl_backend {
    use super::*;

    /// OpenSSL-based SRTP crypto backend.
    ///
    /// Uses libssl/libcrypto for SRTP operations. Currently a structural
    /// placeholder that will integrate with the `openssl` crate.
    pub struct OpenSslSrtp {
        #[allow(dead_code)]
        key_material: SrtpKeyMaterial,
    }

    impl OpenSslSrtp {
        pub fn new(key_material: SrtpKeyMaterial) -> Result<Self, SrtpError> {
            key_material.validate()?;
            Ok(Self { key_material })
        }
    }

    impl SrtpCrypto for OpenSslSrtp {
        fn backend_name(&self) -> &str {
            "openssl"
        }

        fn protect_rtp(&mut self, _packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            Err(SrtpError::ProtectFailed(
                "openssl SRTP not yet implemented".into(),
            ))
        }

        fn unprotect_rtp(&mut self, _packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            Err(SrtpError::UnprotectFailed(
                "openssl SRTP not yet implemented".into(),
            ))
        }

        fn protect_rtcp(&mut self, _packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            Err(SrtpError::ProtectFailed(
                "openssl SRTCP not yet implemented".into(),
            ))
        }

        fn unprotect_rtcp(&mut self, _packet: &mut Vec<u8>) -> Result<(), SrtpError> {
            Err(SrtpError::UnprotectFailed(
                "openssl SRTCP not yet implemented".into(),
            ))
        }
    }
}

#[cfg(feature = "openssl-crypto")]
pub use openssl_backend::OpenSslSrtp;

// ---------------------------------------------------------------------------
// AEAD AES-GCM backend (RFC 7714)
// ---------------------------------------------------------------------------

/// SRTP AEAD AES-GCM backend.
///
/// Implements AEAD_AES_128_GCM and AEAD_AES_256_GCM per RFC 7714.
/// Uses the `aes-gcm` crate for authenticated encryption.
pub struct AeadGcmSrtp {
    /// Original keying material.
    key_material: SrtpKeyMaterial,
    /// Session encryption key.
    session_key: Vec<u8>,
    /// Session salt (IV base), 12 bytes.
    session_salt: Vec<u8>,
    /// Rollover counter for RTP.
    roc: u32,
    /// Last received RTP sequence number.
    last_seq: u16,
    /// Whether we have received any RTP packet yet.
    seq_initialized: bool,
    /// Replay protection window.
    replay_window: ReplayWindow,
    /// SRTCP index counter.
    srtcp_index: u32,
    /// SRTCP replay window.
    srtcp_replay_window: ReplayWindow,
}

impl AeadGcmSrtp {
    /// Create a new AEAD GCM SRTP context.
    pub fn new(key_material: SrtpKeyMaterial) -> Result<Self, SrtpError> {
        key_material.validate()?;
        if !key_material.suite.is_aead() {
            return Err(SrtpError::ProtectFailed(
                "AeadGcmSrtp requires an AEAD suite".into(),
            ));
        }

        Ok(Self {
            session_key: key_material.key.clone(),
            session_salt: key_material.salt.clone(),
            key_material,
            roc: 0,
            last_seq: 0,
            seq_initialized: false,
            replay_window: ReplayWindow::new(),
            srtcp_index: 0,
            srtcp_replay_window: ReplayWindow::new(),
        })
    }

    /// Construct the 12-byte IV per RFC 7714 Section 8.1.
    ///
    /// IV = (0x00000000 || SSRC || 0x00000000) XOR salt
    /// Then XOR the packet index into the last 4 bytes.
    fn construct_rtp_iv(&self, ssrc: u32, packet_index: u64) -> [u8; 12] {
        let mut iv = [0u8; 12];
        // Copy salt as base
        iv.copy_from_slice(&self.session_salt[..12]);
        // XOR SSRC into bytes 4..8
        let ssrc_bytes = ssrc.to_be_bytes();
        iv[4] ^= ssrc_bytes[0];
        iv[5] ^= ssrc_bytes[1];
        iv[6] ^= ssrc_bytes[2];
        iv[7] ^= ssrc_bytes[3];
        // XOR packet index (48-bit) into bytes 6..12
        let pi_bytes = packet_index.to_be_bytes(); // 8 bytes
        iv[6] ^= pi_bytes[2];
        iv[7] ^= pi_bytes[3];
        iv[8] ^= pi_bytes[4];
        iv[9] ^= pi_bytes[5];
        iv[10] ^= pi_bytes[6];
        iv[11] ^= pi_bytes[7];
        iv
    }

    /// Construct the 12-byte IV for SRTCP.
    fn construct_rtcp_iv(&self, ssrc: u32, srtcp_index: u32) -> [u8; 12] {
        let mut iv = [0u8; 12];
        iv.copy_from_slice(&self.session_salt[..12]);
        let ssrc_bytes = ssrc.to_be_bytes();
        iv[4] ^= ssrc_bytes[0];
        iv[5] ^= ssrc_bytes[1];
        iv[6] ^= ssrc_bytes[2];
        iv[7] ^= ssrc_bytes[3];
        let idx_bytes = srtcp_index.to_be_bytes();
        iv[8] ^= idx_bytes[0];
        iv[9] ^= idx_bytes[1];
        iv[10] ^= idx_bytes[2];
        iv[11] ^= idx_bytes[3];
        iv
    }

    /// Estimate packet index (same algorithm as PureRustSrtp).
    fn estimate_packet_index(&self, seq: u16) -> (u64, u32) {
        if !self.seq_initialized {
            return (seq as u64, 0);
        }
        let v = self.estimate_roc_standard(seq);
        let index = ((v as u64) << 16) | (seq as u64);
        (index, v)
    }

    fn estimate_roc_standard(&self, seq: u16) -> u32 {
        let diff = seq.wrapping_sub(self.last_seq) as i16;
        if diff > 0 {
            if seq < self.last_seq {
                self.roc.wrapping_add(1)
            } else {
                self.roc
            }
        } else if diff < 0 {
            if seq > self.last_seq {
                self.roc.wrapping_sub(1)
            } else {
                self.roc
            }
        } else {
            self.roc
        }
    }

    fn update_roc(&mut self, seq: u16, estimated_roc: u32) {
        if !self.seq_initialized {
            self.last_seq = seq;
            self.roc = 0;
            self.seq_initialized = true;
            return;
        }
        if estimated_roc > self.roc
            || (estimated_roc == self.roc && seq > self.last_seq)
        {
            self.last_seq = seq;
            self.roc = estimated_roc;
        }
    }
}

impl SrtpCrypto for AeadGcmSrtp {
    fn backend_name(&self) -> &str {
        "aead-gcm"
    }

    fn protect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        use aes_gcm::{Aes128Gcm, Aes256Gcm, AeadInPlace, KeyInit, Nonce};

        if packet.len() < RTP_HEADER_MIN {
            return Err(SrtpError::PacketTooShort {
                min: RTP_HEADER_MIN,
                actual: packet.len(),
            });
        }

        let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        let cc = (packet[0] & 0x0F) as usize;
        let mut header_len = RTP_HEADER_MIN + cc * 4;

        // Handle extension headers
        if (packet[0] & 0x10) != 0 && packet.len() >= header_len + 4 {
            let ext_len = u16::from_be_bytes([
                packet[header_len + 2],
                packet[header_len + 3],
            ]) as usize;
            header_len = header_len + 4 + ext_len * 4;
        }

        if header_len > packet.len() {
            return Err(SrtpError::PacketTooShort {
                min: header_len,
                actual: packet.len(),
            });
        }

        let (packet_index, estimated_roc) = self.estimate_packet_index(seq);
        let iv = self.construct_rtp_iv(ssrc, packet_index);
        let nonce = aes_gcm::Nonce::from_slice(&iv);

        // AAD = RTP header (authenticated but not encrypted)
        let aad = packet[..header_len].to_vec();
        // Payload to encrypt
        let mut payload = packet[header_len..].to_vec();

        // Encrypt and authenticate
        let tag = match self.key_material.suite {
            SrtpCryptoSuite::AeadAes128Gcm => {
                let cipher = Aes128Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::ProtectFailed(format!("AES-128-GCM key init: {}", e)))?;
                cipher
                    .encrypt_in_place_detached(nonce, &aad, &mut payload)
                    .map_err(|e| SrtpError::ProtectFailed(format!("AES-128-GCM encrypt: {}", e)))?
            }
            SrtpCryptoSuite::AeadAes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::ProtectFailed(format!("AES-256-GCM key init: {}", e)))?;
                cipher
                    .encrypt_in_place_detached(nonce, &aad, &mut payload)
                    .map_err(|e| SrtpError::ProtectFailed(format!("AES-256-GCM encrypt: {}", e)))?
            }
            _ => return Err(SrtpError::ProtectFailed("not a GCM suite".into())),
        };

        // Rebuild packet: header + encrypted payload + 16-byte auth tag
        packet.truncate(header_len);
        packet.extend_from_slice(&payload);
        packet.extend_from_slice(&tag);

        self.update_roc(seq, estimated_roc);
        Ok(())
    }

    fn unprotect_rtp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        use aes_gcm::{Aes128Gcm, Aes256Gcm, AeadInPlace, KeyInit, Tag};

        let tag_len = 16; // GCM tag is always 16 bytes
        if packet.len() < RTP_HEADER_MIN + tag_len {
            return Err(SrtpError::PacketTooShort {
                min: RTP_HEADER_MIN + tag_len,
                actual: packet.len(),
            });
        }

        let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        let cc = (packet[0] & 0x0F) as usize;
        let mut header_len = RTP_HEADER_MIN + cc * 4;

        if (packet[0] & 0x10) != 0 && packet.len() >= header_len + 4 {
            let ext_len = u16::from_be_bytes([
                packet[header_len + 2],
                packet[header_len + 3],
            ]) as usize;
            header_len = header_len + 4 + ext_len * 4;
        }

        let (packet_index, estimated_roc) = self.estimate_packet_index(seq);

        if !self.replay_window.check(packet_index) {
            return Err(SrtpError::ReplayDetected(packet_index));
        }

        let iv = self.construct_rtp_iv(ssrc, packet_index);
        let nonce = aes_gcm::Nonce::from_slice(&iv);

        let aad = packet[..header_len].to_vec();

        // Extract tag from end
        let tag_start = packet.len() - tag_len;
        let mut tag_bytes = [0u8; 16];
        tag_bytes.copy_from_slice(&packet[tag_start..]);
        let tag = Tag::from_slice(&tag_bytes);

        // Ciphertext (between header and tag)
        let mut ciphertext = packet[header_len..tag_start].to_vec();

        // Decrypt and verify
        match self.key_material.suite {
            SrtpCryptoSuite::AeadAes128Gcm => {
                let cipher = Aes128Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::UnprotectFailed(format!("AES-128-GCM key init: {}", e)))?;
                cipher
                    .decrypt_in_place_detached(nonce, &aad, &mut ciphertext, tag)
                    .map_err(|_| SrtpError::AuthenticationFailed)?;
            }
            SrtpCryptoSuite::AeadAes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::UnprotectFailed(format!("AES-256-GCM key init: {}", e)))?;
                cipher
                    .decrypt_in_place_detached(nonce, &aad, &mut ciphertext, tag)
                    .map_err(|_| SrtpError::AuthenticationFailed)?;
            }
            _ => return Err(SrtpError::UnprotectFailed("not a GCM suite".into())),
        };

        // Rebuild packet: header + decrypted payload
        packet.truncate(header_len);
        packet.extend_from_slice(&ciphertext);

        self.replay_window.update(packet_index);
        self.update_roc(seq, estimated_roc);
        Ok(())
    }

    fn protect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        use aes_gcm::{Aes128Gcm, Aes256Gcm, AeadInPlace, KeyInit};

        if packet.len() < RTCP_HEADER_MIN {
            return Err(SrtpError::PacketTooShort {
                min: RTCP_HEADER_MIN,
                actual: packet.len(),
            });
        }

        let ssrc = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let srtcp_index = self.srtcp_index;
        self.srtcp_index = self.srtcp_index.wrapping_add(1) & 0x7FFFFFFF;

        let iv = self.construct_rtcp_iv(ssrc, srtcp_index);
        let nonce = aes_gcm::Nonce::from_slice(&iv);

        // AAD = RTCP header (first 8 bytes) + E||index
        let e_index = 0x80000000u32 | srtcp_index;
        let mut aad = packet[..RTCP_HEADER_MIN].to_vec();
        aad.extend_from_slice(&e_index.to_be_bytes());

        // Payload to encrypt
        let mut payload = if packet.len() > RTCP_HEADER_MIN {
            packet[RTCP_HEADER_MIN..].to_vec()
        } else {
            Vec::new()
        };

        let tag = match self.key_material.suite {
            SrtpCryptoSuite::AeadAes128Gcm => {
                let cipher = Aes128Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::ProtectFailed(format!("RTCP GCM key: {}", e)))?;
                cipher
                    .encrypt_in_place_detached(nonce, &aad, &mut payload)
                    .map_err(|e| SrtpError::ProtectFailed(format!("RTCP GCM encrypt: {}", e)))?
            }
            SrtpCryptoSuite::AeadAes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::ProtectFailed(format!("RTCP GCM key: {}", e)))?;
                cipher
                    .encrypt_in_place_detached(nonce, &aad, &mut payload)
                    .map_err(|e| SrtpError::ProtectFailed(format!("RTCP GCM encrypt: {}", e)))?
            }
            _ => return Err(SrtpError::ProtectFailed("not GCM".into())),
        };

        // Rebuild: header + encrypted payload + E||index + tag
        packet.truncate(RTCP_HEADER_MIN);
        packet.extend_from_slice(&payload);
        packet.extend_from_slice(&e_index.to_be_bytes());
        packet.extend_from_slice(&tag);

        Ok(())
    }

    fn unprotect_rtcp(&mut self, packet: &mut Vec<u8>) -> Result<(), SrtpError> {
        use aes_gcm::{Aes128Gcm, Aes256Gcm, AeadInPlace, KeyInit, Tag};

        let tag_len = 16;
        let min_len = RTCP_HEADER_MIN + 4 + tag_len; // header + E||index + tag
        if packet.len() < min_len {
            return Err(SrtpError::PacketTooShort {
                min: min_len,
                actual: packet.len(),
            });
        }

        // Extract tag (last 16 bytes)
        let tag_start = packet.len() - tag_len;
        let mut tag_bytes = [0u8; 16];
        tag_bytes.copy_from_slice(&packet[tag_start..]);
        let tag = Tag::from_slice(&tag_bytes);

        // Extract E||index (4 bytes before tag)
        let e_index_start = tag_start - 4;
        let e_index = u32::from_be_bytes([
            packet[e_index_start],
            packet[e_index_start + 1],
            packet[e_index_start + 2],
            packet[e_index_start + 3],
        ]);
        let _is_encrypted = (e_index & 0x80000000) != 0;
        let srtcp_index = e_index & 0x7FFFFFFF;

        if !self.srtcp_replay_window.check(srtcp_index as u64) {
            return Err(SrtpError::ReplayDetected(srtcp_index as u64));
        }

        let ssrc = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let iv = self.construct_rtcp_iv(ssrc, srtcp_index);
        let nonce = aes_gcm::Nonce::from_slice(&iv);

        // AAD = header + E||index
        let mut aad = packet[..RTCP_HEADER_MIN].to_vec();
        aad.extend_from_slice(&e_index.to_be_bytes());

        // Ciphertext = between header and E||index
        let mut ciphertext = packet[RTCP_HEADER_MIN..e_index_start].to_vec();

        match self.key_material.suite {
            SrtpCryptoSuite::AeadAes128Gcm => {
                let cipher = Aes128Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::UnprotectFailed(format!("RTCP GCM key: {}", e)))?;
                cipher
                    .decrypt_in_place_detached(nonce, &aad, &mut ciphertext, tag)
                    .map_err(|_| SrtpError::AuthenticationFailed)?;
            }
            SrtpCryptoSuite::AeadAes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.session_key)
                    .map_err(|e| SrtpError::UnprotectFailed(format!("RTCP GCM key: {}", e)))?;
                cipher
                    .decrypt_in_place_detached(nonce, &aad, &mut ciphertext, tag)
                    .map_err(|_| SrtpError::AuthenticationFailed)?;
            }
            _ => return Err(SrtpError::UnprotectFailed("not GCM".into())),
        };

        self.srtcp_replay_window.update(srtcp_index as u64);

        // Rebuild: header + decrypted payload
        packet.truncate(RTCP_HEADER_MIN);
        packet.extend_from_slice(&ciphertext);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory: create the appropriate backend
// ---------------------------------------------------------------------------

/// Create an SRTP crypto instance using the best available backend.
///
/// For AEAD suites (AES-GCM), uses the `AeadGcmSrtp` backend directly.
/// For non-AEAD suites, uses the feature-gated backend (pure-Rust or OpenSSL).
pub fn create_srtp_crypto(
    key_material: SrtpKeyMaterial,
) -> Result<Box<dyn SrtpCrypto>, SrtpError> {
    // AEAD suites always use the GCM backend.
    if key_material.suite.is_aead() {
        return Ok(Box::new(AeadGcmSrtp::new(key_material)?));
    }

    #[cfg(feature = "pure-rust-crypto")]
    {
        return Ok(Box::new(PureRustSrtp::new(key_material)?));
    }

    #[cfg(feature = "openssl-crypto")]
    {
        return Ok(Box::new(OpenSslSrtp::new(key_material)?));
    }

    #[cfg(not(any(feature = "pure-rust-crypto", feature = "openssl-crypto")))]
    {
        Err(SrtpError::BackendUnavailable(
            "No SRTP crypto backend enabled. Enable 'pure-rust-crypto' or 'openssl-crypto' feature."
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Crypto suite property tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_crypto_suite_properties() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        assert_eq!(suite.key_length(), 16);
        assert_eq!(suite.salt_length(), 14);
        assert_eq!(suite.master_key_length(), 30);
        assert_eq!(suite.auth_tag_length(), 10);
        assert_eq!(suite.sdp_name(), "AES_CM_128_HMAC_SHA1_80");
    }

    #[test]
    fn test_crypto_suite_256() {
        let suite = SrtpCryptoSuite::Aes256CmHmacSha1_80;
        assert_eq!(suite.key_length(), 32);
        assert_eq!(suite.master_key_length(), 46);
    }

    #[test]
    fn test_crypto_suite_from_sdp() {
        assert_eq!(
            SrtpCryptoSuite::from_sdp_name("AES_CM_128_HMAC_SHA1_80"),
            Some(SrtpCryptoSuite::AesCm128HmacSha1_80)
        );
        assert_eq!(SrtpCryptoSuite::from_sdp_name("UNKNOWN"), None);
    }

    #[test]
    fn test_key_material_validation() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0u8; 16];
        let salt = vec![0u8; 14];
        let km = SrtpKeyMaterial::new(suite, key, salt);
        assert!(km.validate().is_ok());
    }

    #[test]
    fn test_key_material_invalid_key() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0u8; 8]; // Too short
        let salt = vec![0u8; 14];
        let km = SrtpKeyMaterial::new(suite, key, salt);
        assert!(km.validate().is_err());
    }

    #[test]
    fn test_create_srtp_crypto() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0u8; 16];
        let salt = vec![0u8; 14];
        let km = SrtpKeyMaterial::new(suite, key, salt);
        let result = create_srtp_crypto(km);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Replay window tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_replay_window_new_accepts_first() {
        let window = ReplayWindow::new();
        assert!(window.check(0));
        assert!(window.check(100));
    }

    #[test]
    fn test_replay_window_rejects_duplicate() {
        let mut window = ReplayWindow::new();
        window.update(100);
        assert!(!window.check(100)); // Already received.
    }

    #[test]
    fn test_replay_window_accepts_new() {
        let mut window = ReplayWindow::new();
        window.update(100);
        assert!(window.check(101)); // New, higher.
        assert!(window.check(200)); // Much higher.
    }

    #[test]
    fn test_replay_window_accepts_within_window() {
        let mut window = ReplayWindow::new();
        window.update(100);
        // Index 50 is within the window (100 - 50 = 50 < 128).
        assert!(window.check(50));
    }

    #[test]
    fn test_replay_window_rejects_old() {
        let mut window = ReplayWindow::new();
        window.update(200);
        // Index 50 is outside the window (200 - 50 = 150 >= 128).
        assert!(!window.check(50));
    }

    #[test]
    fn test_replay_window_sliding() {
        let mut window = ReplayWindow::new();
        // Receive packets 0..200.
        for i in 0..200 {
            assert!(window.check(i));
            window.update(i);
        }
        // All should be marked as received.
        for i in 72..200 {
            // Only those within window of highest (199).
            assert!(!window.check(i));
        }
        // Next packet should be fine.
        assert!(window.check(200));
    }

    // -----------------------------------------------------------------------
    // AES-CM and HMAC tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_aes_cm_encrypt_decrypt_roundtrip() {
        let key = [0u8; 16]; // All zeros key.
        let salt = [0u8; 14]; // All zeros salt.
        let ssrc = 0x12345678u32;
        let packet_index = 0u64;
        let original_payload = b"Hello, SRTP world! This is a test payload.".to_vec();
        let mut payload = original_payload.clone();

        // Encrypt.
        aes_cm_encrypt_decrypt(&key, &salt, ssrc, packet_index, &mut payload);
        // Payload should be different after encryption.
        assert_ne!(payload, original_payload);

        // Decrypt (AES-CM is symmetric: encrypt again = decrypt).
        aes_cm_encrypt_decrypt(&key, &salt, ssrc, packet_index, &mut payload);
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn test_aes_cm_different_ssrc_different_ciphertext() {
        let key = [0u8; 16];
        let salt = [0u8; 14];
        let payload_orig = vec![0xAA; 64];

        let mut p1 = payload_orig.clone();
        aes_cm_encrypt_decrypt(&key, &salt, 1, 0, &mut p1);

        let mut p2 = payload_orig.clone();
        aes_cm_encrypt_decrypt(&key, &salt, 2, 0, &mut p2);

        // Different SSRCs should produce different ciphertexts.
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_aes_cm_different_index_different_ciphertext() {
        let key = [0u8; 16];
        let salt = [0u8; 14];
        let payload_orig = vec![0xBB; 64];

        let mut p1 = payload_orig.clone();
        aes_cm_encrypt_decrypt(&key, &salt, 1, 0, &mut p1);

        let mut p2 = payload_orig.clone();
        aes_cm_encrypt_decrypt(&key, &salt, 1, 1, &mut p2);

        // Different packet indices should produce different ciphertexts.
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_hmac_sha1_deterministic() {
        let key = [0u8; 20];
        let data = b"test data for hmac";
        let roc = 0u32;

        let h1 = compute_hmac_sha1(&key, data, roc);
        let h2 = compute_hmac_sha1(&key, data, roc);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 20); // Full HMAC-SHA1 output.
    }

    #[test]
    fn test_hmac_sha1_different_roc() {
        let key = [0u8; 20];
        let data = b"test data";

        let h1 = compute_hmac_sha1(&key, data, 0);
        let h2 = compute_hmac_sha1(&key, data, 1);
        assert_ne!(h1, h2);
    }

    // -----------------------------------------------------------------------
    // Key derivation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_key_derivation_deterministic() {
        let master_key = [0u8; 16];
        let master_salt = [0u8; 14];

        let k1 = derive_session_key(&master_key, &master_salt, LABEL_CIPHER_KEY, 16);
        let k2 = derive_session_key(&master_key, &master_salt, LABEL_CIPHER_KEY, 16);
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 16);
    }

    #[test]
    fn test_key_derivation_different_labels() {
        let master_key = [0u8; 16];
        let master_salt = [0u8; 14];

        let cipher_key = derive_session_key(&master_key, &master_salt, LABEL_CIPHER_KEY, 16);
        let auth_key = derive_session_key(&master_key, &master_salt, LABEL_AUTH_KEY, 20);

        // Different labels must produce different keys.
        assert_ne!(&cipher_key[..16], &auth_key[..16]);
    }

    // -----------------------------------------------------------------------
    // Full SRTP protect/unprotect roundtrip tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_protect_unprotect_roundtrip() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0x0E; 16];
        let salt = vec![0x0F; 14];

        let km_protect = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_unprotect = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_protect).unwrap();
        let mut unprotector = PureRustSrtp::new(km_unprotect).unwrap();

        // Build a minimal RTP packet.
        // V=2, P=0, X=0, CC=0, M=0, PT=0, Seq=1, TS=160, SSRC=0xDEADBEEF
        let mut packet = vec![
            0x80, 0x00, // V=2, PT=0
            0x00, 0x01, // Seq=1
            0x00, 0x00, 0x00, 0xA0, // TS=160
            0xDE, 0xAD, 0xBE, 0xEF, // SSRC
        ];
        // 20 bytes of payload.
        let payload = b"Hello SRTP World!!!!";
        packet.extend_from_slice(payload);

        let original_packet = packet.clone();

        // Protect (encrypt + authenticate).
        protector.protect_rtp(&mut packet).unwrap();

        // Packet should be larger (auth tag appended).
        assert_eq!(
            packet.len(),
            original_packet.len() + suite.auth_tag_length()
        );

        // Header should be unchanged, but payload should be encrypted.
        assert_eq!(&packet[..12], &original_packet[..12]); // Header intact.
        assert_ne!(&packet[12..12 + 20], payload); // Payload encrypted.

        // Unprotect (verify + decrypt).
        unprotector.unprotect_rtp(&mut packet).unwrap();

        // Should recover original packet.
        assert_eq!(packet, original_packet);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_protect_unprotect_32bit_tag() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_32;
        let key = vec![0x1A; 16];
        let salt = vec![0x2B; 14];

        let km_p = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_u = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_p).unwrap();
        let mut unprotector = PureRustSrtp::new(km_u).unwrap();

        let mut packet = vec![
            0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xA0,
            0x12, 0x34, 0x56, 0x78,
        ];
        packet.extend_from_slice(&[0xCC; 32]);

        let original = packet.clone();
        protector.protect_rtp(&mut packet).unwrap();
        assert_eq!(packet.len(), original.len() + 4); // 4-byte auth tag.

        unprotector.unprotect_rtp(&mut packet).unwrap();
        assert_eq!(packet, original);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_replay_detection() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0x33; 16];
        let salt = vec![0x44; 14];

        let km_p = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_u = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_p).unwrap();
        let mut unprotector = PureRustSrtp::new(km_u).unwrap();

        // Build and protect a packet.
        let mut packet = vec![
            0x80, 0x00, 0x00, 0x05, // Seq=5
            0x00, 0x00, 0x03, 0x20, // TS=800
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        packet.extend_from_slice(&[0x55; 20]);

        protector.protect_rtp(&mut packet).unwrap();
        let protected_copy = packet.clone();

        // First unprotect should succeed.
        unprotector.unprotect_rtp(&mut packet).unwrap();

        // Replaying the same protected packet should fail.
        let mut replay = protected_copy;
        let result = unprotector.unprotect_rtp(&mut replay);
        assert!(matches!(result, Err(SrtpError::ReplayDetected(_))));
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_tampered_packet_rejected() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0x55; 16];
        let salt = vec![0x66; 14];

        let km_p = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_u = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_p).unwrap();
        let mut unprotector = PureRustSrtp::new(km_u).unwrap();

        let mut packet = vec![
            0x80, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0xA0,
            0x11, 0x22, 0x33, 0x44,
        ];
        packet.extend_from_slice(&[0x77; 16]);

        protector.protect_rtp(&mut packet).unwrap();

        // Tamper with the encrypted payload.
        if packet.len() > 14 {
            packet[13] ^= 0xFF;
        }

        let result = unprotector.unprotect_rtp(&mut packet);
        assert!(matches!(result, Err(SrtpError::AuthenticationFailed)));
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_multiple_packets_sequential() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0xAA; 16];
        let salt = vec![0xBB; 14];

        let km_p = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_u = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_p).unwrap();
        let mut unprotector = PureRustSrtp::new(km_u).unwrap();

        // Send 10 packets with increasing sequence numbers.
        for seq in 0u16..10 {
            let mut packet = vec![
                0x80,
                0x00,
                (seq >> 8) as u8,
                seq as u8,
                0x00,
                0x00,
                0x00,
                (seq as u8).wrapping_mul(160),
                0xDE,
                0xAD,
                0xBE,
                0xEF,
            ];
            let payload_byte = seq as u8;
            packet.extend_from_slice(&[payload_byte; 20]);
            let original = packet.clone();

            protector.protect_rtp(&mut packet).unwrap();
            unprotector.unprotect_rtp(&mut packet).unwrap();

            assert_eq!(packet, original);
        }
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtcp_protect_unprotect_roundtrip() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0xCC; 16];
        let salt = vec![0xDD; 14];

        let km_p = SrtpKeyMaterial::new(suite, key.clone(), salt.clone());
        let km_u = SrtpKeyMaterial::new(suite, key, salt);

        let mut protector = PureRustSrtp::new(km_p).unwrap();
        let mut unprotector = PureRustSrtp::new(km_u).unwrap();

        // Minimal RTCP SR packet.
        let mut packet = vec![
            0x80, 0xC8, // V=2, PT=200 (SR)
            0x00, 0x06, // Length=6 (words-1)
            0xDE, 0xAD, 0xBE, 0xEF, // SSRC
            // NTP timestamp (8 bytes) + RTP timestamp (4) + counts (8)
            0x00, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x02,
            0x00, 0x00, 0x00, 0x03,
            0x00, 0x00, 0x00, 0x04,
            0x00, 0x00, 0x00, 0x05,
        ];
        let original = packet.clone();

        protector.protect_rtcp(&mut packet).unwrap();
        // Should have E||index (4 bytes) + auth tag appended.
        assert!(packet.len() > original.len());

        unprotector.unprotect_rtcp(&mut packet).unwrap();
        assert_eq!(packet, original);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_packet_too_short() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let key = vec![0u8; 16];
        let salt = vec![0u8; 14];
        let km = SrtpKeyMaterial::new(suite, key, salt);
        let mut srtp = PureRustSrtp::new(km).unwrap();

        let mut short_packet = vec![0x80, 0x00, 0x00]; // Only 3 bytes.
        let result = srtp.protect_rtp(&mut short_packet);
        assert!(matches!(result, Err(SrtpError::PacketTooShort { .. })));
    }

    // -----------------------------------------------------------------------
    // RFC 3711 Appendix B test vectors
    // -----------------------------------------------------------------------
    // The RFC provides test vectors for AES-128-CM with specific keys.
    // We verify our AES-CM implementation against these.

    #[test]
    fn test_rfc3711_aes_cm_keystream() {
        // RFC 3711 Appendix B.2: AES-128-CM test vectors
        //
        // Session Key:      2B7E151628AED2A6ABF7158809CF4F3C
        // Session Salt:      F0F1F2F3F4F5F6F7F8F9FAFBFCFD0000
        //                    (14 bytes of salt, but we use standard 14-byte salt)
        //
        // The RFC defines the IV and expected keystream. We verify by encrypting
        // a zero payload and comparing the resulting keystream.
        let session_key: [u8; 16] = [
            0x2B, 0x7E, 0x15, 0x16, 0x28, 0xAE, 0xD2, 0xA6,
            0xAB, 0xF7, 0x15, 0x88, 0x09, 0xCF, 0x4F, 0x3C,
        ];

        // For the test we use a zero salt and craft the IV manually
        // to match the RFC test vector conditions.
        // The RFC test uses IV = F0F1F2F3 F4F5F6F7 F8F9FAFB FCFD0000
        // which is salt XOR (SSRC=0, index=0) = salt itself.
        let session_salt: [u8; 14] = [
            0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7,
            0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD,
        ];

        // Encrypt zeros to get the raw keystream.
        let mut keystream = vec![0u8; 32]; // 2 AES blocks.
        aes_cm_encrypt_decrypt(
            &session_key,
            &session_salt,
            0, // SSRC = 0
            0, // packet_index = 0
            &mut keystream,
        );

        // The first keystream block with IV = F0F1F2F3 F4F5F6F7 F8F9FAFB FCFD0000
        // and key = 2B7E1516 28AED2A6 ABF71588 09CF4F3C
        // should produce: E03EAD0935C95E80E166B16DD92B4EB4
        //
        // This is AES_ECB(key, IV) which is the standard AES-128 test.
        let expected_block0: [u8; 16] = [
            0xE0, 0x3E, 0xAD, 0x09, 0x35, 0xC9, 0x5E, 0x80,
            0xE1, 0x66, 0xB1, 0x6D, 0xD9, 0x2B, 0x4E, 0xB4,
        ];

        assert_eq!(
            &keystream[..16],
            &expected_block0,
            "AES-CM keystream block 0 mismatch"
        );

        // Second block: AES_ECB(key, IV with 16-bit counter = 1)
        // SRTP AES-CM uses a 16-bit block counter in bytes [14..16].
        // IV+1 = F0F1F2F3 F4F5F6F7 F8F9FAFB FCFD 0001
        // (Note: This is different from NIST CTR mode which increments
        // the full 128-bit counter. SRTP only uses the last 16 bits.)
        //
        // We verify block 1 is deterministic and different from block 0.
        assert_ne!(
            &keystream[16..32],
            &keystream[..16],
            "AES-CM blocks 0 and 1 must differ"
        );

        // Verify the block 1 value is the AES-ECB encryption of
        // F0F1F2F3 F4F5F6F7 F8F9FAFB FCFD0001 under the given key.
        // Pre-computed via AES-128-ECB:
        let expected_block1: [u8; 16] = [
            0xD2, 0x35, 0x13, 0x16, 0x2B, 0x02, 0xD0, 0xF7,
            0x2A, 0x43, 0xA2, 0xFE, 0x4A, 0x5F, 0x97, 0xAB,
        ];

        assert_eq!(
            &keystream[16..32],
            &expected_block1,
            "AES-CM keystream block 1 mismatch"
        );
    }

    #[test]
    fn test_aes_cm_empty_payload() {
        // Edge case: encrypting an empty payload should succeed without panic.
        let key = [0u8; 16];
        let salt = [0u8; 14];
        let mut payload = vec![];
        aes_cm_encrypt_decrypt(&key, &salt, 0, 0, &mut payload);
        assert!(payload.is_empty());
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL SRTP TESTS
    // -----------------------------------------------------------------------

    /// Helper to build a minimal RTP packet with given params.
    fn build_rtp_packet(seq: u16, ts: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
        let mut pkt = vec![
            0x80, 0x00,
            (seq >> 8) as u8, seq as u8,
            (ts >> 24) as u8, (ts >> 16) as u8, (ts >> 8) as u8, ts as u8,
            (ssrc >> 24) as u8, (ssrc >> 16) as u8, (ssrc >> 8) as u8, ssrc as u8,
        ];
        pkt.extend_from_slice(payload);
        pkt
    }

    #[cfg(feature = "pure-rust-crypto")]
    fn make_srtp_pair(key: &[u8; 16], salt: &[u8; 14]) -> (PureRustSrtp, PureRustSrtp) {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let km_p = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        let km_u = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        (PureRustSrtp::new(km_p).unwrap(), PureRustSrtp::new(km_u).unwrap())
    }

    // --- Encrypt/decrypt roundtrip for various sizes ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_roundtrip_1_byte_payload() {
        let (mut p, mut u) = make_srtp_pair(&[0xAA; 16], &[0xBB; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, &[0x42]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_roundtrip_160_bytes_payload() {
        let (mut p, mut u) = make_srtp_pair(&[0x11; 16], &[0x22; 14]);
        let payload = vec![0xCC; 160]; // Typical voice frame
        let mut pkt = build_rtp_packet(100, 16000, 0x12345678, &payload);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_roundtrip_1400_bytes_payload() {
        let (mut p, mut u) = make_srtp_pair(&[0x33; 16], &[0x44; 14]);
        let payload: Vec<u8> = (0..1400).map(|i| i as u8).collect();
        let mut pkt = build_rtp_packet(5000, 800000, 0xABCDEF01, &payload);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    // --- SSRC edge cases ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_ssrc_zero() {
        let (mut p, mut u) = make_srtp_pair(&[0x55; 16], &[0x66; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0x00000000, &[0xAA; 20]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_ssrc_max() {
        let (mut p, mut u) = make_srtp_pair(&[0x77; 16], &[0x88; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0xFFFFFFFF, &[0xBB; 20]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    // --- Sequence number wrap: 65535 -> 0 (ROC must increment) ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_sequence_wrap_roc_increment() {
        let (mut p, mut u) = make_srtp_pair(&[0x99; 16], &[0xAA; 14]);
        // Send packets around the seq wrap point
        for seq in 65530u16..=65535 {
            let mut pkt = build_rtp_packet(seq, (seq as u32) * 160, 0xDEADBEEF, &[seq as u8; 20]);
            let orig = pkt.clone();
            p.protect_rtp(&mut pkt).unwrap();
            u.unprotect_rtp(&mut pkt).unwrap();
            assert_eq!(pkt, orig, "Failed at pre-wrap seq={}", seq);
        }
        // Now wrap to seq 0, 1, 2
        for seq in 0u16..=5 {
            let mut pkt = build_rtp_packet(seq, 65536 * 160 + (seq as u32) * 160, 0xDEADBEEF, &[seq as u8; 20]);
            let orig = pkt.clone();
            p.protect_rtp(&mut pkt).unwrap();
            u.unprotect_rtp(&mut pkt).unwrap();
            assert_eq!(pkt, orig, "Failed at post-wrap seq={}", seq);
        }
    }

    // --- Replay window: accept, reject duplicate, window boundary ---

    #[test]
    fn test_replay_window_accept_then_reject_duplicate() {
        let mut w = ReplayWindow::new();
        assert!(w.check(42));
        w.update(42);
        assert!(!w.check(42), "duplicate must be rejected");
    }

    #[test]
    fn test_replay_window_just_inside_window() {
        let mut w = ReplayWindow::new();
        w.update(200);
        // 200 - 127 = 73, so index 73 is just inside (delta = 127 < 128)
        assert!(w.check(73), "index at window edge should be accepted");
    }

    #[test]
    fn test_replay_window_just_outside_window() {
        let mut w = ReplayWindow::new();
        w.update(200);
        // delta = 200 - 72 = 128, >= 128 so outside
        assert!(!w.check(72), "index just outside window must be rejected");
    }

    #[test]
    fn test_replay_window_large_jump_clears() {
        let mut w = ReplayWindow::new();
        for i in 0..128 {
            w.update(i);
        }
        // Jump far ahead
        w.update(1000);
        // Old indices should now be outside the window
        assert!(!w.check(0));
        assert!(!w.check(127));
        // But 1000 is received
        assert!(!w.check(1000));
        // 999 should be inside
        assert!(w.check(999));
    }

    // --- Tampered payload: flip one bit ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_tampered_payload_one_bit_flip() {
        let (mut p, mut u) = make_srtp_pair(&[0xBB; 16], &[0xCC; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0x11223344, &[0xDD; 40]);
        p.protect_rtp(&mut pkt).unwrap();
        // Flip one bit in the encrypted payload
        pkt[12] ^= 0x01;
        assert!(matches!(u.unprotect_rtp(&mut pkt), Err(SrtpError::AuthenticationFailed)));
    }

    // --- Tampered auth tag ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_tampered_auth_tag() {
        let (mut p, mut u) = make_srtp_pair(&[0xEE; 16], &[0xFF; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0x55667788, &[0xAA; 20]);
        p.protect_rtp(&mut pkt).unwrap();
        // Flip last byte of auth tag
        let last = pkt.len() - 1;
        pkt[last] ^= 0xFF;
        assert!(matches!(u.unprotect_rtp(&mut pkt), Err(SrtpError::AuthenticationFailed)));
    }

    // --- Zero-length payload ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_zero_length_payload() {
        let (mut p, mut u) = make_srtp_pair(&[0x01; 16], &[0x02; 14]);
        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, &[]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        // Should have auth tag appended but no encrypted payload
        assert_eq!(pkt.len(), orig.len() + 10);
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    // --- Wrong key produces auth failure ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_wrong_key_auth_fails() {
        let suite = SrtpCryptoSuite::AesCm128HmacSha1_80;
        let km_p = SrtpKeyMaterial::new(suite, vec![0xAA; 16], vec![0xBB; 14]);
        let km_u = SrtpKeyMaterial::new(suite, vec![0xCC; 16], vec![0xDD; 14]); // Different key!
        let mut p = PureRustSrtp::new(km_p).unwrap();
        let mut u = PureRustSrtp::new(km_u).unwrap();

        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, &[0x42; 20]);
        p.protect_rtp(&mut pkt).unwrap();
        assert!(matches!(u.unprotect_rtp(&mut pkt), Err(SrtpError::AuthenticationFailed)));
    }

    // --- SRTCP roundtrip and tamper ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtcp_tampered_rejected() {
        let (mut p, mut u) = make_srtp_pair(&[0x11; 16], &[0x22; 14]);
        let mut pkt = vec![
            0x80, 0xC8, 0x00, 0x06, 0xDE, 0xAD, 0xBE, 0xEF,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
            0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04,
            0x00, 0x00, 0x00, 0x05,
        ];
        p.protect_rtcp(&mut pkt).unwrap();
        // Tamper payload
        pkt[8] ^= 0xFF;
        assert!(matches!(u.unprotect_rtcp(&mut pkt), Err(SrtpError::AuthenticationFailed)));
    }

    // --- AES-CM with specific keystream verification ---

    #[test]
    fn test_aes_cm_single_byte_payload() {
        let key = [0u8; 16];
        let salt = [0u8; 14];
        let mut payload = vec![0xFF];
        aes_cm_encrypt_decrypt(&key, &salt, 0, 0, &mut payload);
        // Should be XOR with first keystream byte
        let mut ks = vec![0u8; 1];
        aes_cm_encrypt_decrypt(&key, &salt, 0, 0, &mut ks);
        // Since we XOR'd 0xFF and then XOR'd 0x00, ks should be the keystream
        // Decrypt again to get back to 0xFF
        aes_cm_encrypt_decrypt(&key, &salt, 0, 0, &mut payload);
        assert_eq!(payload, vec![0xFF]);
    }

    // --- Key derivation: different master keys produce different session keys ---

    #[test]
    fn test_key_derivation_different_master_keys() {
        let master_salt = [0u8; 14];
        let k1 = derive_session_key(&[0x00; 16], &master_salt, LABEL_CIPHER_KEY, 16);
        let k2 = derive_session_key(&[0x01; 16], &master_salt, LABEL_CIPHER_KEY, 16);
        assert_ne!(k1, k2, "Different master keys must produce different session keys");
    }

    // --- SRTP protect then unprotect multiple packets with out-of-order delivery ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtp_out_of_order_within_window() {
        let (mut p, mut u) = make_srtp_pair(&[0xDD; 16], &[0xEE; 14]);

        // Protect packets 1..=10
        let mut protected: Vec<(u16, Vec<u8>)> = Vec::new();
        for seq in 1u16..=10 {
            let mut pkt = build_rtp_packet(seq, seq as u32 * 160, 0xCAFEBABE, &[seq as u8; 20]);
            p.protect_rtp(&mut pkt).unwrap();
            protected.push((seq, pkt));
        }

        // Receive packet 10 first (highest), then go backwards
        // This way the receiver establishes seq=10 first, then earlier packets
        // are within the replay window.
        let last = protected.pop().unwrap();
        u.unprotect_rtp(&mut last.1.clone()).unwrap();

        // Now receive 1-9 out of order (they are within the window of highest=10)
        for (seq, mut pkt) in protected.into_iter().rev() {
            let result = u.unprotect_rtp(&mut pkt);
            assert!(result.is_ok(), "Out-of-order packet seq={} should be accepted", seq);
        }
    }

    // --- HMAC-SHA1: different data produces different MACs ---

    #[test]
    fn test_hmac_sha1_different_data() {
        let key = [0xAA; 20];
        let h1 = compute_hmac_sha1(&key, b"data1", 0);
        let h2 = compute_hmac_sha1(&key, b"data2", 0);
        assert_ne!(h1, h2);
    }

    // --- Multiple sequential SRTCP packets ---

    #[cfg(feature = "pure-rust-crypto")]
    #[test]
    fn test_srtcp_multiple_sequential() {
        let (mut p, mut u) = make_srtp_pair(&[0xA1; 16], &[0xB2; 14]);
        for i in 0u32..5 {
            let mut pkt = vec![
                0x80, 0xC8, 0x00, 0x06,
                (i >> 24) as u8, (i >> 16) as u8, (i >> 8) as u8, i as u8,
                0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
                0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04,
                0x00, 0x00, 0x00, 0x05,
            ];
            let orig = pkt.clone();
            p.protect_rtcp(&mut pkt).unwrap();
            u.unprotect_rtcp(&mut pkt).unwrap();
            assert_eq!(pkt, orig, "SRTCP roundtrip failed at index {}", i);
        }
    }

    // -----------------------------------------------------------------------
    // AEAD AES-GCM tests (RFC 7714)
    // -----------------------------------------------------------------------

    fn make_gcm_pair_128(key: &[u8; 16], salt: &[u8; 12]) -> (AeadGcmSrtp, AeadGcmSrtp) {
        let suite = SrtpCryptoSuite::AeadAes128Gcm;
        let km_p = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        let km_u = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        (AeadGcmSrtp::new(km_p).unwrap(), AeadGcmSrtp::new(km_u).unwrap())
    }

    fn make_gcm_pair_256(key: &[u8; 32], salt: &[u8; 12]) -> (AeadGcmSrtp, AeadGcmSrtp) {
        let suite = SrtpCryptoSuite::AeadAes256Gcm;
        let km_p = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        let km_u = SrtpKeyMaterial::new(suite, key.to_vec(), salt.to_vec());
        (AeadGcmSrtp::new(km_p).unwrap(), AeadGcmSrtp::new(km_u).unwrap())
    }

    #[test]
    fn test_gcm_128_rtp_roundtrip() {
        let (mut p, mut u) = make_gcm_pair_128(&[0xAA; 16], &[0xBB; 12]);
        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, b"Hello GCM World!");
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        // Should be header + encrypted payload + 16-byte tag
        assert_eq!(pkt.len(), orig.len() + 16);
        // Header should be unchanged
        assert_eq!(&pkt[..12], &orig[..12]);
        // Payload should be encrypted
        assert_ne!(&pkt[12..12 + 16], &orig[12..]);
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[test]
    fn test_gcm_256_rtp_roundtrip() {
        let (mut p, mut u) = make_gcm_pair_256(&[0xCC; 32], &[0xDD; 12]);
        let mut pkt = build_rtp_packet(1, 160, 0x12345678, &[0xAA; 80]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[test]
    fn test_gcm_128_tampered_rejected() {
        let (mut p, mut u) = make_gcm_pair_128(&[0x11; 16], &[0x22; 12]);
        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, &[0xCC; 20]);
        p.protect_rtp(&mut pkt).unwrap();
        // Tamper with encrypted payload
        pkt[13] ^= 0xFF;
        assert!(matches!(u.unprotect_rtp(&mut pkt), Err(SrtpError::AuthenticationFailed)));
    }

    #[test]
    fn test_gcm_128_rtcp_roundtrip() {
        let (mut p, mut u) = make_gcm_pair_128(&[0x33; 16], &[0x44; 12]);
        let mut pkt = vec![
            0x80, 0xC8, 0x00, 0x06,
            0xDE, 0xAD, 0xBE, 0xEF,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02,
            0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04,
            0x00, 0x00, 0x00, 0x05,
        ];
        let orig = pkt.clone();
        p.protect_rtcp(&mut pkt).unwrap();
        assert!(pkt.len() > orig.len());
        u.unprotect_rtcp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[test]
    fn test_gcm_multiple_packets() {
        let (mut p, mut u) = make_gcm_pair_128(&[0x55; 16], &[0x66; 12]);
        for seq in 0u16..10 {
            let mut pkt = build_rtp_packet(seq, seq as u32 * 160, 0xCAFEBABE, &[seq as u8; 20]);
            let orig = pkt.clone();
            p.protect_rtp(&mut pkt).unwrap();
            u.unprotect_rtp(&mut pkt).unwrap();
            assert_eq!(pkt, orig);
        }
    }

    #[test]
    fn test_gcm_replay_detection() {
        let (mut p, mut u) = make_gcm_pair_128(&[0x77; 16], &[0x88; 12]);
        let mut pkt = build_rtp_packet(5, 800, 0xDEADBEEF, &[0x55; 20]);
        p.protect_rtp(&mut pkt).unwrap();
        let copy = pkt.clone();
        u.unprotect_rtp(&mut pkt).unwrap();
        // Replay should fail
        let mut replay = copy;
        assert!(matches!(u.unprotect_rtp(&mut replay), Err(SrtpError::ReplayDetected(_))));
    }

    #[test]
    fn test_gcm_suite_properties() {
        let suite = SrtpCryptoSuite::AeadAes128Gcm;
        assert_eq!(suite.key_length(), 16);
        assert_eq!(suite.salt_length(), 12);
        assert_eq!(suite.auth_tag_length(), 16);
        assert!(suite.is_aead());
        assert_eq!(suite.sdp_name(), "AEAD_AES_128_GCM");

        let suite256 = SrtpCryptoSuite::AeadAes256Gcm;
        assert_eq!(suite256.key_length(), 32);
        assert_eq!(suite256.salt_length(), 12);
        assert!(suite256.is_aead());
    }

    #[test]
    fn test_gcm_from_sdp_name() {
        assert_eq!(
            SrtpCryptoSuite::from_sdp_name("AEAD_AES_128_GCM"),
            Some(SrtpCryptoSuite::AeadAes128Gcm)
        );
        assert_eq!(
            SrtpCryptoSuite::from_sdp_name("AEAD_AES_256_GCM"),
            Some(SrtpCryptoSuite::AeadAes256Gcm)
        );
    }

    #[test]
    fn test_gcm_zero_length_payload() {
        let (mut p, mut u) = make_gcm_pair_128(&[0x01; 16], &[0x02; 12]);
        let mut pkt = build_rtp_packet(1, 160, 0xDEADBEEF, &[]);
        let orig = pkt.clone();
        p.protect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt.len(), orig.len() + 16); // Just tag
        u.unprotect_rtp(&mut pkt).unwrap();
        assert_eq!(pkt, orig);
    }

    #[test]
    fn test_gcm_create_via_factory() {
        let suite = SrtpCryptoSuite::AeadAes128Gcm;
        let km = SrtpKeyMaterial::new(suite, vec![0xAA; 16], vec![0xBB; 12]);
        let result = create_srtp_crypto(km);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().backend_name(), "aead-gcm");
    }
}
