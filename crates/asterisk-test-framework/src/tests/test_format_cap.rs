//! Port of asterisk/tests/test_format_cap.c
//!
//! Tests FormatCap operations: creation, add, compatibility,
//! joint capability, best by type, format removal, and empty cap.

use asterisk_codecs::builtin_codecs::{ID_ALAW, ID_SLIN8, ID_ULAW};
use asterisk_codecs::format_cap::FormatCap;
use asterisk_codecs::registry::CodecRegistry;
use asterisk_types::MediaType;
use std::sync::Arc;

/// Helper to get a registry with builtins.
fn registry() -> CodecRegistry {
    CodecRegistry::with_builtins()
}

/// Port of AST_TEST_DEFINE(format_cap_alloc) from test_format_cap.c.
///
/// Test that allocation of a format capabilities structure succeeds.
#[test]
fn test_format_cap_alloc() {
    let cap = FormatCap::new();
    assert_eq!(cap.count(), 0);
    assert!(cap.is_empty());
}

/// Port of AST_TEST_DEFINE(format_cap_append_single) from test_format_cap.c.
///
/// Test that adding a single format to a format capabilities structure succeeds.
#[test]
fn test_format_cap_append_single() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW format");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 42);

    assert_eq!(cap.count(), 1);
    assert!(!cap.is_empty());

    // Retrieve and verify
    let retrieved = cap.get_format(0).expect("Should have format at index 0");
    assert_eq!(retrieved.codec_name(), fmt_ulaw.codec_name());

    // Check framing
    let framing = cap.get_format_framing(&fmt_ulaw);
    assert_eq!(framing, 42);
}

/// Port of AST_TEST_DEFINE(format_cap_append_multiple) from test_format_cap.c.
///
/// Test that adding multiple formats works correctly.
#[test]
fn test_format_cap_append_multiple() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW format");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW format");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 42);
    cap.add(Arc::clone(&fmt_alaw), 84);

    assert_eq!(cap.count(), 2);

    // First format should be ulaw (preference order)
    let first = cap.get_format(0).unwrap();
    assert_eq!(first.codec_name(), "ulaw");

    // Second format should be alaw
    let second = cap.get_format(1).unwrap();
    assert_eq!(second.codec_name(), "alaw");
}

/// Port of format compatibility checking from test_format_cap.c.
///
/// Test that two format caps with common codecs are compatible.
#[test]
fn test_format_cap_compatibility() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");
    let fmt_slin = reg.get_format(ID_SLIN8).expect("SLIN");

    // Cap1: ulaw + alaw
    let mut cap1 = FormatCap::new();
    cap1.add(Arc::clone(&fmt_ulaw), 0);
    cap1.add(Arc::clone(&fmt_alaw), 0);

    // Cap2: alaw + slin
    let mut cap2 = FormatCap::new();
    cap2.add(Arc::clone(&fmt_alaw), 0);
    cap2.add(Arc::clone(&fmt_slin), 0);

    // They share alaw, so they should be compatible
    assert!(cap1.is_compatible(&cap2));
    assert!(cap2.is_compatible(&cap1));

    // Cap3: slin only
    let mut cap3 = FormatCap::new();
    cap3.add(Arc::clone(&fmt_slin), 0);

    // Cap1 (ulaw + alaw) vs Cap3 (slin) -- no common codec
    assert!(!cap1.is_compatible(&cap3));
}

/// Port of joint capability computation from test_format_cap.c.
///
/// Test that getting joint capabilities produces the intersection.
#[test]
fn test_format_cap_joint() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");
    let fmt_slin = reg.get_format(ID_SLIN8).expect("SLIN");

    let mut cap1 = FormatCap::new();
    cap1.add(Arc::clone(&fmt_ulaw), 0);
    cap1.add(Arc::clone(&fmt_alaw), 0);

    let mut cap2 = FormatCap::new();
    cap2.add(Arc::clone(&fmt_alaw), 0);
    cap2.add(Arc::clone(&fmt_slin), 0);

    let joint = cap1.get_joint(&cap2);
    assert_eq!(joint.count(), 1);
    assert_eq!(joint.get_format(0).unwrap().codec_name(), "alaw");
}

