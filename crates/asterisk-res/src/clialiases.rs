//! CLI aliases.
//!
//! Port of `res/res_clialiases.c`. Allows administrators to define
//! shorthand aliases for frequently used CLI commands, loaded from
//! `cli_aliases.conf`.

use std::collections::HashMap;
use std::fmt;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum CliAliasError {
    #[error("alias already exists: {0}")]
    AlreadyExists(String),
    #[error("alias not found: {0}")]
    NotFound(String),
    #[error("circular alias detected: {0} -> {1}")]
    CircularAlias(String, String),
    #[error("config parse error: {0}")]
    ConfigError(String),
}

pub type CliAliasResult<T> = Result<T, CliAliasError>;

// ---------------------------------------------------------------------------
// CLI alias
// ---------------------------------------------------------------------------

/// A single CLI alias mapping.
#[derive(Debug, Clone)]
pub struct CliAlias {
    /// The alias command string (what the user types).
    pub alias: String,
    /// The actual command to execute.
    pub actual_command: String,
}

impl CliAlias {
    pub fn new(alias: &str, actual_command: &str) -> Self {
        Self {
            alias: alias.trim().to_string(),
            actual_command: actual_command.trim().to_string(),
        }
    }
}

impl fmt::Display for CliAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} => {}", self.alias, self.actual_command)
    }
}

// ---------------------------------------------------------------------------
// Alias manager
// ---------------------------------------------------------------------------

/// Manages CLI command aliases.
pub struct CliAliasManager {
    /// Aliases keyed by the alias command string.
    aliases: RwLock<HashMap<String, CliAlias>>,
}

impl CliAliasManager {
    pub fn new() -> Self {
        Self {
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new alias.
    pub fn register(&self, alias: CliAlias) -> CliAliasResult<()> {
        let key = alias.alias.clone();

        // Check for circular references.
        let actual_lower = alias.actual_command.to_lowercase();
        let alias_lower = alias.alias.to_lowercase();
        if actual_lower == alias_lower {
            return Err(CliAliasError::CircularAlias(
                alias.alias.clone(),
                alias.actual_command.clone(),
            ));
        }

        let mut aliases = self.aliases.write();
        // Check if the actual command is itself an alias (one level).
        if aliases.contains_key(&alias.actual_command) {
            // Allow but log -- Asterisk C code allows chaining up to a point.
            debug!(
                alias = %alias.alias,
                actual = %alias.actual_command,
                "Alias target is itself an alias"
            );
        }

        if aliases.contains_key(&key) {
            return Err(CliAliasError::AlreadyExists(key));
        }

        info!(alias = %alias.alias, command = %alias.actual_command, "CLI alias registered");
        aliases.insert(key, alias);
        Ok(())
    }

    /// Unregister an alias.
    pub fn unregister(&self, alias_name: &str) -> CliAliasResult<CliAlias> {
        self.aliases
            .write()
            .remove(alias_name)
            .ok_or_else(|| CliAliasError::NotFound(alias_name.to_string()))
    }

    /// Resolve a command string through aliases.
    ///
    /// If `input` matches a registered alias, returns the actual command.
    /// If `input` starts with an alias (with additional arguments), the alias
    /// portion is replaced. Otherwise returns `None`.
    pub fn resolve(&self, input: &str) -> Option<String> {
        let aliases = self.aliases.read();

        // Exact match first.
        if let Some(alias) = aliases.get(input) {
            return Some(alias.actual_command.clone());
        }

        // Check if input starts with an alias followed by a space.
        // This handles cases like "sc" aliased to "sip show channels" and
        // the user typing "sc verbose" -> "sip show channels verbose".
        for (alias_key, alias) in aliases.iter() {
            if input.starts_with(alias_key.as_str()) {
                let rest = &input[alias_key.len()..];
                if rest.is_empty() {
                    return Some(alias.actual_command.clone());
                }
                if rest.starts_with(' ') {
                    return Some(format!("{}{}", alias.actual_command, rest));
                }
            }
        }

        None
    }

    /// Load aliases from config text (INI-style: `alias = command`).
    ///
    /// Format expected:
    /// ```text
    /// [general]
    /// sc = sip show channels
    /// sp = sip show peers
    /// ```
    pub fn load_from_config(&self, config_text: &str) -> CliAliasResult<usize> {
        let mut count = 0;
        let mut in_section = false;

        for line in config_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_section = true;
                continue;
            }
            if !in_section {
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let alias_name = line[..eq_pos].trim();
                let actual_cmd = line[eq_pos + 1..].trim();
                if alias_name.is_empty() || actual_cmd.is_empty() {
                    continue;
                }

                match self.register(CliAlias::new(alias_name, actual_cmd)) {
                    Ok(()) => count += 1,
                    Err(e) => {
                        debug!(error = %e, "Failed to register alias from config");
                    }
                }
            }
        }

        info!(count, "CLI aliases loaded from configuration");
        Ok(count)
    }

