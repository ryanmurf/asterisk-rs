//! Stasis application framework core.
//!
//! Port of `res/res_stasis.c` and `res/stasis/app.c`. Provides the core
//! Stasis application infrastructure: app registration, channel control
//! delegation, bridge management, and event routing. Stasis apps receive
//! JSON-serialised channel/bridge events via callbacks and issue control
//! commands back through `StasisControl` handles.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value as JsonValue;
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum StasisError {
    #[error("application not found: {0}")]
    AppNotFound(String),
    #[error("application already registered: {0}")]
    AppAlreadyRegistered(String),
    #[error("channel not found: {0}")]
    ChannelNotFound(String),
    #[error("bridge not found: {0}")]
    BridgeNotFound(String),
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("stasis error: {0}")]
    Other(String),
}

pub type StasisResult<T> = Result<T, StasisError>;

// ---------------------------------------------------------------------------
// Stasis event callback
// ---------------------------------------------------------------------------

/// Callback invoked when a Stasis application receives an event.
///
/// Mirrors `stasis_app_cb` from the C source. The JSON value contains
/// the serialised event (StasisStart, StasisEnd, ChannelStateChange, etc.).
pub type StasisAppCallback = Arc<dyn Fn(&str, &JsonValue) + Send + Sync>;

// ---------------------------------------------------------------------------
// Channel control commands
// ---------------------------------------------------------------------------

/// Commands that can be issued to a channel under Stasis control.
///
/// Mirrors the command queue in `stasis/control.c`.
#[derive(Debug, Clone)]
pub enum StasisCommand {
    /// Answer the channel.
    Answer,
    /// Hang up the channel with a cause code.
    Hangup { cause: u32 },
    /// Start music-on-hold with a given class.
    MohStart { moh_class: String },
    /// Stop music-on-hold.
    MohStop,
    /// Start silence generator.
    SilenceStart,
    /// Stop silence generator.
    SilenceStop,
    /// Place channel into a bridge.
    AddToBridge { bridge_id: String },
    /// Remove channel from a bridge.
    RemoveFromBridge { bridge_id: String },
    /// Continue the channel back to the dialplan.
    Continue {
        context: Option<String>,
        extension: Option<String>,
        priority: Option<i32>,
    },
    /// Set a channel variable.
    SetVariable { name: String, value: String },
    /// Redirect (transfer) the channel to a new extension.
    Redirect { endpoint: String },
    /// Ring the channel.
    Ring,
    /// Stop ringing.
    RingStop,
    /// Send DTMF to the channel.
    Dtmf {
        dtmf: String,
        before_ms: u32,
        between_ms: u32,
        duration_ms: u32,
        after_ms: u32,
    },
    /// Mute the channel.
    Mute { direction: MuteDirection },
    /// Unmute the channel.
    Unmute { direction: MuteDirection },
    /// Hold the channel.
    Hold,
    /// Unhold the channel.
    Unhold,
}

/// Direction for mute/unmute operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteDirection {
    In,
    Out,
    Both,
}

impl MuteDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::Both => "both",
        }
    }
}

// ---------------------------------------------------------------------------
// Stasis control handle
// ---------------------------------------------------------------------------

/// Control handle for a channel under Stasis application control.
///
/// Mirrors `struct stasis_app_control` from `stasis/control.c`.
#[derive(Debug)]
pub struct StasisControl {
    /// Channel unique ID.
    pub channel_id: String,
    /// Application name this control belongs to.
    pub app_name: String,
    /// Pending command queue.
    commands: RwLock<Vec<StasisCommand>>,
}

impl StasisControl {
    pub fn new(channel_id: &str, app_name: &str) -> Self {
        Self {
            channel_id: channel_id.to_string(),
            app_name: app_name.to_string(),
            commands: RwLock::new(Vec::new()),
        }
    }

    /// Enqueue a command to be executed on the controlled channel.
    pub fn send_command(&self, command: StasisCommand) {
        debug!(
            channel = %self.channel_id,
            app = %self.app_name,
            cmd = ?command,
            "Queuing Stasis command"
        );
        self.commands.write().push(command);
    }

