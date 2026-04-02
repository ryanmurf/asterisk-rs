//! AST_CONFIG() function - read variables from Asterisk configuration files.
//!
//! Port of func_config.c from Asterisk C.
//!
//! Provides:
//! - AST_CONFIG(filename,category,variable[,index]) - read a config variable

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A simple in-memory config store for testing and simulation.
///
/// In production, this would interface with the asterisk-config crate
/// to load .conf files from disk.
#[derive(Debug, Default, Clone)]
pub struct ConfigStore {
    /// Map of filename -> (category -> [(variable_name, value)])
    files: HashMap<String, HashMap<String, Vec<(String, String)>>>,
}

impl ConfigStore {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
    }

    /// Add a variable to the config store.
    pub fn set(&mut self, filename: &str, category: &str, variable: &str, value: &str) {
        self.files
            .entry(filename.to_string())
            .or_default()
            .entry(category.to_string())
            .or_default()
            .push((variable.to_string(), value.to_string()));
    }

    /// Get a variable from the config store.
    /// index: 0 = first match, -1 = last match, N = Nth match
    pub fn get(&self, filename: &str, category: &str, variable: &str, index: i32) -> Option<String> {
        let file = self.files.get(filename)?;
        let cat = file.get(category)?;
        let matches: Vec<&String> = cat
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(variable))
            .map(|(_, val)| val)
            .collect();

        if matches.is_empty() {
            return None;
        }

        if index == -1 {
            matches.last().map(|v| (*v).clone())
        } else {
            let idx = index as usize;
            matches.get(idx).map(|v| (*v).clone())
        }
    }
}

/// Thread-safe handle to a shared config store.
pub type SharedConfigStore = Arc<RwLock<ConfigStore>>;

/// AST_CONFIG() function.
///
/// Reads a variable from an Asterisk configuration file.
///
/// Usage: AST_CONFIG(filename,category,variable[,index])
///
/// - filename: The config file name (e.g., "sip.conf")
/// - category: The section/category (e.g., "general")
/// - variable: The variable name to look up
/// - index: Optional; 0 = first (default), -1 = last, N = Nth occurrence
pub struct FuncAstConfig;

impl DialplanFunc for FuncAstConfig {
    fn name(&self) -> &str {
        "AST_CONFIG"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(4, ',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "AST_CONFIG: requires filename,category,variable arguments".to_string(),
            ));
        }

        let filename = parts[0].trim();
        let category = parts[1].trim();
        let variable = parts[2].trim();

        if filename.is_empty() {
            return Err(FuncError::InvalidArgument(
                "AST_CONFIG: filename is required".to_string(),
            ));
        }
        if category.is_empty() {
            return Err(FuncError::InvalidArgument(
                "AST_CONFIG: category is required".to_string(),
            ));
        }
        if variable.is_empty() {
            return Err(FuncError::InvalidArgument(
                "AST_CONFIG: variable is required".to_string(),
            ));
        }

        let index: i32 = if parts.len() > 3 {
            parts[3].trim().parse().map_err(|_| {
                FuncError::InvalidArgument("AST_CONFIG: index must be an integer".to_string())
            })?
        } else {
            0
        };

        // Look up in context variables for simulation
        // Format: __CONFIG_<filename>_<category>_<variable>_<index>
        // Or try the full store
        let lookup_key = format!("__CONFIG_{}_{}_{}_{}", filename, category, variable, index);
        if let Some(val) = ctx.get_variable(&lookup_key) {
            return Ok(val.clone());
        }

        // Try without index (get all values and select by index)
        let base_key = format!("__CONFIG_{}_{}_{}", filename, category, variable);
        if let Some(val) = ctx.get_variable(&base_key) {
            if index == -1 {
                // Return last value (comma-separated list)
                return Ok(val.split(',').next_back().unwrap_or("").to_string());
            }
            let vals: Vec<&str> = val.split(',').collect();
            let idx = index as usize;
            if idx < vals.len() {
                return Ok(vals[idx].to_string());
            }
        }

        Err(FuncError::DataNotAvailable(format!(
            "AST_CONFIG: '{}' not found in [{}] of '{}'",
            variable, category, filename
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_basic() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__CONFIG_sip.conf_general_bindport", "5060");
        let func = FuncAstConfig;
        let result = func.read(&ctx, "sip.conf,general,bindport").unwrap();
        assert_eq!(result, "5060");
    }

    #[test]
    fn test_config_with_index() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__CONFIG_sip.conf_general_allow", "ulaw,alaw,g729");
        let func = FuncAstConfig;
        assert_eq!(
            func.read(&ctx, "sip.conf,general,allow,0").unwrap(),
            "ulaw"
        );
        assert_eq!(
            func.read(&ctx, "sip.conf,general,allow,-1").unwrap(),
            "g729"
        );
    }

    #[test]
    fn test_config_not_found() {
        let ctx = FuncContext::new();
        let func = FuncAstConfig;
        assert!(func.read(&ctx, "sip.conf,general,nosuchvar").is_err());
    }

    #[test]
    fn test_config_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncAstConfig;
        assert!(func.read(&ctx, "sip.conf,general").is_err());
    }

    #[test]
    fn test_config_store() {
        let mut store = ConfigStore::new();
        store.set("sip.conf", "general", "bindport", "5060");
        store.set("sip.conf", "general", "allow", "ulaw");
        store.set("sip.conf", "general", "allow", "alaw");

        assert_eq!(
            store.get("sip.conf", "general", "bindport", 0),
            Some("5060".to_string())
        );
        assert_eq!(
            store.get("sip.conf", "general", "allow", 0),
            Some("ulaw".to_string())
        );
        assert_eq!(
            store.get("sip.conf", "general", "allow", 1),
            Some("alaw".to_string())
        );
        assert_eq!(
            store.get("sip.conf", "general", "allow", -1),
            Some("alaw".to_string())
        );
        assert_eq!(store.get("sip.conf", "general", "nosuch", 0), None);
    }
}
