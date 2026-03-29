//! Typed configuration framework.
//!
//! Port of `main/config_options.c`. Provides a framework for declaring
//! typed configuration options with validation, default values, and
//! change detection on reload. This replaces the C `aco_*` (Asterisk
//! Configuration Options) API.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;

use thiserror::Error;
// tracing used by consuming code

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ConfigOptionError {
    #[error("config option '{0}': invalid value '{1}': {2}")]
    InvalidValue(String, String, String),
    #[error("config option '{0}': required but not set")]
    Required(String),
    #[error("config option '{0}': unknown option")]
    Unknown(String),
    #[error("config section '{0}': not found")]
    SectionNotFound(String),
    #[error("config error: {0}")]
    Other(String),
}

pub type ConfigOptionResult<T> = Result<T, ConfigOptionError>;

// ---------------------------------------------------------------------------
// Option types (from aco_option_type)
// ---------------------------------------------------------------------------

/// The type of a configuration option value.
///
/// Mirrors the `aco_option_type` enum from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigOptionType {
    /// String value.
    String,
    /// Signed integer.
    Int,
    /// Unsigned integer.
    Uint,
    /// Double-precision floating point.
    Double,
    /// Boolean (yes/no, true/false, on/off, 1/0).
    Bool,
    /// Socket address (ip:port).
    Sockaddr,
    /// Codec/format name.
    Codec,
    /// Custom type with a user-provided parser.
    Custom,
}

// ---------------------------------------------------------------------------
// Parsed config value
// ---------------------------------------------------------------------------

/// A parsed configuration value.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    String(String),
    Int(i64),
    Uint(u64),
    Double(f64),
    Bool(bool),
    Sockaddr(SocketAddr),
    /// Raw value for custom-parsed types.
    Custom(String),
}

