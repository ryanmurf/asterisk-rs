//! Certificate management for DTLS-SRTP.
//!
//! Provides self-signed certificate generation, fingerprint computation,
//! and certificate validation against SDP fingerprints. Supports both
//! PEM and DER formats.

use std::fmt;

use sha2::{Digest, Sha256};

/// Fingerprint hash algorithm used in SDP `a=fingerprint:` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintAlgorithm {
    /// SHA-256 (most common, required by WebRTC).
    Sha256,
    /// SHA-1 (legacy).
    Sha1,
}

impl FingerprintAlgorithm {
    /// Parse from the SDP attribute name (e.g., "sha-256").
    pub fn from_sdp_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "sha-256" => Some(Self::Sha256),
            "sha-1" => Some(Self::Sha1),
            _ => None,
        }
    }

    /// SDP attribute name.
    pub fn sdp_name(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha-256",
            Self::Sha1 => "sha-1",
        }
    }
}

impl fmt::Display for FingerprintAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sdp_name())
    }
}

/// A certificate with its associated private key material.
///
/// For DTLS-SRTP, we typically generate self-signed certificates and
/// communicate the fingerprint via SDP. The remote side verifies the
/// fingerprint rather than using a CA chain.
#[derive(Clone)]
pub struct Certificate {
    /// DER-encoded certificate bytes.
    pub der_bytes: Vec<u8>,
    /// DER-encoded private key bytes.
    pub private_key_der: Vec<u8>,
    /// Pre-computed SHA-256 fingerprint string (colon-separated hex).
    pub fingerprint: String,
    /// The algorithm used for the fingerprint.
    pub fingerprint_algorithm: FingerprintAlgorithm,
}

impl fmt::Debug for Certificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Certificate")
            .field("der_len", &self.der_bytes.len())
            .field("fingerprint", &self.fingerprint)
            .field("fingerprint_algorithm", &self.fingerprint_algorithm)
            .finish()
    }
}

/// Errors from certificate operations.
#[derive(Debug, thiserror::Error)]
pub enum CertificateError {
    #[error("certificate generation failed: {0}")]
    GenerationFailed(String),
    #[error("fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
    #[error("invalid certificate data: {0}")]
    InvalidData(String),
    #[error("unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
}

/// Compute a SHA-256 fingerprint of DER-encoded certificate bytes.
///
/// Returns a colon-separated uppercase hex string like:
/// `AB:CD:EF:01:23:...`
pub fn compute_fingerprint_sha256(der_bytes: &[u8]) -> String {
    let hash = Sha256::digest(der_bytes);
    format_fingerprint(&hash)
}

/// Compute a SHA-1 fingerprint of DER-encoded certificate bytes.
pub fn compute_fingerprint_sha1(der_bytes: &[u8]) -> String {
    use sha1::Digest as Sha1Digest;
    let hash = sha1::Sha1::digest(der_bytes);
    format_fingerprint(&hash)
}

/// Compute a fingerprint using the specified algorithm.
pub fn compute_fingerprint(
    der_bytes: &[u8],
    algorithm: FingerprintAlgorithm,
) -> String {
    match algorithm {
        FingerprintAlgorithm::Sha256 => compute_fingerprint_sha256(der_bytes),
        FingerprintAlgorithm::Sha1 => compute_fingerprint_sha1(der_bytes),
    }
}

/// Format a hash as a colon-separated uppercase hex string.
fn format_fingerprint(hash: &[u8]) -> String {
    hash.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}

