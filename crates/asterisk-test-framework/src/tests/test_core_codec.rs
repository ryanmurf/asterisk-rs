//! Port of asterisk/tests/test_core_codec.c
//!
//! Tests core codec API:
//!
//! - Registering a codec succeeds
//! - Double registration fails
//! - Registering an unknown media type fails
//! - Registering audio codec without sample rate fails
//! - Getting a registered codec by name succeeds
//! - Getting an unregistered codec fails
//! - Getting a codec by name with unknown type succeeds (wildcard match)
//! - Getting a codec by ID succeeds
//!
//! Since we do not have the Asterisk codec registry, we model it
//! with a local HashMap-based registry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Codec model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaType {
    Audio,
    Video,
    Text,
    Image,
    Unknown,
}

#[derive(Debug, Clone)]
struct Codec {
    id: u32,
    name: String,
    description: String,
    media_type: MediaType,
    sample_rate: u32,
    minimum_ms: u32,
    maximum_ms: u32,
    default_ms: u32,
}

static NEXT_CODEC_ID: AtomicU32 = AtomicU32::new(1);

struct CodecRegistry {
    codecs: HashMap<String, Codec>,
    by_id: HashMap<u32, String>,
}

impl CodecRegistry {
    fn new() -> Self {
        Self {
            codecs: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    /// Register a codec. Returns Err if:
    /// - Media type is Unknown
    /// - Audio codec has no sample rate
    /// - Codec is already registered
    fn register(&mut self, mut codec: Codec) -> Result<(), String> {
        if codec.media_type == MediaType::Unknown {
            return Err("Cannot register codec with unknown media type".to_string());
        }
        if codec.media_type == MediaType::Audio && codec.sample_rate == 0 {
            return Err("Audio codec must have a sample rate".to_string());
        }
        if self.codecs.contains_key(&codec.name) {
            return Err(format!("Codec '{}' is already registered", codec.name));
        }
        let id = NEXT_CODEC_ID.fetch_add(1, Ordering::SeqCst);
        codec.id = id;
        self.by_id.insert(id, codec.name.clone());
        self.codecs.insert(codec.name.clone(), codec);
        Ok(())
    }

    /// Get a codec by name, media type, and sample rate.
    /// If media_type is Unknown, match any type.
    fn get(&self, name: &str, media_type: MediaType, sample_rate: u32) -> Option<&Codec> {
        let codec = self.codecs.get(name)?;
        if media_type != MediaType::Unknown && codec.media_type != media_type {
            return None;
        }
        if sample_rate != 0 && codec.sample_rate != sample_rate {
            return None;
        }
        Some(codec)
    }

    /// Get a codec by its assigned ID.
    fn get_by_id(&self, id: u32) -> Option<&Codec> {
        let name = self.by_id.get(&id)?;
        self.codecs.get(name)
    }
}

fn make_audio_codec(name: &str, sample_rate: u32) -> Codec {
    Codec {
        id: 0,
        name: name.to_string(),
        description: "Unit test codec".to_string(),
        media_type: MediaType::Audio,
        sample_rate,
        minimum_ms: 10,
        maximum_ms: 150,
        default_ms: 20,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(codec_register).
#[test]
fn test_codec_register() {
    let mut reg = CodecRegistry::new();
    let codec = make_audio_codec("unit_test", 8000);
    assert!(reg.register(codec).is_ok());
}

/// Port of AST_TEST_DEFINE(codec_register_twice).
#[test]
fn test_codec_register_twice() {
    let mut reg = CodecRegistry::new();
    let codec1 = make_audio_codec("unit_test_double", 8000);
    let codec2 = make_audio_codec("unit_test_double", 8000);
    assert!(reg.register(codec1).is_ok());
    assert!(reg.register(codec2).is_err(), "Double registration should fail");
}

/// Port of AST_TEST_DEFINE(codec_register_unknown).
#[test]
fn test_codec_register_unknown() {
    let mut reg = CodecRegistry::new();
    let codec = Codec {
        id: 0,
        name: "unit_test_unknown".to_string(),
        description: "Unit test codec".to_string(),
        media_type: MediaType::Unknown,
        sample_rate: 8000,
        minimum_ms: 10,
        maximum_ms: 150,
        default_ms: 20,
    };
    assert!(reg.register(codec).is_err(), "Unknown media type should fail");
}

/// Port of AST_TEST_DEFINE(codec_register_audio_no_sample_rate).
#[test]
fn test_codec_register_audio_no_sample_rate() {
    let mut reg = CodecRegistry::new();
    let codec = make_audio_codec("unit_test_no_rate", 0);
    assert!(
        reg.register(codec).is_err(),
        "Audio codec without sample rate should fail"
    );
}

/// Port of AST_TEST_DEFINE(codec_get).
#[test]
fn test_codec_get() {
    let mut reg = CodecRegistry::new();
    let codec = make_audio_codec("unit_test_audio_get", 8000);
    reg.register(codec).unwrap();

    let found = reg.get("unit_test_audio_get", MediaType::Audio, 8000);
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "unit_test_audio_get");
    assert_eq!(found.media_type, MediaType::Audio);
    assert_eq!(found.sample_rate, 8000);
}

/// Port of AST_TEST_DEFINE(codec_get_unregistered).
#[test]
fn test_codec_get_unregistered() {
    let reg = CodecRegistry::new();
    let found = reg.get("goats", MediaType::Audio, 8000);
    assert!(found.is_none());
}

/// Port of AST_TEST_DEFINE(codec_get_unknown).
///
/// Getting a codec by name with Unknown type should match any media type.
#[test]
fn test_codec_get_unknown() {
    let mut reg = CodecRegistry::new();
    let codec = make_audio_codec("unit_test_audio_get_unknown", 8000);
    reg.register(codec).unwrap();

    let found = reg.get("unit_test_audio_get_unknown", MediaType::Unknown, 8000);
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "unit_test_audio_get_unknown");
}

/// Port of AST_TEST_DEFINE(codec_get_id).
///
/// Register a codec, get it by name, then get it again by its ID.
#[test]
fn test_codec_get_id() {
    let mut reg = CodecRegistry::new();
    let codec = make_audio_codec("unit_test_audio_get_id", 8000);
    reg.register(codec).unwrap();

    let named = reg.get("unit_test_audio_get_id", MediaType::Audio, 8000).unwrap();
    let id = named.id;

    let by_id = reg.get_by_id(id);
    assert!(by_id.is_some());
    assert_eq!(by_id.unwrap().name, "unit_test_audio_get_id");
}
