//! Lua dialplan scripting (stub interface).
//!
//! Port of `res/res_lua.c` and `pbx/pbx_lua.c`. Provides the interface
//! for Lua-based dialplan execution. The actual Lua integration requires
//! FFI to a Lua runtime; this module defines the Rust-side API and types.

use std::collections::HashMap;

use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum LuaError {
    #[error("Lua runtime not available")]
    RuntimeNotAvailable,
    #[error("Lua script error: {0}")]
    ScriptError(String),
    #[error("Lua function not found: {0}")]
    FunctionNotFound(String),
    #[error("Lua error: {0}")]
    Other(String),
}

pub type LuaResult<T> = Result<T, LuaError>;

// ---------------------------------------------------------------------------
// Lua dialplan config
// ---------------------------------------------------------------------------

/// Configuration for the Lua dialplan module.
#[derive(Debug, Clone)]
pub struct LuaDialplanConfig {
    /// Path to the Lua extensions file (e.g., /etc/asterisk/extensions.lua).
    pub script_path: String,
    /// Whether to reload the script on each call.
    pub auto_reload: bool,
}

impl LuaDialplanConfig {
    pub fn new(script_path: &str) -> Self {
        Self {
            script_path: script_path.to_string(),
            auto_reload: false,
        }
    }
}

impl Default for LuaDialplanConfig {
    fn default() -> Self {
        Self::new("/etc/asterisk/extensions.lua")
    }
}

// ---------------------------------------------------------------------------
// Lua dialplan context
// ---------------------------------------------------------------------------

/// Represents the channel context available to Lua scripts.
///
/// This is the set of information passed to Lua when a dialplan
/// extension is being executed.
#[derive(Debug, Clone)]
pub struct LuaChannelContext {
    /// Channel name.
    pub channel: String,
    /// Dialplan context.
    pub context: String,
    /// Dialplan extension.
    pub extension: String,
    /// Dialplan priority.
    pub priority: i32,
    /// Channel variables.
    pub variables: HashMap<String, String>,
}

impl LuaChannelContext {
    pub fn new(channel: &str, context: &str, extension: &str, priority: i32) -> Self {
        Self {
            channel: channel.to_string(),
            context: context.to_string(),
            extension: extension.to_string(),
            priority,
            variables: HashMap::new(),
        }
    }

    /// Set a channel variable.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    /// Get a channel variable.
    pub fn get_variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Lua dialplan interface (stub)
// ---------------------------------------------------------------------------

/// Execute a Lua dialplan extension.
///
/// Stub - requires Lua FFI runtime for actual execution.
pub fn exec_extension(
    _config: &LuaDialplanConfig,
    ctx: &LuaChannelContext,
) -> LuaResult<i32> {
    debug!(
        channel = %ctx.channel,
        context = %ctx.context,
        extension = %ctx.extension,
        priority = ctx.priority,
        "Lua dialplan exec (stub - requires Lua FFI)"
    );
    Err(LuaError::RuntimeNotAvailable)
}

/// Check if an extension exists in the Lua dialplan.
///
/// Stub - requires Lua FFI runtime.
pub fn extension_exists(
    _config: &LuaDialplanConfig,
    context: &str,
    extension: &str,
) -> LuaResult<bool> {
    debug!(
        context = context,
        extension = extension,
        "Lua extension_exists (stub)"
    );
    Err(LuaError::RuntimeNotAvailable)
}

/// Reload the Lua script file.
pub fn reload(_config: &LuaDialplanConfig) -> LuaResult<()> {
    debug!("Lua reload (stub)");
    Err(LuaError::RuntimeNotAvailable)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config() {
        let config = LuaDialplanConfig::default();
        assert!(config.script_path.contains("extensions.lua"));
        assert!(!config.auto_reload);
    }

    #[test]
    fn test_channel_context() {
        let mut ctx = LuaChannelContext::new("SIP/alice-001", "default", "s", 1);
        ctx.set_variable("CALLERID(num)", "1001");
        assert_eq!(ctx.get_variable("CALLERID(num)"), Some("1001"));
    }

    #[test]
    fn test_exec_stub() {
        let config = LuaDialplanConfig::default();
        let ctx = LuaChannelContext::new("SIP/alice-001", "default", "s", 1);
        assert!(exec_extension(&config, &ctx).is_err());
    }

    #[test]
    fn test_extension_exists_stub() {
        let config = LuaDialplanConfig::default();
        assert!(extension_exists(&config, "default", "s").is_err());
    }
}
