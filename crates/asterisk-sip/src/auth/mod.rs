//! SIP Digest Authentication (RFC 2617 / RFC 7616).
//!
//! Implements HTTP Digest authentication for SIP, supporting MD5 and SHA-256.

use md5::{Md5, Digest};
use sha2::Sha256;

/// A digest authentication challenge parsed from WWW-Authenticate or
/// Proxy-Authenticate headers.
#[derive(Debug, Clone)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub algorithm: DigestAlgorithm,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    pub stale: bool,
    pub domain: Option<String>,
}

/// Supported digest algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Md5,
    Md5Sess,
    Sha256,
    Sha256Sess,
}

impl DigestAlgorithm {
    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "md5" | "" => Self::Md5,
            "md5-sess" => Self::Md5Sess,
            "sha-256" | "sha256" => Self::Sha256,
            "sha-256-sess" | "sha256-sess" => Self::Sha256Sess,
            _ => Self::Md5, // Default fallback
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Md5Sess => "MD5-sess",
            Self::Sha256 => "SHA-256",
            Self::Sha256Sess => "SHA-256-sess",
        }
    }
}

/// Credentials for digest authentication.
#[derive(Debug, Clone)]
pub struct DigestCredentials {
    pub username: String,
    pub password: String,
    pub realm: String,
}

impl DigestChallenge {
    /// Parse a digest challenge from a WWW-Authenticate or Proxy-Authenticate
    /// header value.
    pub fn parse(header_value: &str) -> Option<Self> {
        let value = header_value.trim();
        let rest = value.strip_prefix("Digest")?.trim();

        let mut realm = String::new();
        let mut nonce = String::new();
        let mut algorithm = DigestAlgorithm::Md5;
        let mut qop = None;
        let mut opaque = None;
        let mut stale = false;
        let mut domain = None;

        for param in split_digest_params(rest) {
            let (key, val) = match param.split_once('=') {
                Some((k, v)) => (k.trim().to_lowercase(), unquote(v.trim())),
                None => continue,
            };

            match key.as_str() {
                "realm" => realm = val,
                "nonce" => nonce = val,
                "algorithm" => algorithm = DigestAlgorithm::from_name(&val),
                "qop" => qop = Some(val),
                "opaque" => opaque = Some(val),
                "stale" => stale = val.eq_ignore_ascii_case("true"),
                "domain" => domain = Some(val),
                _ => {}
            }
        }

        if realm.is_empty() || nonce.is_empty() {
            return None;
        }

        Some(DigestChallenge {
            realm,
            nonce,
            algorithm,
            qop,
            opaque,
            stale,
            domain,
        })
    }
}

/// Create a digest response string for an Authorization or Proxy-Authorization header.
pub fn create_digest_response(
    challenge: &DigestChallenge,
    credentials: &DigestCredentials,
    method: &str,
    uri: &str,
) -> String {
    let cnonce = generate_cnonce();
    let nc = "00000001";

    // HA1 = H(username:realm:password)
    let ha1 = compute_hash(
        &format!("{}:{}:{}", credentials.username, challenge.realm, credentials.password),
        challenge.algorithm,
    );

    // For -sess algorithms, HA1 = H(H(username:realm:password):nonce:cnonce)
    let ha1 = if matches!(
        challenge.algorithm,
        DigestAlgorithm::Md5Sess | DigestAlgorithm::Sha256Sess
    ) {
        compute_hash(
            &format!("{}:{}:{}", ha1, challenge.nonce, cnonce),
            challenge.algorithm,
        )
    } else {
        ha1
    };

    // HA2 = H(method:uri)
    let ha2 = compute_hash(
        &format!("{}:{}", method, uri),
        challenge.algorithm,
    );

    // Response
    let response = if let Some(ref qop) = challenge.qop {
        if qop.contains("auth") {
            // response = H(HA1:nonce:nc:cnonce:qop:HA2)
            compute_hash(
                &format!("{}:{}:{}:{}:auth:{}", ha1, challenge.nonce, nc, cnonce, ha2),
                challenge.algorithm,
            )
        } else {
            // response = H(HA1:nonce:HA2)
            compute_hash(
                &format!("{}:{}:{}", ha1, challenge.nonce, ha2),
                challenge.algorithm,
            )
        }
    } else {
        // response = H(HA1:nonce:HA2)
        compute_hash(
            &format!("{}:{}:{}", ha1, challenge.nonce, ha2),
            challenge.algorithm,
        )
    };

    // Build the Authorization header value
    let mut auth = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
        credentials.username, challenge.realm, challenge.nonce, uri, response
    );

    auth.push_str(&format!(", algorithm={}", challenge.algorithm.as_str()));

    if let Some(ref qop) = challenge.qop {
        if qop.contains("auth") {
            auth.push_str(&format!(", qop=auth, nc={}, cnonce=\"{}\"", nc, cnonce));
        }
    }

    if let Some(ref opaque) = challenge.opaque {
        auth.push_str(&format!(", opaque=\"{}\"", opaque));
    }

    auth
}

