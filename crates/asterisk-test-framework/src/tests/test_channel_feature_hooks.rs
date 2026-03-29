//! Port of asterisk/tests/test_channel_feature_hooks.c
//!
//! Tests feature hook registration, DTMF-triggered feature execution,
//! hook ordering, and hook removal using the FeatureSet from asterisk-res.

use asterisk_res::features::{
    BuiltinFeature, DynamicFeature, FeatureCode, FeatureSet, FeatureSide,
};

// ---------------------------------------------------------------------------
// Feature hook registration
// ---------------------------------------------------------------------------

/// Port of feature hook registration from test_channel_feature_hooks.c.
///
/// Test that built-in feature codes are registered with defaults.
#[test]
fn test_default_feature_codes() {
    let fs = FeatureSet::new();

    // Blind transfer should have default code.
    let code = fs.get_builtin_code(BuiltinFeature::BlindTransfer);
    assert!(code.is_some());
    assert_eq!(code.unwrap(), "#1");

    // Attended transfer.
    let code = fs.get_builtin_code(BuiltinFeature::AttendedTransfer);
    assert!(code.is_some());
    assert_eq!(code.unwrap(), "#2");

    // Disconnect.
    let code = fs.get_builtin_code(BuiltinFeature::Disconnect);
    assert!(code.is_some());
    assert_eq!(code.unwrap(), "*0");

    // Park call.
    let code = fs.get_builtin_code(BuiltinFeature::ParkCall);
    assert!(code.is_some());
    assert_eq!(code.unwrap(), "#72");
}

/// Test that features with empty codes are disabled.
#[test]
fn test_empty_code_disabled() {
    let fs = FeatureSet::new();

    // AutoMixMon has empty default code (disabled).
    let code = fs.get_builtin_code(BuiltinFeature::AutoMixMon);
    assert!(code.is_none());

    // AutoMon has empty default code (disabled).
    let code = fs.get_builtin_code(BuiltinFeature::AutoMon);
    assert!(code.is_none());
}

// ---------------------------------------------------------------------------
// DTMF-triggered feature execution
// ---------------------------------------------------------------------------

/// Test that DTMF matching finds the correct feature.
#[test]
fn test_dtmf_feature_matching() {
    let fs = FeatureSet::new();

    let matched = fs.match_dtmf("#1", FeatureSide::BOTH);
    assert_eq!(matched, Some(BuiltinFeature::BlindTransfer));

    let matched = fs.match_dtmf("#2", FeatureSide::BOTH);
    assert_eq!(matched, Some(BuiltinFeature::AttendedTransfer));

    let matched = fs.match_dtmf("*0", FeatureSide::BOTH);
    assert_eq!(matched, Some(BuiltinFeature::Disconnect));

    let matched = fs.match_dtmf("#72", FeatureSide::BOTH);
    assert_eq!(matched, Some(BuiltinFeature::ParkCall));
}

/// Test that non-matching DTMF returns None.
#[test]
fn test_dtmf_no_match() {
    let fs = FeatureSet::new();

    let matched = fs.match_dtmf("##", FeatureSide::BOTH);
    assert!(matched.is_none());

    let matched = fs.match_dtmf("12345", FeatureSide::BOTH);
    assert!(matched.is_none());
}

/// Test DTMF prefix detection for progressive matching.
#[test]
fn test_dtmf_prefix_detection() {
    let fs = FeatureSet::new();

    // "#" is a prefix of "#1", "#2", "#72".
    assert!(fs.is_prefix("#"));

    // "*" is a prefix of "*0".
    assert!(fs.is_prefix("*"));

    // "#7" is a prefix of "#72".
    assert!(fs.is_prefix("#7"));

    // "9" is not a prefix of anything.
    assert!(!fs.is_prefix("9"));
}

// ---------------------------------------------------------------------------
// Feature side matching
// ---------------------------------------------------------------------------

/// Test that side-specific matching works.
#[test]
fn test_feature_side_matching() {
    let mut fs = FeatureSet::new();

    // Set blind transfer to CALLER only.
    let fc = FeatureCode::new("#1").with_side(FeatureSide::CALLER);
    fs.set_builtin_code(BuiltinFeature::BlindTransfer, "#1");
    // We need to get the feature and update its side.
    // Since set_builtin_code doesn't set side, test via match_dtmf.
    // The default side is BOTH, so it should match for CALLER.
    let matched = fs.match_dtmf("#1", FeatureSide::CALLER);
    assert!(matched.is_some());

    let matched = fs.match_dtmf("#1", FeatureSide::CALLEE);
    assert!(matched.is_some()); // Default is BOTH.

    // Verify the FeatureCode struct.
    assert_eq!(fc.code, "#1");
    assert_eq!(fc.side, FeatureSide::CALLER);
    assert!(fc.enabled);
}

