//! Call features: transfer, disconnect, parking DTMF codes.
//!
//! Port of `main/features.c` and `main/features_config.c`. Provides
//! in-call DTMF feature detection (blind transfer, attended transfer,
//! disconnect, parking, automixmon) and per-channel feature overrides.

use std::collections::HashMap;
use std::fmt;

use thiserror::Error;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum FeatureError {
    #[error("feature not found: {0}")]
    NotFound(String),
    #[error("feature configuration error: {0}")]
    Config(String),
    #[error("feature code conflict: '{0}' already assigned to '{1}'")]
    CodeConflict(String, String),
}

pub type FeatureResult<T> = Result<T, FeatureError>;

// ---------------------------------------------------------------------------
// Built-in feature types
// ---------------------------------------------------------------------------

/// Built-in call feature types.
///
/// These correspond to the features defined in `features.conf` and the
/// `ast_feature_flag` enum from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinFeature {
    /// Blind (unattended) transfer.
    BlindTransfer,
    /// Attended (consultative) transfer.
    AttendedTransfer,
    /// One-touch disconnect.
    Disconnect,
    /// One-touch parking.
    ParkCall,
    /// One-touch MixMonitor record toggle.
    AutoMixMon,
    /// One-touch auto-monitor.
    AutoMon,
}

impl BuiltinFeature {
    /// The default DTMF code for this feature.
    pub fn default_code(&self) -> &'static str {
        match self {
            Self::BlindTransfer => "#1",
            Self::AttendedTransfer => "#2",
            Self::Disconnect => "*0",
            Self::ParkCall => "#72",
            Self::AutoMixMon => "",
            Self::AutoMon => "",
        }
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::BlindTransfer => "blindxfer",
            Self::AttendedTransfer => "atxfer",
            Self::Disconnect => "disconnect",
            Self::ParkCall => "parkcall",
            Self::AutoMixMon => "automixmon",
            Self::AutoMon => "automon",
        }
    }

    /// Parse from config name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "blindxfer" | "blindtransfer" => Some(Self::BlindTransfer),
            "atxfer" | "attendedtransfer" => Some(Self::AttendedTransfer),
            "disconnect" => Some(Self::Disconnect),
            "parkcall" => Some(Self::ParkCall),
            "automixmon" => Some(Self::AutoMixMon),
            "automon" => Some(Self::AutoMon),
            _ => None,
        }
    }

    /// All builtin features.
    pub fn all() -> &'static [BuiltinFeature] {
        &[
            Self::BlindTransfer,
            Self::AttendedTransfer,
            Self::Disconnect,
            Self::ParkCall,
            Self::AutoMixMon,
            Self::AutoMon,
        ]
    }
}

impl fmt::Display for BuiltinFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Feature flags (who can use a feature)
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Flags controlling which side of a bridge can use a feature.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FeatureSide: u8 {
        /// Feature available to the caller (bridge initiator).
        const CALLER = 1 << 0;
        /// Feature available to the callee (bridged party).
        const CALLEE = 1 << 1;
        /// Feature available to both sides.
        const BOTH   = Self::CALLER.bits() | Self::CALLEE.bits();
    }
}

// ---------------------------------------------------------------------------
// Feature code mapping
// ---------------------------------------------------------------------------

/// A single feature code assignment.
#[derive(Debug, Clone)]
pub struct FeatureCode {
    /// The DTMF code that activates this feature.
    pub code: String,
    /// Which side of the bridge can use this feature.
    pub side: FeatureSide,
    /// Whether this feature is enabled.
    pub enabled: bool,
}

impl FeatureCode {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            side: FeatureSide::BOTH,
            enabled: !code.is_empty(),
        }
    }

    pub fn with_side(mut self, side: FeatureSide) -> Self {
        self.side = side;
        self
    }
}

// ---------------------------------------------------------------------------
// Feature set (global configuration)
// ---------------------------------------------------------------------------