    /// Drain all pending commands.
    pub fn drain_commands(&self) -> Vec<StasisCommand> {
        let mut cmds = self.commands.write();
        std::mem::take(&mut *cmds)
    }

    /// Number of pending commands.
    pub fn pending_count(&self) -> usize {
        self.commands.read().len()
    }
}

// ---------------------------------------------------------------------------
// Bridge types
// ---------------------------------------------------------------------------

/// Stasis bridge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StasisBridgeType {
    /// Mixing bridge (all participants hear each other).
    Mixing,
    /// Holding bridge (participants hear MOH/silence).
    Holding,
}

impl StasisBridgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mixing => "mixing",
            Self::Holding => "holding",
        }
    }
}

/// A bridge managed by a Stasis application.
#[derive(Debug, Clone)]
pub struct StasisBridge {
    /// Unique bridge ID.
    pub id: String,
    /// Bridge type.
    pub bridge_type: StasisBridgeType,
    /// Bridge name (optional).
    pub name: String,
    /// Channels currently in this bridge.
    pub channels: Vec<String>,
}

impl StasisBridge {
    pub fn new(bridge_type: StasisBridgeType, name: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            bridge_type,
            name: name.to_string(),
            channels: Vec::new(),
        }
    }

    /// Add a channel to this bridge.
    pub fn add_channel(&mut self, channel_id: &str) {
        if !self.channels.contains(&channel_id.to_string()) {
            self.channels.push(channel_id.to_string());
        }
    }

    /// Remove a channel from this bridge.
    pub fn remove_channel(&mut self, channel_id: &str) {
        self.channels.retain(|c| c != channel_id);
    }

    /// Number of channels in the bridge.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

// ---------------------------------------------------------------------------
// Registered Stasis application
// ---------------------------------------------------------------------------

/// A registered Stasis application.
struct StasisApp {
    /// Application name.
    name: String,
    /// Event callback.
    handler: StasisAppCallback,
    /// Channels subscribed to this app (beyond the controlling channel).
    subscriptions: Vec<String>,
}