impl ConfigValue {
    /// Get as string reference.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) | Self::Custom(s) => Some(s),
            _ => None,
        }
    }

    /// Get as i64.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Uint(v) => i64::try_from(*v).ok(),
            _ => None,
        }
    }

    /// Get as u64.
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            Self::Uint(v) => Some(*v),
            Self::Int(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    /// Get as f64.
    pub fn as_double(&self) -> Option<f64> {
        match self {
            Self::Double(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            Self::Uint(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// Get as bool.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Get as SocketAddr.
    pub fn as_sockaddr(&self) -> Option<&SocketAddr> {
        match self {
            Self::Sockaddr(v) => Some(v),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(v) => write!(f, "{}", v),
            Self::Int(v) => write!(f, "{}", v),
            Self::Uint(v) => write!(f, "{}", v),
            Self::Double(v) => write!(f, "{}", v),
            Self::Bool(v) => write!(f, "{}", if *v { "yes" } else { "no" }),
            Self::Sockaddr(v) => write!(f, "{}", v),
            Self::Custom(v) => write!(f, "{}", v),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse a string into a boolean value.
///
/// Accepts: yes/no, true/false, on/off, 1/0 (case-insensitive).
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "yes" | "true" | "on" | "1" | "enabled" => Some(true),
        "no" | "false" | "off" | "0" | "disabled" => Some(false),
        _ => None,
    }
}

/// Parse a raw string into a typed `ConfigValue` according to the option type.
pub fn parse_value(
    option_type: ConfigOptionType,
    raw: &str,
) -> ConfigOptionResult<ConfigValue> {
    match option_type {
        ConfigOptionType::String => Ok(ConfigValue::String(raw.to_string())),
        ConfigOptionType::Int => {
            let v: i64 = raw.parse().map_err(|_| {
                ConfigOptionError::InvalidValue(
                    String::new(),
                    raw.to_string(),
                    "expected integer".into(),
                )
            })?;
            Ok(ConfigValue::Int(v))
        }
        ConfigOptionType::Uint => {
            let v: u64 = raw.parse().map_err(|_| {
                ConfigOptionError::InvalidValue(
                    String::new(),
                    raw.to_string(),
                    "expected unsigned integer".into(),
                )
            })?;
            Ok(ConfigValue::Uint(v))
        }
        ConfigOptionType::Double => {
            let v: f64 = raw.parse().map_err(|_| {
                ConfigOptionError::InvalidValue(
                    String::new(),
                    raw.to_string(),
                    "expected floating point number".into(),
                )
            })?;
            Ok(ConfigValue::Double(v))
        }
        ConfigOptionType::Bool => {
            let v = parse_bool(raw).ok_or_else(|| {
                ConfigOptionError::InvalidValue(
                    String::new(),
                    raw.to_string(),
                    "expected yes/no, true/false, on/off, or 1/0".into(),
                )
            })?;
            Ok(ConfigValue::Bool(v))
        }
        ConfigOptionType::Sockaddr => {
            let v: SocketAddr = raw.parse().map_err(|_| {
                ConfigOptionError::InvalidValue(
                    String::new(),
                    raw.to_string(),
                    "expected socket address (ip:port)".into(),
                )
            })?;
            Ok(ConfigValue::Sockaddr(v))
        }
        ConfigOptionType::Codec | ConfigOptionType::Custom => {
            Ok(ConfigValue::Custom(raw.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// Option definition
// ---------------------------------------------------------------------------

/// Type alias for a custom validation function.
pub type ValidateFn = Box<dyn Fn(&str) -> ConfigOptionResult<ConfigValue> + Send + Sync>;

/// Definition of a single configuration option.
pub struct ConfigOptionDef {
    /// Option name as it appears in the config file.
    pub name: String,
    /// Value type.
    pub option_type: ConfigOptionType,
    /// Default value (as a raw string).
    pub default: Option<String>,
    /// Whether this option is required.
    pub required: bool,
    /// Description of the option.
    pub description: String,
    /// Optional custom validator/parser.
    pub validate: Option<ValidateFn>,
    /// Minimum value for numeric types.
    pub min: Option<f64>,
    /// Maximum value for numeric types.
    pub max: Option<f64>,
}

impl ConfigOptionDef {
    /// Create a new option definition.
    pub fn new(name: &str, option_type: ConfigOptionType) -> Self {
        Self {
            name: name.to_string(),
            option_type,
            default: None,
            required: false,
            description: String::new(),
            validate: None,
            min: None,
            max: None,
        }
    }

    /// Set the default value.
    pub fn with_default(mut self, default: &str) -> Self {
        self.default = Some(default.to_string());
        self
    }

    /// Mark as required.
    pub fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Set the description.
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = desc.to_string();
        self
    }

    /// Set a numeric range constraint.
    pub fn with_range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    /// Set a custom validator.
    pub fn with_validator<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> ConfigOptionResult<ConfigValue> + Send + Sync + 'static,
    {
        self.validate = Some(Box::new(f));
        self
    }

    /// Parse and validate a raw value for this option.
    pub fn parse_value(&self, raw: &str) -> ConfigOptionResult<ConfigValue> {
        // Use custom validator if provided.
        if let Some(ref validate_fn) = self.validate {
            return validate_fn(raw);
        }

        let value = parse_value(self.option_type, raw).map_err(|e| {
            match e {
                ConfigOptionError::InvalidValue(_, v, reason) => {
                    ConfigOptionError::InvalidValue(self.name.clone(), v, reason)
                }
                other => other,
            }
        })?;

        // Validate numeric range.
        if let (Some(min), Some(max)) = (self.min, self.max) {
            let num_val = match &value {
                ConfigValue::Int(v) => Some(*v as f64),
                ConfigValue::Uint(v) => Some(*v as f64),
                ConfigValue::Double(v) => Some(*v),
                _ => None,
            };
            if let Some(v) = num_val {
                if v < min || v > max {
                    return Err(ConfigOptionError::InvalidValue(
                        self.name.clone(),
                        raw.to_string(),
                        format!("value {} out of range [{}, {}]", v, min, max),
                    ));
                }
            }
        }

        Ok(value)
    }

    /// Get the default value, parsed.
    pub fn default_value(&self) -> Option<ConfigOptionResult<ConfigValue>> {
        self.default.as_ref().map(|d| self.parse_value(d))
    }
}

impl fmt::Debug for ConfigOptionDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigOptionDef")
            .field("name", &self.name)
            .field("type", &self.option_type)
            .field("default", &self.default)
            .field("required", &self.required)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Config option set (a section's options)
// ---------------------------------------------------------------------------

/// A set of option definitions for a configuration section.
///
/// Mirrors the concept of `aco_type` registering options from the C source.
pub struct ConfigOptionSet {
    /// Section name (e.g., "general", "transport-udp").
    pub section: String,
    /// Option definitions keyed by name (lowercase).
    options: HashMap<String, ConfigOptionDef>,
}

impl ConfigOptionSet {
    /// Create a new option set for a section.
    pub fn new(section: &str) -> Self {
        Self {
            section: section.to_string(),
            options: HashMap::new(),
        }
    }

    /// Add an option definition.
    pub fn add_option(&mut self, def: ConfigOptionDef) {
        self.options.insert(def.name.to_lowercase(), def);
    }

    /// Convenience: add a string option.
    pub fn add_string(&mut self, name: &str, default: Option<&str>) {
        let mut def = ConfigOptionDef::new(name, ConfigOptionType::String);
        if let Some(d) = default {
            def.default = Some(d.to_string());
        }
        self.add_option(def);
    }

    /// Convenience: add a boolean option.
    pub fn add_bool(&mut self, name: &str, default: bool) {
        let def = ConfigOptionDef::new(name, ConfigOptionType::Bool)
            .with_default(if default { "yes" } else { "no" });
        self.add_option(def);
    }

    /// Convenience: add an unsigned integer option.
    pub fn add_uint(&mut self, name: &str, default: u64) {
        let def = ConfigOptionDef::new(name, ConfigOptionType::Uint)
            .with_default(&default.to_string());
        self.add_option(def);
    }

    /// Convenience: add a signed integer option with range.
    pub fn add_int_range(&mut self, name: &str, default: i64, min: i64, max: i64) {
        let def = ConfigOptionDef::new(name, ConfigOptionType::Int)
            .with_default(&default.to_string())
            .with_range(min as f64, max as f64);
        self.add_option(def);
    }

    /// Get an option definition by name (case-insensitive).
    pub fn get_option(&self, name: &str) -> Option<&ConfigOptionDef> {
        self.options.get(&name.to_lowercase())
    }

    /// Parse a set of raw key-value pairs into typed values.
    ///
    /// Returns a map of option name -> parsed value, plus any errors.
    pub fn parse_values(
        &self,
        raw: &HashMap<String, String>,
    ) -> (HashMap<String, ConfigValue>, Vec<ConfigOptionError>) {
        let mut values = HashMap::new();
        let mut errors = Vec::new();

        // Parse provided values.
        for (key, raw_val) in raw {
            let key_lower = key.to_lowercase();
            match self.options.get(&key_lower) {
                Some(def) => {
                    match def.parse_value(raw_val) {
                        Ok(val) => {
                            values.insert(key_lower, val);
                        }
                        Err(e) => errors.push(e),
                    }
                }
                None => {
                    errors.push(ConfigOptionError::Unknown(key.clone()));
                }
            }
        }

        // Apply defaults and check required.
        for (name, def) in &self.options {
            if !values.contains_key(name) {
                if let Some(default_str) = &def.default {
                    match def.parse_value(default_str) {
                        Ok(val) => {
                            values.insert(name.clone(), val);
                        }
                        Err(e) => errors.push(e),
                    }
                } else if def.required {
                    errors.push(ConfigOptionError::Required(def.name.clone()));
                }
            }
        }

        (values, errors)
    }

    /// List all option names.
    pub fn option_names(&self) -> Vec<String> {
        self.options.keys().cloned().collect()
    }
}

impl fmt::Debug for ConfigOptionSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigOptionSet")
            .field("section", &self.section)
            .field("options", &self.options.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Change detection for config reload
// ---------------------------------------------------------------------------

/// Detect changes between two sets of parsed config values.
///
/// Returns a list of (option_name, old_value, new_value) tuples for changed options.
pub fn detect_changes(
    old: &HashMap<String, ConfigValue>,
    new: &HashMap<String, ConfigValue>,
) -> Vec<(String, Option<ConfigValue>, Option<ConfigValue>)> {
    let mut changes = Vec::new();

    // Check for changed or removed values.
    for (key, old_val) in old {
        match new.get(key) {
            Some(new_val) if new_val != old_val => {
                changes.push((key.clone(), Some(old_val.clone()), Some(new_val.clone())));
            }
            None => {
                changes.push((key.clone(), Some(old_val.clone()), None));
            }
            _ => {} // Unchanged.
        }
    }

    // Check for added values.
    for (key, new_val) in new {
        if !old.contains_key(key) {
            changes.push((key.clone(), None, Some(new_val.clone())));
        }
    }

    changes
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bool_values() {
        assert_eq!(parse_bool("yes"), Some(true));
        assert_eq!(parse_bool("no"), Some(false));
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("on"), Some(true));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn test_parse_value_string() {
        let v = parse_value(ConfigOptionType::String, "hello").unwrap();
        assert_eq!(v.as_str(), Some("hello"));
    }

    #[test]
    fn test_parse_value_int() {
        let v = parse_value(ConfigOptionType::Int, "-42").unwrap();
        assert_eq!(v.as_int(), Some(-42));
    }

    #[test]
    fn test_parse_value_uint() {
        let v = parse_value(ConfigOptionType::Uint, "42").unwrap();
        assert_eq!(v.as_uint(), Some(42));

        assert!(parse_value(ConfigOptionType::Uint, "-1").is_err());
    }

    #[test]
    fn test_parse_value_double() {
        let v = parse_value(ConfigOptionType::Double, "3.14").unwrap();
        assert!((v.as_double().unwrap() - 3.14).abs() < 0.001);
    }

    #[test]
    fn test_parse_value_bool() {
        let v = parse_value(ConfigOptionType::Bool, "yes").unwrap();
        assert_eq!(v.as_bool(), Some(true));

        assert!(parse_value(ConfigOptionType::Bool, "maybe").is_err());
    }

    #[test]
    fn test_parse_value_sockaddr() {
        let v = parse_value(ConfigOptionType::Sockaddr, "127.0.0.1:5060").unwrap();
        assert!(v.as_sockaddr().is_some());
        assert_eq!(v.as_sockaddr().unwrap().port(), 5060);

        assert!(parse_value(ConfigOptionType::Sockaddr, "not_an_addr").is_err());
    }

    #[test]
    fn test_config_value_display() {
        assert_eq!(format!("{}", ConfigValue::String("hello".into())), "hello");
        assert_eq!(format!("{}", ConfigValue::Int(-5)), "-5");
        assert_eq!(format!("{}", ConfigValue::Bool(true)), "yes");
        assert_eq!(format!("{}", ConfigValue::Bool(false)), "no");
    }

    #[test]
    fn test_config_option_def_with_range() {
        let def = ConfigOptionDef::new("port", ConfigOptionType::Uint)
            .with_range(1.0, 65535.0);

        assert!(def.parse_value("5060").is_ok());
        assert!(def.parse_value("0").is_err());
        assert!(def.parse_value("70000").is_err());
    }

    #[test]
    fn test_config_option_set_parse() {
        let mut opts = ConfigOptionSet::new("general");
        opts.add_string("name", Some("default"));
        opts.add_bool("enabled", true);
        opts.add_uint("timeout", 30);

        let mut raw = HashMap::new();
        raw.insert("name".to_string(), "custom".to_string());
        raw.insert("enabled".to_string(), "no".to_string());

        let (values, errors) = opts.parse_values(&raw);
        assert!(errors.is_empty());
        assert_eq!(values.get("name"), Some(&ConfigValue::String("custom".into())));
        assert_eq!(values.get("enabled"), Some(&ConfigValue::Bool(false)));
        // timeout uses default.
        assert_eq!(values.get("timeout"), Some(&ConfigValue::Uint(30)));
    }

    #[test]
    fn test_config_option_set_required() {
        let mut opts = ConfigOptionSet::new("test");
        opts.add_option(
            ConfigOptionDef::new("secret", ConfigOptionType::String)
                .with_required(true),
        );

        let raw = HashMap::new();
        let (_, errors) = opts.parse_values(&raw);
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], ConfigOptionError::Required(n) if n == "secret"));
    }

    #[test]
    fn test_config_option_set_unknown() {
        let opts = ConfigOptionSet::new("test");
        let mut raw = HashMap::new();
        raw.insert("unknown_key".to_string(), "value".to_string());

        let (_, errors) = opts.parse_values(&raw);
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], ConfigOptionError::Unknown(n) if n == "unknown_key"));
    }

    #[test]
    fn test_detect_changes() {
        let mut old = HashMap::new();
        old.insert("a".to_string(), ConfigValue::Int(1));
        old.insert("b".to_string(), ConfigValue::String("hello".into()));
        old.insert("c".to_string(), ConfigValue::Bool(true));

        let mut new = HashMap::new();
        new.insert("a".to_string(), ConfigValue::Int(2)); // Changed.
        new.insert("b".to_string(), ConfigValue::String("hello".into())); // Same.
        new.insert("d".to_string(), ConfigValue::Bool(false)); // Added.
        // c removed.

        let changes = detect_changes(&old, &new);
        assert_eq!(changes.len(), 3); // a changed, c removed, d added.

        let a_change = changes.iter().find(|(k, _, _)| k == "a").unwrap();
        assert_eq!(a_change.1, Some(ConfigValue::Int(1)));
        assert_eq!(a_change.2, Some(ConfigValue::Int(2)));
    }

    #[test]
    fn test_config_option_custom_validator() {
        let def = ConfigOptionDef::new("direction", ConfigOptionType::Custom)
            .with_validator(|s| {
                match s {
                    "inbound" | "outbound" | "both" => Ok(ConfigValue::Custom(s.to_string())),
                    _ => Err(ConfigOptionError::InvalidValue(
                        "direction".into(),
                        s.into(),
                        "must be inbound, outbound, or both".into(),
                    )),
                }
            });

        assert!(def.parse_value("inbound").is_ok());
        assert!(def.parse_value("invalid").is_err());
    }
}