/// The global set of feature code mappings.
///
/// Corresponds to the features.conf configuration and the
/// `features_config` structure from the C source.
pub struct FeatureSet {
    /// Built-in feature codes keyed by feature type.
    builtins: HashMap<BuiltinFeature, FeatureCode>,
    /// Dynamically registered feature codes keyed by a unique name.
    dynamic: HashMap<String, DynamicFeature>,
    /// DTMF detection timeout in milliseconds.
    pub dtmf_timeout_ms: u32,
    /// Transfer digit timeout in milliseconds.
    pub transfer_digit_timeout_ms: u32,
    /// Attended transfer abort DTMF code.
    pub atxfer_abort: String,
    /// Attended transfer complete DTMF code.
    pub atxfer_complete: String,
    /// Attended transfer three-way DTMF code.
    pub atxfer_threeway: String,
    /// Attended transfer swap DTMF code.
    pub atxfer_swap: String,
    /// Pickup exten.
    pub pickup_exten: String,
    /// Feature digit timeout.
    pub feature_digit_timeout_ms: u32,
}

impl FeatureSet {
    /// Create a new feature set with default codes.
    pub fn new() -> Self {
        let mut builtins = HashMap::new();
        for feature in BuiltinFeature::all() {
            builtins.insert(*feature, FeatureCode::new(feature.default_code()));
        }

        Self {
            builtins,
            dynamic: HashMap::new(),
            dtmf_timeout_ms: 500,
            transfer_digit_timeout_ms: 3000,
            atxfer_abort: "*1".to_string(),
            atxfer_complete: "*2".to_string(),
            atxfer_threeway: "*3".to_string(),
            atxfer_swap: "*4".to_string(),
            pickup_exten: "*8".to_string(),
            feature_digit_timeout_ms: 1000,
        }
    }

    /// Set the DTMF code for a built-in feature.
    pub fn set_builtin_code(&mut self, feature: BuiltinFeature, code: &str) {
        self.builtins
            .entry(feature)
            .and_modify(|fc| {
                fc.code = code.to_string();
                fc.enabled = !code.is_empty();
            })
            .or_insert_with(|| FeatureCode::new(code));
        debug!(feature = %feature, code, "Set feature code");
    }

    /// Get the DTMF code for a built-in feature.
    pub fn get_builtin_code(&self, feature: BuiltinFeature) -> Option<&str> {
        self.builtins
            .get(&feature)
            .filter(|fc| fc.enabled)
            .map(|fc| fc.code.as_str())
    }

    /// Get the FeatureCode for a built-in feature.
    pub fn get_builtin(&self, feature: BuiltinFeature) -> Option<&FeatureCode> {
        self.builtins.get(&feature)
    }

    /// Check whether a DTMF sequence matches any enabled feature.
    ///
    /// Returns the matching feature if found. This implements the feature
    /// code detection during bridged calls.
    pub fn match_dtmf(&self, dtmf: &str, side: FeatureSide) -> Option<BuiltinFeature> {
        for (feature, fc) in &self.builtins {
            if fc.enabled && fc.code == dtmf && fc.side.intersects(side) {
                return Some(*feature);
            }
        }
        None
    }

    /// Check if a DTMF sequence is a prefix of any feature code.
    ///
    /// Used for progressive DTMF matching: if the user has typed "*" and
    /// there are codes starting with "*", we need to wait for more digits.
    pub fn is_prefix(&self, dtmf: &str) -> bool {
        for fc in self.builtins.values() {
            if fc.enabled && fc.code.starts_with(dtmf) && fc.code.len() > dtmf.len() {
                return true;
            }
        }
        for df in self.dynamic.values() {
            if df.code.enabled && df.code.code.starts_with(dtmf) && df.code.code.len() > dtmf.len()
            {
                return true;
            }
        }
        false
    }

    /// Register a dynamic feature.
    pub fn register_dynamic(&mut self, name: &str, feature: DynamicFeature) -> FeatureResult<()> {
        // Check for code conflicts.
        for (existing_name, existing) in &self.dynamic {
            if existing.code.code == feature.code.code && existing.code.enabled {
                return Err(FeatureError::CodeConflict(
                    feature.code.code.clone(),
                    existing_name.clone(),
                ));
            }
        }

        info!(name, code = %feature.code.code, "Registered dynamic feature");
        self.dynamic.insert(name.to_string(), feature);
        Ok(())
    }

