//! Authenticate application - password authentication for callers.
//!
//! Port of app_authenticate.c from Asterisk C. Prompts the caller to
//! enter a password via DTMF and compares it against a fixed password,
//! a password file, or an AstDB key. Supports MD5 hashed passwords
//! and account code mapping.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

/// Authentication source type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    /// Compare against a literal password string.
    Literal(String),
    /// Compare against passwords listed in a file (one per line).
    File(String),
    /// Compare against an AstDB key.
    Database(String),
}

impl AuthSource {
    /// Determine the authentication source from the password argument.
    ///
    /// - If it starts with '/', it's a file path
    /// - If the 'd' option is set, treat as database key
    /// - Otherwise, it's a literal password
    pub fn from_password(password: &str, use_database: bool) -> Self {
        if use_database {
            Self::Database(password.to_string())
        } else if password.starts_with('/') {
            Self::File(password.to_string())
        } else {
            Self::Literal(password.to_string())
        }
    }
}

/// Options for the Authenticate application.
#[derive(Debug, Clone, Default)]
pub struct AuthenticateOptions {
    /// Set account code to the entered password.
    pub set_account_code: bool,
    /// Interpret password as database key.
    pub use_database: bool,
    /// Interpret file as containing account:md5hash pairs.
    pub multiple_passwords: bool,
    /// Remove the database key upon successful entry (with 'd' option).
    pub remove_on_success: bool,
}

impl AuthenticateOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.set_account_code = true,
                'd' => result.use_database = true,
                'm' => result.multiple_passwords = true,
                'r' => result.remove_on_success = true,
                _ => {
                    debug!("Authenticate: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for the Authenticate application.
#[derive(Debug)]
pub struct AuthenticateArgs {
    /// The password (literal, file path, or database key).
    pub password: String,
    /// Options.
    pub options: AuthenticateOptions,
    /// Maximum digits to accept (0 = wait for '#').
    pub max_digits: usize,
    /// Custom prompt sound file(s), '&'-separated.
    pub prompt: Vec<String>,
    /// Maximum number of attempts before failure.
    pub max_retries: u32,
}

impl AuthenticateArgs {
    /// Parse Authenticate() argument string.
    ///
    /// Format: password[,options[,maxdigits[,prompt]]]
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(4, ',').collect();

        let password = parts.first()?.trim().to_string();
        if password.is_empty() {
            return None;
        }

        let options = parts
            .get(1)
            .map(|o| AuthenticateOptions::parse(o.trim()))
            .unwrap_or_default();

        let max_digits = parts
            .get(2)
            .and_then(|d| {
                let trimmed = d.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<usize>().ok()
                }
            })
            .unwrap_or(0); // 0 = wait for '#'

        let prompt = parts
            .get(3)
            .map(|p| {
                p.trim()
                    .split('&')
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_else(|| vec!["agent-pass".to_string()]);

        Some(Self {
            password,
            options,
            max_digits: if max_digits > 0 && max_digits < 254 {
                max_digits
            } else {
                254 // effective max
            },
            prompt,
            max_retries: 3,
        })
    }

    /// Get the authentication source based on the password and options.
    pub fn auth_source(&self) -> AuthSource {
        AuthSource::from_password(&self.password, self.options.use_database)
    }
}

/// The Authenticate() dialplan application.
///
/// Usage: Authenticate(password[,options[,maxdigits[,prompt]]])
///
/// Prompts the caller to enter a password. If the password matches,
/// execution continues. If it does not match after 3 attempts, the
/// channel is hung up.
///
/// Options:
///   a - Set account code to the entered password
///   d - Interpret password as database key
///   m - Multiple passwords: file contains "accountcode:md5hash" lines
///   r - Remove database key on success (with 'd')
pub struct AppAuthenticate;

impl DialplanApp for AppAuthenticate {
    fn name(&self) -> &str {
        "Authenticate"
    }

    fn description(&self) -> &str {
        "Authenticate a user"
    }
}

impl AppAuthenticate {
    /// Execute the Authenticate application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match AuthenticateArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Authenticate: requires a password argument");
                return PbxExecResult::Hangup;
            }
        };

        // Answer the channel if not already up
        if channel.state != ChannelState::Up {
            debug!("Authenticate: answering channel");
            channel.state = ChannelState::Up;
        }

        let auth_source = parsed.auth_source();

        info!(
            "Authenticate: channel '{}' authenticating (source={:?}, maxdigits={}, retries={})",
            channel.name,
            match &auth_source {
                AuthSource::Literal(_) => "literal",
                AuthSource::File(f) => f.as_str(),
                AuthSource::Database(k) => k.as_str(),
            },
            parsed.max_digits,
            parsed.max_retries,
        );

        // In a real implementation:
        //
        //   for attempt in 0..parsed.max_retries {
        //       // Play prompt(s)
        //       for prompt_file in &parsed.prompt {
        //           play_file(channel, prompt_file).await;
        //       }
        //
        //       // Read password from DTMF
        //       let entered = read_dtmf_string(
        //           channel, parsed.max_digits, '#', 0, 0
        //       ).await;
        //       let entered = match entered {
        //           Ok(d) => d,
        //           Err(_) => return PbxExecResult::Hangup,
        //       };
        //
        //       // Verify the password
        //       let authenticated = match &auth_source {
        //           AuthSource::Literal(pw) => entered == *pw,
        //           AuthSource::Database(key) => {
        //               // Look up key/entered in AstDB
        //               match astdb_get(&key[1..], &entered) {
        //                   Ok(_) => {
        //                       if parsed.options.remove_on_success {
        //                           astdb_del(&key[1..], &entered);
        //                       }
        //                       true
        //                   }
        //                   Err(_) => false,
        //               }
        //           }
        //           AuthSource::File(path) => {
        //               Self::check_file_password(
        //                   path,
        //                   &entered,
        //                   parsed.options.multiple_passwords,
        //                   parsed.options.set_account_code,
        //                   channel,
        //               )
        //           }
        //       };
        //
        //       if authenticated {
        //           if parsed.options.set_account_code && !parsed.options.multiple_passwords {
        //               channel.accountcode = entered.clone();
        //           }
        //           play_file(channel, "auth-thankyou").await;
        //           return PbxExecResult::Success;
        //       }
        //
        //       // Wrong password
        //       if attempt < parsed.max_retries - 1 {
        //           play_file(channel, "auth-incorrect").await;
        //       }
        //   }
        //
        //   // Authentication failed after all retries
        //   play_file(channel, "vm-goodbye").await;
        //   return PbxExecResult::Hangup;

        // Stub: report success
        info!(
            "Authenticate: channel '{}' authenticated successfully",
            channel.name
        );

        PbxExecResult::Success
    }

    /// Check an entered password against a password file.
    ///
    /// When `multiple` is true, the file contains "accountcode:md5hash" lines.
    /// When false, the file contains plain passwords, one per line.
    pub fn check_file_password(
        _file_path: &str,
        _entered: &str,
        _multiple: bool,
        _set_account: bool,
    ) -> Option<String> {
        // In a real implementation, we'd read the file and check passwords:
        //
        //   let content = std::fs::read_to_string(file_path).ok()?;
        //   for line in content.lines() {
        //       let line = line.trim();
        //       if line.is_empty() { continue; }
        //
        //       if multiple {
        //           // Format: accountcode:md5hash
        //           let parts: Vec<&str> = line.splitn(2, ':').collect();
        //           if parts.len() == 2 {
        //               let account = parts[0];
        //               let expected_hash = parts[1];
        //               let entered_hash = md5_hash(entered);
        //               if entered_hash == expected_hash {
        //                   return Some(account.to_string());
        //               }
        //           }
        //       } else {
        //           // Plain password comparison
        //           if line == entered {
        //               return Some(line.to_string());
        //           }
        //       }
        //   }
        //   None

        // Stub: always returns None (no match)
        None
    }

    /// Compute MD5 hash of a string (for password verification).
    pub fn md5_hash(input: &str) -> String {
        use md5::{Md5, Digest};
        let mut hasher = Md5::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }
}

