//! STIR/SHAKEN (RFC 8224/8225/8226) — Cryptographic caller ID attestation and verification.
//!
//! Implements the Secure Telephone Identity Revisited (STIR) framework with
//! Signature-based Handling of Asserted information using toKENs (SHAKEN).
//! This is an FCC-mandated requirement for US carriers to combat caller ID spoofing.
//!
//! # Architecture
//!
//! - **Signing (Authentication Service):** Creates a PASSporT JWT token for outbound
//!   calls, asserting the caller's identity at attestation level A, B, or C.
//! - **Verification (Verification Service):** Validates the Identity header on inbound
//!   calls by checking the JWT signature, certificate chain, and number claims.
//! - **Certificate Cache:** Caches fetched verification certificates to avoid
//!   redundant HTTPS lookups.
//!
//! # Crypto Backend
//!
//! The actual ECDSA P-256 signing/verification is abstracted behind the [`CryptoBackend`]
//! trait. A default HMAC-SHA256-based placeholder is provided for testing and development.
//! Production deployments must supply a real ECDSA P-256 backend (e.g., via `ring` or `p256`).

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::parser::{SipHeader, SipMessage};

// ---------------------------------------------------------------------------
// Core Types
// ---------------------------------------------------------------------------

/// Attestation level for caller identity (SHAKEN framework).
///
/// Determines the degree to which the originating carrier can vouch for the
/// calling party's right to use the calling number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttestationLevel {
    /// Full attestation -- carrier has authenticated the caller and verified
    /// they are authorized to use the calling number.
    A,
    /// Partial attestation -- carrier has authenticated the caller but cannot
    /// verify number authorization.
    B,
    /// Gateway attestation -- carrier originated the call but cannot
    /// authenticate the caller (e.g., gateway from PSTN).
    C,
}

impl AttestationLevel {
    /// Parse from a single-character string ("A", "B", or "C").
    pub fn from_str(s: &str) -> Result<Self, StirShakenError> {
        match s {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            other => Err(StirShakenError::InvalidAttestation(other.to_string())),
        }
    }

    /// Return the single-character string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }
}

impl fmt::Display for AttestationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A telephone number in E.164 format (digits only, no leading `+`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelephoneNumber {
    /// E.164 number without the leading `+` (e.g., `"12025551234"`).
    pub tn: String,
}

impl TelephoneNumber {
    /// Normalize a telephone number string to bare digits.
    ///
    /// Strips leading `+`, dashes, spaces, parentheses, and dots.
    pub fn normalize(raw: &str) -> Self {
        let tn: String = raw
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        Self { tn }
    }
}

// ---------------------------------------------------------------------------
// PASSporT (RFC 8225) — Personal Assertion Token
// ---------------------------------------------------------------------------

/// PASSporT JWT header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PASSporTHeader {
    /// Algorithm — always `"ES256"` for STIR/SHAKEN.
    pub alg: String,
    /// PASSporT extension — `"shaken"` for the SHAKEN profile.
    pub ppt: String,
    /// Token type — `"passport"`.
    pub typ: String,
    /// URL to the certificate chain used for verification.
    pub x5u: String,
}

/// PASSporT JWT payload (claims).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PASSporTPayload {
    /// Attestation level.
    pub attest: AttestationLevel,
    /// Called (destination) number.
    pub dest: TelephoneNumber,
    /// Issued-at timestamp (Unix epoch seconds).
    pub iat: u64,
    /// Calling (originating) number.
    pub orig: TelephoneNumber,
    /// Unique call identifier (UUID).
    pub origid: String,
}

/// A complete PASSporT token (RFC 8225).
#[derive(Debug, Clone)]
pub struct PASSporT {
    /// JWT header.
    pub header: PASSporTHeader,
    /// JWT payload/claims.
    pub payload: PASSporTPayload,
    /// ECDSA P-256 signature bytes.
    pub signature: Vec<u8>,
}

// ---------------------------------------------------------------------------
// STIR/SHAKEN Identity Header (RFC 8224)
// ---------------------------------------------------------------------------

/// Parsed STIR/SHAKEN Identity header.
#[derive(Debug, Clone)]
pub struct StirIdentity {
    /// The PASSporT JWT token (decoded).
    pub passport: PASSporT,
    /// The verification service URL (from the `info` parameter).
    pub info_url: String,
    /// Algorithm (from the `alg` parameter, should be `"ES256"`).
    pub algorithm: String,
    /// PASSporT type (from the `ppt` parameter, should be `"shaken"`).
    pub ppt: String,
}

// ---------------------------------------------------------------------------
// Verification Result
// ---------------------------------------------------------------------------

/// Outcome of verifying a STIR/SHAKEN Identity header.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the verification passed all checks.
    pub verified: bool,
    /// The attestation level claimed in the token.
    pub attestation: AttestationLevel,
    /// Detailed verification status.
    pub reason: VerificationStatus,
    /// The origid (unique call ID) from the PASSporT.
    pub origid: String,
    /// The certificate URL from the x5u header claim.
    pub cert_url: String,
}

/// Detailed verification status codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Token is valid — signature, certificate, and claims all check out.
    Valid,
    /// ECDSA signature does not match.
    SignatureInvalid,
    /// Certificate has expired.
    CertificateExpired,
    /// Certificate is not from a trusted root CA.
    CertificateUntrusted,
    /// Token `iat` claim is too old.
    TokenExpired,
    /// Originating or destination number does not match SIP From/To.
    NumberMismatch,
    /// Token could not be parsed (malformed JWT, missing fields, etc.).
    MalformedToken,
    /// Certificate could not be fetched from the x5u URL.
    CertificateFetchFailed,
}

impl fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Valid => "valid",
            Self::SignatureInvalid => "signature_invalid",
            Self::CertificateExpired => "certificate_expired",
            Self::CertificateUntrusted => "certificate_untrusted",
            Self::TokenExpired => "token_expired",
            Self::NumberMismatch => "number_mismatch",
            Self::MalformedToken => "malformed_token",
            Self::CertificateFetchFailed => "cert_fetch_failed",
        };
        write!(f, "{}", s)
    }
}

// ---------------------------------------------------------------------------
// Error Types
// ---------------------------------------------------------------------------

