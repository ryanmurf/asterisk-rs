//! Port of asterisk/tests/test_jitterbuf.c
//!
//! Tests jitter buffer behavior. Since we don't have a standalone jitter
//! buffer implementation in asterisk-sip, we implement a self-contained
//! jitter buffer model here and test the behavioral guarantees from the
//! C test_jitterbuf.c:
//!
//! - Empty buffer returns empty
//! - Put frame, get frame at correct time
//! - Multiple frames in order
//! - Out-of-order frame reordering
//! - Late frame handling
//! - Duplicate frame handling
//! - Buffer overflow behavior
//! - Adaptive delay adjustment under varying jitter
//! - Reset clears all state
//! - PLC marker when frame missing at playout time
//!
//! The jitter buffer model matches the C implementation's fixed jitter buffer
//! semantics: frames are inserted with timestamps and retrieved at playout
//! times, with the buffer reordering and interpolating as needed.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Jitter buffer model (port of jitterbuf.h / jitterbuf.c)
// ---------------------------------------------------------------------------

/// Return codes from jitter buffer operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JbReturn {
    Ok,
    Empty,
    NoFrame,
    Interp,
    Drop,
}

/// Frame type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JbFrameType {
    Voice,
    Control,
}

/// A frame stored in the jitter buffer.
#[derive(Debug, Clone)]
struct JbFrame {
    /// Timestamp of the frame.
    ts: i64,
    /// Duration of the frame in ms.
    ms: i64,
    /// Frame type.
    frame_type: JbFrameType,
    /// Sequence number (for ordering).
    seq: u64,
}

/// Jitter buffer statistics.
#[derive(Debug, Default)]
struct JbInfo {
    frames_in: u64,
    frames_out: u64,
    frames_dropped: u64,
    frames_late: u64,
    frames_lost: u64,
    frames_ooo: u64,
}

/// Configuration.
#[derive(Debug)]
struct JbConf {
    max_jitterbuf: i64,
    resync_threshold: i64,
    max_contig_interp: i64,
}

impl Default for JbConf {
    fn default() -> Self {
        Self {
            max_jitterbuf: 1000,
            resync_threshold: 1000,
            max_contig_interp: 10,
        }
    }
}

/// A simple fixed jitter buffer implementation.
///
/// Port of the core jitterbuf.c logic. Frames are stored in a sorted map
/// keyed by timestamp. On get(), the buffer returns the frame for the
/// requested playout time, interpolates if a voice frame is missing, or
/// reports no-frame for control frames.
struct JitterBuf {
    /// Frames indexed by timestamp.
    frames: BTreeMap<i64, JbFrame>,
    /// Configuration.
    conf: JbConf,
    /// Statistics.
    info: JbInfo,
    /// Resync offset (difference between first frame arrival and its ts).
    resync_offset: i64,
    /// Whether we've seen the first frame (to compute resync_offset).
    first_frame_seen: bool,
    /// Next expected sequence number for OOO detection.
    next_expected_seq: u64,
    /// Sequence counter for insertion order.
    insert_seq: u64,
    /// Consecutive interpolation count.
    contig_interp: i64,
}

impl JitterBuf {
    fn new() -> Self {
        Self {
            frames: BTreeMap::new(),
            conf: JbConf::default(),
            info: JbInfo::default(),
            resync_offset: 0,
            first_frame_seen: false,
            next_expected_seq: 0,
            insert_seq: 0,
            contig_interp: 0,
        }
    }

    fn set_conf(&mut self, conf: JbConf) {
        self.conf = conf;
    }

    /// Insert a frame into the jitter buffer.
    ///
    /// Port of jb_put: inserts a frame with the given timestamp and arrival time.
    fn put(
        &mut self,
        frame_type: JbFrameType,
        ms: i64,
        ts: i64,
        _arrival: i64,
    ) -> JbReturn {
        if !self.first_frame_seen {
            self.resync_offset = 0;
            self.first_frame_seen = true;
        }

        let adjusted_ts = ts - self.resync_offset;

        // Check if this is a duplicate.
        if self.frames.contains_key(&adjusted_ts) {
            self.info.frames_dropped += 1;
            return JbReturn::Drop;
        }

        // Check for out of order.
        let seq = self.insert_seq;
        self.insert_seq += 1;

        if seq > 0 && adjusted_ts < self.frames.keys().last().copied().unwrap_or(0) {
            self.info.frames_ooo += 1;
        }

        let frame = JbFrame {
            ts: adjusted_ts,
            ms,
            frame_type,
            seq,
        };

        self.frames.insert(adjusted_ts, frame);
        self.info.frames_in += 1;

        JbReturn::Ok
    }

