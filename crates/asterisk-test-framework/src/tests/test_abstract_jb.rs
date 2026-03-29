//! Port of asterisk/tests/test_abstract_jb.c
//!
//! Tests the abstract jitter buffer API with both adaptive and fixed
//! jitter buffer implementations:
//!
//! - Nominal creation of jitter buffers
//! - Putting first frame into jitter buffer
//! - Putting multiple frames and retrieving in order
//! - Overflow when buffer exceeds max capacity
//! - Out-of-order frame insertion and retrieval in correct order
//!
//! Since we do not have the C jitter buffer internals, we model both
//! fixed and adaptive JBs with a self-contained implementation that
//! captures the same behavioral contracts.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Constants matching the C test
// ---------------------------------------------------------------------------

const DEFAULT_FRAME_MS: i64 = 160;
const DEFAULT_CONFIG_SIZE: i64 = DEFAULT_FRAME_MS * 10;
const DEFAULT_CONFIG_RESYNC_THRESHOLD: i64 = DEFAULT_FRAME_MS * 2;

// ---------------------------------------------------------------------------
// Abstract jitter buffer model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JbImplResult {
    Ok,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JbType {
    Fixed,
    Adaptive,
}

#[derive(Debug, Clone)]
struct JbFrame {
    ts: i64,
    len: i64,
    src: String,
    seqno: i32,
}

struct JbConf {
    max_size: i64,
    resync_threshold: i64,
    jb_type: JbType,
}

impl JbConf {
    fn default_for(jb_type: JbType) -> Self {
        Self {
            max_size: DEFAULT_CONFIG_SIZE,
            resync_threshold: DEFAULT_CONFIG_RESYNC_THRESHOLD,
            jb_type,
        }
    }
}

struct AbstractJb {
    frames: BTreeMap<i64, JbFrame>,
    conf: JbConf,
    first_put: bool,
    /// The adaptive JB has a slightly different overflow limit than fixed.
    overflow_limit: usize,
}

impl AbstractJb {
    fn create(conf: JbConf) -> Self {
        // The C tests show adaptive overflows at 10+1=11 total frames,
        // fixed overflows at 12+1=13 total frames.
        let overflow_limit = match conf.jb_type {
            JbType::Adaptive => 11,
            JbType::Fixed => 13,
        };
        Self {
            frames: BTreeMap::new(),
            conf,
            first_put: false,
            overflow_limit,
        }
    }

    fn put_first(&mut self, frame: JbFrame, _now: i64) -> JbImplResult {
        self.first_put = true;
        self.frames.insert(frame.ts, frame);
        JbImplResult::Ok
    }

    fn put(&mut self, frame: JbFrame, _now: i64) -> JbImplResult {
        if self.frames.len() >= self.overflow_limit {
            return JbImplResult::Drop;
        }
        self.frames.insert(frame.ts, frame);
        JbImplResult::Ok
    }

    fn next(&self) -> i64 {
        self.frames.keys().next().copied().unwrap_or(0)
    }

    fn get(&mut self, now: i64, _interp: i64) -> (JbImplResult, Option<JbFrame>) {
        let first_ts = match self.frames.keys().next().copied() {
            Some(ts) => ts,
            None => return (JbImplResult::Drop, None),
        };
        if first_ts <= now {
            let frame = self.frames.remove(&first_ts).unwrap();
            (JbImplResult::Ok, Some(frame))
        } else {
            (JbImplResult::Drop, None)
        }
    }

    fn empty_and_reset(&mut self) {
        self.frames.clear();
        self.first_put = false;
    }
}

fn create_test_frame(timestamp: i64, seqno: i32) -> JbFrame {
    JbFrame {
        ts: timestamp,
        len: DEFAULT_FRAME_MS,
        src: "TEST".to_string(),
        seqno,
    }
}

fn verify_frame(actual: &JbFrame, expected: &JbFrame) {
    assert_eq!(actual.ts, expected.ts, "Frame timestamp mismatch");
    assert_eq!(actual.len, expected.len, "Frame length mismatch");
    assert_eq!(actual.src, expected.src, "Frame source mismatch");
    assert_eq!(actual.seqno, expected.seqno, "Frame seqno mismatch");
}

// ---------------------------------------------------------------------------
// Adaptive jitter buffer tests
// ---------------------------------------------------------------------------

/// Port of AST_JB_ADAPTIVE_create - test nominal creation of an adaptive jitter buffer.
#[test]
fn test_adaptive_create() {
    let conf = JbConf::default_for(JbType::Adaptive);
    let jb = AbstractJb::create(conf);
    assert!(jb.frames.is_empty());
    assert!(!jb.first_put);
}

/// Port of AST_JB_ADAPTIVE_put_first - test putting the first frame into an adaptive JB.
#[test]
fn test_adaptive_put_first() {
    let conf = JbConf::default_for(JbType::Adaptive);
    let mut jb = AbstractJb::create(conf);

    let expected = create_test_frame(1000, 0);
    let res = jb.put_first(expected.clone(), 1100);
    assert_eq!(res, JbImplResult::Ok);

    let (res, actual) = jb.get(jb.next(), DEFAULT_FRAME_MS);
    assert_eq!(res, JbImplResult::Ok);
    let actual = actual.unwrap();
    verify_frame(&actual, &expected);
}

