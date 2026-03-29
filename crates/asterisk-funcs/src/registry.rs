//! Function registry - tracks all registered dialplan functions.

use crate::DialplanFunc;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of all available dialplan functions.
pub struct FuncRegistry {
    funcs: HashMap<String, Arc<dyn DialplanFunc>>,
}

impl FuncRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            funcs: HashMap::new(),
        }
    }

    /// Create a registry pre-populated with all built-in functions.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        // CallerID
        registry.register(Arc::new(crate::callerid::FuncCallerId));
        // Channel
        registry.register(Arc::new(crate::channel::FuncChannel));
        // String functions
        registry.register(Arc::new(crate::strings::FuncLen));
        registry.register(Arc::new(crate::strings::FuncToLower));
        registry.register(Arc::new(crate::strings::FuncToUpper));
        registry.register(Arc::new(crate::strings::FuncFieldQty));
        registry.register(Arc::new(crate::strings::FuncCut));
        registry.register(Arc::new(crate::strings::FuncFilter));
        registry.register(Arc::new(crate::strings::FuncReplace));
        registry.register(Arc::new(crate::strings::FuncShift));
        registry.register(Arc::new(crate::strings::FuncPop));
        registry.register(Arc::new(crate::strings::FuncPush));
        registry.register(Arc::new(crate::strings::FuncUnshift));
        // Math
        registry.register(Arc::new(crate::math::FuncMath));
        // Logic
        registry.register(Arc::new(crate::logic::FuncIf));
        registry.register(Arc::new(crate::logic::FuncIfTime));
        registry.register(Arc::new(crate::logic::FuncSet));
        registry.register(Arc::new(crate::logic::FuncExists));
        registry.register(Arc::new(crate::logic::FuncIsNull));
        // Global
        registry.register(Arc::new(crate::global::FuncGlobal));
        // CDR
        registry.register(Arc::new(crate::cdr::FuncCdr));
        // Volume
        registry.register(Arc::new(crate::volume::FuncVolume));
        // Periodic Hook
        registry.register(Arc::new(crate::periodic_hook::FuncPeriodicHook));
        // ENUM
        registry.register(Arc::new(crate::enum_func::FuncEnumLookup));
        registry.register(Arc::new(crate::enum_func::FuncEnumQuery::new()));
        registry.register(Arc::new(crate::enum_func::FuncEnumResult));
        // Blacklist
        registry.register(Arc::new(crate::blacklist::FuncBlacklist));
        // Config
        registry.register(Arc::new(crate::config::FuncAstConfig));
        // Jitter Buffer
        registry.register(Arc::new(crate::jitterbuf::FuncJitterBuffer));
        // Hold Intercept
        registry.register(Arc::new(crate::holdintercept::FuncHoldIntercept));
        // Talk Detect
        registry.register(Arc::new(crate::talkdetect::FuncTalkDetect));
        // Pitch Shift
        registry.register(Arc::new(crate::pitchshift::FuncPitchShift));
        // Hash functions
        registry.register(Arc::new(crate::hash::FuncHash));
        registry.register(Arc::new(crate::hash::FuncHashKeys));
        registry.register(Arc::new(crate::hash::FuncKeypadHash));
        // URI encode/decode
        registry.register(Arc::new(crate::uri::FuncUriEncode));
        registry.register(Arc::new(crate::uri::FuncUriDecode));
        // Base64
        registry.register(Arc::new(crate::base64_func::FuncBase64Encode));
        registry.register(Arc::new(crate::base64_func::FuncBase64Decode));
        // AES
        registry.register(Arc::new(crate::aes::FuncAesEncrypt));
        registry.register(Arc::new(crate::aes::FuncAesDecrypt));
        // JSON
        registry.register(Arc::new(crate::json::FuncJsonDecode));
        registry.register(Arc::new(crate::json::FuncJsonEncode));
        // SayFiles
        registry.register(Arc::new(crate::sayfiles::FuncSayFiles));
        registry
    }

    /// Register a dialplan function.
    pub fn register(&mut self, func: Arc<dyn DialplanFunc>) {
        let name = func.name().to_string();
        tracing::debug!("FuncRegistry: registering function '{}'", name);
        self.funcs.insert(name, func);
    }

    /// Look up a function by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn DialplanFunc>> {
        self.funcs.get(name)
    }

    /// List all registered function names.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.funcs.keys().cloned().collect();
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

    #[test]
    fn test_registry_builtins() {
        let registry = FuncRegistry::with_builtins();
        assert!(registry.count() >= 20);
        assert!(registry.get("CALLERID").is_some());
        assert!(registry.get("CHANNEL").is_some());
        assert!(registry.get("LEN").is_some());
        assert!(registry.get("MATH").is_some());
        assert!(registry.get("IF").is_some());
        assert!(registry.get("GLOBAL").is_some());
        assert!(registry.get("CDR").is_some());
    }
}