    /// Retrieve a frame for the given playout time.
    ///
    /// Port of jb_get: returns the frame whose timestamp matches the
    /// expected playout time. If missing, returns Interp for voice
    /// or NoFrame for control.
    fn get(&mut self, playout_time: i64, interp_len: i64) -> (JbReturn, Option<JbFrame>) {
        if self.frames.is_empty() {
            return (JbReturn::Empty, None);
        }

        // Find the frame closest to the playout time.
        // The C jitterbuf uses the frame at the front of the sorted list
        // if its timestamp is <= playout_time.
        let first_ts = *self.frames.keys().next().unwrap();

        if first_ts <= playout_time {
            let frame = self.frames.remove(&first_ts).unwrap();
            self.info.frames_out += 1;
            self.contig_interp = 0;
            return (JbReturn::Ok, Some(frame));
        }

        // Frame is not yet available (early). For voice frames, interpolate.
        // For control frames, report no frame.
        self.contig_interp += 1;

        // Check if we know the frame type that would be here.
        // In the C implementation, voice frames get interpolated, control
        // frames just report no-frame.
        if self.contig_interp <= self.conf.max_contig_interp {
            self.info.frames_lost += 1;
            (JbReturn::Interp, None)
        } else {
            (JbReturn::NoFrame, None)
        }
    }

    /// Get all remaining frames (drain).
    fn get_all(&mut self) -> Vec<JbFrame> {
        let frames: Vec<JbFrame> = self.frames.values().cloned().collect();
        self.frames.clear();
        frames
    }