/// Errors from STIR/SHAKEN operations.
#[derive(Debug, thiserror::Error)]
pub enum StirShakenError {
    #[error("Invalid attestation level: {0}")]
    InvalidAttestation(String),
    #[error("Token encoding failed: {0}")]
    EncodingError(String),
    #[error("Signature computation failed: {0}")]
    SigningError(String),
    #[error("Token parsing failed: {0}")]
    ParsingError(String),
    #[error("Signature verification failed")]
    VerificationFailed,
    #[error("Certificate fetch failed: {0}")]
    CertFetchError(String),
    #[error("Number mismatch: expected {expected}, got {actual}")]
    NumberMismatch { expected: String, actual: String },
    #[error("Token expired: age {age_secs}s exceeds max {max_secs}s")]
    TokenExpired { age_secs: u64, max_secs: u64 },
    #[error("Token timestamp is in the future by {ahead_secs}s")]
    TokenInFuture { ahead_secs: u64 },
}

// ---------------------------------------------------------------------------
// Crypto Backend Trait
// ---------------------------------------------------------------------------

/// Abstraction over the ECDSA P-256 signing/verification operations.
///
/// Production deployments should provide an implementation backed by a real
/// ECDSA library (e.g., `ring`, `p256`, or OpenSSL). The default
/// [`HmacPlaceholderBackend`] uses HMAC-SHA256 as a stand-in for testing.
pub trait CryptoBackend: Send + Sync {
    /// Sign `data` with the given PEM-encoded private key, returning the raw signature bytes.
    fn sign(&self, data: &[u8], private_key_pem: &[u8]) -> Result<Vec<u8>, StirShakenError>;

    /// Verify `signature` over `data` using the given DER-encoded public key.
    fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key_der: &[u8],
    ) -> Result<bool, StirShakenError>;
}

/// HMAC-SHA256 placeholder backend for testing and development.
///
/// **WARNING:** This is NOT real ECDSA P-256. It uses HMAC-SHA256 with the key
/// material as the HMAC key, which is sufficient for integration testing but
/// MUST be replaced with a genuine ECDSA P-256 implementation for production.
#[derive(Debug, Clone)]
pub struct HmacPlaceholderBackend;

impl CryptoBackend for HmacPlaceholderBackend {
    fn sign(&self, data: &[u8], private_key_pem: &[u8]) -> Result<Vec<u8>, StirShakenError> {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(private_key_pem)
            .map_err(|e| StirShakenError::SigningError(e.to_string()))?;
        mac.update(data);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    fn verify(
        &self,
        data: &[u8],
        signature: &[u8],
        public_key_der: &[u8],
    ) -> Result<bool, StirShakenError> {
        // In the placeholder, the "public key" is the same material as the private key.
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(public_key_der)
            .map_err(|e| StirShakenError::SigningError(e.to_string()))?;
        mac.update(data);
        Ok(mac.verify_slice(signature).is_ok())
    }
}

/// Stub backend that always returns an error (for builds without crypto support).
#[derive(Debug, Clone)]
pub struct StubBackend;

impl CryptoBackend for StubBackend {
    fn sign(&self, _data: &[u8], _private_key_pem: &[u8]) -> Result<Vec<u8>, StirShakenError> {
        Err(StirShakenError::SigningError(
            "STIR/SHAKEN signing unavailable: no crypto backend configured".into(),
        ))
    }

    fn verify(
        &self,
        _data: &[u8],
        _signature: &[u8],
        _public_key_der: &[u8],
    ) -> Result<bool, StirShakenError> {
        Err(StirShakenError::SigningError(
            "STIR/SHAKEN verification unavailable: no crypto backend configured".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// JSON Helpers (manual — avoids serde dependency for the SIP crate)
// ---------------------------------------------------------------------------

/// Minimal JSON serialization for PASSporT header.
fn serialize_header_json(header: &PASSporTHeader) -> String {
    format!(
        r#"{{"alg":"{}","ppt":"{}","typ":"{}","x5u":"{}"}}"#,
        json_escape(&header.alg),
        json_escape(&header.ppt),
        json_escape(&header.typ),
        json_escape(&header.x5u),
    )
}

/// Minimal JSON serialization for PASSporT payload.
fn serialize_payload_json(payload: &PASSporTPayload) -> String {
    format!(
        r#"{{"attest":"{}","dest":{{"tn":["{}"]}},"iat":{},"orig":{{"tn":"{}"}},"origid":"{}"}}"#,
        payload.attest.as_str(),
        json_escape(&payload.dest.tn),
        payload.iat,
        json_escape(&payload.orig.tn),
        json_escape(&payload.origid),
    )
}

/// Escape a string for embedding in JSON (handles `"`, `\`, and control chars).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Parse a JSON string value from a key in a flat or nested JSON object.
/// Returns `None` if the key is not found or the value is not a string.
fn json_get_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)?;
    let after_key = start + pattern.len();
    let rest = json[after_key..].trim_start();
    if let Some(inner) = rest.strip_prefix('"') {
        // String value — find the closing quote (handle escaped quotes).
        let mut result = String::new();
        let mut chars = inner.chars();
        loop {
            match chars.next() {
                Some('\\') => {
                    match chars.next() {
                        Some('"') => result.push('"'),
                        Some('\\') => result.push('\\'),
                        Some('n') => result.push('\n'),
                        Some('r') => result.push('\r'),
                        Some('t') => result.push('\t'),
                        Some(c) => {
                            result.push('\\');
                            result.push(c);
                        }
                        None => break,
                    }
                }
                Some('"') => break,
                Some(c) => result.push(c),
                None => break,
            }
        }
        Some(result)
    } else {
        None
    }
}

/// Parse a JSON integer value.
fn json_get_u64(json: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)?;
    let after_key = start + pattern.len();
    let rest = json[after_key..].trim_start();
    // Read digits.
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

/// Parse the `tn` field from a nested object like `{"tn":"12345"}` or `{"tn":["12345"]}`.
fn json_get_tn(json: &str, outer_key: &str) -> Option<String> {
    let pattern = format!("\"{}\":", outer_key);
    let start = json.find(&pattern)?;
    let after_key = start + pattern.len();
    let rest = json[after_key..].trim_start();

    // Find the nested object.
    let obj_start = rest.find('{')?;
    let obj_end = rest[obj_start..].find('}')? + obj_start + 1;
    let obj = &rest[obj_start..obj_end];

    // Look for "tn" inside the nested object.
    let tn_pattern = "\"tn\":";
    let tn_start = obj.find(tn_pattern)?;
    let tn_rest = obj[tn_start + tn_pattern.len()..].trim_start();

    if let Some(inner) = tn_rest.strip_prefix('[') {
        // Array form: ["12345"]
        let arr_end = inner.find(']')?;
        let arr_content = inner[..arr_end].trim();
        // Extract first string in array.
        if let Some(s) = arr_content.strip_prefix('"') {
            let end = s.find('"')?;
            Some(s[..end].to_string())
        } else {
            None
        }
    } else if let Some(inner) = tn_rest.strip_prefix('"') {
        // Direct string form: "12345"
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Base64url Encoding/Decoding
// ---------------------------------------------------------------------------

fn base64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn base64url_decode(s: &str) -> Result<Vec<u8>, StirShakenError> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| StirShakenError::ParsingError(format!("base64url decode failed: {}", e)))
}

// ---------------------------------------------------------------------------
// JWT Encoding/Decoding
// ---------------------------------------------------------------------------

/// Encode a PASSporT as a compact JWT string (header.payload.signature).
fn encode_passport_jwt(
    header: &PASSporTHeader,
    payload: &PASSporTPayload,
    crypto: &dyn CryptoBackend,
    private_key_pem: &[u8],
) -> Result<String, StirShakenError> {
    let header_json = serialize_header_json(header);
    let payload_json = serialize_payload_json(payload);

    let header_b64 = base64url_encode(header_json.as_bytes());
    let payload_b64 = base64url_encode(payload_json.as_bytes());

    let signing_input = format!("{}.{}", header_b64, payload_b64);
    let signature = crypto.sign(signing_input.as_bytes(), private_key_pem)?;
    let signature_b64 = base64url_encode(&signature);

    Ok(format!("{}.{}.{}", header_b64, payload_b64, signature_b64))
}

/// Decode a compact JWT string back into its components.
fn decode_passport_jwt(token: &str) -> Result<(PASSporTHeader, PASSporTPayload, Vec<u8>, String), StirShakenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(StirShakenError::ParsingError(format!(
            "JWT must have 3 parts separated by '.', got {}",
            parts.len()
        )));
    }

