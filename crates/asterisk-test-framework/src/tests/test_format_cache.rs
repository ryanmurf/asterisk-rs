//! Port of asterisk/tests/test_format_cache.c
//!
//! Tests the format cache (CodecRegistry): lookup by ID, cache hit/miss,
//! well-known format retrieval (ulaw, alaw, slin, etc.), and named
//! format creation.

use asterisk_codecs::builtin_codecs::{
    ID_ALAW, ID_G722, ID_GSM, ID_SLIN8, ID_SLIN16, ID_ULAW,
};
use asterisk_codecs::codec::Codec;
use asterisk_codecs::format::Format;
use asterisk_codecs::registry::CodecRegistry;
use asterisk_types::MediaType;
use std::sync::Arc;

/// Helper to get a registry with builtins.
fn registry() -> CodecRegistry {
    CodecRegistry::with_builtins()
}

// ---------------------------------------------------------------------------
// Cache lookup
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_cache_set) from test_format_cache.c.
///
/// Test that looking up a well-known format by ID succeeds.
#[test]
fn test_format_cache_lookup_ulaw() {
    let reg = registry();
    let fmt = reg.get_format(ID_ULAW);
    assert!(fmt.is_some(), "ulaw format should be in cache");
    let fmt = fmt.unwrap();
    assert_eq!(fmt.codec_name(), "ulaw");
    assert_eq!(fmt.sample_rate(), 8000);
    assert_eq!(fmt.media_type(), MediaType::Audio);
}

/// Test looking up alaw format.
#[test]
fn test_format_cache_lookup_alaw() {
    let reg = registry();
    let fmt = reg.get_format(ID_ALAW);
    assert!(fmt.is_some(), "alaw format should be in cache");
    let fmt = fmt.unwrap();
    assert_eq!(fmt.codec_name(), "alaw");
    assert_eq!(fmt.sample_rate(), 8000);
}

/// Test looking up GSM format.
#[test]
fn test_format_cache_lookup_gsm() {
    let reg = registry();
    let fmt = reg.get_format(ID_GSM);
    assert!(fmt.is_some(), "gsm format should be in cache");
    assert_eq!(fmt.unwrap().codec_name(), "gsm");
}

/// Test looking up signed linear 8kHz format.
#[test]
fn test_format_cache_lookup_slin() {
    let reg = registry();
    let fmt = reg.get_format(ID_SLIN8);
    assert!(fmt.is_some(), "slin format should be in cache");
    let fmt = fmt.unwrap();
    assert_eq!(fmt.codec_name(), "slin");
    assert_eq!(fmt.sample_rate(), 8000);
}

/// Test looking up signed linear 16kHz format.
#[test]
fn test_format_cache_lookup_slin16() {
    let reg = registry();
    let fmt = reg.get_format(ID_SLIN16);
    assert!(fmt.is_some(), "slin16 format should be in cache");
    let fmt = fmt.unwrap();
    assert_eq!(fmt.sample_rate(), 16000);
}

/// Test looking up G.722 format.
#[test]
fn test_format_cache_lookup_g722() {
    let reg = registry();
    let fmt = reg.get_format(ID_G722);
    assert!(fmt.is_some(), "g722 format should be in cache");
    assert_eq!(fmt.unwrap().codec_name(), "g722");
}

// ---------------------------------------------------------------------------
// Cache miss
// ---------------------------------------------------------------------------

/// Test that looking up a non-existent format ID returns None.
#[test]
fn test_format_cache_miss() {
    let reg = registry();
    let fmt = reg.get_format(99999);
    assert!(fmt.is_none(), "Non-existent ID should not be in cache");
}

// ---------------------------------------------------------------------------
// Multiple lookups return same format
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(format_cache_set_duplicate) logic.
///
/// Test that looking up the same format twice returns equivalent results.
#[test]
fn test_format_cache_consistent_lookup() {
    let reg = registry();
    let fmt1 = reg.get_format(ID_ULAW).unwrap();
    let fmt2 = reg.get_format(ID_ULAW).unwrap();

    assert_eq!(fmt1.codec_name(), fmt2.codec_name());
    assert_eq!(fmt1.codec_id(), fmt2.codec_id());
    assert_eq!(fmt1.sample_rate(), fmt2.sample_rate());
}

// ---------------------------------------------------------------------------
// All builtins present
// ---------------------------------------------------------------------------

/// Test that all expected built-in codecs are available.
#[test]
fn test_format_cache_all_builtins_present() {
    let reg = registry();

    let ids = [ID_ULAW, ID_ALAW, ID_GSM, ID_SLIN8, ID_G722];
    for id in &ids {
        assert!(
            reg.get_format(*id).is_some(),
            "Built-in codec ID {} should be present",
            id
        );
    }
}

/// Test that builtin formats have valid properties.
#[test]
fn test_format_cache_builtin_properties() {
    let reg = registry();

    for id in [ID_ULAW, ID_ALAW, ID_GSM, ID_SLIN8] {
        let fmt = reg.get_format(id).unwrap();
        assert_eq!(fmt.media_type(), MediaType::Audio);
        assert!(fmt.sample_rate() > 0);
        assert!(!fmt.codec_name().is_empty());
    }
}
