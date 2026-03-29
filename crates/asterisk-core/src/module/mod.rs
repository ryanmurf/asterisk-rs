//! Module system -- loading, unloading, and managing Asterisk modules.
//!
//! Modeled after Asterisk's module.h. Modules are the unit of
//! extensibility -- channel drivers, applications, functions, etc.
//! are all loaded as modules.

use std::fmt;
use std::sync::Arc;
use parking_lot::RwLock;

/// Result of loading a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleLoadResult {
    /// Module loaded successfully
    Success,
    /// Module declined to load (not an error, but won't be available)
    Decline,
    /// Module failed to load (error condition)
    Failure,
    /// Module was skipped
    Skip,
}

/// Module support level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModuleSupportLevel {
    #[default]
    Unknown,
    Core,
    Extended,
    Deprecated,
}

/// Trait that all Asterisk modules implement.
///
/// Each module provides load/unload/reload lifecycle hooks and
/// metadata about itself.
pub trait Module: Send + Sync + fmt::Debug {
    /// The module name (e.g., "chan_sip", "app_dial", "func_callerid").
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str {
        ""
    }

    /// Load the module. Called once during startup or when explicitly loaded.
    fn load(&self) -> ModuleLoadResult;

    /// Reload the module configuration. Called when `module reload` is issued.
    fn reload(&self) -> bool {
        // Default: reload not supported
        false
    }

    /// Unload the module. Called during shutdown or explicit unload.
    fn unload(&self) -> bool;

    /// Module load priority (lower = loaded earlier). Default is 100.
    fn priority(&self) -> u32 {
        100
    }

    /// Module support level.
    fn support_level(&self) -> ModuleSupportLevel {
        ModuleSupportLevel::Unknown
    }
}

/// A registered module entry.
struct ModuleEntry {
    module: Arc<dyn Module>,
    loaded: bool,
}

impl fmt::Debug for ModuleEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleEntry")
            .field("name", &self.module.name())
            .field("loaded", &self.loaded)
            .finish()
    }
}

/// Registry of all modules in the system.
#[derive(Debug)]
pub struct ModuleRegistry {
    modules: RwLock<Vec<ModuleEntry>>,
}

impl ModuleRegistry {
    /// Create a new empty module registry.
    pub fn new() -> Self {
        ModuleRegistry {
            modules: RwLock::new(Vec::new()),
        }
    }

    /// Register a module. Does not load it yet.
    pub fn register(&self, module: Arc<dyn Module>) {
        let mut modules = self.modules.write();
        // Check for duplicates
        if modules.iter().any(|e| e.module.name() == module.name()) {
            tracing::warn!(module = module.name(), "module already registered");
            return;
        }
        tracing::info!(module = module.name(), "module registered");
        modules.push(ModuleEntry {
            module,
            loaded: false,
        });
    }

    /// Load all registered modules, sorted by priority.
    pub fn load_all(&self) -> Vec<(String, ModuleLoadResult)> {
        let mut modules = self.modules.write();

        // Sort by priority (lower first)
        modules.sort_by_key(|e| e.module.priority());

        let mut results = Vec::new();

        for entry in modules.iter_mut() {
            if entry.loaded {
                results.push((entry.module.name().to_string(), ModuleLoadResult::Skip));
                continue;
            }

            let name = entry.module.name().to_string();
            tracing::info!(module = %name, "loading module");

            let result = entry.module.load();
            if result == ModuleLoadResult::Success {
                entry.loaded = true;
            }

            tracing::info!(module = %name, result = ?result, "module load result");
            results.push((name, result));
        }

        results
    }

    /// Find a module by name.
    pub fn find(&self, name: &str) -> Option<Arc<dyn Module>> {
        let modules = self.modules.read();
        modules
            .iter()
            .find(|e| e.module.name() == name)
            .map(|e| Arc::clone(&e.module))
    }

    /// Unload a specific module by name.
    pub fn unload(&self, name: &str) -> bool {
        let mut modules = self.modules.write();
        if let Some(entry) = modules.iter_mut().find(|e| e.module.name() == name) {
            if entry.loaded {
                let success = entry.module.unload();
                if success {
                    entry.loaded = false;
                    tracing::info!(module = name, "module unloaded");
                }
                return success;
            }
        }
        false
    }

    /// Reload a specific module by name.
    pub fn reload(&self, name: &str) -> bool {
        let modules = self.modules.read();
        if let Some(entry) = modules.iter().find(|e| e.module.name() == name) {
            if entry.loaded {
                return entry.module.reload();
            }
        }
        false
    }

    /// Get a list of all registered module names and their loaded status.
    pub fn list(&self) -> Vec<(String, bool)> {
        let modules = self.modules.read();
        modules
            .iter()
            .map(|e| (e.module.name().to_string(), e.loaded))
            .collect()
    }

    /// Get the count of loaded modules.
    pub fn loaded_count(&self) -> usize {
        let modules = self.modules.read();
        modules.iter().filter(|e| e.loaded).count()
    }

    /// Unload all modules (in reverse order).
    pub fn unload_all(&self) {
        let mut modules = self.modules.write();
        for entry in modules.iter_mut().rev() {
            if entry.loaded {
                let name = entry.module.name().to_string();
                let success = entry.module.unload();
                if success {
                    entry.loaded = false;
                    tracing::info!(module = %name, "module unloaded");
                } else {
                    tracing::warn!(module = %name, "module failed to unload");
                }
            }
        }
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}
