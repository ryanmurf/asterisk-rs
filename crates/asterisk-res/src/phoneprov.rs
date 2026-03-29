//! Phone provisioning.
//!
//! Port of `res/res_phoneprov.c`. Provides automatic provisioning for
//! IP phones by serving configuration files over HTTP with template
//! variable expansion.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::info;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum PhoneProvError {
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("user not found (MAC: {0})")]
    UserNotFound(String),
    #[error("template error: {0}")]
    TemplateError(String),
    #[error("phoneprov error: {0}")]
    Other(String),
}

pub type PhoneProvResult<T> = Result<T, PhoneProvError>;

// ---------------------------------------------------------------------------
// Phone profile
// ---------------------------------------------------------------------------

/// A phone provisioning profile.
///
/// Defines the set of files (templates) served for a particular phone
/// model, including static files (firmware, backgrounds) and dynamic
/// files (phone-specific configuration).
#[derive(Debug, Clone)]
pub struct PhoneProfile {
    /// Profile name (e.g., "polycom", "grandstream").
    pub name: String,
    /// Static files: (request_path, local_file_path).
    pub static_files: Vec<(String, String)>,
    /// Dynamic (templated) files: (request_path, template_content).
    pub dynamic_files: Vec<(String, String)>,
    /// Default MIME type for served files.
    pub mime_type: String,
    /// Default variable values for this profile.
    pub default_vars: HashMap<String, String>,
}

impl PhoneProfile {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            static_files: Vec::new(),
            dynamic_files: Vec::new(),
            mime_type: "text/plain".to_string(),
            default_vars: HashMap::new(),
        }
    }

    /// Add a static file mapping.
    pub fn add_static_file(&mut self, request_path: &str, local_path: &str) {
        self.static_files
            .push((request_path.to_string(), local_path.to_string()));
    }

    /// Add a dynamic (templated) file.
    pub fn add_dynamic_file(&mut self, request_path: &str, template: &str) {
        self.dynamic_files
            .push((request_path.to_string(), template.to_string()));
    }

    /// Set a default variable value.
    pub fn set_default_var(&mut self, key: &str, value: &str) {
        self.default_vars.insert(key.to_string(), value.to_string());
    }
}

// ---------------------------------------------------------------------------
// Phone user
// ---------------------------------------------------------------------------

/// A provisioned phone user.
///
/// Associates a MAC address with a profile and per-user variable values.
#[derive(Debug, Clone)]
pub struct PhoneUser {
    /// MAC address (normalized to lowercase, no separators).
    pub mac_address: String,
    /// Profile name.
    pub profile: String,
    /// Per-user variable overrides.
    pub variables: HashMap<String, String>,
}

impl PhoneUser {
    /// Create a new phone user.
    pub fn new(mac_address: &str, profile: &str) -> Self {
        Self {
            mac_address: normalize_mac(mac_address),
            profile: profile.to_string(),
            variables: HashMap::new(),
        }
    }

    /// Set a variable for this user.
    pub fn set_variable(&mut self, key: &str, value: &str) {
        self.variables.insert(key.to_string(), value.to_string());
    }
}

// ---------------------------------------------------------------------------
// Template variable expansion
// ---------------------------------------------------------------------------

/// Well-known template variables.
pub mod template_vars {
    pub const MAC: &str = "MAC";
    pub const USERNAME: &str = "USERNAME";
    pub const DISPLAY_NAME: &str = "DISPLAY_NAME";
    pub const SECRET: &str = "SECRET";
    pub const LABEL: &str = "LABEL";
    pub const CALLERID: &str = "CALLERID";
    pub const TIMEZONE: &str = "TIMEZONE";
    pub const SERVER_IP: &str = "SERVER_IP";
    pub const SERVER_PORT: &str = "SERVER_PORT";
    pub const PROFILE: &str = "PROFILE";
}