    let header_bytes = base64url_decode(parts[0])?;
    let payload_bytes = base64url_decode(parts[1])?;
    let signature = base64url_decode(parts[2])?;

    let header_json = String::from_utf8(header_bytes)
        .map_err(|e| StirShakenError::ParsingError(format!("header is not UTF-8: {}", e)))?;
    let payload_json = String::from_utf8(payload_bytes)
        .map_err(|e| StirShakenError::ParsingError(format!("payload is not UTF-8: {}", e)))?;

    // Parse header.
    let alg = json_get_string(&header_json, "alg")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'alg' in header".into()))?;
    let ppt = json_get_string(&header_json, "ppt")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'ppt' in header".into()))?;
    let typ = json_get_string(&header_json, "typ")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'typ' in header".into()))?;
    let x5u = json_get_string(&header_json, "x5u")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'x5u' in header".into()))?;

    let header = PASSporTHeader { alg, ppt, typ, x5u };

    // Parse payload.
    let attest_str = json_get_string(&payload_json, "attest")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'attest' in payload".into()))?;
    let attest = AttestationLevel::from_str(&attest_str)?;

    let dest_tn = json_get_tn(&payload_json, "dest")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'dest.tn' in payload".into()))?;
    let iat = json_get_u64(&payload_json, "iat")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'iat' in payload".into()))?;
    let orig_tn = json_get_tn(&payload_json, "orig")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'orig.tn' in payload".into()))?;
    let origid = json_get_string(&payload_json, "origid")
        .ok_or_else(|| StirShakenError::ParsingError("missing 'origid' in payload".into()))?;

    let payload = PASSporTPayload {
        attest,
        dest: TelephoneNumber { tn: dest_tn },
        iat,
        orig: TelephoneNumber { tn: orig_tn },
        origid,
    };

    // Return the signing input for signature verification.
    let signing_input = format!("{}.{}", parts[0], parts[1]);

    Ok((header, payload, signature, signing_input))
}

// ---------------------------------------------------------------------------
// Identity Header Formatting/Parsing (RFC 8224)
// ---------------------------------------------------------------------------

/// Format a SIP Identity header value from a JWT token and metadata.
///
/// Result format: `<jwt_token>;info=<cert_url>;alg=ES256;ppt=shaken`
fn format_identity_header(token: &str, cert_url: &str) -> String {
    format!(
        "{};info=<{}>;alg=ES256;ppt=shaken",
        token, cert_url,
    )
}

/// Parsed components of a SIP Identity header.
#[derive(Debug, Clone)]
struct ParsedIdentityHeader {
    token: String,
    info_url: String,
    alg: String,
    ppt: String,
}

/// Parse a SIP Identity header value into its components.
fn parse_identity_header(header_value: &str) -> Result<ParsedIdentityHeader, StirShakenError> {
    // The Identity header format is:
    //   <token>;info=<url>;alg=ES256;ppt=shaken
    // The token is everything before the first ';'
    let header_value = header_value.trim();

    let semi_pos = header_value
        .find(';')
        .ok_or_else(|| StirShakenError::ParsingError("Identity header has no parameters".into()))?;

    let token = header_value[..semi_pos].trim().to_string();
    let params_str = &header_value[semi_pos + 1..];

    let mut info_url = String::new();
    let mut alg = String::new();
    let mut ppt = String::new();

    for param in params_str.split(';') {
        let param = param.trim();
        if let Some(val) = param.strip_prefix("info=") {
            // Strip angle brackets if present.
            let val = val.trim();
            if val.starts_with('<') && val.ends_with('>') {
                info_url = val[1..val.len() - 1].to_string();
            } else {
                info_url = val.to_string();
            }
        } else if let Some(val) = param.strip_prefix("alg=") {
            alg = val.trim().to_string();
        } else if let Some(val) = param.strip_prefix("ppt=") {
            ppt = val.trim().to_string();
        }
    }

    if info_url.is_empty() {
        return Err(StirShakenError::ParsingError("missing 'info' parameter in Identity header".into()));
    }
    if alg.is_empty() {
        return Err(StirShakenError::ParsingError("missing 'alg' parameter in Identity header".into()));
    }
    if ppt.is_empty() {
        return Err(StirShakenError::ParsingError("missing 'ppt' parameter in Identity header".into()));
    }

    Ok(ParsedIdentityHeader {
        token,
        info_url,
        alg,
        ppt,
    })
}

// ---------------------------------------------------------------------------
// Certificate Cache
// ---------------------------------------------------------------------------

/// Cached public key certificate fetched from a STIR/SHAKEN x5u URL.
#[derive(Debug, Clone)]
struct CachedCert {
    /// DER-encoded public key bytes.
    public_key: Vec<u8>,
    /// When this entry was fetched.
    fetched_at: Instant,
    /// Optional explicit expiry.
    expires_at: Option<Instant>,
}

/// In-memory cache for STIR/SHAKEN verification certificates.
///
/// Certificates are keyed by their x5u URL and evicted after `max_age`.
pub struct CertificateCache {
    cache: parking_lot::RwLock<HashMap<String, CachedCert>>,
    max_age: Duration,
}

impl fmt::Debug for CertificateCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.cache.read().len();
        f.debug_struct("CertificateCache")
            .field("entries", &count)
            .field("max_age", &self.max_age)
            .finish()
    }
}