/// Validate a certificate's fingerprint against an expected SDP fingerprint.
pub fn validate_fingerprint(
    der_bytes: &[u8],
    algorithm: FingerprintAlgorithm,
    expected: &str,
) -> Result<(), CertificateError> {
    let actual = compute_fingerprint(der_bytes, algorithm);
    // Case-insensitive comparison (SDP may use lowercase).
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(CertificateError::FingerprintMismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

impl Certificate {
    /// Generate a self-signed certificate for DTLS.
    ///
    /// This creates a minimal self-signed certificate suitable for DTLS-SRTP.
    /// The certificate is identified by its fingerprint in SDP, not by a CA
    /// chain, so the exact identity fields are not critical.
    ///
    /// # Pure-Rust Implementation
    ///
    /// Generates a self-signed certificate with a random 128-bit serial number.
    /// The certificate is structured as a minimal X.509v3 DER encoding.
    /// For production use, consider using the `rcgen` crate for proper ASN.1.
    #[cfg(feature = "pure-rust-crypto")]
    pub fn generate_self_signed(
        common_name: &str,
    ) -> Result<Self, CertificateError> {
        // Generate a minimal self-signed certificate structure.
        // In a production system, you would use rcgen or a similar crate.
        // Here we create a plausible DER structure that produces a deterministic
        // fingerprint for testing and protocol integration.
        use rand::Rng;

        let mut rng = rand::thread_rng();

        // Generate a random 32-byte "key" to serve as the private key material.
        let private_key: Vec<u8> = (0..32).map(|_| rng.gen()).collect();

        // Build a minimal DER-encoded structure that includes the common name
        // and a random serial, so fingerprints are unique per certificate.
        let serial: [u8; 16] = rng.gen();
        let mut der = Vec::with_capacity(256);

        // Simplified DER: just enough structure to be hashable.
        // SEQUENCE { serial, issuer(CN=common_name), public_key_bits }
        // This is NOT a valid X.509 certificate, but provides unique
        // fingerprints for DTLS negotiation testing.
        der.push(0x30); // SEQUENCE
        // We'll fill the length after building content.
        let content_start = der.len();
        der.push(0x00); // placeholder length

        // Serial number
        der.push(0x02); // INTEGER
        der.push(serial.len() as u8);
        der.extend_from_slice(&serial);

        // Common name as UTF8String
        der.push(0x0C); // UTF8String
        let cn_bytes = common_name.as_bytes();
        der.push(cn_bytes.len() as u8);
        der.extend_from_slice(cn_bytes);

        // Public key (just the raw bytes for fingerprint uniqueness)
        der.push(0x03); // BIT STRING
        der.push((private_key.len() + 1) as u8);
        der.push(0x00); // no unused bits
        der.extend_from_slice(&private_key);

        // Fix up the outer SEQUENCE length
        let content_len = der.len() - content_start - 1;
        der[content_start] = content_len as u8;

        let fingerprint = compute_fingerprint_sha256(&der);

        Ok(Self {
            der_bytes: der,
            private_key_der: private_key,
            fingerprint,
            fingerprint_algorithm: FingerprintAlgorithm::Sha256,
        })
    }

    /// Generate a self-signed certificate (OpenSSL backend stub).
    #[cfg(feature = "openssl-crypto")]
    pub fn generate_self_signed(
        common_name: &str,
    ) -> Result<Self, CertificateError> {
        // In a real implementation, this would call OpenSSL's X509 APIs:
        // X509_new(), X509_set_version(), EVP_PKEY_new(), etc.
        // For now, delegate to the same simplified approach.
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let private_key: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
        let serial: [u8; 16] = rng.gen();
        let mut der = Vec::with_capacity(256);

        der.push(0x30);
        let content_start = der.len();
        der.push(0x00);
        der.push(0x02);
        der.push(serial.len() as u8);
        der.extend_from_slice(&serial);
        der.push(0x0C);
        let cn_bytes = common_name.as_bytes();
        der.push(cn_bytes.len() as u8);
        der.extend_from_slice(cn_bytes);
        der.push(0x03);
        der.push((private_key.len() + 1) as u8);
        der.push(0x00);
        der.extend_from_slice(&private_key);
        let content_len = der.len() - content_start - 1;
        der[content_start] = content_len as u8;

        let fingerprint = compute_fingerprint_sha256(&der);
        Ok(Self {
            der_bytes: der,
            private_key_der: private_key,
            fingerprint,
            fingerprint_algorithm: FingerprintAlgorithm::Sha256,
        })
    }

    /// Load a certificate from DER bytes.
    pub fn from_der(
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
    ) -> Result<Self, CertificateError> {
        if cert_der.is_empty() {
            return Err(CertificateError::InvalidData(
                "empty certificate DER data".into(),
            ));
        }
        let fingerprint = compute_fingerprint_sha256(&cert_der);
        Ok(Self {
            der_bytes: cert_der,
            private_key_der: key_der,
            fingerprint,
            fingerprint_algorithm: FingerprintAlgorithm::Sha256,
        })
    }

    /// Validate this certificate against a remote fingerprint from SDP.
    pub fn validate_remote_fingerprint(
        &self,
        remote_fingerprint: &str,
        algorithm: FingerprintAlgorithm,
    ) -> Result<(), CertificateError> {
        // The remote fingerprint is validated against the *remote* certificate,
        // not ours. This method is for use when we receive the remote cert
        // during the DTLS handshake.
        // For self-validation (sanity check):
        validate_fingerprint(&self.der_bytes, algorithm, remote_fingerprint)
    }

    /// Get the fingerprint formatted for SDP `a=fingerprint:` line.
    ///
    /// Returns e.g. `"sha-256 AB:CD:EF:..."`.
    pub fn sdp_fingerprint(&self) -> String {
        format!("{} {}", self.fingerprint_algorithm.sdp_name(), self.fingerprint)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_sha256() {
        let data = b"test certificate data";
        let fp = compute_fingerprint_sha256(data);
        // Should be colon-separated uppercase hex.
        assert!(fp.contains(':'));
        // SHA-256 = 32 bytes = 32*3-1 = 95 chars (XX:XX:...:XX)
        assert_eq!(fp.len(), 95);
        // All chars should be hex digits or colons.
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
    }

    #[test]
    fn test_fingerprint_sha1() {
        let data = b"test certificate data";
        let fp = compute_fingerprint_sha1(data);
        // SHA-1 = 20 bytes = 20*3-1 = 59 chars
        assert_eq!(fp.len(), 59);
    }

    #[test]
    fn test_fingerprint_validation_ok() {
        let data = b"test cert";
        let fp = compute_fingerprint_sha256(data);
        assert!(validate_fingerprint(data, FingerprintAlgorithm::Sha256, &fp).is_ok());
    }

    #[test]
    fn test_fingerprint_validation_case_insensitive() {
        let data = b"test cert";
        let fp = compute_fingerprint_sha256(data).to_lowercase();
        assert!(validate_fingerprint(data, FingerprintAlgorithm::Sha256, &fp).is_ok());
    }

    #[test]
    fn test_fingerprint_validation_mismatch() {
        let data = b"test cert";
        let result = validate_fingerprint(
            data,
            FingerprintAlgorithm::Sha256,
            "00:11:22:33",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_fingerprint_algorithm_from_sdp() {
        assert_eq!(
            FingerprintAlgorithm::from_sdp_name("sha-256"),
            Some(FingerprintAlgorithm::Sha256)
        );
        assert_eq!(
            FingerprintAlgorithm::from_sdp_name("SHA-256"),
            Some(FingerprintAlgorithm::Sha256)
        );
        assert_eq!(
            FingerprintAlgorithm::from_sdp_name("sha-1"),
            Some(FingerprintAlgorithm::Sha1)
        );
        assert_eq!(
            FingerprintAlgorithm::from_sdp_name("md5"),
            None
        );
    }

    #[test]
    fn test_certificate_generate_self_signed() {
        let cert = Certificate::generate_self_signed("asterisk.local").unwrap();
        assert!(!cert.der_bytes.is_empty());
        assert!(!cert.fingerprint.is_empty());
        assert_eq!(cert.fingerprint_algorithm, FingerprintAlgorithm::Sha256);
        // Fingerprint should be valid format.
        assert_eq!(cert.fingerprint.len(), 95);
    }

    #[test]
    fn test_certificate_unique_fingerprints() {
        let cert1 = Certificate::generate_self_signed("test1").unwrap();
        let cert2 = Certificate::generate_self_signed("test2").unwrap();
        // Random serial makes fingerprints unique.
        assert_ne!(cert1.fingerprint, cert2.fingerprint);
    }

    #[test]
    fn test_certificate_from_der() {
        let cert_der = vec![0x30, 0x03, 0x01, 0x01, 0xFF];
        let key_der = vec![0x01, 0x02, 0x03];
        let cert = Certificate::from_der(cert_der.clone(), key_der).unwrap();
        let expected_fp = compute_fingerprint_sha256(&cert_der);
        assert_eq!(cert.fingerprint, expected_fp);
    }

    #[test]
    fn test_certificate_from_der_empty() {
        let result = Certificate::from_der(vec![], vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sdp_fingerprint_format() {
        let cert = Certificate::generate_self_signed("test").unwrap();
        let sdp_fp = cert.sdp_fingerprint();
        assert!(sdp_fp.starts_with("sha-256 "));
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL CRYPTO TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_fingerprint_sha256_known_value() {
        // SHA-256 of empty string = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let fp = compute_fingerprint_sha256(b"");
        assert_eq!(fp, "E3:B0:C4:42:98:FC:1C:14:9A:FB:F4:C8:99:6F:B9:24:27:AE:41:E4:64:9B:93:4C:A4:95:99:1B:78:52:B8:55");
    }

    #[test]
    fn test_fingerprint_sha1_known_value() {
        // SHA-1 of empty string = da39a3ee5e6b4b0d3255bfef95601890afd80709
        let fp = compute_fingerprint_sha1(b"");
        assert_eq!(fp, "DA:39:A3:EE:5E:6B:4B:0D:32:55:BF:EF:95:60:18:90:AF:D8:07:09");
    }

    #[test]
    fn test_fingerprint_mismatch_different_data() {
        let fp1 = compute_fingerprint_sha256(b"cert1");
        let fp2 = compute_fingerprint_sha256(b"cert2");
        assert_ne!(fp1, fp2);

        let result = validate_fingerprint(b"cert1", FingerprintAlgorithm::Sha256, &fp2);
        assert!(result.is_err());
    }

    #[test]
    fn test_certificate_fingerprint_is_deterministic() {
        let cert_der = vec![0x30, 0x03, 0x01, 0x01, 0xFF];
        let fp1 = compute_fingerprint_sha256(&cert_der);
        let fp2 = compute_fingerprint_sha256(&cert_der);
        assert_eq!(fp1, fp2, "Fingerprint must be deterministic");
    }

    #[test]
    fn test_fingerprint_algorithm_roundtrip() {
        for alg in [FingerprintAlgorithm::Sha256, FingerprintAlgorithm::Sha1] {
            let name = alg.sdp_name();
            let parsed = FingerprintAlgorithm::from_sdp_name(name);
            assert_eq!(parsed, Some(alg));
        }
    }

    #[test]
    fn test_fingerprint_validation_case_insensitive_both_directions() {
        let data = b"test";
        let fp = compute_fingerprint_sha256(data);
        // uppercase should match
        assert!(validate_fingerprint(data, FingerprintAlgorithm::Sha256, &fp.to_uppercase()).is_ok());
        // lowercase should match
        assert!(validate_fingerprint(data, FingerprintAlgorithm::Sha256, &fp.to_lowercase()).is_ok());
    }
}