    /// Unregister a dynamic feature.
    pub fn unregister_dynamic(&mut self, name: &str) -> bool {
        self.dynamic.remove(name).is_some()
    }

    /// Get a dynamic feature by name.
    pub fn get_dynamic(&self, name: &str) -> Option<&DynamicFeature> {
        self.dynamic.get(name)
    }

    /// List all registered dynamic feature names.
    pub fn dynamic_names(&self) -> Vec<String> {
        self.dynamic.keys().cloned().collect()
    }
}

impl Default for FeatureSet {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FeatureSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeatureSet")
            .field("builtins", &self.builtins.len())
            .field("dynamic", &self.dynamic.len())
            .field("dtmf_timeout_ms", &self.dtmf_timeout_ms)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Dynamic feature
// ---------------------------------------------------------------------------

/// A dynamically registered call feature.
#[derive(Debug, Clone)]
pub struct DynamicFeature {
    /// Feature code.
    pub code: FeatureCode,
    /// Application to execute when activated.
    pub app: String,
    /// Application data/arguments.
    pub app_data: String,
    /// MOH class to play while executing.
    pub moh_class: Option<String>,
}

impl DynamicFeature {
    pub fn new(code: &str, app: &str) -> Self {
        Self {
            code: FeatureCode::new(code),
            app: app.to_string(),
            app_data: String::new(),
            moh_class: None,
        }
    }

    pub fn with_app_data(mut self, data: &str) -> Self {
        self.app_data = data.to_string();
        self
    }

    pub fn with_moh(mut self, class: &str) -> Self {
        self.moh_class = Some(class.to_string());
        self
    }
}

// ---------------------------------------------------------------------------
// Per-channel feature overrides
// ---------------------------------------------------------------------------

/// Per-channel feature overrides.
///
/// Channels can override the global feature codes via channel variables
/// or application arguments.
#[derive(Debug, Clone, Default)]
pub struct ChannelFeatureOverrides {
    /// Overridden feature codes.
    overrides: HashMap<BuiltinFeature, FeatureCode>,
}

impl ChannelFeatureOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an override for a built-in feature.
    pub fn set(&mut self, feature: BuiltinFeature, code: &str) {
        self.overrides.insert(feature, FeatureCode::new(code));
    }

    /// Remove an override.
    pub fn remove(&mut self, feature: BuiltinFeature) {
        self.overrides.remove(&feature);
    }