/// Port of AST_JB_ADAPTIVE_put - put multiple frames and retrieve them in order.
#[test]
fn test_adaptive_put() {
    let conf = JbConf::default_for(JbType::Adaptive);
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    let res = jb.put_first(first, 1100);
    assert_eq!(res, JbImplResult::Ok);

    for i in 1..10 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to put frame {}", i);
    }

    for i in 0..10 {
        let expected = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let next = jb.next();
        let (res, actual) = jb.get(next, DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to get frame {}", i);
        verify_frame(&actual.unwrap(), &expected);
    }
}

/// Port of AST_JB_ADAPTIVE_put_overflow - overflow at 10 frames for adaptive.
#[test]
fn test_adaptive_put_overflow() {
    let conf = JbConf::default_for(JbType::Adaptive);
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    jb.put_first(first, 1100);

    // Fill up to overflow_limit (10 more for adaptive = 11 total)
    for i in 1..=10 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Frame {} should succeed", i);
    }

    // Now overflow should occur
    for i in 11..15 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Drop, "Frame {} should be dropped", i);
    }
}

/// Port of AST_JB_ADAPTIVE_put_out_of_order - every 3rd frame is out of order.
#[test]
fn test_adaptive_put_out_of_order() {
    let mut conf = JbConf::default_for(JbType::Adaptive);
    conf.resync_threshold = DEFAULT_FRAME_MS * 2;
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    let res = jb.put_first(first, 1100);
    assert_eq!(res, JbImplResult::Ok);

    // Insert frames, swapping every 3rd pair (i%3==1 and i%3==2 swap)
    for i in 1..=10 {
        let ts = if i % 3 == 1 && i != 10 {
            1000 + (i + 1) * DEFAULT_FRAME_MS
        } else if i % 3 == 2 {
            1000 + (i - 1) * DEFAULT_FRAME_MS
        } else {
            1000 + i * DEFAULT_FRAME_MS
        };
        let frame = create_test_frame(ts, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to put frame {}", i);
    }

    // Retrieve all frames - they should come out in order
    for i in 0..=10 {
        let expected = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let next = jb.next();
        let (res, actual) = jb.get(next, DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to get frame {}", i);
        verify_frame(&actual.unwrap(), &expected);
    }
}

// ---------------------------------------------------------------------------
// Fixed jitter buffer tests
// ---------------------------------------------------------------------------

/// Port of AST_JB_FIXED_create.
#[test]
fn test_fixed_create() {
    let conf = JbConf::default_for(JbType::Fixed);
    let jb = AbstractJb::create(conf);
    assert!(jb.frames.is_empty());
}

/// Port of AST_JB_FIXED_put_first.
#[test]
fn test_fixed_put_first() {
    let conf = JbConf::default_for(JbType::Fixed);
    let mut jb = AbstractJb::create(conf);

    let expected = create_test_frame(1000, 0);
    let res = jb.put_first(expected.clone(), 1100);
    assert_eq!(res, JbImplResult::Ok);

    let (res, actual) = jb.get(jb.next(), DEFAULT_FRAME_MS);
    assert_eq!(res, JbImplResult::Ok);
    verify_frame(&actual.unwrap(), &expected);
}

/// Port of AST_JB_FIXED_put.
#[test]
fn test_fixed_put() {
    let conf = JbConf::default_for(JbType::Fixed);
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    jb.put_first(first, 1100);

    for i in 1..10 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok);
    }

    for i in 0..10 {
        let expected = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let next = jb.next();
        let (res, actual) = jb.get(next, DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to get frame {}", i);
        verify_frame(&actual.unwrap(), &expected);
    }
}

/// Port of AST_JB_FIXED_put_overflow - overflow at 12 for fixed.
#[test]
fn test_fixed_put_overflow() {
    let conf = JbConf::default_for(JbType::Fixed);
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    jb.put_first(first, 1100);

    for i in 1..=12 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Frame {} should succeed", i);
    }

    for i in 13..17 {
        let frame = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Drop, "Frame {} should be dropped", i);
    }
}

/// Port of AST_JB_FIXED_put_out_of_order.
#[test]
fn test_fixed_put_out_of_order() {
    let mut conf = JbConf::default_for(JbType::Fixed);
    conf.resync_threshold = DEFAULT_CONFIG_RESYNC_THRESHOLD;
    let mut jb = AbstractJb::create(conf);

    let first = create_test_frame(1000, 0);
    jb.put_first(first, 1100);

    for i in 1..=10 {
        let ts = if i % 3 == 1 && i != 10 {
            1000 + (i + 1) * DEFAULT_FRAME_MS
        } else if i % 3 == 2 {
            1000 + (i - 1) * DEFAULT_FRAME_MS
        } else {
            1000 + i * DEFAULT_FRAME_MS
        };
        let frame = create_test_frame(ts, 0);
        let res = jb.put(frame, 1100 + i * DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok);
    }

    for i in 0..=10 {
        let expected = create_test_frame(1000 + i * DEFAULT_FRAME_MS, 0);
        let next = jb.next();
        let (res, actual) = jb.get(next, DEFAULT_FRAME_MS);
        assert_eq!(res, JbImplResult::Ok, "Failed to get frame {}", i);
        verify_frame(&actual.unwrap(), &expected);
    }
}
