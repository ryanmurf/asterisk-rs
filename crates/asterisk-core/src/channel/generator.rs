//! Generator framework -- produces audio mixed into the read pipeline.
//!
//! Mirrors C Asterisk's `struct ast_generator`, `ast_activate_generator`,
//! and `ast_deactivate_generator` (channel.c ~2904-2993).
//!
//! Generators are used by music-on-hold, tone playback, and silence
//! generators.  When active, the generator's `generate()` method is called
//! on every read-pipeline tick, and the resulting frame is delivered instead
//! of (or mixed with) the channel driver's own frames.

use asterisk_types::Frame;

/// A generator produces frames to be injected into the read pipeline.
///
/// This mirrors C Asterisk's `struct ast_generator` with its `alloc`,
/// `release`, and `generate` callbacks.
pub trait Generator: Send + Sync {
    /// Called when the generator is activated on a channel.
    /// Use this to allocate per-channel state (buffers, etc.).
    fn alloc(&mut self) {}

    /// Called when the generator is deactivated.  Clean up resources.
    fn release(&mut self) {}

    /// Produce a frame of audio.
    ///
    /// `samples` is the number of samples to generate (typically
    /// sample_rate / 50 for a 20 ms frame at 50 fps).
    ///
    /// Return `Some(frame)` to inject audio, or `None` to skip this tick
    /// (which will auto-deactivate the generator, matching C behavior).
    fn generate(&mut self, samples: usize) -> Option<Frame>;

    /// Optional: called when a DTMF end is received while the generator is
    /// active.  Some generators (like MOH) need to know about digits.
    fn digit(&mut self, _digit: char) {}

    /// Optional: called when the channel's write format changes while the
    /// generator is active.
    fn write_format_change(&mut self) {}
}

/// Container for the active generator on a channel.
///
/// Stored directly in `Channel::generator`.
#[derive(Default)]
pub struct GeneratorState {
    inner: Option<Box<dyn Generator>>,
}

impl GeneratorState {
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Activate a generator, deactivating any existing one first.
    /// Mirrors `ast_activate_generator`.
    pub fn activate(&mut self, mut gen: Box<dyn Generator>) {
        // Deactivate the previous generator if any
        self.deactivate();
        gen.alloc();
        self.inner = Some(gen);
        tracing::debug!("generator activated");
    }

    /// Deactivate the current generator.
    /// Mirrors `ast_deactivate_generator`.
    pub fn deactivate(&mut self) {
        if let Some(mut gen) = self.inner.take() {
            gen.release();
            tracing::debug!("generator deactivated");
        }
    }

    /// Returns `true` if a generator is currently active.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    /// Ask the generator to produce a frame.  Returns `None` and
    /// auto-deactivates if the generator itself returns `None`.
    pub fn generate(&mut self, samples: usize) -> Option<Frame> {
        let gen = self.inner.as_mut()?;
        match gen.generate(samples) {
            Some(frame) => Some(frame),
            None => {
                // Generator returned None -- auto-deactivate (matches C behavior
                // where a non-zero return from generate triggers deactivation).
                tracing::debug!("generator returned None, auto-deactivating");
                self.deactivate();
                None
            }
        }
    }

    /// Forward a DTMF digit to the active generator.
    pub fn digit(&mut self, digit: char) {
        if let Some(gen) = self.inner.as_mut() {
            gen.digit(digit);
        }
    }

    /// Notify the generator that the write format changed.
    pub fn write_format_change(&mut self) {
        if let Some(gen) = self.inner.as_mut() {
            gen.write_format_change();
        }
    }
}

impl std::fmt::Debug for GeneratorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratorState")
            .field("active", &self.is_active())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CountingGenerator {
        count: Arc<AtomicU32>,
        max: u32,
        allocated: bool,
        released: bool,
    }

    impl CountingGenerator {
        fn new(count: Arc<AtomicU32>, max: u32) -> Self {
            Self {
                count,
                max,
                allocated: false,
                released: false,
            }
        }
    }

    impl Generator for CountingGenerator {
        fn alloc(&mut self) {
            self.allocated = true;
        }
        fn release(&mut self) {
            self.released = true;
        }
        fn generate(&mut self, _samples: usize) -> Option<Frame> {
            let n = self.count.fetch_add(1, Ordering::Relaxed);
            if n >= self.max {
                return None; // triggers auto-deactivate
            }
            Some(Frame::voice(0, 160, Bytes::from(vec![0u8; 320])))
        }
    }

    #[test]
    fn activate_and_generate() {
        let counter = Arc::new(AtomicU32::new(0));
        let gen = CountingGenerator::new(Arc::clone(&counter), 5);

        let mut state = GeneratorState::new();
        assert!(!state.is_active());

        state.activate(Box::new(gen));
        assert!(state.is_active());

        // Generate 5 frames
        for _ in 0..5 {
            assert!(state.generate(160).is_some());
        }

        // 6th should auto-deactivate
        assert!(state.generate(160).is_none());
        assert!(!state.is_active());
    }

    #[test]
    fn deactivate_explicitly() {
        let counter = Arc::new(AtomicU32::new(0));
        let gen = CountingGenerator::new(Arc::clone(&counter), 100);

        let mut state = GeneratorState::new();
        state.activate(Box::new(gen));
        assert!(state.is_active());

        state.deactivate();
        assert!(!state.is_active());
    }
}
