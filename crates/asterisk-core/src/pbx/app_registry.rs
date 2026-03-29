//! Application registry for dialplan applications.
//!
//! Mirrors the C `ast_register_application2` / `pbx_findapp` mechanism.
//! Each dialplan application (Answer, Dial, Playback, Hangup, etc.) registers
//! itself here so the PBX execution loop can look it up by name.

use crate::pbx::DialplanApp;
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};

/// Global application registry.
pub static APP_REGISTRY: LazyLock<AppRegistry> = LazyLock::new(AppRegistry::new);

/// Registry of dialplan applications, keyed by name.
///
/// Thread-safe via `DashMap`. Applications register themselves at module load
/// time and are looked up by the PBX execution loop when executing priorities.
pub struct AppRegistry {
    apps: DashMap<String, Arc<dyn DialplanApp>>,
}

impl AppRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            apps: DashMap::new(),
        }
    }

    /// Register a dialplan application.
    ///
    /// If an application with the same name is already registered, it is replaced
    /// and a warning is logged.
    pub fn register(&self, app: Arc<dyn DialplanApp>) {
        let name = app.name().to_string();
        if self.apps.contains_key(&name) {
            tracing::warn!(
                "Application '{}' already registered, replacing",
                name
            );
        }
        tracing::debug!("Registered application: {}", name);
        self.apps.insert(name, app);
    }

    /// Unregister a dialplan application by name.
    ///
    /// Returns `true` if the application was found and removed.
    pub fn unregister(&self, name: &str) -> bool {
        let removed = self.apps.remove(name).is_some();
        if removed {
            tracing::debug!("Unregistered application: {}", name);
        }
        removed
    }

    /// Find a dialplan application by name.
    pub fn find(&self, name: &str) -> Option<Arc<dyn DialplanApp>> {
        self.apps.get(name).map(|entry| entry.value().clone())
    }

    /// List all registered application names (sorted).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.apps.iter().map(|e| e.key().clone()).collect();
        names.sort();
        names
    }

    /// Get the count of registered applications.
    pub fn count(&self) -> usize {
        self.apps.len()
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;
    use crate::pbx::PbxResult;

    #[derive(Debug)]
    struct TestApp {
        app_name: String,
    }

    #[async_trait::async_trait]
    impl DialplanApp for TestApp {
        fn name(&self) -> &str {
            &self.app_name
        }

        fn synopsis(&self) -> &str {
            "Test application"
        }

        async fn execute(&self, _channel: &mut Channel, _args: &str) -> PbxResult {
            PbxResult::Success
        }
    }

    #[test]
    fn test_register_and_find() {
        let registry = AppRegistry::new();
        let app = Arc::new(TestApp {
            app_name: "TestApp".to_string(),
        });
        registry.register(app);

        assert!(registry.find("TestApp").is_some());
        assert!(registry.find("NonExistent").is_none());
    }

    #[test]
    fn test_unregister() {
        let registry = AppRegistry::new();
        let app = Arc::new(TestApp {
            app_name: "RemoveMe".to_string(),
        });
        registry.register(app);

        assert!(registry.find("RemoveMe").is_some());
        assert!(registry.unregister("RemoveMe"));
        assert!(registry.find("RemoveMe").is_none());
        assert!(!registry.unregister("RemoveMe")); // already removed
    }

    #[test]
    fn test_list() {
        let registry = AppRegistry::new();

        registry.register(Arc::new(TestApp {
            app_name: "Bravo".to_string(),
        }));
        registry.register(Arc::new(TestApp {
            app_name: "Alpha".to_string(),
        }));
        registry.register(Arc::new(TestApp {
            app_name: "Charlie".to_string(),
        }));

        let names = registry.list();
        assert_eq!(names, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn test_replace() {
        let registry = AppRegistry::new();

        registry.register(Arc::new(TestApp {
            app_name: "App".to_string(),
        }));
        registry.register(Arc::new(TestApp {
            app_name: "App".to_string(),
        }));

        assert_eq!(registry.count(), 1);
    }
}
