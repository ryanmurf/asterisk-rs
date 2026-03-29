//! Translation framework for converting between codecs.
//!
//! Port of translate.h / translate.c - provides Dijkstra-based shortest path
//! computation between codecs and a framework for chaining translators.

use crate::codec::CodecId;
use asterisk_types::Frame;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::cmp::Ordering;
use std::sync::Arc;
use thiserror::Error;

/// Translation cost table values, mirroring `ast_trans_cost_table` from translate.h.
pub struct TransCost;

impl TransCost {
    pub const LL_LL_ORIGSAMP: u32 = 400_000;
    pub const LL_LY_ORIGSAMP: u32 = 600_000;
    pub const LL_LL_UPSAMP: u32 = 800_000;
    pub const LL_LY_UPSAMP: u32 = 825_000;
    pub const LL_LL_DOWNSAMP: u32 = 850_000;
    pub const LL_LY_DOWNSAMP: u32 = 875_000;
    pub const LL_UNKNOWN: u32 = 885_000;
    pub const LY_LL_ORIGSAMP: u32 = 900_000;
    pub const LY_LY_ORIGSAMP: u32 = 915_000;
    pub const LY_LL_UPSAMP: u32 = 930_000;
    pub const LY_LY_UPSAMP: u32 = 945_000;
    pub const LY_LL_DOWNSAMP: u32 = 960_000;
    pub const LY_LY_DOWNSAMP: u32 = 975_000;
    pub const LY_UNKNOWN: u32 = 985_000;
}

/// Errors that can occur during translation.
#[derive(Error, Debug)]
pub enum TranslateError {
    #[error("no translation path from codec {src} to codec {dst}")]
    NoPath { src: CodecId, dst: CodecId },
    #[error("translation failed: {0}")]
    Failed(String),
    #[error("translator not found: {0}")]
    NotFound(String),
}

/// Trait for a translator that can convert between two codecs.
pub trait Translator: Send + Sync {
    /// Human-readable name.
    fn name(&self) -> &str;
    /// Source codec ID.
    fn src_codec_id(&self) -> CodecId;
    /// Destination codec ID.
    fn dst_codec_id(&self) -> CodecId;
    /// Cost of this translation (lower = preferred).
    fn table_cost(&self) -> u32;
    /// Create a new instance for a single translation session.
    fn new_instance(&self) -> Box<dyn TranslatorInstance>;
}

/// Trait for an active translation instance (per-call/per-stream).
pub trait TranslatorInstance: Send {
    /// Feed an input frame.
    fn frame_in(&mut self, frame: &Frame) -> Result<(), TranslateError>;
    /// Retrieve the next output frame, if available.
    fn frame_out(&mut self) -> Option<Frame>;
    /// Clean up resources.
    fn destroy(&mut self) {}
}

#[derive(Eq, PartialEq)]
struct DijkstraNode {
    codec_id: CodecId,
    cost: u32,
}

impl Ord for DijkstraNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.cmp(&self.cost)
    }
}

impl PartialOrd for DijkstraNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone)]
struct TranslationEdge {
    dst_codec_id: CodecId,
    cost: u32,
    translator_index: usize,
}

/// Translation matrix: holds all registered translators and computes
/// shortest paths between codecs using Dijkstra's algorithm.
pub struct TranslationMatrix {
    translators: Vec<Arc<dyn Translator>>,
    edges: HashMap<CodecId, Vec<TranslationEdge>>,
}

impl TranslationMatrix {
    pub fn new() -> Self {
        Self {
            translators: Vec::new(),
            edges: HashMap::new(),
        }
    }

    /// Register a translator.
    pub fn register(&mut self, translator: Arc<dyn Translator>) {
        let index = self.translators.len();
        let src = translator.src_codec_id();
        let dst = translator.dst_codec_id();
        let cost = translator.table_cost();
        self.edges.entry(src).or_default().push(TranslationEdge {
            dst_codec_id: dst,
            cost,
            translator_index: index,
        });
        self.translators.push(translator);
    }