    /// List all registered aliases.
    pub fn list(&self) -> Vec<CliAlias> {
        let aliases = self.aliases.read();
        let mut list: Vec<CliAlias> = aliases.values().cloned().collect();
        list.sort_by(|a, b| a.alias.cmp(&b.alias));
        list
    }

    /// Number of registered aliases.
    pub fn count(&self) -> usize {
        self.aliases.read().len()
    }

    /// Clear all aliases.
    pub fn clear(&self) {
        self.aliases.write().clear();
    }
}

impl Default for CliAliasManager {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CliAliasManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliAliasManager")
            .field("count", &self.aliases.read().len())
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
    fn test_register_and_resolve() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("sc", "sip show channels"))
            .unwrap();

        assert_eq!(mgr.resolve("sc"), Some("sip show channels".to_string()));
    }

    #[test]
    fn test_resolve_with_args() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("sc", "sip show channels"))
            .unwrap();

        assert_eq!(
            mgr.resolve("sc verbose"),
            Some("sip show channels verbose".to_string())
        );
    }

    #[test]
    fn test_resolve_no_match() {
        let mgr = CliAliasManager::new();
        assert_eq!(mgr.resolve("nonexistent"), None);
    }

    #[test]
    fn test_duplicate_registration() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("sc", "sip show channels"))
            .unwrap();
        assert!(mgr
            .register(CliAlias::new("sc", "something else"))
            .is_err());
    }

    #[test]
    fn test_circular_alias() {
        let mgr = CliAliasManager::new();
        assert!(mgr.register(CliAlias::new("foo", "foo")).is_err());
    }

    #[test]
    fn test_unregister() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("sc", "sip show channels"))
            .unwrap();
        mgr.unregister("sc").unwrap();
        assert_eq!(mgr.count(), 0);
        assert_eq!(mgr.resolve("sc"), None);
    }

    #[test]
    fn test_load_from_config() {
        let mgr = CliAliasManager::new();
        let config = r#"
; CLI aliases configuration
[general]
sc = sip show channels
sp = sip show peers
sr = sip show registry
"#;
        let count = mgr.load_from_config(config).unwrap();
        assert_eq!(count, 3);
        assert_eq!(mgr.resolve("sp"), Some("sip show peers".to_string()));
    }

    #[test]
    fn test_list() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("b", "cmd_b")).unwrap();
        mgr.register(CliAlias::new("a", "cmd_a")).unwrap();

        let list = mgr.list();
        assert_eq!(list.len(), 2);
        // Should be sorted alphabetically.
        assert_eq!(list[0].alias, "a");
        assert_eq!(list[1].alias, "b");
    }

    #[test]
    fn test_clear() {
        let mgr = CliAliasManager::new();
        mgr.register(CliAlias::new("a", "b")).unwrap();
        mgr.clear();
        assert_eq!(mgr.count(), 0);
    }
}
