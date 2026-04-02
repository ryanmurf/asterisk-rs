//! SIP Digest Authenticator -- inbound and outbound
//! (port of res_pjsip_authenticator_digest.c + res_pjsip_outbound_authenticator_digest.c).
//!
//! Inbound: verifies digest credentials in incoming REGISTER/INVITE
//! requests against stored passwords. Generates 401/407 challenges.
//!
//! Outbound: responds to 401/407 challenges from remote servers by
//! computing digest credentials and attaching Authorization headers.

use tracing::{debug, warn};

use crate::auth::{
    create_digest_response, DigestAlgorithm, DigestChallenge, DigestCredentials,
};
use crate::parser::{header_names, SipHeader, SipMessage, StartLine, StatusLine};

// ---------------------------------------------------------------------------
// Credential storage
// ---------------------------------------------------------------------------

/// Stored authentication credentials for a SIP identity.
#[derive(Debug, Clone)]
pub struct AuthCredentials {
    /// Authentication username.
    pub username: String,
    /// Plain-text password (or pre-hashed HA1 if `is_ha1` is true).
    pub password: String,
    /// Authentication realm. Empty matches any realm (wildcard).
    pub realm: String,
    /// If true, `password` contains a pre-computed HA1 hex digest
    /// instead of a plain-text password.
    pub is_ha1: bool,
}

impl AuthCredentials {
    pub fn new(username: &str, password: &str, realm: &str) -> Self {
        Self {
            username: username.to_string(),
            password: password.to_string(),
            realm: realm.to_string(),
            is_ha1: false,
        }
    }

