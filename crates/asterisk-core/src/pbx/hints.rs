//! Hint/Extension State system for BLF (Busy Lamp Field) and presence.
//!
//! Mirrors the C `ast_extension_state`, `ast_add_hint`, and the hint
//! watcher subscription mechanism. Hints map extensions to devices,
//! allowing the PBX to track and report the state of extensions
//! (InUse, Ringing, OnHold, etc.) for features like BLF on SIP phones.

use dashmap::DashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Global hint registry.
pub static HINT_REGISTRY: LazyLock<HintRegistry> = LazyLock::new(HintRegistry::new);

/// Extension state values, matching `ast_extension_states` from pbx.h.
///
/// These can be combined as flags (e.g., `InUse | Ringing` = `RingInUse`),
/// but we also provide the combined variants explicitly for clarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ExtensionState {
    /// Extension removed.
    Removed = -2,
    /// Extension hint removed / deactivated.
    Deactivated = -1,
    /// No device INUSE or BUSY.
    NotInUse = 0,
    /// One or more devices INUSE.
    InUse = 1,
    /// All devices BUSY.
    Busy = 2,
    /// All devices UNAVAILABLE/UNREGISTERED.
    Unavailable = 4,
    /// All devices RINGING.
    Ringing = 8,
    /// All devices ONHOLD.
    OnHold = 16,
    /// Combination: InUse + Ringing.
    RingInUse = 9,
}

impl std::fmt::Display for ExtensionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Removed => write!(f, "Removed"),
            Self::Deactivated => write!(f, "Deactivated"),
            Self::NotInUse => write!(f, "Idle"),
            Self::InUse => write!(f, "InUse"),
            Self::Busy => write!(f, "Busy"),
            Self::Unavailable => write!(f, "Unavailable"),
            Self::Ringing => write!(f, "Ringing"),
            Self::OnHold => write!(f, "OnHold"),
            Self::RingInUse => write!(f, "InUse&Ringing"),
        }
    }
}

/// Callback type for hint state change watchers.
pub type HintCallback =
    Arc<dyn Fn(&str, &str, ExtensionState) + Send + Sync>;

/// Unique identifier for a hint watcher subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HintWatcherId(u64);

/// A watcher registered for state changes on a hint.
struct HintWatcher {
    id: HintWatcherId,
    callback: HintCallback,
}

/// A hint mapping an extension@context to one or more device strings.
///
/// In Asterisk, hints are created via:
/// ```text
/// exten => 100,hint,SIP/alice&SIP/alice-mobile
/// ```
///
/// The device string can reference multiple devices separated by `&`.
struct Hint {
    /// The extension pattern.
    exten: String,
    /// The context name.
    context: String,
    /// The device state expression (e.g., "SIP/alice&SIP/alice-mobile").
    device: String,
    /// Current cached state.
    state: ExtensionState,
    /// Watchers subscribed to state changes on this hint.
    watchers: Vec<HintWatcher>,
}

/// Key for hint lookups: `exten@context`.
fn hint_key(context: &str, exten: &str) -> String {
    format!("{}@{}", exten, context)
}

/// Registry of all hints in the system.
pub struct HintRegistry {
    hints: DashMap<String, Mutex<Hint>>,
    next_watcher_id: Mutex<u64>,
}

impl HintRegistry {
    /// Create a new empty hint registry.
    pub fn new() -> Self {
        Self {
            hints: DashMap::new(),
            next_watcher_id: Mutex::new(1),
        }
    }

    /// Add a hint for an extension in a context.
    ///
    /// The `device` string specifies which device(s) to monitor for state,
    /// e.g. `"SIP/alice"` or `"SIP/alice&SIP/alice-mobile"`.
    pub fn add_hint(&self, context: &str, exten: &str, device: &str) {
        let key = hint_key(context, exten);
        let hint = Hint {
            exten: exten.to_string(),
            context: context.to_string(),
            device: device.to_string(),
            state: ExtensionState::NotInUse,
            watchers: Vec::new(),
        };
        self.hints.insert(key, Mutex::new(hint));
        tracing::debug!(
            "Added hint: {}@{} -> {}",
            exten,
            context,
            device
        );
    }

    /// Remove a hint for an extension in a context.
    pub fn remove_hint(&self, context: &str, exten: &str) -> bool {
        let key = hint_key(context, exten);
        self.hints.remove(&key).is_some()
    }

    /// Get the current extension state for a hint.
    ///
    /// Returns `None` if no hint is registered for the given extension@context.
    pub fn get_extension_state(&self, context: &str, exten: &str) -> Option<ExtensionState> {
        let key = hint_key(context, exten);
        self.hints
            .get(&key)
            .map(|entry| entry.value().lock().unwrap().state)
    }

    /// Get the device string for a hint.
    pub fn get_hint_device(&self, context: &str, exten: &str) -> Option<String> {
        let key = hint_key(context, exten);
        self.hints
            .get(&key)
            .map(|entry| entry.value().lock().unwrap().device.clone())
    }