    /// Get the effective feature code, checking overrides first then global set.
    pub fn effective_code<'a>(
        &'a self,
        feature: BuiltinFeature,
        global: &'a FeatureSet,
    ) -> Option<&'a str> {
        if let Some(fc) = self.overrides.get(&feature) {
            if fc.enabled {
                return Some(&fc.code);
            } else {
                return None;
            }
        }
        global.get_builtin_code(feature)
    }

    /// Check if any overrides are set.
    pub fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_feature_defaults() {
        let fs = FeatureSet::new();
        assert_eq!(fs.get_builtin_code(BuiltinFeature::BlindTransfer), Some("#1"));
        assert_eq!(fs.get_builtin_code(BuiltinFeature::AttendedTransfer), Some("#2"));
        assert_eq!(fs.get_builtin_code(BuiltinFeature::Disconnect), Some("*0"));
        assert_eq!(fs.get_builtin_code(BuiltinFeature::ParkCall), Some("#72"));
        // AutoMixMon has empty code by default -> disabled.
        assert_eq!(fs.get_builtin_code(BuiltinFeature::AutoMixMon), None);
    }

    #[test]
    fn test_set_builtin_code() {
        let mut fs = FeatureSet::new();
        fs.set_builtin_code(BuiltinFeature::BlindTransfer, "*1");
        assert_eq!(fs.get_builtin_code(BuiltinFeature::BlindTransfer), Some("*1"));
    }

    #[test]
    fn test_disable_feature() {
        let mut fs = FeatureSet::new();
        fs.set_builtin_code(BuiltinFeature::BlindTransfer, "");
        assert_eq!(fs.get_builtin_code(BuiltinFeature::BlindTransfer), None);
    }

    #[test]
    fn test_match_dtmf() {
        let fs = FeatureSet::new();

        assert_eq!(
            fs.match_dtmf("#1", FeatureSide::CALLER),
            Some(BuiltinFeature::BlindTransfer)
        );
        assert_eq!(
            fs.match_dtmf("*0", FeatureSide::CALLEE),
            Some(BuiltinFeature::Disconnect)
        );
        assert_eq!(fs.match_dtmf("99", FeatureSide::BOTH), None);
    }

    #[test]
    fn test_is_prefix() {
        let fs = FeatureSet::new();

        // "#" is a prefix of "#1", "#2", "#72".
        assert!(fs.is_prefix("#"));
        // "#7" is a prefix of "#72".
        assert!(fs.is_prefix("#7"));
        // "#1" matches exactly, not a prefix of anything longer.
        assert!(!fs.is_prefix("#1"));
        // "9" is not a prefix of anything.
        assert!(!fs.is_prefix("9"));
    }

    #[test]
    fn test_builtin_feature_from_name() {
        assert_eq!(
            BuiltinFeature::from_name("blindxfer"),
            Some(BuiltinFeature::BlindTransfer)
        );
        assert_eq!(
            BuiltinFeature::from_name("ATXFER"),
            Some(BuiltinFeature::AttendedTransfer)
        );
        assert_eq!(
            BuiltinFeature::from_name("parkcall"),
            Some(BuiltinFeature::ParkCall)
        );
        assert_eq!(BuiltinFeature::from_name("unknown"), None);
    }

    #[test]
    fn test_dynamic_feature_registration() {
        let mut fs = FeatureSet::new();
        let df = DynamicFeature::new("**", "Record")
            .with_app_data("some_args")
            .with_moh("default");

        fs.register_dynamic("record_toggle", df).unwrap();
        assert!(fs.get_dynamic("record_toggle").is_some());
        assert_eq!(
            fs.get_dynamic("record_toggle").unwrap().app,
            "Record"
        );
    }

    #[test]
    fn test_dynamic_feature_conflict() {
        let mut fs = FeatureSet::new();
        fs.register_dynamic("f1", DynamicFeature::new("**", "App1")).unwrap();

        let result = fs.register_dynamic("f2", DynamicFeature::new("**", "App2"));
        assert!(matches!(result, Err(FeatureError::CodeConflict(_, _))));
    }

    #[test]
    fn test_channel_feature_overrides() {
        let fs = FeatureSet::new();
        let mut overrides = ChannelFeatureOverrides::new();

        // Without override, use global.
        assert_eq!(
            overrides.effective_code(BuiltinFeature::BlindTransfer, &fs),
            Some("#1")
        );

        // With override.
        overrides.set(BuiltinFeature::BlindTransfer, "*9");
        assert_eq!(
            overrides.effective_code(BuiltinFeature::BlindTransfer, &fs),
            Some("*9")
        );

        // Override with empty code disables the feature.
        overrides.set(BuiltinFeature::BlindTransfer, "");
        assert_eq!(
            overrides.effective_code(BuiltinFeature::BlindTransfer, &fs),
            None
        );

        // Remove override -> back to global.
        overrides.remove(BuiltinFeature::BlindTransfer);
        assert_eq!(
            overrides.effective_code(BuiltinFeature::BlindTransfer, &fs),
            Some("#1")
        );
    }

    #[test]
    fn test_feature_side_flags() {
        let caller = FeatureSide::CALLER;
        let both = FeatureSide::BOTH;

        assert!(both.contains(caller));
        assert!(both.intersects(FeatureSide::CALLEE));
        assert!(!caller.contains(FeatureSide::CALLEE));
    }
}
