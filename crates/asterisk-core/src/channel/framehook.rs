//! Framehook framework -- intercept and transform frames in the read/write pipeline.
//!
//! Mirrors C Asterisk's `framehook.c`.  Framehooks are a lighter-weight
//! alternative to audiohooks: they see every frame type (not just voice)
//! and can transform or replace frames arbitrarily.
//!
//! Used by features like T.38 gateway, SRTP, DTLS, and various ARI/AMI
//! interception points.

use std::sync::atomic::{AtomicU32, Ordering};

use asterisk_types::Frame;

/// Global counter for assigning unique framehook IDs.
static NEXT_FRAMEHOOK_ID: AtomicU32 = AtomicU32::new(1);

/// Events that a framehook callback can receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramehookEvent {
    /// The framehook has just been attached to a channel.
    Attached,
    /// A frame is being read from the channel.
    Read,
    /// A frame is being written to the channel.
    Write,
    /// The framehook is being detached from the channel.
    Detached,
}

/// The callback type for framehooks.
///
/// Receives the current frame and the event type.  Returns:
/// - `Some(frame)` to pass through (possibly modified)
/// - `None` to drop the frame entirely
///
/// NOTE: The callback does NOT receive `&mut Channel` because the channel is
/// already locked during pipeline processing.  If you need channel state, use
/// a closure that captures what it needs.
pub type FramehookCallback =
    Box<dyn Fn(&Frame, FramehookEvent) -> Option<Frame> + Send + Sync>;

/// A single framehook instance.
pub struct Framehook {
    /// Unique identifier for this framehook.
    pub id: u32,
    /// The callback function.
    callback: FramehookCallback,
}

impl Framehook {
    fn new(callback: FramehookCallback) -> Self {
        Self {
            id: NEXT_FRAMEHOOK_ID.fetch_add(1, Ordering::Relaxed),
            callback,
        }
    }

    /// Invoke the callback with a frame and event.
    fn process(&self, frame: &Frame, event: FramehookEvent) -> Option<Frame> {
        (self.callback)(frame, event)
    }
}

impl std::fmt::Debug for Framehook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Framehook").field("id", &self.id).finish()
    }
}

/// Ordered list of framehooks attached to a channel.
#[derive(Default, Debug)]
pub struct FramehookList {
    hooks: Vec<Framehook>,
}

impl FramehookList {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Attach a new framehook.  Returns the assigned hook ID.
    ///
    /// The callback is immediately notified with `FramehookEvent::Attached`
    /// (passing a null frame).
    pub fn attach(&mut self, callback: FramehookCallback) -> u32 {
        let hook = Framehook::new(callback);
        let id = hook.id;
        // Notify attached
        let null_frame = Frame::Null;
        let _ = hook.process(&null_frame, FramehookEvent::Attached);
        self.hooks.push(hook);
        tracing::debug!(hook_id = id, "framehook attached");
        id
    }

    /// Detach a framehook by its ID.  Returns `true` if found and removed.
    ///
    /// The callback is notified with `FramehookEvent::Detached` before removal.
    pub fn detach(&mut self, id: u32) -> bool {
        if let Some(pos) = self.hooks.iter().position(|h| h.id == id) {
            let hook = &self.hooks[pos];
            let null_frame = Frame::Null;
            let _ = hook.process(&null_frame, FramehookEvent::Detached);
            self.hooks.remove(pos);
            tracing::debug!(hook_id = id, "framehook detached");
            true
        } else {
            false
        }
    }

    /// Returns `true` if there are no framehooks attached.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Process a frame through all framehooks for a read event.
    ///
    /// Each hook sees the output of the previous hook.  If any hook returns
    /// `None`, the frame is dropped and `None` is returned.
    pub fn process_read(&self, frame: &Frame) -> Option<Frame> {
        self.process_event(frame, FramehookEvent::Read)
    }

    /// Process a frame through all framehooks for a write event.
    pub fn process_write(&self, frame: &Frame) -> Option<Frame> {
        self.process_event(frame, FramehookEvent::Write)
    }

    fn process_event(&self, frame: &Frame, event: FramehookEvent) -> Option<Frame> {
        let mut current = frame.clone();
        for hook in &self.hooks {
            match hook.process(&current, event) {
                Some(f) => current = f,
                None => return None,
            }
        }
        Some(current)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn attach_and_detach() {
        let mut list = FramehookList::new();

        let id = list.attach(Box::new(|frame, _event| Some(frame.clone())));
        assert!(!list.is_empty());

        assert!(list.detach(id));
        assert!(list.is_empty());
    }

    #[test]
    fn detach_nonexistent_returns_false() {
        let mut list = FramehookList::new();
        assert!(!list.detach(999));
    }

    #[test]
    fn read_passes_through() {
        let mut list = FramehookList::new();
        list.attach(Box::new(|frame, _event| Some(frame.clone())));

        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let result = list.process_read(&frame);
        assert!(result.is_some());
    }

    #[test]
    fn hook_can_drop_frame() {
        let mut list = FramehookList::new();
        list.attach(Box::new(|_frame, _event| None));

        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let result = list.process_read(&frame);
        assert!(result.is_none());
    }

    #[test]
    fn hooks_chain_in_order() {
        let mut list = FramehookList::new();

        // First hook: convert any voice frame to a null frame
        list.attach(Box::new(|_frame, event| {
            if event == FramehookEvent::Read {
                Some(Frame::Null)
            } else {
                Some(_frame.clone())
            }
        }));

        // Second hook: pass through whatever it gets
        list.attach(Box::new(|frame, _event| Some(frame.clone())));

        let frame = Frame::voice(0, 160, Bytes::from(vec![0u8; 320]));
        let result = list.process_read(&frame);
        assert!(result.is_some());
        assert!(matches!(result.unwrap(), Frame::Null));
    }

    #[test]
    fn unique_ids() {
        let mut list = FramehookList::new();
        let id1 = list.attach(Box::new(|f, _| Some(f.clone())));
        let id2 = list.attach(Box::new(|f, _| Some(f.clone())));
        assert_ne!(id1, id2);
    }
}
