//! Port of asterisk/tests/test_core_format.c
//!
//! Tests for our Format/Codec types in asterisk-codecs:
//! - Format creation with codec
//! - Format comparison (same codec = compatible)
//! - Format with attributes
//! - Format clone/copy
//! - Format joint computation
//! - Format cap operations (see also test_format_cap.rs for FormatCap tests)

use asterisk_codecs::builtin_codecs::{
    CODEC_ALAW, CODEC_GSM, CODEC_ULAW, ID_ALAW, ID_SLIN8, ID_ULAW,
};
use asterisk_codecs::codec::Codec;
use asterisk_codecs::format::{Format, FormatCmp};
use asterisk_codecs::registry::CodecRegistry;
use asterisk_types::MediaType;
use std::sync::Arc;

/// Helper to get a registry with builtins.
fn registry() -> CodecRegistry {
    CodecRegistry::with_builtins()
}

// ---------------------------------------------------------------------------
// Format creation with codec
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_create) from test_core_format.c.
///
/// Test that creating a format from a codec produces a valid format
/// with the correct name, sample rate, and media type.
#[test]
fn test_format_create_from_codec() {
    let reg = registry();
    let fmt = reg.get_format(ID_ULAW).expect("ULAW format");

    assert_eq!(fmt.codec_name(), "ulaw");
    assert_eq!(fmt.sample_rate(), 8000);
    assert_eq!(fmt.media_type(), MediaType::Audio);
    assert_eq!(fmt.default_ms(), 20);
    assert!(fmt.can_be_smoothed());
}

/// Test format creation with ALAW codec.
#[test]
fn test_format_create_alaw() {
    let reg = registry();
    let fmt = reg.get_format(ID_ALAW).expect("ALAW format");

    assert_eq!(fmt.codec_name(), "alaw");
    assert_eq!(fmt.sample_rate(), 8000);
    assert_eq!(fmt.media_type(), MediaType::Audio);
}

/// Test format creation with SLIN codec.
#[test]
fn test_format_create_slin() {
    let reg = registry();
    let fmt = reg.get_format(ID_SLIN8).expect("SLIN format");

    assert_eq!(fmt.codec_name(), "slin");
    assert_eq!(fmt.sample_rate(), 8000);
    assert_eq!(fmt.media_type(), MediaType::Audio);
}

// ---------------------------------------------------------------------------
// Format comparison
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_cmp_same) from test_core_format.c.
///
/// Test that two formats with the same codec are Equal.
#[test]
fn test_format_compare_same_codec() {
    let reg = registry();
    let fmt1 = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt2 = reg.get_format(ID_ULAW).expect("ULAW");

    assert_eq!(fmt1.compare(&fmt2), FormatCmp::Equal);
    assert_eq!(*fmt1, *fmt2);
}

/// Port of AST_TEST_DEFINE(format_cmp_different) from test_core_format.c.
///
/// Test that two formats with different codecs are NotEqual.
#[test]
fn test_format_compare_different_codec() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    assert_eq!(fmt_ulaw.compare(&fmt_alaw), FormatCmp::NotEqual);
    assert_ne!(*fmt_ulaw, *fmt_alaw);
}

/// Port of AST_TEST_DEFINE(format_cmp_subset) from test_core_format.c.
///
/// Test that a format without attributes is a Subset of the same codec
/// with attributes.
#[test]
fn test_format_compare_subset() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let fmt_plain = Format::new(Arc::clone(&codec));
    let mut fmt_attrs = Format::new(Arc::clone(&codec));
    fmt_attrs.set_attribute("ptime", "20");

    // Plain (no attrs) vs with-attrs should be Subset.
    assert_eq!(fmt_plain.compare(&fmt_attrs), FormatCmp::Subset);
    assert_eq!(fmt_attrs.compare(&fmt_plain), FormatCmp::Subset);
}

// ---------------------------------------------------------------------------
// Format with attributes
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_attribute_set_get) from test_core_format.c.
///
/// Test setting and getting attributes on a format.
#[test]
fn test_format_attributes() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut fmt = Format::new(codec);

    assert!(fmt.get_attribute("fec").is_none());

    fmt.set_attribute("fec", "1");
    fmt.set_attribute("stereo", "0");
    fmt.set_attribute("cbr", "32000");

    assert_eq!(fmt.get_attribute("fec"), Some("1"));
    assert_eq!(fmt.get_attribute("stereo"), Some("0"));
    assert_eq!(fmt.get_attribute("cbr"), Some("32000"));

    // Non-existent attribute.
    assert!(fmt.get_attribute("dtx").is_none());
}

/// Test that two formats with the same codec but different attributes are NotEqual.
#[test]
fn test_format_different_attributes_not_equal() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut fmt1 = Format::new(Arc::clone(&codec));
    let mut fmt2 = Format::new(Arc::clone(&codec));

    fmt1.set_attribute("fec", "1");
    fmt2.set_attribute("fec", "0");

    assert_eq!(fmt1.compare(&fmt2), FormatCmp::NotEqual);
}

/// Test that two formats with the same codec and same attributes are Equal.
#[test]
fn test_format_same_attributes_equal() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut fmt1 = Format::new(Arc::clone(&codec));
    let mut fmt2 = Format::new(Arc::clone(&codec));

    fmt1.set_attribute("fec", "1");
    fmt2.set_attribute("fec", "1");

    assert_eq!(fmt1.compare(&fmt2), FormatCmp::Equal);
}

