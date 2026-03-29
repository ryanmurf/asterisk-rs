//! Function registry for dialplan functions.
//!
//! Mirrors the C `ast_custom_function_register` mechanism.
//! Dialplan functions (CALLERID, CHANNEL, LEN, REGEX, etc.) register here
//! and are invoked by the variable substitution engine when encountering
//! `${FUNC(args)}` syntax.

use crate::pbx::DialplanFunction;
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};

/// Global function registry.
pub static FUNC_REGISTRY: LazyLock<FuncRegistry> = LazyLock::new(FuncRegistry::new);

/// Registry of dialplan functions, keyed by name.
///
/// Thread-safe via `DashMap`. Functions register themselves at module load
/// time and are looked up by the variable substitution engine.
pub struct FuncRegistry {
    funcs: DashMap<String, Arc<dyn DialplanFunction>>,
}

impl FuncRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            funcs: DashMap::new(),
        }
    }

    /// Register a dialplan function.
    ///
    /// If a function with the same name is already registered, it is replaced
    /// and a warning is logged.
    pub fn register(&self, func: Arc<dyn DialplanFunction>) {
        let name = func.name().to_string();
        if self.funcs.contains_key(&name) {
            tracing::warn!(
                "Function '{}' already registered, replacing",
                name
            );
        }
        tracing::debug!("Registered function: {}", name);
        self.funcs.insert(name, func);
    }

    /// Unregister a dialplan function by name.
    ///
    /// Returns `true` if the function was found and removed.
    pub fn unregister(&self, name: &str) -> bool {
        let removed = self.funcs.remove(name).is_some();
        if removed {
            tracing::debug!("Unregistered function: {}", name);
        }
        removed
    }

    /// Find a dialplan function by name.
    pub fn find(&self, name: &str) -> Option<Arc<dyn DialplanFunction>> {
        self.funcs.get(name).map(|entry| entry.value().clone())
    }

    /// List all registered function names (sorted).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.funcs.iter().map(|e| e.key().clone()).collect();
        names.sort();
        names
    }

    /// Get the count of registered functions.
    pub fn count(&self) -> usize {
        self.funcs.len()
    }
}

impl Default for FuncRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;

    #[derive(Debug)]
    struct TestFunc {
        func_name: String,
    }

    #[async_trait::async_trait]
    impl DialplanFunction for TestFunc {
        fn name(&self) -> &str {
            &self.func_name
        }

        fn synopsis(&self) -> &str {
            "Test function"
        }

        async fn read(&self, _channel: &Channel, args: &str) -> Result<String, String> {
            Ok(format!("result:{}", args))
        }
    }

    #[test]
    fn test_register_and_find() {
        let registry = FuncRegistry::new();
        let func = Arc::new(TestFunc {
            func_name: "TEST".to_string(),
        });
        registry.register(func);

        assert!(registry.find("TEST").is_some());
        assert!(registry.find("NONEXISTENT").is_none());
    }

    #[test]
    fn test_unregister() {
        let registry = FuncRegistry::new();
        let func = Arc::new(TestFunc {
            func_name: "REMOVE".to_string(),
        });
        registry.register(func);

        assert!(registry.find("REMOVE").is_some());
        assert!(registry.unregister("REMOVE"));
        assert!(registry.find("REMOVE").is_none());
        assert!(!registry.unregister("REMOVE"));
    }

    #[test]
    fn test_list() {
        let registry = FuncRegistry::new();

        registry.register(Arc::new(TestFunc {
            func_name: "LEN".to_string(),
        }));
        registry.register(Arc::new(TestFunc {
            func_name: "CALLERID".to_string(),
        }));
        registry.register(Arc::new(TestFunc {
            func_name: "REGEX".to_string(),
        }));

        let names = registry.list();
        assert_eq!(names, vec!["CALLERID", "LEN", "REGEX"]);
    }

    #[test]
    fn test_count() {
        let registry = FuncRegistry::new();
        assert_eq!(registry.count(), 0);

        registry.register(Arc::new(TestFunc {
            func_name: "A".to_string(),
        }));
        assert_eq!(registry.count(), 1);

        registry.register(Arc::new(TestFunc {
            func_name: "B".to_string(),
        }));
        assert_eq!(registry.count(), 2);
    }
}
