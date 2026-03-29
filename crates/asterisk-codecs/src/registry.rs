//! Global codec registry.
//!
//! Provides a thread-safe global registry for codecs and formats,
//! analogous to the codec registration system in Asterisk's core.

use crate::builtin_codecs;
use crate::codec::{Codec, CodecId};
use crate::format::Format;
use crate::translate::TranslationMatrix;
use crate::builtin_translators;
use asterisk_types::MediaType;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Global codec and format registry.
///
/// Provides lookup by ID, by name/type/sample_rate, and maintains
/// the translation matrix for converting between codecs.
pub struct CodecRegistry {
    /// Codecs indexed by ID.
    codecs_by_id: HashMap<CodecId, Arc<Codec>>,
    /// Codecs indexed by (name, media_type, sample_rate).
    codecs_by_key: HashMap<(String, MediaType, u32), Arc<Codec>>,
    /// Cached default formats (one per codec).
    formats: HashMap<CodecId, Arc<Format>>,
    /// Translation matrix.
    translation_matrix: TranslationMatrix,
    /// Next codec ID for dynamic registration.
    next_id: u32,
}

impl CodecRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            codecs_by_id: HashMap::new(),
            codecs_by_key: HashMap::new(),
            formats: HashMap::new(),
            translation_matrix: TranslationMatrix::new(),
            next_id: 100, // Reserve 1-99 for built-in codecs
        }
    }

    /// Create a registry pre-populated with all built-in codecs and translators.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register all built-in codecs and translators.
    pub fn register_builtins(&mut self) {
        for codec in builtin_codecs::all_builtin_codecs() {
            self.register_codec(codec.clone());
        }
        builtin_translators::register_builtin_translators(&mut self.translation_matrix);
    }

    /// Register a codec.
    pub fn register_codec(&mut self, codec: Codec) {
        let codec = Arc::new(codec);
        let id = codec.id;
        let key = (
            codec.name.to_string(),
            codec.media_type,
            codec.sample_rate,
        );
        // Create a default format for this codec
        let format_name = if codec.name == "slin" && codec.sample_rate != 8000 {
            format!("slin{}", codec.sample_rate / 1000)
        } else {
            codec.name.to_string()
        };
        let format = Arc::new(Format::new_named(format_name, Arc::clone(&codec)));
        self.codecs_by_id.insert(id, Arc::clone(&codec));
        self.codecs_by_key.insert(key, codec);
        self.formats.insert(id, format);
    }

    /// Look up a codec by ID.
    pub fn get_codec(&self, id: CodecId) -> Option<Arc<Codec>> {
        self.codecs_by_id.get(&id).cloned()
    }

    /// Look up a codec by name, type, and sample rate.
    pub fn get_codec_by_name(
        &self,
        name: &str,
        media_type: MediaType,
        sample_rate: u32,
    ) -> Option<Arc<Codec>> {
        self.codecs_by_key
            .get(&(name.to_string(), media_type, sample_rate))
            .cloned()
    }

    /// Get the default format for a codec.
    pub fn get_format(&self, codec_id: CodecId) -> Option<Arc<Format>> {
        self.formats.get(&codec_id).cloned()
    }

    /// Get a reference to the translation matrix.
    pub fn translation_matrix(&self) -> &TranslationMatrix {
        &self.translation_matrix
    }

    /// Get a mutable reference to the translation matrix.
    pub fn translation_matrix_mut(&mut self) -> &mut TranslationMatrix {
        &mut self.translation_matrix
    }

    /// Get the maximum codec ID currently registered.
    pub fn max_codec_id(&self) -> CodecId {
        self.codecs_by_id.keys().copied().max().unwrap_or(0)
    }

    /// Get the total number of registered codecs.
    pub fn codec_count(&self) -> usize {
        self.codecs_by_id.len()
    }

    /// Iterate over all registered codecs.
    pub fn iter_codecs(&self) -> impl Iterator<Item = &Arc<Codec>> {
        self.codecs_by_id.values()
    }

    /// Allocate a new codec ID for dynamic registration.
    pub fn allocate_id(&mut self) -> CodecId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Default for CodecRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Thread-safe global codec registry singleton.
///
/// Use `global_registry()` to access.
static GLOBAL_REGISTRY: once_cell::sync::Lazy<RwLock<CodecRegistry>> =
    once_cell::sync::Lazy::new(|| RwLock::new(CodecRegistry::with_builtins()));

/// Get a read lock on the global codec registry.
pub fn global_registry() -> parking_lot::RwLockReadGuard<'static, CodecRegistry> {
    GLOBAL_REGISTRY.read()
}

/// Get a write lock on the global codec registry (for registration).
pub fn global_registry_mut() -> parking_lot::RwLockWriteGuard<'static, CodecRegistry> {
    GLOBAL_REGISTRY.write()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_codecs;

    #[test]
    fn test_builtin_codecs_registered() {
        let registry = CodecRegistry::with_builtins();
        assert!(registry.codec_count() > 0);

        // Check specific codecs exist
        let ulaw = registry.get_codec(builtin_codecs::ID_ULAW);
        assert!(ulaw.is_some());
        let ulaw = ulaw.unwrap();
        assert_eq!(ulaw.name, "ulaw");
        assert_eq!(ulaw.sample_rate, 8000);
        assert_eq!(ulaw.quality, 100);
    }

    #[test]
    fn test_codec_lookup_by_name() {
        let registry = CodecRegistry::with_builtins();
        let codec = registry.get_codec_by_name("ulaw", MediaType::Audio, 8000);
        assert!(codec.is_some());
        assert_eq!(codec.unwrap().id, builtin_codecs::ID_ULAW);
    }

    #[test]
    fn test_translation_path() {
        let registry = CodecRegistry::with_builtins();
        let matrix = registry.translation_matrix();

        // ulaw -> slin should be a single step
        let path = matrix.build_path(builtin_codecs::ID_ULAW, builtin_codecs::ID_SLIN8);
        assert!(path.is_ok());
        let path = path.unwrap();
        assert_eq!(path.steps.len(), 1);

        // ulaw -> alaw should be two steps (ulaw -> slin -> alaw)
        let path = matrix.build_path(builtin_codecs::ID_ULAW, builtin_codecs::ID_ALAW);
        assert!(path.is_ok());
        let path = path.unwrap();
        assert_eq!(path.steps.len(), 2);
    }

    #[test]
    fn test_format_lookup() {
        let registry = CodecRegistry::with_builtins();
        let format = registry.get_format(builtin_codecs::ID_ULAW);
        assert!(format.is_some());
        assert_eq!(format.unwrap().name, "ulaw");
    }
}