impl fmt::Debug for StasisApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StasisApp")
            .field("name", &self.name)
            .field("subscriptions", &self.subscriptions.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Stasis application manager
// ---------------------------------------------------------------------------

/// Manages Stasis application registrations, channel controls, and bridges.
///
/// This is the central coordinator for the Stasis application framework.
pub struct StasisAppManager {
    /// Registered applications keyed by name.
    apps: RwLock<HashMap<String, StasisApp>>,
    /// Active channel controls keyed by channel ID.
    controls: RwLock<HashMap<String, Arc<StasisControl>>>,
    /// Active bridges keyed by bridge ID.
    bridges: RwLock<HashMap<String, StasisBridge>>,
}

impl StasisAppManager {
    pub fn new() -> Self {
        Self {
            apps: RwLock::new(HashMap::new()),
            controls: RwLock::new(HashMap::new()),
            bridges: RwLock::new(HashMap::new()),
        }
    }

    // -- Application registration ------------------------------------------

    /// Register a Stasis application.
    pub fn register_app(&self, name: &str, handler: StasisAppCallback) -> StasisResult<()> {
        let mut apps = self.apps.write();
        if apps.contains_key(name) {
            return Err(StasisError::AppAlreadyRegistered(name.to_string()));
        }
        apps.insert(
            name.to_string(),
            StasisApp {
                name: name.to_string(),
                handler,
                subscriptions: Vec::new(),
            },
        );
        info!(app = name, "Registered Stasis application");
        Ok(())
    }

    /// Unregister a Stasis application.
    pub fn unregister_app(&self, name: &str) -> StasisResult<()> {
        self.apps
            .write()
            .remove(name)
            .ok_or_else(|| StasisError::AppNotFound(name.to_string()))?;
        info!(app = name, "Unregistered Stasis application");
        Ok(())
    }

    /// List registered application names.
    pub fn app_names(&self) -> Vec<String> {
        self.apps.read().keys().cloned().collect()
    }

    /// Check if an application is registered.
    pub fn is_registered(&self, name: &str) -> bool {
        self.apps.read().contains_key(name)
    }

    // -- Event dispatch ----------------------------------------------------

    /// Send a JSON event to a named application.
    pub fn send_event(&self, app_name: &str, event: &JsonValue) -> StasisResult<()> {
        let apps = self.apps.read();
        let app = apps
            .get(app_name)
            .ok_or_else(|| StasisError::AppNotFound(app_name.to_string()))?;
        debug!(app = app_name, event_type = ?event.get("type"), "Dispatching Stasis event");
        (app.handler)(app_name, event);
        Ok(())
    }

    // -- Channel control ---------------------------------------------------

    /// Create a control handle for a channel entering a Stasis application.
    pub fn create_control(
        &self,
        channel_id: &str,
        app_name: &str,
    ) -> StasisResult<Arc<StasisControl>> {
        if !self.is_registered(app_name) {
            return Err(StasisError::AppNotFound(app_name.to_string()));
        }
        let control = Arc::new(StasisControl::new(channel_id, app_name));
        self.controls
            .write()
            .insert(channel_id.to_string(), Arc::clone(&control));
        debug!(channel = channel_id, app = app_name, "Created Stasis control");
        Ok(control)
    }

    /// Get the control handle for a channel.
    pub fn get_control(&self, channel_id: &str) -> StasisResult<Arc<StasisControl>> {
        self.controls
            .read()
            .get(channel_id)
            .cloned()
            .ok_or_else(|| StasisError::ChannelNotFound(channel_id.to_string()))
    }

    /// Remove the control handle when a channel leaves Stasis.
    pub fn remove_control(&self, channel_id: &str) -> StasisResult<Arc<StasisControl>> {
        self.controls
            .write()
            .remove(channel_id)
            .ok_or_else(|| StasisError::ChannelNotFound(channel_id.to_string()))
    }

    /// Number of channels currently under Stasis control.
    pub fn active_channel_count(&self) -> usize {
        self.controls.read().len()
    }

    // -- Bridge management -------------------------------------------------

    /// Create a new Stasis bridge.
    pub fn create_bridge(
        &self,
        bridge_type: StasisBridgeType,
        name: &str,
    ) -> StasisBridge {
        let bridge = StasisBridge::new(bridge_type, name);
        let id = bridge.id.clone();
        self.bridges.write().insert(id.clone(), bridge.clone());
        info!(bridge_id = %id, bridge_type = bridge_type.as_str(), "Created Stasis bridge");
        bridge
    }

    /// Get a bridge by ID.
    pub fn get_bridge(&self, bridge_id: &str) -> StasisResult<StasisBridge> {
        self.bridges
            .read()
            .get(bridge_id)
            .cloned()
            .ok_or_else(|| StasisError::BridgeNotFound(bridge_id.to_string()))
    }

    /// Destroy a bridge.
    pub fn destroy_bridge(&self, bridge_id: &str) -> StasisResult<StasisBridge> {
        self.bridges
            .write()
            .remove(bridge_id)
            .ok_or_else(|| StasisError::BridgeNotFound(bridge_id.to_string()))
    }

    /// Add a channel to a bridge.
    pub fn add_channel_to_bridge(
        &self,
        bridge_id: &str,
        channel_id: &str,
    ) -> StasisResult<()> {
        let mut bridges = self.bridges.write();
        let bridge = bridges
            .get_mut(bridge_id)
            .ok_or_else(|| StasisError::BridgeNotFound(bridge_id.to_string()))?;
        bridge.add_channel(channel_id);
        debug!(bridge = bridge_id, channel = channel_id, "Added channel to Stasis bridge");
        Ok(())
    }

    /// Remove a channel from a bridge.
    pub fn remove_channel_from_bridge(
        &self,
        bridge_id: &str,
        channel_id: &str,
    ) -> StasisResult<()> {
        let mut bridges = self.bridges.write();
        let bridge = bridges
            .get_mut(bridge_id)
            .ok_or_else(|| StasisError::BridgeNotFound(bridge_id.to_string()))?;
        bridge.remove_channel(channel_id);
        debug!(bridge = bridge_id, channel = channel_id, "Removed channel from Stasis bridge");
        Ok(())
    }

    /// List all active bridge IDs.
    pub fn bridge_ids(&self) -> Vec<String> {
        self.bridges.read().keys().cloned().collect()
    }

    // -- Subscriptions -----------------------------------------------------

    /// Subscribe an application to events on an additional resource.
    pub fn subscribe(&self, app_name: &str, resource_id: &str) -> StasisResult<()> {
        let mut apps = self.apps.write();
        let app = apps
            .get_mut(app_name)
            .ok_or_else(|| StasisError::AppNotFound(app_name.to_string()))?;
        if !app.subscriptions.contains(&resource_id.to_string()) {
            app.subscriptions.push(resource_id.to_string());
        }
        Ok(())
    }

    /// Unsubscribe an application from a resource.
    pub fn unsubscribe(&self, app_name: &str, resource_id: &str) -> StasisResult<()> {
        let mut apps = self.apps.write();
        let app = apps
            .get_mut(app_name)
            .ok_or_else(|| StasisError::AppNotFound(app_name.to_string()))?;
        app.subscriptions.retain(|r| r != resource_id);
        Ok(())
    }
}

impl Default for StasisAppManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StasisAppManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StasisAppManager")
            .field("apps", &self.apps.read().len())
            .field("controls", &self.controls.read().len())
            .field("bridges", &self.bridges.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_register_unregister_app() {
        let mgr = StasisAppManager::new();
        let cb: StasisAppCallback = Arc::new(|_, _| {});
        mgr.register_app("myapp", cb).unwrap();
        assert!(mgr.is_registered("myapp"));
        mgr.unregister_app("myapp").unwrap();
        assert!(!mgr.is_registered("myapp"));
    }

    #[test]
    fn test_duplicate_registration() {
        let mgr = StasisAppManager::new();
        let cb: StasisAppCallback = Arc::new(|_, _| {});
        mgr.register_app("myapp", cb.clone()).unwrap();
        assert!(mgr.register_app("myapp", cb).is_err());
    }

    #[test]
    fn test_event_dispatch() {
        let mgr = StasisAppManager::new();
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);
        let cb: StasisAppCallback = Arc::new(move |_, _| {
            c.fetch_add(1, Ordering::Relaxed);
        });
        mgr.register_app("myapp", cb).unwrap();

        let event = serde_json::json!({"type": "StasisStart"});
        mgr.send_event("myapp", &event).unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_channel_control() {
        let mgr = StasisAppManager::new();
        let cb: StasisAppCallback = Arc::new(|_, _| {});
        mgr.register_app("myapp", cb).unwrap();

        let ctrl = mgr.create_control("chan-001", "myapp").unwrap();
        ctrl.send_command(StasisCommand::Answer);
        ctrl.send_command(StasisCommand::Ring);
        assert_eq!(ctrl.pending_count(), 2);

        let drained = ctrl.drain_commands();
        assert_eq!(drained.len(), 2);
        assert_eq!(ctrl.pending_count(), 0);
    }

    #[test]
    fn test_bridge_management() {
        let mgr = StasisAppManager::new();
        let bridge = mgr.create_bridge(StasisBridgeType::Mixing, "conf");
        let bid = bridge.id.clone();

        mgr.add_channel_to_bridge(&bid, "chan-001").unwrap();
        mgr.add_channel_to_bridge(&bid, "chan-002").unwrap();

        let b = mgr.get_bridge(&bid).unwrap();
        assert_eq!(b.channel_count(), 2);

        mgr.remove_channel_from_bridge(&bid, "chan-001").unwrap();
        let b = mgr.get_bridge(&bid).unwrap();
        assert_eq!(b.channel_count(), 1);

        mgr.destroy_bridge(&bid).unwrap();
        assert!(mgr.get_bridge(&bid).is_err());
    }

    #[test]
    fn test_subscription() {
        let mgr = StasisAppManager::new();
        let cb: StasisAppCallback = Arc::new(|_, _| {});
        mgr.register_app("myapp", cb).unwrap();

        mgr.subscribe("myapp", "channel:chan-001").unwrap();
        mgr.subscribe("myapp", "bridge:br-001").unwrap();
        mgr.unsubscribe("myapp", "channel:chan-001").unwrap();
    }
}
