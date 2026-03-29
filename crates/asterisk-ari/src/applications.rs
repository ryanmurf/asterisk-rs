//! /ari/applications resource + Stasis application framework.
//!
//! Port of res/ari/resource_applications.c and the Stasis application registry.
//! Provides registration/management of Stasis applications, event filtering,
//! event subscription to specific resources (channels, bridges, endpoints,
//! device states), and event dispatch to WebSocket clients.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use crate::websocket::WebSocketSessionManager;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

/// A registered Stasis application.
///
/// Tracks subscriptions to channels, bridges, endpoints, and device states,
/// as well as event filtering rules and connected WebSocket sessions.
pub struct StasisApp {
    /// Application name.
    pub name: String,
    /// Channel IDs this app is subscribed to.
    pub channel_ids: RwLock<HashSet<String>>,
    /// Bridge IDs this app is subscribed to.
    pub bridge_ids: RwLock<HashSet<String>>,
    /// Endpoint IDs (tech/resource) this app is subscribed to.
    pub endpoint_ids: RwLock<HashSet<String>>,
    /// Device state names this app is subscribed to.
    pub device_names: RwLock<HashSet<String>>,
    /// Allowed event types (empty means all allowed).
    pub events_allowed: RwLock<Vec<EventTypeFilter>>,
    /// Disallowed event types (empty means none disallowed).
    pub events_disallowed: RwLock<Vec<EventTypeFilter>>,
    /// Whether this app is subscribed to all events.
    pub subscribe_all: RwLock<bool>,
}