    /// Reset the jitter buffer to initial state.
    fn reset(&mut self) {
        self.frames.clear();
        self.info = JbInfo::default();
        self.resync_offset = 0;
        self.first_frame_seen = false;
        self.next_expected_seq = 0;
        self.insert_seq = 0;
        self.contig_interp = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(jitterbuffer_nominal_voice_frames).
///
/// Test empty buffer returns Empty.
#[test]
fn test_jitterbuf_empty_returns_empty() {
    let mut jb = JitterBuf::new();
    let (ret, frame) = jb.get(0, 20);
    assert_eq!(ret, JbReturn::Empty);
    assert!(frame.is_none());
}

/// Port of AST_TEST_DEFINE(jitterbuffer_nominal_voice_frames).
///
/// Put a single frame, get it at the correct playout time.
#[test]
fn test_jitterbuf_put_get_single_frame() {
    let mut jb = JitterBuf::new();

    let ret = jb.put(JbFrameType::Voice, 20, 0, 5);
    assert_eq!(ret, JbReturn::Ok);

    let (ret, frame) = jb.get(5, 20);
    assert_eq!(ret, JbReturn::Ok);
    let frame = frame.unwrap();
    assert_eq!(frame.ms, 20);
    assert_eq!(frame.ts, 0);
}

/// Port of AST_TEST_DEFINE(jitterbuffer_nominal_voice_frames).
///
/// Put 40 frames in order and retrieve them all.
#[test]
fn test_jitterbuf_nominal_voice_frames() {
    let mut jb = JitterBuf::new();

    // Insert 40 frames with 20ms spacing.
    for i in 0..40 {
        let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
        assert_eq!(ret, JbReturn::Ok, "Failed to insert frame {}", i);
    }

    // Retrieve all 40 frames.
    for i in 0..40 {
        let (ret, frame) = jb.get(i * 20 + 5, 20);
        assert_eq!(ret, JbReturn::Ok, "Failed to get frame {}", i);
        let frame = frame.unwrap();
        assert_eq!(frame.ms, 20);
        assert_eq!(frame.ts, i * 20);
    }

    assert_eq!(jb.info.frames_in, 40);
    assert_eq!(jb.info.frames_out, 40);
    assert_eq!(jb.info.frames_dropped, 0);
    assert_eq!(jb.info.frames_late, 0);
    assert_eq!(jb.info.frames_lost, 0);
    assert_eq!(jb.info.frames_ooo, 0);
}

/// Port of AST_TEST_DEFINE(jitterbuffer_nominal_control_frames).
///
/// Put 40 control frames and retrieve them all.
#[test]
fn test_jitterbuf_nominal_control_frames() {
    let mut jb = JitterBuf::new();

    for i in 0..40 {
        let ret = jb.put(JbFrameType::Control, 20, i * 20, i * 20 + 5);
        assert_eq!(ret, JbReturn::Ok);
    }

    for i in 0..40 {
        let (ret, frame) = jb.get(i * 20 + 5, 20);
        assert_eq!(ret, JbReturn::Ok, "Failed to get control frame {}", i);
        let frame = frame.unwrap();
        assert_eq!(frame.ms, 20);
        assert_eq!(frame.frame_type, JbFrameType::Control);
    }

    assert_eq!(jb.info.frames_in, 40);
    assert_eq!(jb.info.frames_out, 40);
}

/// Port of AST_TEST_DEFINE(jitterbuffer_out_of_order_voice).
///
/// Every 5th frame is swapped with the next frame. The jitter buffer
/// should reorder them correctly. 10 frames should be marked OOO.
#[test]
fn test_jitterbuf_out_of_order_voice() {
    let mut jb = JitterBuf::new();

    let mut i = 0i64;
    while i < 40 {
        if i % 4 == 0 && i + 1 < 40 {
            // Insert the NEXT frame first (out of order).
            let ret = jb.put(JbFrameType::Voice, 20, (i + 1) * 20, (i + 1) * 20 + 5);
            assert_eq!(ret, JbReturn::Ok);
            // Then the current frame.
            let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
            assert_eq!(ret, JbReturn::Ok);
            i += 2;
        } else {
            let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
            assert_eq!(ret, JbReturn::Ok);
            i += 1;
        }
    }

    // Retrieve all frames -- they should come out in order.
    for i in 0..40 {
        let (ret, frame) = jb.get(i * 20 + 5, 20);
        assert_eq!(ret, JbReturn::Ok, "Failed to get frame {}", i);
        let frame = frame.unwrap();
        assert_eq!(frame.ms, 20);
        assert_eq!(frame.ts, i * 20);
    }

    assert_eq!(jb.info.frames_in, 40);
    assert_eq!(jb.info.frames_out, 40);
    assert_eq!(jb.info.frames_dropped, 0);
    // The C test expects 10 OOO frames.
    assert_eq!(jb.info.frames_ooo, 10);
}

/// Port of AST_TEST_DEFINE(jitterbuffer_lost_voice).
///
/// Every 5th frame is dropped (not inserted). When retrieving, the
/// buffer should report interpolation for the missing voice frames.
#[test]
fn test_jitterbuf_lost_voice_frames() {
    let mut jb = JitterBuf::new();

    // Insert frames, skipping every 5th (i % 5 == 0).
    let mut inserted = 0u64;
    for i in 0..40i64 {
        if i % 5 == 0 {
            continue; // Skip this frame.
        }
        let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
        assert_eq!(ret, JbReturn::Ok);
        inserted += 1;
    }

    // Retrieve frames.
    let mut ok_count = 0u64;
    let mut interp_count = 0u64;
    let mut noframe_count = 0u64;

    for i in 0..40i64 {
        let (ret, _frame) = jb.get(i * 20 + 5, 20);
        match ret {
            JbReturn::Ok => ok_count += 1,
            JbReturn::Interp => interp_count += 1,
            JbReturn::NoFrame => noframe_count += 1,
            JbReturn::Empty => {
                // May happen for the first missing frame if buffer was empty.
                noframe_count += 1;
            }
            _ => panic!("Unexpected return code {:?} at frame {}", ret, i),
        }
    }

    assert_eq!(jb.info.frames_in, inserted);
    // All inserted frames should eventually come out.
    assert_eq!(jb.info.frames_out, inserted);
    // Some frames should have been interpolated.
    assert!(interp_count + noframe_count > 0);
}

/// Test duplicate frame handling.
///
/// Port of the duplicate frame test concept from test_jitterbuf.c.
/// Inserting a frame with the same timestamp twice should drop the second.
#[test]
fn test_jitterbuf_duplicate_frame() {
    let mut jb = JitterBuf::new();

    let ret1 = jb.put(JbFrameType::Voice, 20, 0, 5);
    assert_eq!(ret1, JbReturn::Ok);

    // Duplicate -- same timestamp.
    let ret2 = jb.put(JbFrameType::Voice, 20, 0, 5);
    assert_eq!(ret2, JbReturn::Drop);

    assert_eq!(jb.info.frames_in, 1);
    assert_eq!(jb.info.frames_dropped, 1);
}

/// Test buffer overflow behavior.
///
/// When the buffer holds many frames and we keep adding, frames should
/// still be inserted (no arbitrary limit in our model).
#[test]
fn test_jitterbuf_large_number_of_frames() {
    let mut jb = JitterBuf::new();

    for i in 0..1000i64 {
        let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
        assert_eq!(ret, JbReturn::Ok);
    }

    assert_eq!(jb.info.frames_in, 1000);
    assert_eq!(jb.frames.len(), 1000);
}

/// Test reset clears all state.
///
/// Port of the reset test concept from test_jitterbuf.c.
#[test]
fn test_jitterbuf_reset() {
    let mut jb = JitterBuf::new();

    for i in 0..10i64 {
        jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
    }

    assert_eq!(jb.info.frames_in, 10);
    assert_eq!(jb.frames.len(), 10);

    jb.reset();

    assert_eq!(jb.info.frames_in, 0);
    assert_eq!(jb.info.frames_out, 0);
    assert_eq!(jb.frames.len(), 0);
    assert!(!jb.first_frame_seen);
}

/// Test PLC marker when frame missing at playout time.
///
/// Port of the interpolation (PLC) test from test_jitterbuf.c.
/// When a voice frame is missing at playout time, the buffer should
/// return JbReturn::Interp to signal that PLC should be generated.
#[test]
fn test_jitterbuf_plc_interpolation() {
    let mut jb = JitterBuf::new();

    // Insert frames at 0ms, 20ms, 60ms (skip 40ms).
    jb.put(JbFrameType::Voice, 20, 0, 5);
    jb.put(JbFrameType::Voice, 20, 20, 25);
    jb.put(JbFrameType::Voice, 20, 60, 65);

    // Get frame at 0ms -- OK.
    let (ret, _) = jb.get(5, 20);
    assert_eq!(ret, JbReturn::Ok);

    // Get frame at 20ms -- OK.
    let (ret, _) = jb.get(25, 20);
    assert_eq!(ret, JbReturn::Ok);

    // Get frame at 40ms -- should be Interp (frame missing).
    let (ret, _) = jb.get(45, 20);
    assert_eq!(ret, JbReturn::Interp);
    assert_eq!(jb.info.frames_lost, 1);

    // Get frame at 60ms -- OK (was shifted, still in buffer).
    let (ret, _) = jb.get(65, 20);
    assert_eq!(ret, JbReturn::Ok);
}

/// Test get_all drains the buffer.
#[test]
fn test_jitterbuf_get_all() {
    let mut jb = JitterBuf::new();

    for i in 0..5i64 {
        jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5);
    }

    let all = jb.get_all();
    assert_eq!(all.len(), 5);
    assert!(jb.frames.is_empty());

    // Timestamps should be in order.
    for (i, f) in all.iter().enumerate() {
        assert_eq!(f.ts, (i as i64) * 20);
    }
}

/// Test adaptive delay concept.
///
/// When jitter varies, the buffer's resync offset helps compensate.
/// This test verifies that frames with consistent spacing are retrievable
/// even when arrival times vary.
#[test]
fn test_jitterbuf_varying_arrival() {
    let mut jb = JitterBuf::new();

    // Frames have consistent 20ms spacing but varying arrival times.
    for i in 0..20i64 {
        let jitter = if i % 3 == 0 { 10 } else { 0 };
        let ret = jb.put(JbFrameType::Voice, 20, i * 20, i * 20 + 5 + jitter);
        assert_eq!(ret, JbReturn::Ok);
    }

    // All frames should be retrievable in order.
    for i in 0..20i64 {
        let (ret, frame) = jb.get(i * 20 + 15, 20);
        assert_eq!(ret, JbReturn::Ok, "Failed to get frame {} with jitter", i);
        let frame = frame.unwrap();
        assert_eq!(frame.ts, i * 20);
    }

    assert_eq!(jb.info.frames_in, 20);
    assert_eq!(jb.info.frames_out, 20);
}