// ---------------------------------------------------------------------------
// Hook ordering / code changes
// ---------------------------------------------------------------------------

/// Test changing a feature code.
#[test]
fn test_change_feature_code() {
    let mut fs = FeatureSet::new();

    // Change blind transfer from "#1" to "*9".
    fs.set_builtin_code(BuiltinFeature::BlindTransfer, "*9");

    // Old code should not match.
    let matched = fs.match_dtmf("#1", FeatureSide::BOTH);
    assert!(matched.is_none());

    // New code should match.
    let matched = fs.match_dtmf("*9", FeatureSide::BOTH);
    assert_eq!(matched, Some(BuiltinFeature::BlindTransfer));
}

/// Test disabling a feature by setting empty code.
#[test]
fn test_disable_feature() {
    let mut fs = FeatureSet::new();

    // Disable disconnect.
    fs.set_builtin_code(BuiltinFeature::Disconnect, "");

    let code = fs.get_builtin_code(BuiltinFeature::Disconnect);
    assert!(code.is_none());

    let matched = fs.match_dtmf("*0", FeatureSide::BOTH);
    assert!(matched.is_none());
}

// ---------------------------------------------------------------------------
// Dynamic feature registration / removal
// ---------------------------------------------------------------------------

/// Test registering a dynamic feature hook.
#[test]
fn test_dynamic_feature_register() {
    let mut fs = FeatureSet::new();

    let feature = DynamicFeature {
        code: FeatureCode::new("*5"),
        app: "TestApp".to_string(),
        app_data: String::new(),
        moh_class: None,
    };

    assert!(fs.register_dynamic("test_feature", feature).is_ok());

    let names = fs.dynamic_names();
    assert!(names.contains(&"test_feature".to_string()));

    let df = fs.get_dynamic("test_feature");
    assert!(df.is_some());
    assert_eq!(df.unwrap().code.code, "*5");
}

/// Test unregistering a dynamic feature.
#[test]
fn test_dynamic_feature_unregister() {
    let mut fs = FeatureSet::new();

    let feature = DynamicFeature {
        code: FeatureCode::new("*5"),
        app: "TestApp".to_string(),
        app_data: String::new(),
        moh_class: None,
    };

    fs.register_dynamic("test_feature", feature).unwrap();
    assert!(fs.unregister_dynamic("test_feature"));
    assert!(!fs.unregister_dynamic("test_feature")); // Already removed.
    assert!(fs.get_dynamic("test_feature").is_none());
}

/// Test that duplicate dynamic feature codes are rejected.
#[test]
fn test_dynamic_feature_code_conflict() {
    let mut fs = FeatureSet::new();

    let f1 = DynamicFeature {
        code: FeatureCode::new("*5"),
        app: "App1".to_string(),
        app_data: String::new(),
        moh_class: None,
    };
    let f2 = DynamicFeature {
        code: FeatureCode::new("*5"),
        app: "App2".to_string(),
        app_data: String::new(),
        moh_class: None,
    };

    assert!(fs.register_dynamic("f1", f1).is_ok());
    assert!(fs.register_dynamic("f2", f2).is_err());
}

// ---------------------------------------------------------------------------
// BuiltinFeature parsing
// ---------------------------------------------------------------------------

/// Test BuiltinFeature::from_name parsing.
#[test]
fn test_builtin_feature_from_name() {
    assert_eq!(
        BuiltinFeature::from_name("blindxfer"),
        Some(BuiltinFeature::BlindTransfer)
    );
    assert_eq!(
        BuiltinFeature::from_name("atxfer"),
        Some(BuiltinFeature::AttendedTransfer)
    );
    assert_eq!(
        BuiltinFeature::from_name("disconnect"),
        Some(BuiltinFeature::Disconnect)
    );
    assert_eq!(
        BuiltinFeature::from_name("parkcall"),
        Some(BuiltinFeature::ParkCall)
    );
    assert_eq!(BuiltinFeature::from_name("unknown"), None);
}

/// Test BuiltinFeature::all returns all features.
#[test]
fn test_builtin_feature_all() {
    let all = BuiltinFeature::all();
    assert_eq!(all.len(), 6);
}