    /// Update the extension state for a hint and notify watchers.
    ///
    /// Returns `true` if the state actually changed.
    pub fn set_extension_state(
        &self,
        context: &str,
        exten: &str,
        new_state: ExtensionState,
    ) -> bool {
        let key = hint_key(context, exten);
        if let Some(entry) = self.hints.get(&key) {
            let mut hint = entry.value().lock().unwrap();
            if hint.state == new_state {
                return false;
            }
            let old_state = hint.state;
            hint.state = new_state;

            tracing::debug!(
                "Extension state change: {}@{} {} -> {}",
                exten,
                context,
                old_state,
                new_state,
            );

            // Notify watchers (clone the callback list to avoid holding the lock)
            let callbacks: Vec<HintCallback> =
                hint.watchers.iter().map(|w| w.callback.clone()).collect();
            let exten = hint.exten.clone();
            let context = hint.context.clone();
            drop(hint);

            for cb in callbacks {
                cb(&context, &exten, new_state);
            }

            true
        } else {
            false
        }
    }

    /// Watch a hint for state changes (used for BLF subscriptions).
    ///
    /// Returns a `HintWatcherId` that can be used to remove the watcher later.
    pub fn watch_hint(
        &self,
        context: &str,
        exten: &str,
        callback: HintCallback,
    ) -> Option<HintWatcherId> {
        let key = hint_key(context, exten);
        if let Some(entry) = self.hints.get(&key) {
            let id = {
                let mut next_id = self.next_watcher_id.lock().unwrap();
                let id = HintWatcherId(*next_id);
                *next_id += 1;
                id
            };

            let mut hint = entry.value().lock().unwrap();
            hint.watchers.push(HintWatcher {
                id,
                callback,
            });

            tracing::debug!(
                "Added hint watcher {:?} for {}@{}",
                id,
                exten,
                context
            );

            Some(id)
        } else {
            None
        }
    }

    /// Remove a hint watcher by ID.
    ///
    /// Returns `true` if the watcher was found and removed.
    pub fn unwatch_hint(&self, context: &str, exten: &str, watcher_id: HintWatcherId) -> bool {
        let key = hint_key(context, exten);
        if let Some(entry) = self.hints.get(&key) {
            let mut hint = entry.value().lock().unwrap();
            let before = hint.watchers.len();
            hint.watchers.retain(|w| w.id != watcher_id);
            before != hint.watchers.len()
        } else {
            false
        }
    }

    /// List all registered hints as `(context, exten, device, state)` tuples.
    pub fn list(&self) -> Vec<(String, String, String, ExtensionState)> {
        self.hints
            .iter()
            .map(|entry| {
                let hint = entry.value().lock().unwrap();
                (
                    hint.context.clone(),
                    hint.exten.clone(),
                    hint.device.clone(),
                    hint.state,
                )
            })
            .collect()
    }

    /// Get the count of registered hints.
    pub fn count(&self) -> usize {
        self.hints.len()
    }
}

impl Default for HintRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_add_and_get_hint() {
        let registry = HintRegistry::new();
        registry.add_hint("default", "100", "SIP/alice");

        assert_eq!(
            registry.get_extension_state("default", "100"),
            Some(ExtensionState::NotInUse)
        );
        assert_eq!(
            registry.get_hint_device("default", "100"),
            Some("SIP/alice".to_string())
        );
    }

    #[test]
    fn test_remove_hint() {
        let registry = HintRegistry::new();
        registry.add_hint("default", "100", "SIP/alice");
        assert!(registry.remove_hint("default", "100"));
        assert!(registry.get_extension_state("default", "100").is_none());
    }

    #[test]
    fn test_set_extension_state() {
        let registry = HintRegistry::new();
        registry.add_hint("default", "100", "SIP/alice");

        assert!(registry.set_extension_state("default", "100", ExtensionState::InUse));
        assert_eq!(
            registry.get_extension_state("default", "100"),
            Some(ExtensionState::InUse)
        );

        // Same state -> no change
        assert!(!registry.set_extension_state("default", "100", ExtensionState::InUse));
    }

    #[test]
    fn test_watch_hint() {
        let registry = HintRegistry::new();
        registry.add_hint("default", "100", "SIP/alice");

        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();

        let watcher_id = registry
            .watch_hint(
                "default",
                "100",
                Arc::new(move |_ctx, _ext, _state| {
                    called_clone.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();

        // Change state -> callback fires
        registry.set_extension_state("default", "100", ExtensionState::Ringing);
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Change again
        registry.set_extension_state("default", "100", ExtensionState::InUse);
        assert_eq!(called.load(Ordering::SeqCst), 2);

        // Remove watcher
        assert!(registry.unwatch_hint("default", "100", watcher_id));

        // Change again -> callback should NOT fire
        registry.set_extension_state("default", "100", ExtensionState::NotInUse);
        assert_eq!(called.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_nonexistent_hint() {
        let registry = HintRegistry::new();
        assert!(registry.get_extension_state("default", "999").is_none());
        assert!(!registry.set_extension_state("default", "999", ExtensionState::InUse));
    }

    #[test]
    fn test_list_hints() {
        let registry = HintRegistry::new();
        registry.add_hint("default", "100", "SIP/alice");
        registry.add_hint("default", "101", "SIP/bob");

        let list = registry.list();
        assert_eq!(list.len(), 2);
    }
}