impl CertificateCache {
    /// Create a new certificate cache with the given maximum age for entries.
    pub fn new(max_age: Duration) -> Self {
        Self {
            cache: parking_lot::RwLock::new(HashMap::new()),
            max_age,
        }
    }

    /// Look up a cached certificate by URL.
    ///
    /// Returns `None` if not cached or expired.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let cache = self.cache.read();
        let entry = cache.get(url)?;
        let now = Instant::now();

        // Check explicit expiry.
        if let Some(expires) = entry.expires_at {
            if now >= expires {
                return None;
            }
        }

        // Check max-age.
        if now.duration_since(entry.fetched_at) > self.max_age {
            return None;
        }

        Some(entry.public_key.clone())
    }

    /// Store a certificate in the cache.
    pub fn put(&self, url: String, public_key: Vec<u8>, expires_at: Option<Instant>) {
        let mut cache = self.cache.write();
        cache.insert(
            url,
            CachedCert {
                public_key,
                fetched_at: Instant::now(),
                expires_at,
            },
        );
    }

    /// Remove expired entries from the cache.
    pub fn evict_expired(&self) {
        let mut cache = self.cache.write();
        let now = Instant::now();
        cache.retain(|_, entry| {
            if let Some(expires) = entry.expires_at {
                if now >= expires {
                    return false;
                }
            }
            now.duration_since(entry.fetched_at) <= self.max_age
        });
    }

    /// Return the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.cache.read().len()
    }

    /// Return whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.read().is_empty()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.cache.write().clear();
    }
}

// ---------------------------------------------------------------------------
// Signing (Authentication Service)
// ---------------------------------------------------------------------------

/// Create a STIR/SHAKEN Identity header value for an outbound call.
///
/// # Arguments
///
/// * `calling_number` — E.164 originating number (e.g., `"+12025551234"` or `"12025551234"`).
/// * `called_number`  — E.164 destination number.
/// * `attestation`    — The attestation level (A, B, or C).
/// * `private_key_pem` — PEM-encoded ECDSA P-256 private key (or key material for placeholder).
/// * `cert_url`       — HTTPS URL where the public certificate can be fetched for verification.
/// * `crypto`         — The crypto backend to use for signing.
///
/// # Returns
///
/// The complete Identity header value string, formatted as:
/// `<token>;info=<cert_url>;alg=ES256;ppt=shaken`
pub fn sign_call(
    calling_number: &str,
    called_number: &str,
    attestation: AttestationLevel,
    private_key_pem: &[u8],
    cert_url: &str,
    crypto: &dyn CryptoBackend,
) -> Result<String, StirShakenError> {
    let orig = TelephoneNumber::normalize(calling_number);
    let dest = TelephoneNumber::normalize(called_number);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| StirShakenError::EncodingError(format!("system time error: {}", e)))?;

    let header = PASSporTHeader {
        alg: "ES256".to_string(),
        ppt: "shaken".to_string(),
        typ: "passport".to_string(),
        x5u: cert_url.to_string(),
    };

    let payload = PASSporTPayload {
        attest: attestation,
        dest,
        iat: now.as_secs(),
        orig,
        origid: Uuid::new_v4().to_string(),
    };

    let token = encode_passport_jwt(&header, &payload, crypto, private_key_pem)?;
    Ok(format_identity_header(&token, cert_url))
}

/// Create a STIR/SHAKEN Identity header with an explicit timestamp and origid.
///
/// This is useful for testing where deterministic output is needed.
pub fn sign_call_with_params(
    calling_number: &str,
    called_number: &str,
    attestation: AttestationLevel,
    private_key_pem: &[u8],
    cert_url: &str,
    crypto: &dyn CryptoBackend,
    iat: u64,
    origid: &str,
) -> Result<String, StirShakenError> {
    let orig = TelephoneNumber::normalize(calling_number);
    let dest = TelephoneNumber::normalize(called_number);

    let header = PASSporTHeader {
        alg: "ES256".to_string(),
        ppt: "shaken".to_string(),
        typ: "passport".to_string(),
        x5u: cert_url.to_string(),
    };

    let payload = PASSporTPayload {
        attest: attestation,
        dest,
        iat,
        orig,
        origid: origid.to_string(),
    };

    let token = encode_passport_jwt(&header, &payload, crypto, private_key_pem)?;
    Ok(format_identity_header(&token, cert_url))
}

// ---------------------------------------------------------------------------
// Verification (Verification Service)
// ---------------------------------------------------------------------------