    /// Compute shortest translation path from `src` to `dst`.
    pub fn build_path(&self, src: CodecId, dst: CodecId) -> Result<TranslationPath, TranslateError> {
        if src == dst {
            return Ok(TranslationPath { steps: Vec::new(), total_cost: 0 });
        }

        let mut dist: HashMap<CodecId, u32> = HashMap::new();
        let mut prev: HashMap<CodecId, (CodecId, usize)> = HashMap::new();
        let mut heap = BinaryHeap::new();

        dist.insert(src, 0);
        heap.push(DijkstraNode { codec_id: src, cost: 0 });

        while let Some(DijkstraNode { codec_id, cost }) = heap.pop() {
            if codec_id == dst {
                break;
            }
            if cost > *dist.get(&codec_id).unwrap_or(&u32::MAX) {
                continue;
            }
            if let Some(edges) = self.edges.get(&codec_id) {
                for edge in edges {
                    let next_cost = cost + edge.cost;
                    if next_cost < *dist.get(&edge.dst_codec_id).unwrap_or(&u32::MAX) {
                        dist.insert(edge.dst_codec_id, next_cost);
                        prev.insert(edge.dst_codec_id, (codec_id, edge.translator_index));
                        heap.push(DijkstraNode { codec_id: edge.dst_codec_id, cost: next_cost });
                    }
                }
            }
        }

        if !prev.contains_key(&dst) {
            return Err(TranslateError::NoPath { src, dst });
        }

        let mut steps = Vec::new();
        let mut current = dst;
        while current != src {
            let (prev_codec, translator_idx) = prev[&current];
            steps.push(Arc::clone(&self.translators[translator_idx]));
            current = prev_codec;
        }
        steps.reverse();

        let total_cost = *dist.get(&dst).unwrap_or(&0);
        Ok(TranslationPath { steps, total_cost })
    }

    /// Get the number of steps required to convert from src to dst.
    pub fn path_steps(&self, src: CodecId, dst: CodecId) -> Option<usize> {
        self.build_path(src, dst).ok().map(|p| p.steps.len())
    }
}

impl Default for TranslationMatrix {
    fn default() -> Self { Self::new() }
}

/// A computed translation path from source to destination codec.
pub struct TranslationPath {
    pub steps: Vec<Arc<dyn Translator>>,
    pub total_cost: u32,
}

impl TranslationPath {
    /// Returns true if this is a no-op (source == destination).
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }

    /// Create a reusable translation chain.
    pub fn create_chain(&self) -> TranslationChain {
        let instances = self.steps.iter().map(|s| s.new_instance()).collect();
        TranslationChain { instances }
    }

    /// Get a human-readable description of this path.
    pub fn describe(&self) -> String {
        if self.is_noop() {
            return "(no translation needed)".to_string();
        }
        self.steps.iter().map(|s| s.name()).collect::<Vec<_>>().join(" -> ")
    }
}

/// A reusable chain of translator instances for streaming translation.
pub struct TranslationChain {
    instances: Vec<Box<dyn TranslatorInstance>>,
}

impl TranslationChain {
    /// Feed a frame through the entire chain.
    pub fn translate(&mut self, frame: &Frame) -> Result<Option<Frame>, TranslateError> {
        if self.instances.is_empty() {
            return Ok(Some(frame.clone()));
        }
        self.instances[0].frame_in(frame)?;
        let mut current_frame = self.instances[0].frame_out();
        for instance in self.instances.iter_mut().skip(1) {
            if let Some(ref f) = current_frame {
                instance.frame_in(f)?;
                current_frame = instance.frame_out();
            } else {
                return Ok(None);
            }
        }
        Ok(current_frame)
    }

    pub fn destroy(&mut self) {
        for instance in &mut self.instances {
            instance.destroy();
        }
    }
}

impl Drop for TranslationChain {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_codecs::*;
    use crate::builtin_translators::register_builtin_translators;

    /// Helper: build a matrix with all built-in translators.
    fn matrix_with_builtins() -> TranslationMatrix {
        let mut m = TranslationMatrix::new();
        register_builtin_translators(&mut m);
        m
    }