/// Compute a hash using the specified algorithm.
fn compute_hash(input: &str, algorithm: DigestAlgorithm) -> String {
    match algorithm {
        DigestAlgorithm::Md5 | DigestAlgorithm::Md5Sess => {
            let mut hasher = Md5::new();
            hasher.update(input.as_bytes());
            hex::encode(hasher.finalize())
        }
        DigestAlgorithm::Sha256 | DigestAlgorithm::Sha256Sess => {
            let mut hasher = Sha256::new();
            hasher.update(input.as_bytes());
            hex::encode(hasher.finalize())
        }
    }
}

/// Generate a random cnonce value.
fn generate_cnonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

/// Remove surrounding quotes from a string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Split digest parameters, respecting quoted strings.
fn split_digest_params(input: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    params.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        params.push(trimmed);
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_digest_challenge() {
        let header = r#"Digest realm="asterisk", nonce="abc123def456", algorithm=MD5, qop="auth""#;
        let challenge = DigestChallenge::parse(header).unwrap();
        assert_eq!(challenge.realm, "asterisk");
        assert_eq!(challenge.nonce, "abc123def456");
        assert_eq!(challenge.algorithm, DigestAlgorithm::Md5);
        assert_eq!(challenge.qop, Some("auth".to_string()));
    }

    #[test]
    fn test_digest_md5_response() {
        // RFC 2617 test vector
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".to_string(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: Some("auth".to_string()),
            opaque: Some("5ccc069c403ebaf9f0171e9517f40e41".to_string()),
            stale: false,
            domain: None,
        };

        let credentials = DigestCredentials {
            username: "Mufasa".to_string(),
            password: "Circle Of Life".to_string(),
            realm: "testrealm@host.com".to_string(),
        };

        let response = create_digest_response(
            &challenge,
            &credentials,
            "GET",
            "/dir/index.html",
        );

        // Verify the response contains expected fields
        assert!(response.contains("username=\"Mufasa\""));
        assert!(response.contains("realm=\"testrealm@host.com\""));
        assert!(response.contains("algorithm=MD5"));
        assert!(response.contains("qop=auth"));
        assert!(response.contains("response=\""));
    }

    #[test]
    fn test_digest_md5_no_qop() {
        let challenge = DigestChallenge {
            realm: "asterisk".to_string(),
            nonce: "testnonce".to_string(),
            algorithm: DigestAlgorithm::Md5,
            qop: None,
            opaque: None,
            stale: false,
            domain: None,
        };

        let credentials = DigestCredentials {
            username: "alice".to_string(),
            password: "secret".to_string(),
            realm: "asterisk".to_string(),
        };

        let response = create_digest_response(&challenge, &credentials, "REGISTER", "sip:asterisk");

        // Manually verify: HA1 = MD5(alice:asterisk:secret)
        let ha1 = format!("{:x}", Md5::digest(b"alice:asterisk:secret"));
        let ha2 = format!("{:x}", Md5::digest(b"REGISTER:sip:asterisk"));
        let expected = format!(
            "{:x}",
            Md5::digest(format!("{}:testnonce:{}", ha1, ha2).as_bytes())
        );
        assert!(response.contains(&format!("response=\"{}\"", expected)));
    }

    #[test]
    fn test_unquote() {
        assert_eq!(unquote("\"hello\""), "hello");
        assert_eq!(unquote("hello"), "hello");
        assert_eq!(unquote("\"\""), "");
    }
}