/// Expand template variables in the given text.
///
/// Replaces `${VAR_NAME}` with the corresponding value from the variables
/// map. Unresolved variables are left as-is and a warning is logged.
pub fn expand_template(template: &str, variables: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();

    while let Some((_i, ch)) = chars.next() {
        if ch == '$' {
            if let Some(&(_, '{')) = chars.peek() {
                chars.next(); // consume '{'
                let start = if let Some(&(j, _)) = chars.peek() {
                    j
                } else {
                    result.push_str("${");
                    continue;
                };
                // Find closing '}'.
                let mut end = start;
                let mut found_close = false;
                while let Some(&(j, c)) = chars.peek() {
                    chars.next();
                    if c == '}' {
                        end = j;
                        found_close = true;
                        break;
                    }
                }
                if found_close {
                    let var_name = &template[start..end];
                    if let Some(value) = variables.get(var_name) {
                        result.push_str(value);
                    } else {
                        // Leave unresolved variables as-is.
                        result.push_str("${");
                        result.push_str(var_name);
                        result.push('}');
                    }
                } else {
                    result.push_str("${");
                    result.push_str(&template[start..]);
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Provisioning manager
// ---------------------------------------------------------------------------

/// Manages phone profiles and users for provisioning.
pub struct PhoneProvManager {
    /// Registered profiles keyed by name.
    profiles: RwLock<HashMap<String, PhoneProfile>>,
    /// Registered users keyed by normalized MAC address.
    users: RwLock<HashMap<String, PhoneUser>>,
    /// Global variables available to all templates.
    global_vars: RwLock<HashMap<String, String>>,
}

impl PhoneProvManager {
    pub fn new() -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
            users: RwLock::new(HashMap::new()),
            global_vars: RwLock::new(HashMap::new()),
        }
    }

    /// Register a phone profile.
    pub fn register_profile(&self, profile: PhoneProfile) {
        info!(name = %profile.name, "Phone profile registered");
        self.profiles.write().insert(profile.name.clone(), profile);
    }

    /// Register a phone user.
    pub fn register_user(&self, user: PhoneUser) {
        info!(mac = %user.mac_address, profile = %user.profile, "Phone user registered");
        self.users.write().insert(user.mac_address.clone(), user);
    }

    /// Set a global variable.
    pub fn set_global_var(&self, key: &str, value: &str) {
        self.global_vars
            .write()
            .insert(key.to_string(), value.to_string());
    }

    /// Look up a user by MAC address and render the requested file.
    pub fn render_file(
        &self,
        mac_address: &str,
        request_path: &str,
    ) -> PhoneProvResult<String> {
        let mac = normalize_mac(mac_address);
        let users = self.users.read();
        let user = users
            .get(&mac)
            .ok_or_else(|| PhoneProvError::UserNotFound(mac.clone()))?;

        let profiles = self.profiles.read();
        let profile = profiles
            .get(&user.profile)
            .ok_or_else(|| PhoneProvError::ProfileNotFound(user.profile.clone()))?;

        // Find matching dynamic template.
        let template = profile
            .dynamic_files
            .iter()
            .find(|(path, _)| {
                // Expand MAC in the request path pattern.
                let expanded_path =
                    path.replace("${MAC}", &mac).replace("${mac}", &mac);
                expanded_path == request_path
            })
            .map(|(_, tmpl)| tmpl.as_str())
            .ok_or_else(|| {
                PhoneProvError::TemplateError(format!(
                    "No template for path: {}",
                    request_path
                ))
            })?;

        // Build variable map: global -> profile defaults -> user vars.
        let mut vars = self.global_vars.read().clone();
        for (k, v) in &profile.default_vars {
            vars.insert(k.clone(), v.clone());
        }
        for (k, v) in &user.variables {
            vars.insert(k.clone(), v.clone());
        }
        // Auto-populate MAC.
        vars.insert(template_vars::MAC.to_string(), mac.clone());
        vars.insert(template_vars::PROFILE.to_string(), user.profile.clone());

        Ok(expand_template(template, &vars))
    }

    /// List all registered MAC addresses.
    pub fn user_macs(&self) -> Vec<String> {
        self.users.read().keys().cloned().collect()
    }

    /// List all profile names.
    pub fn profile_names(&self) -> Vec<String> {
        self.profiles.read().keys().cloned().collect()
    }
}

impl Default for PhoneProvManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PhoneProvManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhoneProvManager")
            .field("profiles", &self.profiles.read().len())
            .field("users", &self.users.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize a MAC address to lowercase hex without separators.
pub fn normalize_mac(mac: &str) -> String {
    mac.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_mac() {
        assert_eq!(normalize_mac("AA:BB:CC:DD:EE:FF"), "aabbccddeeff");
        assert_eq!(normalize_mac("aa-bb-cc-dd-ee-ff"), "aabbccddeeff");
        assert_eq!(normalize_mac("AABBCCDDEEFF"), "aabbccddeeff");
    }

    #[test]
    fn test_expand_template_basic() {
        let mut vars = HashMap::new();
        vars.insert("USERNAME".to_string(), "alice".to_string());
        vars.insert("SECRET".to_string(), "s3cret".to_string());

        let template = "username=${USERNAME}\npassword=${SECRET}\n";
        let result = expand_template(template, &vars);
        assert_eq!(result, "username=alice\npassword=s3cret\n");
    }

    #[test]
    fn test_expand_template_unresolved() {
        let vars = HashMap::new();
        let template = "value=${MISSING}";
        let result = expand_template(template, &vars);
        assert_eq!(result, "value=${MISSING}");
    }

    #[test]
    fn test_phone_user() {
        let mut user = PhoneUser::new("AA:BB:CC:DD:EE:FF", "polycom");
        user.set_variable("USERNAME", "1001");
        assert_eq!(user.mac_address, "aabbccddeeff");
        assert_eq!(user.variables.get("USERNAME"), Some(&"1001".to_string()));
    }

    #[test]
    fn test_render_file() {
        let mgr = PhoneProvManager::new();

        let mut profile = PhoneProfile::new("test");
        profile.add_dynamic_file(
            "${MAC}.cfg",
            "reg.1.address=${SERVER_IP}\nreg.1.auth.userId=${USERNAME}\n",
        );
        profile.set_default_var("SERVER_IP", "10.0.0.1");
        mgr.register_profile(profile);

        let mut user = PhoneUser::new("00:11:22:33:44:55", "test");
        user.set_variable("USERNAME", "1001");
        mgr.register_user(user);

        let result = mgr.render_file("00:11:22:33:44:55", "001122334455.cfg");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("reg.1.address=10.0.0.1"));
        assert!(content.contains("reg.1.auth.userId=1001"));
    }

    #[test]
    fn test_render_file_user_not_found() {
        let mgr = PhoneProvManager::new();
        let result = mgr.render_file("FF:FF:FF:FF:FF:FF", "file.cfg");
        assert!(matches!(result, Err(PhoneProvError::UserNotFound(_))));
    }
}