    #[test]
    fn test_same_codec_noop() {
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_ULAW).unwrap();
        assert!(path.is_noop(), "Same codec should produce a no-op path");
        assert_eq!(path.total_cost, 0);
        assert_eq!(path.describe(), "(no translation needed)");
    }

    #[test]
    fn test_one_step_ulaw_to_slin() {
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_SLIN8).unwrap();
        assert_eq!(path.steps.len(), 1, "ULAW->SLIN should be 1 step");
        assert_eq!(path.steps[0].name(), "ulawtolin");
    }

    #[test]
    fn test_one_step_slin_to_ulaw() {
        let m = matrix_with_builtins();
        let path = m.build_path(ID_SLIN8, ID_ULAW).unwrap();
        assert_eq!(path.steps.len(), 1, "SLIN->ULAW should be 1 step");
        assert_eq!(path.steps[0].name(), "lintoulaw");
    }

    #[test]
    fn test_two_step_ulaw_to_alaw() {
        // ULAW -> SLIN8 -> ALAW (2 steps)
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_ALAW).unwrap();
        assert_eq!(
            path.steps.len(),
            2,
            "ULAW->ALAW should be 2 steps (via SLIN)"
        );
        assert_eq!(path.steps[0].name(), "ulawtolin");
        assert_eq!(path.steps[1].name(), "lintoalaw");
    }

    #[test]
    fn test_two_step_alaw_to_ulaw() {
        // ALAW -> SLIN8 -> ULAW (2 steps)
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ALAW, ID_ULAW).unwrap();
        assert_eq!(
            path.steps.len(),
            2,
            "ALAW->ULAW should be 2 steps (via SLIN)"
        );
        assert_eq!(path.steps[0].name(), "alawtolin");
        assert_eq!(path.steps[1].name(), "lintoulaw");
    }

    #[test]
    fn test_multistep_ulaw_to_slin16() {
        // ULAW -> SLIN8 -> SLIN16 (via resampler)
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_SLIN16).unwrap();
        assert!(
            path.steps.len() >= 2,
            "ULAW->SLIN16 should need at least 2 steps, got {}",
            path.steps.len()
        );
    }

    #[test]
    fn test_no_path_exists() {
        // Create a matrix with no translators
        let m = TranslationMatrix::new();
        let result = m.build_path(ID_ULAW, ID_ALAW);
        assert!(result.is_err(), "Should fail with no translators");
        match result {
            Err(TranslateError::NoPath { src, dst }) => {
                assert_eq!(src, ID_ULAW);
                assert_eq!(dst, ID_ALAW);
            }
            _ => panic!("Expected NoPath error"),
        }
    }

    #[test]
    fn test_path_steps_convenience() {
        let m = matrix_with_builtins();

        // Same codec: 0 steps
        assert_eq!(m.path_steps(ID_ULAW, ID_ULAW), Some(0));

        // Direct: 1 step
        assert_eq!(m.path_steps(ID_ULAW, ID_SLIN8), Some(1));

        // Via SLIN: 2 steps
        assert_eq!(m.path_steps(ID_ULAW, ID_ALAW), Some(2));

        // Non-existent path
        let empty = TranslationMatrix::new();
        assert_eq!(empty.path_steps(ID_ULAW, ID_ALAW), None);
    }

    #[test]
    fn test_translation_chain_passthrough() {
        // A no-op path should pass frames through unchanged
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_ULAW).unwrap();
        let mut chain = path.create_chain();
        assert!(chain.instances.is_empty());

        let frame = Frame::voice(ID_ULAW, 160, bytes::Bytes::from(vec![0u8; 160]));
        let result = chain.translate(&frame).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_translation_chain_ulaw_to_slin() {
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_SLIN8).unwrap();
        let mut chain = path.create_chain();

        // 160 ulaw samples -> should produce 320 bytes of slin (160 samples * 2 bytes)
        let ulaw_data = vec![0xFFu8; 160]; // mu-law silence
        let frame = Frame::voice(ID_ULAW, 160, bytes::Bytes::from(ulaw_data));

        let result = chain.translate(&frame).unwrap();
        assert!(result.is_some(), "Should produce output frame");

        match result.unwrap() {
            Frame::Voice { codec_id, samples, data, .. } => {
                assert_eq!(codec_id, ID_SLIN8);
                assert_eq!(samples, 160);
                assert_eq!(data.len(), 320); // 160 samples * 2 bytes/sample
            }
            _ => panic!("Expected voice frame"),
        }
    }

    #[test]
    fn test_describe_path() {
        let m = matrix_with_builtins();
        let path = m.build_path(ID_ULAW, ID_ALAW).unwrap();
        let desc = path.describe();
        assert!(desc.contains("ulawtolin"), "Description: {}", desc);
        assert!(desc.contains("lintoalaw"), "Description: {}", desc);
    }
}