impl StasisApp {
    /// Create a new Stasis application with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            channel_ids: RwLock::new(HashSet::new()),
            bridge_ids: RwLock::new(HashSet::new()),
            endpoint_ids: RwLock::new(HashSet::new()),
            device_names: RwLock::new(HashSet::new()),
            events_allowed: RwLock::new(Vec::new()),
            events_disallowed: RwLock::new(Vec::new()),
            subscribe_all: RwLock::new(false),
        }
    }

    /// Add a channel subscription.
    pub fn add_channel(&self, channel_id: &str) {
        self.channel_ids.write().insert(channel_id.to_string());
    }

    /// Remove a channel subscription.
    pub fn remove_channel(&self, channel_id: &str) {
        self.channel_ids.write().remove(channel_id);
    }

    /// Add a bridge subscription.
    pub fn add_bridge(&self, bridge_id: &str) {
        self.bridge_ids.write().insert(bridge_id.to_string());
    }

    /// Remove a bridge subscription.
    pub fn remove_bridge(&self, bridge_id: &str) {
        self.bridge_ids.write().remove(bridge_id);
    }

    /// Add an endpoint subscription.
    pub fn add_endpoint(&self, endpoint_id: &str) {
        self.endpoint_ids.write().insert(endpoint_id.to_string());
    }

    /// Remove an endpoint subscription.
    pub fn remove_endpoint(&self, endpoint_id: &str) {
        self.endpoint_ids.write().remove(endpoint_id);
    }

    /// Add a device state subscription.
    pub fn add_device(&self, device_name: &str) {
        self.device_names.write().insert(device_name.to_string());
    }

    /// Remove a device state subscription.
    pub fn remove_device(&self, device_name: &str) {
        self.device_names.write().remove(device_name);
    }

    /// Subscribe to an event source URI (channel:{id}, bridge:{id}, etc.).
    pub fn subscribe_event_source(&self, source: &str) -> Result<(), String> {
        if let Some(rest) = source.strip_prefix("channel:") {
            self.add_channel(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("bridge:") {
            self.add_bridge(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("endpoint:") {
            self.add_endpoint(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("deviceState:") {
            self.add_device(rest);
            Ok(())
        } else {
            Err(format!("unknown event source scheme: {}", source))
        }
    }

    /// Unsubscribe from an event source URI.
    pub fn unsubscribe_event_source(&self, source: &str) -> Result<(), String> {
        if let Some(rest) = source.strip_prefix("channel:") {
            self.remove_channel(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("bridge:") {
            self.remove_bridge(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("endpoint:") {
            self.remove_endpoint(rest);
            Ok(())
        } else if let Some(rest) = source.strip_prefix("deviceState:") {
            self.remove_device(rest);
            Ok(())
        } else {
            Err(format!("unknown event source scheme: {}", source))
        }
    }

    /// Check if an event type is allowed by this app's filter rules.
    pub fn is_event_allowed(&self, event_type: &str) -> bool {
        let disallowed = self.events_disallowed.read();
        if disallowed.iter().any(|f| f.event_type == event_type) {
            return false;
        }

        let allowed = self.events_allowed.read();
        if allowed.is_empty() {
            return true; // empty allowed list means all events allowed
        }

        allowed.iter().any(|f| f.event_type == event_type)
    }

    /// Create an Application model snapshot for API responses.
    pub fn to_model(&self) -> Application {
        Application {
            name: self.name.clone(),
            channel_ids: self.channel_ids.read().iter().cloned().collect(),
            bridge_ids: self.bridge_ids.read().iter().cloned().collect(),
            endpoint_ids: self.endpoint_ids.read().iter().cloned().collect(),
            device_names: self.device_names.read().iter().cloned().collect(),
            events_allowed: self.events_allowed.read().clone(),
            events_disallowed: self.events_disallowed.read().clone(),
        }
    }
}

impl std::fmt::Debug for StasisApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StasisApp")
            .field("name", &self.name)
            .finish()
    }
}

/// Registry of all Stasis applications.
///
/// When a channel enters the Stasis() dialplan application, the channel is
/// registered here, events are dispatched to the application's WebSocket
/// clients, and ARI controls the channel until Stasis exits.
pub struct StasisAppRegistry {
    /// Registered applications keyed by name.
    apps: DashMap<String, Arc<StasisApp>>,
    /// Reference to the WebSocket session manager for event dispatch.
    ws_manager: Arc<WebSocketSessionManager>,
}

impl StasisAppRegistry {
    /// Create a new app registry.
    pub fn new(ws_manager: Arc<WebSocketSessionManager>) -> Self {
        Self {
            apps: DashMap::new(),
            ws_manager,
        }
    }

    /// Register a new application (or return the existing one).
    pub fn register_app(&self, name: &str) -> Arc<StasisApp> {
        self.apps
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(StasisApp::new(name)))
            .value()
            .clone()
    }

    /// Unregister an application.
    pub fn unregister_app(&self, name: &str) -> Option<Arc<StasisApp>> {
        self.apps.remove(name).map(|(_, app)| app)
    }

    /// Get a registered application by name.
    pub fn get_app(&self, name: &str) -> Option<Arc<StasisApp>> {
        self.apps.get(name).map(|entry| entry.value().clone())
    }

    /// List all registered applications.
    pub fn list_apps(&self) -> Vec<Arc<StasisApp>> {
        self.apps.iter().map(|entry| entry.value().clone()).collect()
    }

    /// Dispatch an ARI event to all matching applications and their WebSocket clients.
    ///
    /// The event is serialized to JSON and sent to each WebSocket session
    /// that is subscribed to the event's application.
    pub fn dispatch_event(&self, event: &AriEvent) {
        let app_name = match event {
            AriEvent::StasisStart { base, .. }
            | AriEvent::StasisEnd { base, .. }
            | AriEvent::ChannelCreated { base, .. }
            | AriEvent::ChannelDestroyed { base, .. }
            | AriEvent::ChannelEnteredBridge { base, .. }
            | AriEvent::ChannelLeftBridge { base, .. }
            | AriEvent::ChannelStateChange { base, .. }
            | AriEvent::ChannelDtmfReceived { base, .. }
            | AriEvent::ChannelHangupRequest { base, .. }
            | AriEvent::ChannelDialplan { base, .. }
            | AriEvent::ChannelCallerId { base, .. }
            | AriEvent::ChannelVarset { base, .. }
            | AriEvent::ChannelHold { base, .. }
            | AriEvent::ChannelUnhold { base, .. }
            | AriEvent::ChannelTalkingStarted { base, .. }
            | AriEvent::ChannelTalkingFinished { base, .. }
            | AriEvent::ChannelConnectedLine { base, .. }
            | AriEvent::ChannelUserevent { base, .. }
            | AriEvent::BridgeCreated { base, .. }
            | AriEvent::BridgeDestroyed { base, .. }
            | AriEvent::BridgeMerged { base, .. }
            | AriEvent::BridgeVideoSourceChanged { base, .. }
            | AriEvent::BridgeBlindTransfer { base, .. }
            | AriEvent::BridgeAttendedTransfer { base, .. }
            | AriEvent::PlaybackStarted { base, .. }
            | AriEvent::PlaybackContinuing { base, .. }
            | AriEvent::PlaybackFinished { base, .. }
            | AriEvent::RecordingStarted { base, .. }
            | AriEvent::RecordingFinished { base, .. }
            | AriEvent::RecordingFailed { base, .. }
            | AriEvent::EndpointStateChange { base, .. }
            | AriEvent::DeviceStateChanged { base, .. }
            | AriEvent::ContactStatusChange { base, .. }
            | AriEvent::PeerStatusChange { base, .. }
            | AriEvent::Dial { base, .. }
            | AriEvent::TextMessageReceived { base, .. }
            | AriEvent::ApplicationMoveFailed { base, .. }
            | AriEvent::ApplicationReplaced { base, .. }
            | AriEvent::ChannelToneDetected { base, .. } => &base.application,
        };

        // Check if the app exists and event is allowed
        if let Some(app) = self.get_app(app_name) {
            let event_type = event_type_name(event);
            if app.is_event_allowed(event_type) {
                // Serialize and dispatch to WebSocket sessions
                if let Ok(json) = serde_json::to_string(event) {
                    self.ws_manager.send_to_app(app_name, &json);
                }
            }
        }
    }
}

impl std::fmt::Debug for StasisAppRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StasisAppRegistry")
            .field("num_apps", &self.apps.len())
            .finish()
    }
}

/// Get the type name string for an event (used for filtering).
fn event_type_name(event: &AriEvent) -> &'static str {
    match event {
        AriEvent::StasisStart { .. } => "StasisStart",
        AriEvent::StasisEnd { .. } => "StasisEnd",
        AriEvent::ChannelCreated { .. } => "ChannelCreated",
        AriEvent::ChannelDestroyed { .. } => "ChannelDestroyed",
        AriEvent::ChannelEnteredBridge { .. } => "ChannelEnteredBridge",
        AriEvent::ChannelLeftBridge { .. } => "ChannelLeftBridge",
        AriEvent::ChannelStateChange { .. } => "ChannelStateChange",
        AriEvent::ChannelDtmfReceived { .. } => "ChannelDtmfReceived",
        AriEvent::ChannelHangupRequest { .. } => "ChannelHangupRequest",
        AriEvent::ChannelDialplan { .. } => "ChannelDialplan",
        AriEvent::ChannelCallerId { .. } => "ChannelCallerId",
        AriEvent::ChannelVarset { .. } => "ChannelVarset",
        AriEvent::ChannelHold { .. } => "ChannelHold",
        AriEvent::ChannelUnhold { .. } => "ChannelUnhold",
        AriEvent::ChannelTalkingStarted { .. } => "ChannelTalkingStarted",
        AriEvent::ChannelTalkingFinished { .. } => "ChannelTalkingFinished",
        AriEvent::ChannelConnectedLine { .. } => "ChannelConnectedLine",
        AriEvent::ChannelUserevent { .. } => "ChannelUserevent",
        AriEvent::BridgeCreated { .. } => "BridgeCreated",
        AriEvent::BridgeDestroyed { .. } => "BridgeDestroyed",
        AriEvent::BridgeMerged { .. } => "BridgeMerged",
        AriEvent::BridgeVideoSourceChanged { .. } => "BridgeVideoSourceChanged",
        AriEvent::BridgeBlindTransfer { .. } => "BridgeBlindTransfer",
        AriEvent::BridgeAttendedTransfer { .. } => "BridgeAttendedTransfer",
        AriEvent::PlaybackStarted { .. } => "PlaybackStarted",
        AriEvent::PlaybackContinuing { .. } => "PlaybackContinuing",
        AriEvent::PlaybackFinished { .. } => "PlaybackFinished",
        AriEvent::RecordingStarted { .. } => "RecordingStarted",
        AriEvent::RecordingFinished { .. } => "RecordingFinished",
        AriEvent::RecordingFailed { .. } => "RecordingFailed",
        AriEvent::EndpointStateChange { .. } => "EndpointStateChange",
        AriEvent::DeviceStateChanged { .. } => "DeviceStateChanged",
        AriEvent::ContactStatusChange { .. } => "ContactStatusChange",
        AriEvent::PeerStatusChange { .. } => "PeerStatusChange",
        AriEvent::Dial { .. } => "Dial",
        AriEvent::TextMessageReceived { .. } => "TextMessageReceived",
        AriEvent::ApplicationMoveFailed { .. } => "ApplicationMoveFailed",
        AriEvent::ApplicationReplaced { .. } => "ApplicationReplaced",
        AriEvent::ChannelToneDetected { .. } => "ChannelToneDetected",
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// Build the /applications route subtree.
pub fn build_applications_routes() -> Arc<RestHandler> {
    // /applications/{applicationName}/subscription
    let subscription = Arc::new(
        RestHandler::new("subscription")
            .on(HttpMethod::Post, handle_subscribe)
            .on(HttpMethod::Delete, handle_unsubscribe),
    );

    // /applications/{applicationName}/eventFilter
    let event_filter = Arc::new(
        RestHandler::new("eventFilter").on(HttpMethod::Put, handle_event_filter),
    );

    // /applications/{applicationName}
    let app_by_name = Arc::new(
        RestHandler::new("{applicationName}")
            .on(HttpMethod::Get, handle_get)
            .child(subscription)
            .child(event_filter),
    );

    // /applications
    let applications = Arc::new(
        RestHandler::new("applications")
            .on(HttpMethod::Get, handle_list)
            .child(app_by_name),
    );

    applications
}

/// GET /applications -- list all Stasis applications.
fn handle_list(_req: &AriRequest, server: &AriServer) -> AriResponse {
    let apps: Vec<Application> = server
        .app_registry
        .list_apps()
        .iter()
        .map(|app| app.to_model())
        .collect();
    AriResponse::ok(&apps)
}

/// GET /applications/{applicationName} -- get application details.
fn handle_get(req: &AriRequest, server: &AriServer) -> AriResponse {
    let app_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing applicationName".into(),
            ));
        }
    };

    match server.app_registry.get_app(app_name) {
        Some(app) => AriResponse::ok(&app.to_model()),
        None => AriResponse::error(&AriErrorKind::NotFound("Application does not exist".into())),
    }
}

/// POST /applications/{applicationName}/subscription -- subscribe to event sources.
fn handle_subscribe(req: &AriRequest, server: &AriServer) -> AriResponse {
    let app_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing applicationName".into(),
            ));
        }
    };

    let event_sources = req.query_params_multi("eventSource");
    if event_sources.is_empty() {
        return AriResponse::error(&AriErrorKind::BadRequest(
            "missing required parameter: eventSource".into(),
        ));
    }

    let app = match server.app_registry.get_app(app_name) {
        Some(app) => app,
        None => {
            return AriResponse::error(&AriErrorKind::NotFound(
                "Application does not exist".into(),
            ));
        }
    };

    for source in &event_sources {
        if let Err(e) = app.subscribe_event_source(source) {
            return AriResponse::error(&AriErrorKind::BadRequest(e));
        }
    }

    AriResponse::ok(&app.to_model())
}

