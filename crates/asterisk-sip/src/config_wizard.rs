//! PJSIP simplified configuration wizard.
//!
//! Port of `res/res_pjsip_config_wizard.c`. Provides a simplified
//! configuration interface that auto-generates the individual PJSIP
//! sorcery objects (endpoint, aor, auth, identify, registration)
//! from a single wizard configuration section.

use std::collections::HashMap;

use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Wizard object types
// ---------------------------------------------------------------------------

/// The set of PJSIP objects that a wizard section can generate.
#[derive(Debug, Clone)]
pub struct WizardObjects {
    /// Endpoint configuration.
    pub endpoint: HashMap<String, String>,
    /// AOR (Address of Record) configuration.
    pub aor: HashMap<String, String>,
    /// Auth configuration (if credentials are provided).
    pub auth: Option<HashMap<String, String>>,
    /// Identify (IP-based endpoint identification) configuration.
    pub identify: Option<HashMap<String, String>>,
    /// Outbound registration configuration.
    pub registration: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Wizard configuration
// ---------------------------------------------------------------------------

/// A parsed wizard configuration section.
///
/// The wizard section is a simplified format where a single `[endpoint_name]`
/// section with `type=wizard` generates all required PJSIP objects.
#[derive(Debug, Clone)]
pub struct WizardConfig {
    /// Section/endpoint name.
    pub name: String,
    /// Raw key-value pairs from the configuration section.
    pub fields: HashMap<String, String>,
}

impl WizardConfig {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            fields: HashMap::new(),
        }
    }

    /// Set a configuration field.
    pub fn set(&mut self, key: &str, value: &str) {
        self.fields.insert(key.to_string(), value.to_string());
    }

    /// Get a configuration field.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }

    /// Whether this wizard has remote registration configured.
    pub fn has_registration(&self) -> bool {
        self.fields.contains_key("remote_hosts")
            || self.fields.contains_key("server_uri")
    }

    /// Whether this wizard has IP-based identification.
    pub fn has_identify(&self) -> bool {
        self.fields.contains_key("match")
            || self.fields.contains_key("remote_hosts")
    }

    /// Whether this wizard has authentication credentials.
    pub fn has_auth(&self) -> bool {
        self.fields.contains_key("inbound_auth/username")
            || self.fields.contains_key("outbound_auth/username")
    }

    /// Expand the wizard into individual PJSIP sorcery objects.
    ///
    /// This is the core of the wizard: a single section is expanded
    /// into the full set of objects needed for a working PJSIP endpoint.
    pub fn expand(&self) -> WizardObjects {
        let mut endpoint = HashMap::new();
        let mut aor = HashMap::new();

        // AOR defaults
        aor.insert("max_contacts".to_string(),
            self.get("max_contacts").unwrap_or("1").to_string());
        if let Some(v) = self.get("contact") {
            aor.insert("contact".to_string(), v.to_string());
        }

        // Endpoint defaults
        endpoint.insert("aors".to_string(), self.name.clone());
        if let Some(v) = self.get("context") {
            endpoint.insert("context".to_string(), v.to_string());
        }
        if let Some(v) = self.get("transport") {
            endpoint.insert("transport".to_string(), v.to_string());
        }
        if let Some(v) = self.get("allow") {
            endpoint.insert("allow".to_string(), v.to_string());
        }
        if let Some(v) = self.get("disallow") {
            endpoint.insert("disallow".to_string(), v.to_string());
        }
        if let Some(v) = self.get("dtmf_mode") {
            endpoint.insert("dtmf_mode".to_string(), v.to_string());
        }

        // Auth
        let auth = if self.has_auth() {
            let mut auth_fields = HashMap::new();
            if let Some(v) = self.get("inbound_auth/username") {
                auth_fields.insert("username".to_string(), v.to_string());
                endpoint.insert("auth".to_string(), self.name.clone());
            }
            if let Some(v) = self.get("inbound_auth/password") {
                auth_fields.insert("password".to_string(), v.to_string());
            }
            auth_fields.insert("auth_type".to_string(), "userpass".to_string());
            Some(auth_fields)
        } else {
            None
        };

        // Identify
        let identify = if self.has_identify() {
            let mut id_fields = HashMap::new();
            id_fields.insert("endpoint".to_string(), self.name.clone());
            if let Some(v) = self.get("match") {
                id_fields.insert("match".to_string(), v.to_string());
            }
            Some(id_fields)
        } else {
            None
        };

        // Registration
        let registration = if self.has_registration() {
            let mut reg_fields = HashMap::new();
            if let Some(v) = self.get("server_uri") {
                reg_fields.insert("server_uri".to_string(), v.to_string());
            }
            if let Some(v) = self.get("client_uri") {
                reg_fields.insert("client_uri".to_string(), v.to_string());
            }
            Some(reg_fields)
        } else {
            None
        };

        info!(
            wizard = %self.name,
            has_auth = auth.is_some(),
            has_identify = identify.is_some(),
            has_registration = registration.is_some(),
            "Expanded PJSIP config wizard"
        );

        WizardObjects {
            endpoint,
            aor,
            auth,
            identify,
            registration,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_wizard() {
        let mut wiz = WizardConfig::new("alice");
        wiz.set("context", "default");
        wiz.set("transport", "udp");
        wiz.set("allow", "!all,ulaw,alaw");

        let objects = wiz.expand();
        assert_eq!(objects.endpoint.get("aors").unwrap(), "alice");
        assert_eq!(objects.endpoint.get("context").unwrap(), "default");
        assert!(objects.auth.is_none());
        assert!(objects.identify.is_none());
        assert!(objects.registration.is_none());
    }

    #[test]
    fn test_wizard_with_auth() {
        let mut wiz = WizardConfig::new("bob");
        wiz.set("context", "default");
        wiz.set("inbound_auth/username", "bob");
        wiz.set("inbound_auth/password", "secret");

        let objects = wiz.expand();
        assert!(objects.auth.is_some());
        let auth = objects.auth.unwrap();
        assert_eq!(auth.get("username").unwrap(), "bob");
    }

    #[test]
    fn test_wizard_with_registration() {
        let mut wiz = WizardConfig::new("trunk");
        wiz.set("server_uri", "sip:provider.com");
        wiz.set("client_uri", "sip:myaccount@provider.com");
        wiz.set("remote_hosts", "provider.com");

        let objects = wiz.expand();
        assert!(objects.registration.is_some());
        assert!(objects.identify.is_some());
    }

    #[test]
    fn test_has_flags() {
        let mut wiz = WizardConfig::new("test");
        assert!(!wiz.has_auth());
        assert!(!wiz.has_registration());
        assert!(!wiz.has_identify());

        wiz.set("inbound_auth/username", "test");
        assert!(wiz.has_auth());

        wiz.set("match", "10.0.0.0/24");
        assert!(wiz.has_identify());
    }
}
