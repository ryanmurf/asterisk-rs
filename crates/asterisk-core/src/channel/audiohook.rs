//! Audiohook framework -- spy, whisper, and manipulate channel audio.
//!
//! Mirrors C Asterisk's `audiohook.c` / `audiohook.h`.  Audiohooks intercept
//! the audio flowing through a channel in the read and/or write direction.
//!
//! - **Spy**: passively records/monitors audio (ChanSpy, MixMonitor)
//! - **Whisper**: injects audio into one direction (whisper to agent)
//! - **Manipulate**: modifies audio in-place (volume adjust, speech recognition)

use asterisk_types::Frame;

/// The type of audiohook, determining when and how it processes frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudiohookType {
    /// Spy -- passively observes audio without altering it.
    Spy,
    /// Whisper -- injects additional audio into the stream.
    Whisper,
    /// Manipulate -- modifies audio frames in-place.
    Manipulate,
}

/// Direction of audio flow relative to the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Audio being read from the channel (incoming from the network).
    Read,
    /// Audio being written to the channel (outgoing to the network).
    Write,
}

/// Trait for implementing an audiohook.
///
/// Audiohooks are attached to a channel and process frames flowing through
/// the read/write pipeline.
pub trait Audiohook: Send + Sync {
    /// What type of audiohook this is.
    fn hook_type(&self) -> AudiohookType;

    /// Process a frame in the read direction.
    ///
    /// - For `Spy`: receive a copy of the frame (return value ignored by pipeline).
    /// - For `Whisper`: return extra audio to mix into the read stream.
    /// - For `Manipulate`: return the modified frame, or `None` to drop it.
    fn read(&mut self, frame: &Frame) -> Option<Frame> {
        Some(frame.clone())
    }

    /// Process a frame in the write direction.
    ///
    /// Same semantics as `read()` but for the write path.
    fn write(&mut self, frame: &Frame) -> Option<Frame> {
        Some(frame.clone())
    }
}

/// Collection of audiohooks attached to a channel, organized by type.
///
/// Mirrors C Asterisk's `struct ast_audiohook_list` which maintains
/// separate lists for spies, whispers, and manipulators.
#[derive(Default)]
pub struct AudiohookList {
    pub spies: Vec<Box<dyn Audiohook>>,
    pub whispers: Vec<Box<dyn Audiohook>>,
    pub manipulators: Vec<Box<dyn Audiohook>>,
}

impl AudiohookList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an audiohook.  It is automatically placed in the correct list
    /// based on its `hook_type()`.
    pub fn attach(&mut self, hook: Box<dyn Audiohook>) {
        match hook.hook_type() {
            AudiohookType::Spy => self.spies.push(hook),
            AudiohookType::Whisper => self.whispers.push(hook),
            AudiohookType::Manipulate => self.manipulators.push(hook),
        }
    }

    /// Remove an audiohook by index within its type's list.
    /// Returns the removed hook, or `None` if the index is out of range.
    pub fn detach(&mut self, hook_type: AudiohookType, index: usize) -> Option<Box<dyn Audiohook>> {
        let list = match hook_type {
            AudiohookType::Spy => &mut self.spies,
            AudiohookType::Whisper => &mut self.whispers,
            AudiohookType::Manipulate => &mut self.manipulators,
        };
        if index < list.len() {
            Some(list.remove(index))
        } else {
            None
        }
    }

    /// Returns `true` if there are no audiohooks of any type attached.
    pub fn is_empty(&self) -> bool {
        self.spies.is_empty() && self.whispers.is_empty() && self.manipulators.is_empty()
    }

    /// Process a frame through all audiohooks in the read direction.
    ///
    /// 1. Spies get a copy (their return value is ignored).
    /// 2. Manipulators may transform the frame.
    /// 3. Whisper frames are not mixed here (the bridge handles mixing).
    ///
    /// Returns the (possibly modified) frame, or `None` if a manipulator
    /// dropped it.
    pub fn process_read(&mut self, frame: &Frame) -> Option<Frame> {
        // Feed spies (they observe but don't change the frame)
        for spy in &mut self.spies {
            let _ = spy.read(frame);
        }

        // Run through manipulators
        let mut current = frame.clone();
        for manip in &mut self.manipulators {
            match manip.read(&current) {
                Some(f) => current = f,
                None => return None, // manipulator dropped the frame
            }
        }

        Some(current)
    }

    /// Process a frame through all audiohooks in the write direction.
    ///
    /// Same logic as `process_read` but for write-direction hooks.
    pub fn process_write(&mut self, frame: &Frame) -> Option<Frame> {
        for spy in &mut self.spies {
            let _ = spy.write(frame);
        }

        let mut current = frame.clone();
        for manip in &mut self.manipulators {
            match manip.write(&current) {
                Some(f) => current = f,
                None => return None,
            }
        }

        Some(current)
    }
}

impl std::fmt::Debug for AudiohookList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudiohookList")
            .field("spies", &self.spies.len())
            .field("whispers", &self.whispers.len())
            .field("manipulators", &self.manipulators.len())
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

    struct SpyHook {
        read_count: Arc<AtomicU32>,
    }

    impl Audiohook for SpyHook {
        fn hook_type(&self) -> AudiohookType {
            AudiohookType::Spy
        }
        fn read(&mut self, frame: &Frame) -> Option<Frame> {
            self.read_count.fetch_add(1, Ordering::Relaxed);
            Some(frame.clone())
        }
    }

    struct DropManipulator;

    impl Audiohook for DropManipulator {
        fn hook_type(&self) -> AudiohookType {
            AudiohookType::Manipulate
        }
        fn read(&mut self, _frame: &Frame) -> Option<Frame> {
            None // drop the frame
        }
    }

    #[test]
    fn spy_receives_frames() {
        let counter = Arc::new(AtomicU32::new(0));
        let spy = SpyHook {
            read_count: Arc::clone(&counter),
        };

        let mut list = AudiohookList::new();
        list.attach(Box::new(spy));

        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let result = list.process_read(&frame);
        assert!(result.is_some());
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn manipulator_can_drop_frame() {
        let mut list = AudiohookList::new();
        list.attach(Box::new(DropManipulator));

        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let result = list.process_read(&frame);
        assert!(result.is_none(), "manipulator should drop frame");
    }

    #[test]
    fn detach_removes_hook() {
        let counter = Arc::new(AtomicU32::new(0));
        let spy = SpyHook {
            read_count: Arc::clone(&counter),
        };

        let mut list = AudiohookList::new();
        list.attach(Box::new(spy));
        assert!(!list.is_empty());

        list.detach(AudiohookType::Spy, 0);
        assert!(list.is_empty());
    }
}