/// DELETE /applications/{applicationName}/subscription -- unsubscribe from event sources.
fn handle_unsubscribe(req: &AriRequest, server: &AriServer) -> AriResponse {
    let app_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing applicationName".into(),
            ));
        }
    };

    let event_sources = req.query_params_multi("eventSource");
    if event_sources.is_empty() {
        return AriResponse::error(&AriErrorKind::BadRequest(
            "missing required parameter: eventSource".into(),
        ));
    }

    let app = match server.app_registry.get_app(app_name) {
        Some(app) => app,
        None => {
            return AriResponse::error(&AriErrorKind::NotFound(
                "Application does not exist".into(),
            ));
        }
    };

    for source in &event_sources {
        if let Err(e) = app.unsubscribe_event_source(source) {
            return AriResponse::error(&AriErrorKind::BadRequest(e));
        }
    }

    AriResponse::ok(&app.to_model())
}

/// PUT /applications/{applicationName}/eventFilter -- set event type filter.
fn handle_event_filter(req: &AriRequest, server: &AriServer) -> AriResponse {
    let app_name = match req.path_var(2) {
        Some(name) => name,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing applicationName".into(),
            ));
        }
    };

    let app = match server.app_registry.get_app(app_name) {
        Some(app) => app,
        None => {
            return AriResponse::error(&AriErrorKind::NotFound(
                "Application does not exist".into(),
            ));
        }
    };

    // Parse the filter from the request body
    let filter: EventFilterRequest = match req.parse_body_optional() {
        Ok(Some(f)) => f,
        Ok(None) => {
            // Empty body: clear both filters
            *app.events_allowed.write() = Vec::new();
            *app.events_disallowed.write() = Vec::new();
            return AriResponse::ok(&app.to_model());
        }
        Err(e) => return AriResponse::error(&e),
    };

    if let Some(allowed) = filter.allowed {
        *app.events_allowed.write() = allowed;
    }
    if let Some(disallowed) = filter.disallowed {
        *app.events_disallowed.write() = disallowed;
    }

    AriResponse::ok(&app.to_model())
}