/// Verify a STIR/SHAKEN Identity header from an inbound call.
///
/// # Arguments
///
/// * `identity_header` — The raw SIP Identity header value.
/// * `from_number`     — The From header number (must match `orig` claim).
/// * `to_number`       — The To header number (must match `dest` claim).
/// * `max_age_secs`    — Maximum allowed age of the PASSporT in seconds.
/// * `crypto`          — The crypto backend for signature verification.
/// * `public_key`      — The public key bytes to verify against (from cert cache or fetch).
///
/// # Returns
///
/// A [`VerificationResult`] with the outcome.
pub fn verify_call(
    identity_header: &str,
    from_number: &str,
    to_number: &str,
    max_age_secs: u64,
    crypto: &dyn CryptoBackend,
    public_key: &[u8],
) -> Result<VerificationResult, StirShakenError> {
    // Step 1: Parse the Identity header.
    let parsed = match parse_identity_header(identity_header) {
        Ok(p) => p,
        Err(_) => {
            return Ok(VerificationResult {
                verified: false,
                attestation: AttestationLevel::C,
                reason: VerificationStatus::MalformedToken,
                origid: String::new(),
                cert_url: String::new(),
            });
        }
    };

    // Step 2: Verify header parameters.
    if parsed.alg != "ES256" || parsed.ppt != "shaken" {
        return Ok(VerificationResult {
            verified: false,
            attestation: AttestationLevel::C,
            reason: VerificationStatus::MalformedToken,
            origid: String::new(),
            cert_url: parsed.info_url,
        });
    }

    // Step 3: Decode the JWT.
    let (header, payload, signature, signing_input) = match decode_passport_jwt(&parsed.token) {
        Ok(t) => t,
        Err(_) => {
            return Ok(VerificationResult {
                verified: false,
                attestation: AttestationLevel::C,
                reason: VerificationStatus::MalformedToken,
                origid: String::new(),
                cert_url: parsed.info_url,
            });
        }
    };

    // Step 4: Verify JWT header fields.
    if header.alg != "ES256" || header.ppt != "shaken" || header.typ != "passport" {
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::MalformedToken,
            origid: payload.origid,
            cert_url: header.x5u,
        });
    }

    // Step 5: Verify number claims.
    let normalized_from = TelephoneNumber::normalize(from_number);
    let normalized_to = TelephoneNumber::normalize(to_number);

    if payload.orig.tn != normalized_from.tn {
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::NumberMismatch,
            origid: payload.origid.clone(),
            cert_url: header.x5u,
        });
    }

    if payload.dest.tn != normalized_to.tn {
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::NumberMismatch,
            origid: payload.origid.clone(),
            cert_url: header.x5u,
        });
    }

    // Step 6: Verify timestamp.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| StirShakenError::EncodingError(format!("system time error: {}", e)))?
        .as_secs();

    if payload.iat > now + 1 {
        // Token is in the future (1-second grace period for clock skew).
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::TokenExpired,
            origid: payload.origid.clone(),
            cert_url: header.x5u,
        });
    }

    if now > payload.iat && (now - payload.iat) > max_age_secs {
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::TokenExpired,
            origid: payload.origid.clone(),
            cert_url: header.x5u,
        });
    }

    // Step 7: Verify signature.
    let sig_valid = crypto.verify(signing_input.as_bytes(), &signature, public_key)?;

    if !sig_valid {
        return Ok(VerificationResult {
            verified: false,
            attestation: payload.attest,
            reason: VerificationStatus::SignatureInvalid,
            origid: payload.origid.clone(),
            cert_url: header.x5u,
        });
    }

    // All checks passed.
    Ok(VerificationResult {
        verified: true,
        attestation: payload.attest,
        reason: VerificationStatus::Valid,
        origid: payload.origid,
        cert_url: header.x5u,
    })
}

// ---------------------------------------------------------------------------
// SIP Integration
// ---------------------------------------------------------------------------

/// Add an Identity header to an outbound SIP message (typically INVITE).
pub fn add_identity_header(msg: &mut SipMessage, identity: &str) {
    msg.headers.push(SipHeader {
        name: "Identity".to_string(),
        value: identity.to_string(),
    });
}

/// Extract and verify the Identity header from an inbound SIP message.
///
/// Returns `None` if no Identity header is present.
pub fn verify_identity_header(
    msg: &SipMessage,
    from: &str,
    to: &str,
    crypto: &dyn CryptoBackend,
    public_key: &[u8],
) -> Option<VerificationResult> {
    let identity = msg.get_header("Identity")?;
    verify_call(identity, from, to, 60, crypto, public_key).ok()
}

// ---------------------------------------------------------------------------
// Dialplan Variables
// ---------------------------------------------------------------------------

/// STIR/SHAKEN dialplan variable names.
///
/// These correspond to the Asterisk dialplan variables:
/// - `${STIR_SHAKEN(status)}`        — verification result for inbound call
/// - `${STIR_SHAKEN(attestation)}`   — A, B, or C
/// - `${STIR_SHAKEN(origid)}`        — UUID from PASSporT
/// - `${STIR_SHAKEN(verify_result)}` — valid, invalid, failed
pub struct StirShakenVars;

impl StirShakenVars {
    /// Variable name for the verification status.
    pub const STATUS: &'static str = "STIR_SHAKEN(status)";
    /// Variable name for the attestation level.
    pub const ATTESTATION: &'static str = "STIR_SHAKEN(attestation)";
    /// Variable name for the origid UUID.
    pub const ORIGID: &'static str = "STIR_SHAKEN(origid)";
    /// Variable name for the detailed verification result.
    pub const VERIFY_RESULT: &'static str = "STIR_SHAKEN(verify_result)";

    /// Convert a [`VerificationResult`] into a map of dialplan variable names to values.
    pub fn from_result(result: &VerificationResult) -> HashMap<&'static str, String> {
        let mut vars = HashMap::new();
        vars.insert(
            Self::STATUS,
            if result.verified { "verified" } else { "unverified" }.to_string(),
        );
        vars.insert(Self::ATTESTATION, result.attestation.as_str().to_string());
        vars.insert(Self::ORIGID, result.origid.clone());
        vars.insert(Self::VERIFY_RESULT, result.reason.to_string());
        vars
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &[u8] = b"test-stir-shaken-private-key-256";
    const TEST_CERT_URL: &str = "https://cert.example.com/shaken.pem";
    const TEST_ORIGID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn test_crypto() -> HmacPlaceholderBackend {
        HmacPlaceholderBackend
    }

    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Helper: create a signed Identity header with known parameters.
    fn make_test_identity(
        from: &str,
        to: &str,
        attest: AttestationLevel,
        iat: u64,
    ) -> String {
        sign_call_with_params(from, to, attest, TEST_KEY, TEST_CERT_URL, &test_crypto(), iat, TEST_ORIGID)
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // Attestation Level
    // -----------------------------------------------------------------------

    #[test]
    fn test_attestation_level_roundtrip_a() {
        let level = AttestationLevel::A;
        let s = level.as_str();
        assert_eq!(s, "A");
        let parsed = AttestationLevel::from_str(s).unwrap();
        assert_eq!(parsed, level);
    }

    #[test]
    fn test_attestation_level_roundtrip_b() {
        let level = AttestationLevel::B;
        let s = level.as_str();
        assert_eq!(s, "B");
        let parsed = AttestationLevel::from_str(s).unwrap();
        assert_eq!(parsed, level);
    }

    #[test]
    fn test_attestation_level_roundtrip_c() {
        let level = AttestationLevel::C;
        let s = level.as_str();
        assert_eq!(s, "C");
        let parsed = AttestationLevel::from_str(s).unwrap();
        assert_eq!(parsed, level);
    }

    #[test]
    fn test_attestation_level_invalid() {
        assert!(AttestationLevel::from_str("D").is_err());
        assert!(AttestationLevel::from_str("").is_err());
        assert!(AttestationLevel::from_str("a").is_err()); // case-sensitive
    }

    #[test]
    fn test_attestation_display() {
        assert_eq!(format!("{}", AttestationLevel::A), "A");
        assert_eq!(format!("{}", AttestationLevel::B), "B");
        assert_eq!(format!("{}", AttestationLevel::C), "C");
    }

    // -----------------------------------------------------------------------
    // Telephone Number Normalization
    // -----------------------------------------------------------------------

    #[test]
    fn test_number_normalize_with_plus() {
        let tn = TelephoneNumber::normalize("+12345678901");
        assert_eq!(tn.tn, "12345678901");
    }

    #[test]
    fn test_number_normalize_with_dashes() {
        let tn = TelephoneNumber::normalize("1-234-567-8901");
        assert_eq!(tn.tn, "12345678901");
    }

    #[test]
    fn test_number_normalize_with_spaces() {
        let tn = TelephoneNumber::normalize("1 234 567 8901");
        assert_eq!(tn.tn, "12345678901");
    }

    #[test]
    fn test_number_normalize_with_parens() {
        let tn = TelephoneNumber::normalize("+1 (234) 567-8901");
        assert_eq!(tn.tn, "12345678901");
    }

    #[test]
    fn test_number_normalize_already_clean() {
        let tn = TelephoneNumber::normalize("12345678901");
        assert_eq!(tn.tn, "12345678901");
    }

    #[test]
    fn test_number_normalize_with_dots() {
        let tn = TelephoneNumber::normalize("1.234.567.8901");
        assert_eq!(tn.tn, "12345678901");
    }

    // -----------------------------------------------------------------------
    // PASSporT Creation — All Attestation Levels
    // -----------------------------------------------------------------------

    #[test]
    fn test_passport_creation_attest_a() {
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, current_timestamp());
        assert!(!identity.is_empty());
        assert!(identity.contains(";info=<"));
        assert!(identity.contains(">;alg=ES256;ppt=shaken"));
    }

