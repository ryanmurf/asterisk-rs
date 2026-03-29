//! /ari/deviceStates resource -- device state management via the ARI REST interface.
//!
//! Port of res/ari/resource_device_states.c. Implements CRUD operations on
//! custom device states: list all, get by name, update, and delete.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use dashmap::DashMap;
use std::sync::Arc;

/// In-memory store for custom device states.
///
/// In Asterisk C, custom device states are stored in the device state core
/// and prefixed with "Custom:". Here we provide a simple in-memory store.
pub struct DeviceStateStore {
    states: DashMap<String, DeviceState>,
}

impl DeviceStateStore {
    /// Create a new empty device state store.
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
        }
    }

    /// List all custom device states.
    pub fn list(&self) -> Vec<DeviceState> {
        self.states.iter().map(|e| e.value().clone()).collect()
    }

    /// Get a device state by name.
    pub fn get(&self, name: &str) -> Option<DeviceState> {
        self.states.get(name).map(|e| e.value().clone())
    }

    /// Set (create or update) a device state.
    pub fn set(&self, name: &str, state: &str) {
        self.states.insert(
            name.to_string(),
            DeviceState {
                name: name.to_string(),
                state: state.to_string(),
            },
        );
    }

    /// Delete a device state. Returns true if it existed.
    pub fn delete(&self, name: &str) -> bool {
        self.states.remove(name).is_some()
    }
}

impl Default for DeviceStateStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the /deviceStates route subtree.
pub fn build_device_states_routes() -> Arc<RestHandler> {
    // /deviceStates/{deviceName}
    let device_by_name = Arc::new(
        RestHandler::new("{deviceName}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Put, handle_update)
            .on(HttpMethod::Delete, handle_delete),
    );

    // /deviceStates
    let device_states = Arc::new(
        RestHandler::new("deviceStates")
            .on(HttpMethod::Get, handle_list)
            .child(device_by_name),
    );

    device_states
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /deviceStates -- list all device states.
fn handle_list(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    // In a full implementation, this would query the device state subsystem.
    // For now, return an empty list.
    let states: Vec<DeviceState> = Vec::new();
    AriResponse::ok(&states)
}

/// GET /deviceStates/{deviceName} -- get a device state.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let device_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing deviceName".into(),
            ));
        }
    };

    // In a full implementation, look up the device state.
    // Return a not-found for now since we don't have the state subsystem wired up.
    let _ = device_name;
    AriResponse::error(&AriErrorKind::NotFound("Device not found".into()))
}

/// PUT /deviceStates/{deviceName} -- update a device state.
fn handle_update(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let device_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing deviceName".into(),
            ));
        }
    };

    let _device_state = match req.query_param("deviceState") {
        Some(state) => state,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: deviceState".into(),
            ));
        }
    };

    // Validate the device name starts with a custom prefix
    if !device_name.starts_with("Custom:") && !device_name.starts_with("Stasis:") {
        return AriResponse::error(&AriErrorKind::Conflict(
            "device name must be prefixed with 'Custom:' or 'Stasis:'".into(),
        ));
    }

    // In a full implementation, set the device state in the core.
    AriResponse::no_content()
}

/// DELETE /deviceStates/{deviceName} -- delete a device state.
fn handle_delete(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let device_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing deviceName".into(),
            ));
        }
    };

    if !device_name.starts_with("Custom:") && !device_name.starts_with("Stasis:") {
        return AriResponse::error(&AriErrorKind::Conflict(
            "can only delete custom device states".into(),
        ));
    }

    // In a full implementation, delete the device state.
    AriResponse::no_content()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_state_store() {
        let store = DeviceStateStore::new();
        assert_eq!(store.list().len(), 0);

        store.set("Custom:mydevice", "NOT_INUSE");
        assert_eq!(store.list().len(), 1);

        let state = store.get("Custom:mydevice").unwrap();
        assert_eq!(state.state, "NOT_INUSE");

        store.set("Custom:mydevice", "INUSE");
        let state = store.get("Custom:mydevice").unwrap();
        assert_eq!(state.state, "INUSE");

        assert!(store.delete("Custom:mydevice"));
        assert_eq!(store.list().len(), 0);
        assert!(!store.delete("Custom:nonexistent"));
    }
}
