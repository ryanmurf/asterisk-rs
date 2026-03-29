//! One-touch recording via SIP INFO.
//!
//! Port of `res/res_pjsip_one_touch_record_info.c`. Handles SIP INFO
//! requests with a `Record` header to toggle call recording on/off.
//! The `Record: on` / `Record: off` header triggers the configured
//! DTMF feature code for recording.

use tracing::debug;

// ---------------------------------------------------------------------------
// Record header values
// ---------------------------------------------------------------------------

/// One-touch recording action parsed from a SIP INFO Record header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordAction {
    /// Start recording (`Record: on`).
    On,
    /// Stop recording (`Record: off`).
    Off,
}

impl RecordAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// Parse from a Record header value.
    pub fn from_header_value(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// One-touch record configuration
// ---------------------------------------------------------------------------

/// Per-endpoint one-touch recording configuration.
#[derive(Debug, Clone)]
pub struct OneTouchRecordConfig {
    /// Whether one-touch recording is enabled for this endpoint.
    pub enabled: bool,
    /// Feature code to simulate when Record: on is received.
    pub on_feature: String,
    /// Feature code to simulate when Record: off is received.
    pub off_feature: String,
}

impl OneTouchRecordConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            on_feature: String::new(),
            off_feature: String::new(),
        }
    }

    /// Get the feature code for a given action.
    pub fn feature_for(&self, action: RecordAction) -> &str {
        match action {
            RecordAction::On => &self.on_feature,
            RecordAction::Off => &self.off_feature,
        }
    }
}

impl Default for OneTouchRecordConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// INFO request handling
// ---------------------------------------------------------------------------

/// Result of processing a one-touch record INFO request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneTouchRecordResult {
    /// Recording toggled successfully (respond 200).
    Success,
    /// Feature not configured (respond 403).
    NotConfigured,
    /// No channel/session available (respond 481).
    NoSession,
    /// Not a record INFO request (pass to next handler).
    NotHandled,
}

/// Process a SIP INFO request for one-touch recording.
///
/// Looks for a `Record` header and matches its value against the
/// endpoint's recording configuration.
pub fn handle_record_info(
    record_header: Option<&str>,
    config: &OneTouchRecordConfig,
    has_channel: bool,
) -> OneTouchRecordResult {
    let header_value = match record_header {
        Some(v) => v,
        None => return OneTouchRecordResult::NotHandled,
    };

    let action = match RecordAction::from_header_value(header_value) {
        Some(a) => a,
        None => return OneTouchRecordResult::NotHandled,
    };

    if !has_channel {
        return OneTouchRecordResult::NoSession;
    }

    if !config.enabled || config.feature_for(action).is_empty() {
        debug!(action = action.as_str(), "One-touch recording not configured");
        return OneTouchRecordResult::NotConfigured;
    }

    debug!(
        action = action.as_str(),
        feature = config.feature_for(action),
        "One-touch recording triggered"
    );
    OneTouchRecordResult::Success
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_action_parse() {
        assert_eq!(RecordAction::from_header_value("on"), Some(RecordAction::On));
        assert_eq!(RecordAction::from_header_value("Off"), Some(RecordAction::Off));
        assert_eq!(RecordAction::from_header_value("toggle"), None);
    }

    #[test]
    fn test_no_record_header() {
        let config = OneTouchRecordConfig::default();
        assert_eq!(
            handle_record_info(None, &config, true),
            OneTouchRecordResult::NotHandled,
        );
    }

    #[test]
    fn test_not_configured() {
        let config = OneTouchRecordConfig::default();
        assert_eq!(
            handle_record_info(Some("on"), &config, true),
            OneTouchRecordResult::NotConfigured,
        );
    }

    #[test]
    fn test_no_session() {
        let config = OneTouchRecordConfig {
            enabled: true,
            on_feature: "automixmon".to_string(),
            off_feature: "automixmon".to_string(),
        };
        assert_eq!(
            handle_record_info(Some("on"), &config, false),
            OneTouchRecordResult::NoSession,
        );
    }

    #[test]
    fn test_success() {
        let config = OneTouchRecordConfig {
            enabled: true,
            on_feature: "automixmon".to_string(),
            off_feature: "automixmon".to_string(),
        };
        assert_eq!(
            handle_record_info(Some("on"), &config, true),
            OneTouchRecordResult::Success,
        );
    }
}