/// Test joint capabilities with no overlap.
#[test]
fn test_format_cap_joint_empty() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_slin = reg.get_format(ID_SLIN8).expect("SLIN");

    let mut cap1 = FormatCap::new();
    cap1.add(Arc::clone(&fmt_ulaw), 0);

    let mut cap2 = FormatCap::new();
    cap2.add(Arc::clone(&fmt_slin), 0);

    let joint = cap1.get_joint(&cap2);
    assert_eq!(joint.count(), 0);
    assert!(joint.is_empty());
}

/// Port of best format by media type from test_format_cap.c.
///
/// Test that best_by_type returns the first added format of that type.
#[test]
fn test_format_cap_best_by_type() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 0);
    cap.add(Arc::clone(&fmt_alaw), 0);

    // Best audio format should be ulaw (first added = most preferred)
    let best = cap.best_by_type(MediaType::Audio);
    assert!(best.is_some());
    assert_eq!(best.unwrap().codec_name(), "ulaw");

    // No video formats
    let best_video = cap.best_by_type(MediaType::Video);
    assert!(best_video.is_none());
}

/// Port of format removal from test_format_cap.c.
///
/// Test that removing a format works correctly.
#[test]
fn test_format_cap_remove() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 0);
    cap.add(Arc::clone(&fmt_alaw), 0);
    assert_eq!(cap.count(), 2);

    // Remove ulaw
    let removed = cap.remove(&fmt_ulaw);
    assert!(removed);
    assert_eq!(cap.count(), 1);
    assert_eq!(cap.get_format(0).unwrap().codec_name(), "alaw");

    // Try removing ulaw again -- should return false
    let removed_again = cap.remove(&fmt_ulaw);
    assert!(!removed_again);
}

/// Port of remove_by_type from test_format_cap.c.
#[test]
fn test_format_cap_remove_by_type() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 0);
    cap.add(Arc::clone(&fmt_alaw), 0);

    // Remove all audio
    cap.remove_by_type(MediaType::Audio);
    assert_eq!(cap.count(), 0);
    assert!(cap.is_empty());
}

/// Test empty cap handling.
#[test]
fn test_format_cap_empty() {
    let cap = FormatCap::new();

    assert_eq!(cap.count(), 0);
    assert!(cap.is_empty());
    assert!(cap.get_format(0).is_none());
    assert!(cap.best_by_type(MediaType::Audio).is_none());
    assert!(!cap.has_type(MediaType::Audio));

    // Joint of two empty caps should be empty
    let other = FormatCap::new();
    let joint = cap.get_joint(&other);
    assert!(joint.is_empty());
}

/// Test format cap names output.
#[test]
fn test_format_cap_names() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut cap = FormatCap::new();
    cap.add(Arc::clone(&fmt_ulaw), 0);
    cap.add(Arc::clone(&fmt_alaw), 0);

    let names = cap.get_names();
    assert!(names.contains("ulaw"));
    assert!(names.contains("alaw"));
}

/// Test identical caps comparison.
#[test]
fn test_format_cap_identical() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut cap1 = FormatCap::new();
    cap1.add(Arc::clone(&fmt_ulaw), 0);
    cap1.add(Arc::clone(&fmt_alaw), 0);

    let mut cap2 = FormatCap::new();
    cap2.add(Arc::clone(&fmt_ulaw), 0);
    cap2.add(Arc::clone(&fmt_alaw), 0);

    assert!(cap1.is_identical(&cap2));

    // Different order is NOT identical (preference matters)
    let mut cap3 = FormatCap::new();
    cap3.add(Arc::clone(&fmt_alaw), 0);
    cap3.add(Arc::clone(&fmt_ulaw), 0);

    assert!(!cap1.is_identical(&cap3));
}

/// Test append_from (copying formats from another cap).
#[test]
fn test_format_cap_append_from() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");
    let fmt_alaw = reg.get_format(ID_ALAW).expect("ALAW");

    let mut src = FormatCap::new();
    src.add(Arc::clone(&fmt_ulaw), 0);
    src.add(Arc::clone(&fmt_alaw), 0);

    let mut dst = FormatCap::new();
    dst.append_from(&src, MediaType::Audio);
    assert_eq!(dst.count(), 2);
}

/// Test has_type check.
#[test]
fn test_format_cap_has_type() {
    let reg = registry();
    let fmt_ulaw = reg.get_format(ID_ULAW).expect("ULAW");

    let mut cap = FormatCap::new();
    assert!(!cap.has_type(MediaType::Audio));

    cap.add(Arc::clone(&fmt_ulaw), 0);
    assert!(cap.has_type(MediaType::Audio));
    assert!(!cap.has_type(MediaType::Video));
}