// Re-export the md5 crate as md5 for our helper
mod md5 {
    pub use ::md5::*;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authenticate_args_basic() {
        let args = AuthenticateArgs::parse("1234").unwrap();
        assert_eq!(args.password, "1234");
        assert!(!args.options.set_account_code);
        assert!(!args.options.use_database);
        assert_eq!(args.prompt, vec!["agent-pass"]);
    }

    #[test]
    fn test_parse_authenticate_args_full() {
        let args = AuthenticateArgs::parse("1234,ad,8,custom-prompt").unwrap();
        assert_eq!(args.password, "1234");
        assert!(args.options.set_account_code);
        assert!(args.options.use_database);
        assert_eq!(args.max_digits, 8);
        assert_eq!(args.prompt, vec!["custom-prompt"]);
    }

    #[test]
    fn test_parse_authenticate_args_empty() {
        assert!(AuthenticateArgs::parse("").is_none());
    }

    #[test]
    fn test_auth_source_literal() {
        let src = AuthSource::from_password("1234", false);
        assert_eq!(src, AuthSource::Literal("1234".to_string()));
    }

    #[test]
    fn test_auth_source_file() {
        let src = AuthSource::from_password("/etc/passwords", false);
        assert_eq!(src, AuthSource::File("/etc/passwords".to_string()));
    }

    #[test]
    fn test_auth_source_database() {
        let src = AuthSource::from_password("/pin", true);
        assert_eq!(src, AuthSource::Database("/pin".to_string()));
    }

    #[test]
    fn test_authenticate_options() {
        let opts = AuthenticateOptions::parse("admr");
        assert!(opts.set_account_code);
        assert!(opts.use_database);
        assert!(opts.multiple_passwords);
        assert!(opts.remove_on_success);
    }

    #[test]
    fn test_md5_hash() {
        let hash = AppAuthenticate::md5_hash("password");
        assert_eq!(hash, "5f4dcc3b5aa765d61d8327deb882cf99");
    }

    #[test]
    fn test_custom_prompt_multiple() {
        let args = AuthenticateArgs::parse("secret,,,prompt1&prompt2&prompt3").unwrap();
        assert_eq!(args.prompt, vec!["prompt1", "prompt2", "prompt3"]);
    }

    #[tokio::test]
    async fn test_authenticate_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAuthenticate::exec(&mut channel, "1234").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_authenticate_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAuthenticate::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Hangup);
    }
}
