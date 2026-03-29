//! AMI authentication - user management and credential verification.
//!
//! Supports two authentication modes:
//! - Plaintext: Username + Secret (password sent in clear text)
//! - MD5 Challenge: Server sends a random challenge, client responds
//!   with MD5(challenge + secret)

use crate::events::EventCategory;
use parking_lot::RwLock;
use std::collections::HashMap;
use tracing::debug;

/// An AMI user, as configured in manager.conf.
#[derive(Debug, Clone)]
pub struct AmiUser {
    /// The username for login.
    pub username: String,
    /// The plaintext secret (password).
    pub secret: String,
    /// Read permission bitmask (which event categories the user can receive).
    pub read_perm: EventCategory,
    /// Write permission bitmask (which action categories the user can execute).
    pub write_perm: EventCategory,
    /// Whether to display connection/disconnection messages.
    pub display_connects: bool,
    /// Whether to allow multiple simultaneous logins.
    pub allow_multiple_login: bool,
}

impl AmiUser {
    /// Create a new AMI user with full permissions.
    pub fn new(username: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            secret: secret.into(),
            read_perm: EventCategory::ALL,
            write_perm: EventCategory::ALL,
            display_connects: true,
            allow_multiple_login: true,
        }
    }

    /// Create a user with specific read/write permissions.
    pub fn with_permissions(
        username: impl Into<String>,
        secret: impl Into<String>,
        read_perm: EventCategory,
        write_perm: EventCategory,
    ) -> Self {
        Self {
            username: username.into(),
            secret: secret.into(),
            read_perm,
            write_perm,
            display_connects: true,
            allow_multiple_login: true,
        }
    }

    /// Check if this user has read permission for a given event category.
    pub fn can_read(&self, category: EventCategory) -> bool {
        self.read_perm.contains(category)
    }

    /// Check if this user has write permission for a given action category.
    pub fn can_write(&self, category: EventCategory) -> bool {
        self.write_perm.contains(category)
    }
}

/// Registry of configured AMI users.
#[derive(Debug)]
pub struct UserRegistry {
    users: RwLock<HashMap<String, AmiUser>>,
}

impl UserRegistry {
    /// Create a new empty user registry.
    pub fn new() -> Self {
        Self {
            users: RwLock::new(HashMap::new()),
        }
    }

    /// Add a user to the registry.
    pub fn add_user(&self, user: AmiUser) {
        debug!("AMI UserRegistry: adding user '{}'", user.username);
        self.users.write().insert(user.username.clone(), user);
    }

    /// Remove a user from the registry.
    pub fn remove_user(&self, username: &str) -> bool {
        self.users.write().remove(username).is_some()
    }

    /// Look up a user by username.
    pub fn find_user(&self, username: &str) -> Option<AmiUser> {
        self.users.read().get(username).cloned()
    }

    /// Get the number of configured users.
    pub fn count(&self) -> usize {
        self.users.read().len()
    }

    /// List all usernames.
    pub fn list_users(&self) -> Vec<String> {
        self.users.read().keys().cloned().collect()
    }
}

impl Default for UserRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Authentication methods supported by AMI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// Plaintext password authentication.
    Plaintext,
    /// MD5 challenge-response authentication.
    Md5Challenge,
}

/// Verify plaintext credentials against a user.
pub fn verify_plaintext(user: &AmiUser, secret: &str) -> bool {
    user.secret == secret
}

/// Generate a random challenge string for MD5 authentication.
pub fn generate_challenge() -> String {
    use uuid::Uuid;
    // Use a UUID-based random string as the challenge, similar to Asterisk's
    // random challenge generation.
    let id = Uuid::new_v4();
    format!("{}", id.as_simple())
}

/// Verify an MD5 challenge response.
///
/// The expected response is MD5(challenge + secret).
pub fn verify_md5_response(challenge: &str, secret: &str, response: &str) -> bool {
    let expected = compute_md5_response(challenge, secret);
    expected.eq_ignore_ascii_case(response)
}

/// Compute the expected MD5 response for a challenge.
///
/// Result = MD5(challenge + secret), hex-encoded.
pub fn compute_md5_response(challenge: &str, secret: &str) -> String {
    use md5::{Md5, Digest};
    let mut hasher = Md5::new();
    hasher.update(challenge.as_bytes());
    hasher.update(secret.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

// Re-export md5 crate
mod md5 {
    pub use ::md5::*;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_creation() {
        let user = AmiUser::new("admin", "secret123");
        assert_eq!(user.username, "admin");
        assert_eq!(user.secret, "secret123");
        assert!(user.can_read(EventCategory::CALL));
        assert!(user.can_write(EventCategory::SYSTEM));
    }

    #[test]
    fn test_user_permissions() {
        let user = AmiUser::with_permissions(
            "readonly",
            "pass",
            EventCategory::CALL.union(EventCategory::SYSTEM),
            EventCategory::NONE,
        );
        assert!(user.can_read(EventCategory::CALL));
        assert!(user.can_read(EventCategory::SYSTEM));
        assert!(!user.can_read(EventCategory::DTMF));
        assert!(!user.can_write(EventCategory::CALL));
    }

    #[test]
    fn test_user_registry() {
        let registry = UserRegistry::new();
        registry.add_user(AmiUser::new("admin", "pass1"));
        registry.add_user(AmiUser::new("monitor", "pass2"));

        assert_eq!(registry.count(), 2);
        assert!(registry.find_user("admin").is_some());
        assert!(registry.find_user("admin").unwrap().secret == "pass1");
        assert!(registry.find_user("nobody").is_none());

        assert!(registry.remove_user("admin"));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_verify_plaintext() {
        let user = AmiUser::new("admin", "correct_password");
        assert!(verify_plaintext(&user, "correct_password"));
        assert!(!verify_plaintext(&user, "wrong_password"));
    }

    #[test]
    fn test_challenge_generation() {
        let c1 = generate_challenge();
        let c2 = generate_challenge();
        assert_ne!(c1, c2); // Should be random
        assert!(!c1.is_empty());
    }

    #[test]
    fn test_md5_challenge_response() {
        let challenge = "test_challenge_12345";
        let secret = "mysecret";

        let response = compute_md5_response(challenge, secret);
        assert!(verify_md5_response(challenge, secret, &response));
        assert!(!verify_md5_response(challenge, "wrongsecret", &response));
    }

    #[test]
    fn test_md5_response_case_insensitive() {
        let challenge = "abc";
        let secret = "def";
        let response = compute_md5_response(challenge, secret);
        let upper = response.to_uppercase();
        assert!(verify_md5_response(challenge, secret, &upper));
    }
}