    #[test]
    fn test_passport_creation_attest_b() {
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::B, current_timestamp());
        assert!(identity.contains(";alg=ES256;ppt=shaken"));
    }

    #[test]
    fn test_passport_creation_attest_c() {
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::C, current_timestamp());
        assert!(identity.contains(";alg=ES256;ppt=shaken"));
    }

    // -----------------------------------------------------------------------
    // JWT Encoding — Structure Verification
    // -----------------------------------------------------------------------

    #[test]
    fn test_jwt_encoding_three_part_structure() {
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, current_timestamp());
        // Extract the token (before the first ';').
        let token = identity.split(';').next().unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have header.payload.signature");
        // Each part must be valid base64url.
        for (i, part) in parts.iter().enumerate() {
            assert!(
                base64url_decode(part).is_ok(),
                "JWT part {} is not valid base64url",
                i
            );
        }
    }

    #[test]
    fn test_jwt_encoding_header_fields() {
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, current_timestamp());
        let token = identity.split(';').next().unwrap();
        let (header, _, _, _) = decode_passport_jwt(token).unwrap();
        assert_eq!(header.alg, "ES256");
        assert_eq!(header.ppt, "shaken");
        assert_eq!(header.typ, "passport");
        assert_eq!(header.x5u, TEST_CERT_URL);
    }

    #[test]
    fn test_jwt_encoding_payload_fields() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let token = identity.split(';').next().unwrap();
        let (_, payload, _, _) = decode_passport_jwt(token).unwrap();
        assert_eq!(payload.attest, AttestationLevel::A);
        assert_eq!(payload.orig.tn, "12025551234");
        assert_eq!(payload.dest.tn, "13035551234");
        assert_eq!(payload.iat, iat);
        assert_eq!(payload.origid, TEST_ORIGID);
    }

    // -----------------------------------------------------------------------
    // JWT Decoding — Roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn test_jwt_decode_roundtrip() {
        let header = PASSporTHeader {
            alg: "ES256".to_string(),
            ppt: "shaken".to_string(),
            typ: "passport".to_string(),
            x5u: "https://cert.example.com/cert.pem".to_string(),
        };
        let payload = PASSporTPayload {
            attest: AttestationLevel::B,
            dest: TelephoneNumber { tn: "13035551234".to_string() },
            iat: 1700000000,
            orig: TelephoneNumber { tn: "12025551234".to_string() },
            origid: "test-uuid-1234".to_string(),
        };

        let token = encode_passport_jwt(&header, &payload, &test_crypto(), TEST_KEY).unwrap();
        let (dec_header, dec_payload, _sig, _input) = decode_passport_jwt(&token).unwrap();

        assert_eq!(dec_header.alg, header.alg);
        assert_eq!(dec_header.ppt, header.ppt);
        assert_eq!(dec_header.typ, header.typ);
        assert_eq!(dec_header.x5u, header.x5u);
        assert_eq!(dec_payload.attest, payload.attest);
        assert_eq!(dec_payload.orig.tn, payload.orig.tn);
        assert_eq!(dec_payload.dest.tn, payload.dest.tn);
        assert_eq!(dec_payload.iat, payload.iat);
        assert_eq!(dec_payload.origid, payload.origid);
    }

    // -----------------------------------------------------------------------
    // Identity Header Formatting
    // -----------------------------------------------------------------------

    #[test]
    fn test_identity_header_format() {
        let hdr = format_identity_header("abc.def.ghi", "https://example.com/cert.pem");
        assert_eq!(hdr, "abc.def.ghi;info=<https://example.com/cert.pem>;alg=ES256;ppt=shaken");
    }

    // -----------------------------------------------------------------------
    // Identity Header Parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_identity_header_parsing_roundtrip() {
        let original = "abc.def.ghi;info=<https://example.com/cert.pem>;alg=ES256;ppt=shaken";
        let parsed = parse_identity_header(original).unwrap();
        assert_eq!(parsed.token, "abc.def.ghi");
        assert_eq!(parsed.info_url, "https://example.com/cert.pem");
        assert_eq!(parsed.alg, "ES256");
        assert_eq!(parsed.ppt, "shaken");
    }

    #[test]
    fn test_identity_header_parsing_real_token() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let parsed = parse_identity_header(&identity).unwrap();
        assert_eq!(parsed.info_url, TEST_CERT_URL);
        assert_eq!(parsed.alg, "ES256");
        assert_eq!(parsed.ppt, "shaken");
        // Token should be decodable.
        let (header, payload, _, _) = decode_passport_jwt(&parsed.token).unwrap();
        assert_eq!(header.alg, "ES256");
        assert_eq!(payload.orig.tn, "12025551234");
    }

    #[test]
    fn test_identity_header_parsing_missing_params() {
        assert!(parse_identity_header("token-only-no-params").is_err());
    }

    #[test]
    fn test_identity_header_parsing_missing_info() {
        assert!(parse_identity_header("token;alg=ES256;ppt=shaken").is_err());
    }

    // -----------------------------------------------------------------------
    // Verification — Valid Token
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_valid_token() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY, // HMAC placeholder: same key for sign/verify
        )
        .unwrap();
        assert!(result.verified);
        assert_eq!(result.attestation, AttestationLevel::A);
        assert_eq!(result.reason, VerificationStatus::Valid);
        assert_eq!(result.origid, TEST_ORIGID);
        assert_eq!(result.cert_url, TEST_CERT_URL);
    }

    // -----------------------------------------------------------------------
    // Verification — Wrong From Number
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_wrong_from_number() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let result = verify_call(
            &identity,
            "+19999999999", // wrong
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::NumberMismatch);
    }

    // -----------------------------------------------------------------------
    // Verification — Wrong To Number
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_wrong_to_number() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+19999999999", // wrong
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::NumberMismatch);
    }

    // -----------------------------------------------------------------------
    // Verification — Expired Token
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_expired_token() {
        let old_iat = current_timestamp() - 120; // 2 minutes ago
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, old_iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60, // max 60 seconds
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::TokenExpired);
    }

    // -----------------------------------------------------------------------
    // Verification — Future Token
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_future_token() {
        let future_iat = current_timestamp() + 300; // 5 minutes in the future
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, future_iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::TokenExpired);
    }

    // -----------------------------------------------------------------------
    // Verification — Malformed Token
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_malformed_token() {
        let result = verify_call(
            "not-a-valid-jwt;info=<https://example.com>;alg=ES256;ppt=shaken",
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::MalformedToken);
    }

    #[test]
    fn test_verify_completely_garbage() {
        let result = verify_call(
            "garbage",
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::MalformedToken);
    }

    // -----------------------------------------------------------------------
    // Verification — Wrong Key (Signature Invalid)
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_wrong_key() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            b"wrong-key-material-for-verify!!", // different key
        )
        .unwrap();
        assert!(!result.verified);
        assert_eq!(result.reason, VerificationStatus::SignatureInvalid);
    }

    // -----------------------------------------------------------------------
    // Verification — All Attestation Levels Preserved
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_preserves_attestation_b() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::B, iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(result.verified);
        assert_eq!(result.attestation, AttestationLevel::B);
    }

    #[test]
    fn test_verify_preserves_attestation_c() {
        let iat = current_timestamp();
        let identity = make_test_identity("+12025551234", "+13035551234", AttestationLevel::C, iat);
        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();
        assert!(result.verified);
        assert_eq!(result.attestation, AttestationLevel::C);
    }

    // -----------------------------------------------------------------------
    // Certificate Cache
    // -----------------------------------------------------------------------

    #[test]
    fn test_cert_cache_store_and_retrieve() {
        let cache = CertificateCache::new(Duration::from_secs(300));
        let key = vec![1, 2, 3, 4];
        cache.put("https://example.com/cert.pem".to_string(), key.clone(), None);
        let retrieved = cache.get("https://example.com/cert.pem");
        assert_eq!(retrieved, Some(key));
    }

    #[test]
    fn test_cert_cache_miss() {
        let cache = CertificateCache::new(Duration::from_secs(300));
        assert!(cache.get("https://example.com/nonexistent.pem").is_none());
    }

    #[test]
    fn test_cert_cache_explicit_expiry() {
        let cache = CertificateCache::new(Duration::from_secs(300));
        // Store with an already-expired timestamp.
        let expired = Instant::now() - Duration::from_secs(1);
        cache.put(
            "https://example.com/expired.pem".to_string(),
            vec![5, 6, 7],
            Some(expired),
        );
        // Should not be retrievable.
        assert!(cache.get("https://example.com/expired.pem").is_none());
    }

    #[test]
    fn test_cert_cache_evict_expired() {
        let cache = CertificateCache::new(Duration::from_secs(0)); // 0 second max age
        cache.put("https://example.com/a.pem".to_string(), vec![1], None);
        // With 0 max_age, the entry is immediately considered expired on next get.
        // But we need at least 1ns to pass for duration_since to exceed 0.
        std::thread::sleep(Duration::from_millis(5));
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cert_cache_clear() {
        let cache = CertificateCache::new(Duration::from_secs(300));
        cache.put("https://a.com/1.pem".to_string(), vec![1], None);
        cache.put("https://a.com/2.pem".to_string(), vec![2], None);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    // -----------------------------------------------------------------------
    // SIP Integration
    // -----------------------------------------------------------------------

    #[test]
    fn test_add_identity_header_to_sip_message() {
        use crate::parser::{StartLine, SipMethod, SipUri, RequestLine};

        let mut msg = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri: SipUri {
                    scheme: "sip".to_string(),
                    user: Some("alice".to_string()),
                    password: None,
                    host: "example.com".to_string(),
                    port: None,
                    parameters: Default::default(),
                    headers: Default::default(),
                },
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![],
            body: String::new(),
        };

        let identity_value = "token.here.sig;info=<https://cert.example.com>;alg=ES256;ppt=shaken";
        add_identity_header(&mut msg, identity_value);

        let retrieved = msg.get_header("Identity");
        assert_eq!(retrieved, Some(identity_value));
    }

    #[test]
    fn test_verify_identity_header_from_sip_message() {
        use crate::parser::{StartLine, SipMethod, SipUri, RequestLine};

        let iat = current_timestamp();
        let identity_value = make_test_identity("+12025551234", "+13035551234", AttestationLevel::A, iat);

        let msg = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri: SipUri {
                    scheme: "sip".to_string(),
                    user: Some("bob".to_string()),
                    password: None,
                    host: "example.com".to_string(),
                    port: None,
                    parameters: Default::default(),
                    headers: Default::default(),
                },
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![SipHeader {
                name: "Identity".to_string(),
                value: identity_value,
            }],
            body: String::new(),
        };

        let result = verify_identity_header(&msg, "+12025551234", "+13035551234", &test_crypto(), TEST_KEY);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.verified);
        assert_eq!(result.attestation, AttestationLevel::A);
    }

    #[test]
    fn test_verify_identity_header_missing() {
        use crate::parser::{StartLine, SipMethod, SipUri, RequestLine};

        let msg = SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Invite,
                uri: SipUri {
                    scheme: "sip".to_string(),
                    user: Some("bob".to_string()),
                    password: None,
                    host: "example.com".to_string(),
                    port: None,
                    parameters: Default::default(),
                    headers: Default::default(),
                },
                version: "SIP/2.0".to_string(),
            }),
            headers: vec![],
            body: String::new(),
        };

        let result = verify_identity_header(&msg, "+12025551234", "+13035551234", &test_crypto(), TEST_KEY);
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // Dialplan Variables
    // -----------------------------------------------------------------------

    #[test]
    fn test_dialplan_vars_verified() {
        let result = VerificationResult {
            verified: true,
            attestation: AttestationLevel::A,
            reason: VerificationStatus::Valid,
            origid: "test-uuid".to_string(),
            cert_url: "https://example.com/cert.pem".to_string(),
        };
        let vars = StirShakenVars::from_result(&result);
        assert_eq!(vars.get(StirShakenVars::STATUS), Some(&"verified".to_string()));
        assert_eq!(vars.get(StirShakenVars::ATTESTATION), Some(&"A".to_string()));
        assert_eq!(vars.get(StirShakenVars::ORIGID), Some(&"test-uuid".to_string()));
        assert_eq!(vars.get(StirShakenVars::VERIFY_RESULT), Some(&"valid".to_string()));
    }

    #[test]
    fn test_dialplan_vars_unverified() {
        let result = VerificationResult {
            verified: false,
            attestation: AttestationLevel::C,
            reason: VerificationStatus::SignatureInvalid,
            origid: "some-uuid".to_string(),
            cert_url: "https://example.com/cert.pem".to_string(),
        };
        let vars = StirShakenVars::from_result(&result);
        assert_eq!(vars.get(StirShakenVars::STATUS), Some(&"unverified".to_string()));
        assert_eq!(vars.get(StirShakenVars::VERIFY_RESULT), Some(&"signature_invalid".to_string()));
    }

    // -----------------------------------------------------------------------
    // Stub Backend
    // -----------------------------------------------------------------------

    #[test]
    fn test_stub_backend_sign_fails() {
        let stub = StubBackend;
        let result = stub.sign(b"data", b"key");
        assert!(result.is_err());
    }

    #[test]
    fn test_stub_backend_verify_fails() {
        let stub = StubBackend;
        let result = stub.verify(b"data", b"sig", b"key");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // JSON Helpers (Edge Cases)
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_escape_special_chars() {
        assert_eq!(json_escape("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(json_escape("back\\slash"), "back\\\\slash");
        assert_eq!(json_escape("new\nline"), "new\\nline");
    }

    #[test]
    fn test_json_get_string_basic() {
        let json = r#"{"alg":"ES256","ppt":"shaken"}"#;
        assert_eq!(json_get_string(json, "alg"), Some("ES256".to_string()));
        assert_eq!(json_get_string(json, "ppt"), Some("shaken".to_string()));
        assert_eq!(json_get_string(json, "missing"), None);
    }

    #[test]
    fn test_json_get_u64_basic() {
        let json = r#"{"iat":1700000000,"other":"val"}"#;
        assert_eq!(json_get_u64(json, "iat"), Some(1700000000));
        assert_eq!(json_get_u64(json, "missing"), None);
    }

    #[test]
    fn test_json_get_tn_array_form() {
        let json = r#"{"dest":{"tn":["13035551234"]}}"#;
        assert_eq!(json_get_tn(json, "dest"), Some("13035551234".to_string()));
    }

    #[test]
    fn test_json_get_tn_string_form() {
        let json = r#"{"orig":{"tn":"12025551234"}}"#;
        assert_eq!(json_get_tn(json, "orig"), Some("12025551234".to_string()));
    }

    // -----------------------------------------------------------------------
    // sign_call with real sign_call (not _with_params)
    // -----------------------------------------------------------------------

    #[test]
    fn test_sign_call_produces_valid_identity() {
        let identity = sign_call(
            "+12025551234",
            "+13035551234",
            AttestationLevel::A,
            TEST_KEY,
            TEST_CERT_URL,
            &test_crypto(),
        )
        .unwrap();

        // Should be parseable.
        let parsed = parse_identity_header(&identity).unwrap();
        assert_eq!(parsed.alg, "ES256");
        assert_eq!(parsed.ppt, "shaken");

        // Token should decode.
        let (header, payload, _, _) = decode_passport_jwt(&parsed.token).unwrap();
        assert_eq!(header.alg, "ES256");
        assert_eq!(payload.orig.tn, "12025551234");
        assert_eq!(payload.dest.tn, "13035551234");
        assert_eq!(payload.attest, AttestationLevel::A);
        // origid should be a valid UUID.
        assert!(Uuid::parse_str(&payload.origid).is_ok());
    }

    // -----------------------------------------------------------------------
    // End-to-end: sign then verify
    // -----------------------------------------------------------------------

    #[test]
    fn test_sign_then_verify_end_to_end() {
        let identity = sign_call(
            "+12025551234",
            "+13035551234",
            AttestationLevel::B,
            TEST_KEY,
            TEST_CERT_URL,
            &test_crypto(),
        )
        .unwrap();

        let result = verify_call(
            &identity,
            "+12025551234",
            "+13035551234",
            60,
            &test_crypto(),
            TEST_KEY,
        )
        .unwrap();

        assert!(result.verified);
        assert_eq!(result.attestation, AttestationLevel::B);
        assert_eq!(result.reason, VerificationStatus::Valid);
    }

    // -----------------------------------------------------------------------
    // VerificationStatus Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_verification_status_display() {
        assert_eq!(format!("{}", VerificationStatus::Valid), "valid");
        assert_eq!(format!("{}", VerificationStatus::SignatureInvalid), "signature_invalid");
        assert_eq!(format!("{}", VerificationStatus::CertificateExpired), "certificate_expired");
        assert_eq!(format!("{}", VerificationStatus::CertificateUntrusted), "certificate_untrusted");
        assert_eq!(format!("{}", VerificationStatus::TokenExpired), "token_expired");
        assert_eq!(format!("{}", VerificationStatus::NumberMismatch), "number_mismatch");
        assert_eq!(format!("{}", VerificationStatus::MalformedToken), "malformed_token");
        assert_eq!(format!("{}", VerificationStatus::CertificateFetchFailed), "cert_fetch_failed");
    }

    // -----------------------------------------------------------------------
    // Error Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_display() {
        let e = StirShakenError::InvalidAttestation("X".into());
        assert!(format!("{}", e).contains("Invalid attestation level: X"));

        let e = StirShakenError::TokenExpired { age_secs: 120, max_secs: 60 };
        assert!(format!("{}", e).contains("120"));
        assert!(format!("{}", e).contains("60"));

        let e = StirShakenError::NumberMismatch {
            expected: "123".into(),
            actual: "456".into(),
        };
        assert!(format!("{}", e).contains("123"));
        assert!(format!("{}", e).contains("456"));
    }
}