    /// Create credentials with a pre-computed HA1 digest.
    pub fn with_ha1(username: &str, ha1: &str, realm: &str) -> Self {
        Self {
            username: username.to_string(),
            password: ha1.to_string(),
            realm: realm.to_string(),
            is_ha1: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Inbound authenticator (UAS side)
// ---------------------------------------------------------------------------

/// Default realm to use when no specific realm is configured.
const DEFAULT_REALM: &str = "asterisk";

/// The inbound authenticator verifies credentials in incoming requests.
#[derive(Debug)]
pub struct InboundAuthenticator {
    /// Default realm for challenges.
    pub default_realm: String,
    /// Supported algorithms (in preference order).
    pub supported_algorithms: Vec<DigestAlgorithm>,
}

impl InboundAuthenticator {
    pub fn new() -> Self {
        Self {
            default_realm: DEFAULT_REALM.to_string(),
            supported_algorithms: vec![DigestAlgorithm::Md5, DigestAlgorithm::Sha256],
        }
    }

    /// Check whether a request requires authentication.
    ///
    /// Returns true if `credentials` is non-empty (i.e. the endpoint has
    /// configured auth).
    pub fn requires_auth(&self, credentials: &[AuthCredentials]) -> bool {
        !credentials.is_empty()
    }

    /// Verify an incoming request against stored credentials.
    ///
    /// Returns `Ok(())` if authentication succeeds, or `Err(challenge_response)`
    /// containing a 401 or 407 response to send.
    #[allow(clippy::result_large_err)]
    pub fn verify(
        &self,
        request: &SipMessage,
        credentials: &[AuthCredentials],
        use_proxy_auth: bool,
    ) -> Result<(), SipMessage> {
        let auth_header_name = if use_proxy_auth {
            header_names::PROXY_AUTHORIZATION
        } else {
            header_names::AUTHORIZATION
        };

        // Look for an Authorization header in the request.
        let auth_hdr = request.get_header(auth_header_name);
        eprintln!("[DEBUG] Auth header name={}, present={}", auth_header_name, auth_hdr.is_some());

        if auth_hdr.is_none() {
            // No credentials provided -- send a challenge.
            return Err(self.build_challenge(request, credentials, use_proxy_auth));
        }

        let auth_value = auth_hdr.unwrap();

        // Parse the digest response from the Authorization header.
        // We need to extract username, realm, nonce, uri, response, algorithm.
        let parsed = parse_authorization(auth_value);
        let parsed = match parsed {
            Some(p) => p,
            None => {
                return Err(self.build_challenge(request, credentials, use_proxy_auth));
            }
        };

        // Find matching credentials.
        let cred = credentials
            .iter()
            .find(|c| {
                c.username == parsed.username
                    && (c.realm.is_empty()
                        || c.realm == "*"
                        || c.realm == parsed.realm)
            });

        let cred = match cred {
            Some(c) => c,
            None => {
                warn!(username = %parsed.username, "No matching credentials found");
                return Err(self.build_challenge(request, credentials, use_proxy_auth));
            }
        };

        // Verify the digest response.
        let method = request
            .method()
            .map(|m| m.as_str())
            .unwrap_or("UNKNOWN");

        let realm = if cred.realm.is_empty() || cred.realm == "*" {
            &self.default_realm
        } else {
            &cred.realm
        };

        let challenge = DigestChallenge {
            realm: realm.to_string(),
            nonce: parsed.nonce.clone(),
            algorithm: parsed.algorithm,
            qop: parsed.qop.clone(),
            opaque: parsed.opaque.clone(),
            stale: false,
            domain: None,
        };

        let digest_creds = DigestCredentials {
            username: cred.username.clone(),
            password: cred.password.clone(),
            realm: realm.to_string(),
        };

        let expected = create_digest_response(&challenge, &digest_creds, method, &parsed.uri);

        // Extract the response= value from the expected header.
        let expected_response = extract_response_value(&expected);
        eprintln!("[DEBUG] Auth verify: username={}, method={}, uri={}, realm={}, nonce={}, expected_response={}, parsed_response={}", 
                  cred.username, method, parsed.uri, realm, parsed.nonce, expected_response, parsed.response);
        if expected_response == parsed.response {
            debug!(username = %cred.username, "Authentication successful");
            Ok(())
        } else {
            warn!(username = %cred.username, "Authentication failed -- wrong password");
            Err(self.build_challenge(request, credentials, use_proxy_auth))
        }
    }

    /// Build a 401/407 challenge response.
    fn build_challenge(
        &self,
        request: &SipMessage,
        credentials: &[AuthCredentials],
        use_proxy: bool,
    ) -> SipMessage {
        let (code, reason, header_name) = if use_proxy {
            (407, "Proxy Authentication Required", header_names::PROXY_AUTHENTICATE)
        } else {
            (401, "Unauthorized", header_names::WWW_AUTHENTICATE)
        };

        let mut response = request
            .create_response(code, reason)
            .unwrap_or_else(|_| SipMessage {
                start_line: StartLine::Response(StatusLine {
                    version: "SIP/2.0".to_string(),
                    status_code: code,
                    reason_phrase: reason.to_string(),
                }),
                headers: Vec::new(),
                body: String::new(),
            });

        // Determine realm from the first credential that has one, else use default.
        let realm = credentials
            .iter()
            .find(|c| !c.realm.is_empty() && c.realm != "*")
            .map(|c| c.realm.as_str())
            .unwrap_or(&self.default_realm);

        let nonce = generate_nonce();

        // Add a challenge for the first preferred algorithm.
        if let Some(algo) = self.supported_algorithms.first() {
            let challenge_value = format!(
                "Digest realm=\"{}\", nonce=\"{}\", algorithm={}, qop=\"auth\"",
                realm, nonce, algo.as_str()
            );
            response.headers.push(SipHeader {
                name: header_name.to_string(),
                value: challenge_value,
            });
        }

        response
    }
}

impl Default for InboundAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Outbound authenticator (UAC side)
// ---------------------------------------------------------------------------

/// The outbound authenticator responds to 401/407 challenges from
/// remote servers.
#[derive(Debug)]
pub struct OutboundAuthenticator;

impl OutboundAuthenticator {
    /// Given a 401/407 challenge response from a remote server and our
    /// credentials, produce a new request with an Authorization header.
    ///
    /// Returns `None` if no matching credentials are found or the challenge
    /// cannot be parsed.
    pub fn create_authenticated_request(
        original_request: &SipMessage,
        challenge_response: &SipMessage,
        credentials: &[AuthCredentials],
    ) -> Option<SipMessage> {
        let status = challenge_response.status_code()?;

        let auth_header_name = match status {
            401 => header_names::WWW_AUTHENTICATE,
            407 => header_names::PROXY_AUTHENTICATE,
            _ => return None,
        };

        let response_header_name = match status {
            401 => header_names::AUTHORIZATION,
            407 => header_names::PROXY_AUTHORIZATION,
            _ => return None,
        };

        // Parse the challenge.
        let challenge_hdr = challenge_response.get_header(auth_header_name)?;
        let challenge = DigestChallenge::parse(challenge_hdr)?;

        // Find matching credentials.
        let cred = credentials.iter().find(|c| {
            c.realm.is_empty()
                || c.realm == "*"
                || c.realm == challenge.realm
        })?;

        let method = original_request
            .method()
            .map(|m| m.as_str())
            .unwrap_or("REGISTER");

        // Determine the request URI.
        let uri = match &original_request.start_line {
            StartLine::Request(r) => r.uri.to_string(),
            _ => return None,
        };

        let digest_creds = DigestCredentials {
            username: cred.username.clone(),
            password: cred.password.clone(),
            realm: challenge.realm.clone(),
        };

        let auth_value = create_digest_response(&challenge, &digest_creds, method, &uri);

        // Clone the original request and add the Authorization header.
        let mut new_request = original_request.clone();

        // Increment CSeq.
        for h in &mut new_request.headers {
            if h.name.eq_ignore_ascii_case(header_names::CSEQ) {
                if let Some((num_str, method_str)) = h.value.split_once(' ') {
                    if let Ok(num) = num_str.trim().parse::<u32>() {
                        h.value = format!("{} {}", num + 1, method_str);
                    }
                }
            }
        }

        // Add (or replace) the Authorization header.
        new_request
            .headers
            .retain(|h| !h.name.eq_ignore_ascii_case(response_header_name));
        new_request.headers.push(SipHeader {
            name: response_header_name.to_string(),
            value: auth_value,
        });

        Some(new_request)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parsed Authorization header fields.
#[derive(Debug)]
pub struct ParsedAuth {
    pub username: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub algorithm: DigestAlgorithm,
    pub qop: Option<String>,
    pub opaque: Option<String>,
}

/// Parse an Authorization or Proxy-Authorization header value.
pub fn parse_authorization(value: &str) -> Option<ParsedAuth> {
    let value = value.trim();
    let rest = value.strip_prefix("Digest")?.trim();

    let mut username = String::new();
    let mut realm = String::new();
    let mut nonce = String::new();
    let mut uri = String::new();
    let mut response = String::new();
    let mut algorithm = DigestAlgorithm::Md5;
    let mut qop = None;
    let mut opaque = None;

    for param in split_params(rest) {
        let (key, val) = match param.split_once('=') {
            Some((k, v)) => (k.trim().to_lowercase(), unquote(v.trim())),
            None => continue,
        };

        match key.as_str() {
            "username" => username = val,
            "realm" => realm = val,
            "nonce" => nonce = val,
            "uri" => uri = val,
            "response" => response = val,
            "algorithm" => algorithm = DigestAlgorithm::from_name(&val),
            "qop" => qop = Some(val),
            "opaque" => opaque = Some(val),
            _ => {}
        }
    }

    if username.is_empty() || nonce.is_empty() || response.is_empty() {
        return None;
    }

    Some(ParsedAuth {
        username,
        realm,
        nonce,
        uri,
        response,
        algorithm,
        qop,
        opaque,
    })
}

fn split_params(input: &str) -> Vec<String> {
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
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        params.push(trimmed);
    }

    params
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Extract the `response="..."` value from a complete Digest auth header value.
fn extract_response_value(header_value: &str) -> String {
    for param in split_params(header_value.strip_prefix("Digest").unwrap_or(header_value)) {
        if let Some((key, val)) = param.split_once('=') {
            if key.trim().eq_ignore_ascii_case("response") {
                return unquote(val.trim());
            }
        }
    }
    String::new()
}

/// Generate a random nonce for authentication challenges.
fn generate_nonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inbound_auth_no_credentials() {
        let auth = InboundAuthenticator::new();
        assert!(!auth.requires_auth(&[]));
    }

    #[test]
    fn test_inbound_auth_challenge() {
        let auth = InboundAuthenticator::new();
        let creds = vec![AuthCredentials::new("alice", "secret", "asterisk")];

        let request = SipMessage::parse(
            b"REGISTER sip:registrar.example.com SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>\r\n\
              Call-ID: auth-test-123\r\n\
              CSeq: 1 REGISTER\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let result = auth.verify(&request, &creds, false);
        assert!(result.is_err());
        let challenge = result.unwrap_err();
        assert_eq!(challenge.status_code(), Some(401));
        assert!(challenge
            .get_header(header_names::WWW_AUTHENTICATE)
            .is_some());
    }

    #[test]
    fn test_parse_authorization() {
        let value = r#"Digest username="alice", realm="asterisk", nonce="abc123", uri="sip:registrar.example.com", response="deadbeef", algorithm=MD5"#;
        let parsed = parse_authorization(value).unwrap();
        assert_eq!(parsed.username, "alice");
        assert_eq!(parsed.realm, "asterisk");
        assert_eq!(parsed.nonce, "abc123");
        assert_eq!(parsed.response, "deadbeef");
        assert_eq!(parsed.algorithm, DigestAlgorithm::Md5);
    }

    #[test]
    fn test_outbound_auth() {
        let request = SipMessage::parse(
            b"REGISTER sip:registrar.example.com SIP/2.0\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>\r\n\
              Call-ID: outauth-test\r\n\
              CSeq: 1 REGISTER\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let challenge_response = SipMessage::parse(
            b"SIP/2.0 401 Unauthorized\r\n\
              Via: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK123\r\n\
              From: <sip:alice@example.com>;tag=abc\r\n\
              To: <sip:alice@example.com>;tag=def\r\n\
              Call-ID: outauth-test\r\n\
              CSeq: 1 REGISTER\r\n\
              WWW-Authenticate: Digest realm=\"asterisk\", nonce=\"testnonce123\", algorithm=MD5, qop=\"auth\"\r\n\
              Content-Length: 0\r\n\
              \r\n",
        )
        .unwrap();

        let creds = vec![AuthCredentials::new("alice", "secret", "asterisk")];

        let result =
            OutboundAuthenticator::create_authenticated_request(&request, &challenge_response, &creds);

        assert!(result.is_some());
        let authed_request = result.unwrap();
        assert!(authed_request
            .get_header(header_names::AUTHORIZATION)
            .is_some());
        // CSeq should be incremented.
        let cseq = authed_request.cseq().unwrap();
        assert!(cseq.starts_with("2 "));
    }
}
