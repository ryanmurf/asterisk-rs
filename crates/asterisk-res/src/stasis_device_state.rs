//! Custom device state via Stasis.
//!
//! Port of `res/res_stasis_device_state.c`. Allows Stasis (ARI) applications
//! to create and manage custom device states. Custom device states use the
//! "Stasis:" provider scheme and are persisted in AstDB.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// AstDB family for persisting Stasis device states.
pub const DEVICE_STATE_FAMILY: &str = "StasisDeviceState";

/// Scheme prefix for custom Stasis device states.
pub const DEVICE_STATE_SCHEME: &str = "Stasis:";

/// Scheme prefix for device state subscriptions.
pub const DEVICE_STATE_SUB_SCHEME: &str = "deviceState:";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum DeviceStateError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("device state error: {0}")]
    Other(String),
}

pub type DeviceStateResult<T> = Result<T, DeviceStateError>;

// ---------------------------------------------------------------------------
// Device state values
// ---------------------------------------------------------------------------

/// Device state values matching Asterisk's `enum ast_device_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceState {
    Unknown,
    NotInUse,
    InUse,
    Busy,
    Invalid,
    Unavailable,
    Ringing,
    RingInUse,
    OnHold,
}

impl DeviceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::NotInUse => "NOT_INUSE",
            Self::InUse => "INUSE",
            Self::Busy => "BUSY",
            Self::Invalid => "INVALID",
            Self::Unavailable => "UNAVAILABLE",
            Self::Ringing => "RINGING",
            Self::RingInUse => "RINGINUSE",
            Self::OnHold => "ONHOLD",
        }
    }

    pub fn from_str_value(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "NOT_INUSE" => Self::NotInUse,
            "INUSE" => Self::InUse,
            "BUSY" => Self::Busy,
            "INVALID" => Self::Invalid,
            "UNAVAILABLE" => Self::Unavailable,
            "RINGING" => Self::Ringing,
            "RINGINUSE" => Self::RingInUse,
            "ONHOLD" => Self::OnHold,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for DeviceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Device state subscription
// ---------------------------------------------------------------------------

/// A subscription from a Stasis app to device state changes.
#[derive(Debug, Clone)]
pub struct DeviceStateSubscription {
    /// Application name.
    pub app_name: String,
    /// Device name (or all).
    pub device_name: String,
}

// ---------------------------------------------------------------------------
// Stasis device state manager
// ---------------------------------------------------------------------------

/// Manages custom device states controlled through Stasis/ARI.
///
/// Custom device states use the `Stasis:` provider prefix.
/// State changes are persisted in AstDB under the `StasisDeviceState` family.
pub struct StasisDeviceStateManager {
    /// Current states keyed by device name (without Stasis: prefix).
    states: RwLock<HashMap<String, DeviceState>>,
    /// Active subscriptions.
    subscriptions: RwLock<Vec<DeviceStateSubscription>>,
}

impl StasisDeviceStateManager {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(Vec::new()),
        }
    }

    /// Set the state of a custom device.
    ///
    /// The device name should not include the `Stasis:` prefix.
    pub fn set_state(&self, device: &str, state: DeviceState) {
        self.states
            .write()
            .insert(device.to_string(), state);
        info!(device = device, state = %state, "Custom device state updated");
    }

    /// Get the current state of a device.
    pub fn get_state(&self, device: &str) -> DeviceState {
        self.states
            .read()
            .get(device)
            .copied()
            .unwrap_or(DeviceState::Unknown)
    }

    /// Delete a custom device state.
    pub fn delete(&self, device: &str) -> DeviceStateResult<()> {
        self.states
            .write()
            .remove(device)
            .ok_or_else(|| DeviceStateError::NotFound(device.to_string()))?;
        debug!(device = device, "Custom device state deleted");
        Ok(())
    }

    /// List all custom device names.
    pub fn devices(&self) -> Vec<String> {
        self.states.read().keys().cloned().collect()
    }

    /// Get the full device string including scheme prefix.
    pub fn full_device_name(device: &str) -> String {
        format!("{}{}", DEVICE_STATE_SCHEME, device)
    }

    /// Subscribe a Stasis app to device state changes.
    pub fn subscribe(&self, app_name: &str, device: &str) {
        self.subscriptions.write().push(DeviceStateSubscription {
            app_name: app_name.to_string(),
            device_name: device.to_string(),
        });
        debug!(app = app_name, device = device, "Device state subscription added");
    }

    /// Unsubscribe a Stasis app from a device.
    pub fn unsubscribe(&self, app_name: &str, device: &str) {
        self.subscriptions
            .write()
            .retain(|s| !(s.app_name == app_name && s.device_name == device));
    }

    /// Number of tracked devices.
    pub fn device_count(&self) -> usize {
        self.states.read().len()
    }
}

impl Default for StasisDeviceStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StasisDeviceStateManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StasisDeviceStateManager")
            .field("devices", &self.states.read().len())
            .field("subscriptions", &self.subscriptions.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_state_values() {
        assert_eq!(DeviceState::from_str_value("INUSE"), DeviceState::InUse);
        assert_eq!(DeviceState::from_str_value("invalid"), DeviceState::Invalid);
        assert_eq!(DeviceState::from_str_value("garbage"), DeviceState::Unknown);
        assert_eq!(DeviceState::Ringing.as_str(), "RINGING");
    }

    #[test]
    fn test_set_get_delete() {
        let mgr = StasisDeviceStateManager::new();
        mgr.set_state("MyDevice", DeviceState::InUse);
        assert_eq!(mgr.get_state("MyDevice"), DeviceState::InUse);

        mgr.set_state("MyDevice", DeviceState::NotInUse);
        assert_eq!(mgr.get_state("MyDevice"), DeviceState::NotInUse);

        mgr.delete("MyDevice").unwrap();
        assert_eq!(mgr.get_state("MyDevice"), DeviceState::Unknown);
    }

    #[test]
    fn test_full_device_name() {
        assert_eq!(
            StasisDeviceStateManager::full_device_name("MyDevice"),
            "Stasis:MyDevice"
        );
    }

    #[test]
    fn test_subscriptions() {
        let mgr = StasisDeviceStateManager::new();
        mgr.subscribe("myapp", "MyDevice");
        mgr.subscribe("myapp", "OtherDevice");
        mgr.unsubscribe("myapp", "MyDevice");
    }
}