// ---------------------------------------------------------------------------
// Format clone/copy
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_clone) from test_core_format.c.
///
/// Test that cloning a format produces an independent copy.
#[test]
fn test_format_clone() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut original = Format::new(codec);
    original.set_attribute("ptime", "20");

    let cloned = original.clone_format();

    // Cloned should be equal.
    assert_eq!(original.compare(&cloned), FormatCmp::Equal);
    assert_eq!(cloned.get_attribute("ptime"), Some("20"));

    // Modifying the original should not affect the clone.
    original.set_attribute("ptime", "30");
    assert_eq!(cloned.get_attribute("ptime"), Some("20"));
    assert_eq!(original.get_attribute("ptime"), Some("30"));
}

/// Test that Clone trait works correctly.
#[test]
fn test_format_clone_trait() {
    let codec = Arc::new(CODEC_ALAW.clone());
    let original = Format::new(codec);
    let cloned = original.clone();

    assert_eq!(original.codec_name(), cloned.codec_name());
    assert_eq!(original.sample_rate(), cloned.sample_rate());
}

// ---------------------------------------------------------------------------
// Format joint computation
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_joint_same_codec) from test_core_format.c.
///
/// Test that the joint of two formats with the same codec succeeds.
#[test]
fn test_format_joint_same_codec() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let fmt1 = Format::new(Arc::clone(&codec));
    let fmt2 = Format::new(Arc::clone(&codec));

    let joint = fmt1.joint(&fmt2);
    assert!(joint.is_some());
    let joint = joint.unwrap();
    assert_eq!(joint.codec_name(), "ulaw");
}

/// Port of AST_TEST_DEFINE(format_joint_different_codec) from test_core_format.c.
///
/// Test that the joint of two formats with different codecs returns None.
#[test]
fn test_format_joint_different_codec() {
    let codec_ulaw = Arc::new(CODEC_ULAW.clone());
    let codec_alaw = Arc::new(CODEC_ALAW.clone());
    let fmt1 = Format::new(codec_ulaw);
    let fmt2 = Format::new(codec_alaw);

    let joint = fmt1.joint(&fmt2);
    assert!(joint.is_none());
}

/// Test that the joint preserves common attributes.
#[test]
fn test_format_joint_with_attributes() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut fmt1 = Format::new(Arc::clone(&codec));
    let mut fmt2 = Format::new(Arc::clone(&codec));

    fmt1.set_attribute("fec", "1");
    fmt1.set_attribute("stereo", "0");

    fmt2.set_attribute("fec", "1");
    fmt2.set_attribute("cbr", "32000");

    let joint = fmt1.joint(&fmt2).unwrap();
    // Only "fec" is common with same value.
    assert_eq!(joint.get_attribute("fec"), Some("1"));
    assert!(joint.get_attribute("stereo").is_none());
    assert!(joint.get_attribute("cbr").is_none());
}

/// Test joint with no common attributes.
#[test]
fn test_format_joint_disjoint_attributes() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let mut fmt1 = Format::new(Arc::clone(&codec));
    let mut fmt2 = Format::new(Arc::clone(&codec));

    fmt1.set_attribute("fec", "1");
    fmt2.set_attribute("fec", "0");

    let joint = fmt1.joint(&fmt2).unwrap();
    // fec values differ, so no attributes in joint.
    assert!(joint.get_attribute("fec").is_none());
}

// ---------------------------------------------------------------------------
// Format Display/Debug
// ---------------------------------------------------------------------------

/// Test format Display implementation.
#[test]
fn test_format_display() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let fmt = Format::new(codec);
    let display = format!("{}", fmt);
    assert_eq!(display, "ulaw");
}

/// Test format Debug implementation.
#[test]
fn test_format_debug() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let fmt = Format::new(codec);
    let debug = format!("{:?}", fmt);
    assert!(debug.contains("ulaw"));
    assert!(debug.contains("Format"));
}

// ---------------------------------------------------------------------------
// Format named constructor
// ---------------------------------------------------------------------------

/// Test Format::new_named creates a format with custom name.
#[test]
fn test_format_new_named() {
    let codec = Arc::new(CODEC_ULAW.clone());
    let fmt = Format::new_named("my-ulaw", codec);
    assert_eq!(fmt.name, "my-ulaw");
    assert_eq!(fmt.codec_name(), "ulaw");
}

// ---------------------------------------------------------------------------
// Codec utility methods
// ---------------------------------------------------------------------------

/// Test codec samples_for_bytes calculation.
#[test]
fn test_codec_samples_for_bytes() {
    // ULAW: 8000Hz, 10ms minimum, 80 bytes minimum
    // 80 bytes = 80 samples (1 byte per sample for ulaw)
    let samples = CODEC_ULAW.samples_for_bytes(160);
    assert_eq!(samples, 160); // 160 bytes = 160 samples
}

/// Test codec length_for_samples calculation.
#[test]
fn test_codec_length_for_samples() {
    // 160 samples at 8000Hz = 20ms
    let length = CODEC_ULAW.length_for_samples(160);
    assert_eq!(length, 20);
}

/// Test codec bytes_for_samples calculation.
#[test]
fn test_codec_bytes_for_samples() {
    let bytes = CODEC_ULAW.bytes_for_samples(160);
    assert_eq!(bytes, 160); // 1 byte per sample for ulaw
}
