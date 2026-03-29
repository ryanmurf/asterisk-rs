//! PJSIP phone provisioning provider.
//!
//! Port of `res/res_pjsip_phoneprov_provider.c`. Bridges the PJSIP
//! endpoint configuration with the phone provisioning framework,
//! allowing PJSIP endpoint data to be used as a source for phone
//! provisioning variables (MAC address, line number, credentials, etc.).

use std::collections::HashMap;

use tracing::debug;

// ---------------------------------------------------------------------------
// Provisioning variables
// ---------------------------------------------------------------------------

/// Standard provisioning variable names used in phone templates.
pub mod vars {
    pub const MAC: &str = "MAC";
    pub const USERNAME: &str = "USERNAME";
    pub const DISPLAY_NAME: &str = "DISPLAY_NAME";
    pub const SECRET: &str = "SECRET";
    pub const LABEL: &str = "LABEL";
    pub const CALLERID: &str = "CALLERID";
    pub const LINE_NUMBER: &str = "LINENUMBER";
    pub const LINE_STATE: &str = "LINESTATE";
    pub const SERVER_HOST: &str = "SERVER";
    pub const SERVER_PORT: &str = "SERVER_PORT";
    pub const TRANSPORT: &str = "TRANSPORT";
    pub const PROFILE: &str = "PROFILE";
}

// ---------------------------------------------------------------------------
// PJSIP provisioning provider
// ---------------------------------------------------------------------------

/// A PJSIP endpoint's provisioning data extracted for phone provisioning.
#[derive(Debug, Clone)]
pub struct PjsipProvisioningData {
    /// Endpoint name (sorcery ID).
    pub endpoint_name: String,
    /// Extracted provisioning variables.
    pub variables: HashMap<String, String>,
}

impl PjsipProvisioningData {
    pub fn new(endpoint_name: &str) -> Self {
        Self {
            endpoint_name: endpoint_name.to_string(),
            variables: HashMap::new(),
        }
    }

    /// Set a provisioning variable.
    pub fn set_var(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    /// Get a provisioning variable.
    pub fn get_var(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }

    /// Build provisioning data from endpoint configuration fields.
    pub fn from_endpoint_fields(
        endpoint_name: &str,
        fields: &HashMap<String, String>,
    ) -> Self {
        let mut data = Self::new(endpoint_name);

        // Map endpoint fields to provisioning variables
        if let Some(v) = fields.get("callerid") {
            data.set_var(vars::CALLERID, v);
        }
        if let Some(v) = fields.get("transport") {
            data.set_var(vars::TRANSPORT, v);
        }
        if let Some(v) = fields.get("mac_address") {
            data.set_var(vars::MAC, v);
        }

        data.set_var(vars::USERNAME, endpoint_name);

        debug!(
            endpoint = endpoint_name,
            vars = data.variables.len(),
            "Built PJSIP provisioning data"
        );
        data
    }
}

/// Provider name for PJSIP phone provisioning.
pub const PROVIDER_NAME: &str = "res_pjsip";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provisioning_data() {
        let mut data = PjsipProvisioningData::new("alice");
        data.set_var(vars::MAC, "00:11:22:33:44:55");
        data.set_var(vars::USERNAME, "alice");

        assert_eq!(data.get_var(vars::MAC), Some("00:11:22:33:44:55"));
        assert_eq!(data.get_var(vars::USERNAME), Some("alice"));
        assert_eq!(data.get_var(vars::SECRET), None);
    }

    #[test]
    fn test_from_endpoint_fields() {
        let mut fields = HashMap::new();
        fields.insert("callerid".to_string(), "Alice <1001>".to_string());
        fields.insert("transport".to_string(), "udp".to_string());

        let data = PjsipProvisioningData::from_endpoint_fields("alice", &fields);
        assert_eq!(data.get_var(vars::CALLERID), Some("Alice <1001>"));
        assert_eq!(data.get_var(vars::TRANSPORT), Some("udp"));
        assert_eq!(data.get_var(vars::USERNAME), Some("alice"));
    }
}
