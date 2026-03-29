//! Port of asterisk/tests/test_data_buffer.c
//!
//! Tests circular/indexed data buffer operations:
//! - Buffer creation with max capacity
//! - Put/get operations by position
//! - Buffer overflow (oldest evicted when full)
//! - Buffer resize
//! - Count/capacity tracking
//! - Remove from head
//! - Remove by position
//! - Nominal usage pattern

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Data buffer implementation mirroring Asterisk's ast_data_buffer
// ---------------------------------------------------------------------------

/// A position-indexed data buffer with a maximum capacity.
/// When full, inserting a new item evicts the oldest entry.
///
/// Mirrors ast_data_buffer from Asterisk.
struct DataBuffer<T> {
    data: BTreeMap<u64, T>,
    max: usize,
}

impl<T> DataBuffer<T> {
    /// Allocate a new data buffer with the given maximum capacity.
    fn new(max: usize) -> Self {
        Self {
            data: BTreeMap::new(),
            max,
        }
    }

    /// Get the current count of items in the buffer.
    fn count(&self) -> usize {
        self.data.len()
    }

    /// Get the maximum capacity.
    fn max(&self) -> usize {
        self.max
    }

    /// Put a payload at the given position.
    /// Returns 0 on success. If the position already exists, does nothing new.
    fn put(&mut self, pos: u64, payload: T) -> i32 {
        if self.data.contains_key(&pos) {
            // Already exists at this position.
            return 0;
        }

        // If buffer is full, evict the oldest (smallest position).
        while self.data.len() >= self.max {
            if let Some((&oldest_key, _)) = self.data.iter().next() {
                self.data.remove(&oldest_key);
            } else {
                break;
            }
        }

        self.data.insert(pos, payload);
        0
    }

    /// Get a reference to the payload at the given position.
    fn get(&self, pos: u64) -> Option<&T> {
        self.data.get(&pos)
    }

    /// Resize the buffer to a new maximum capacity.
    fn resize(&mut self, new_max: usize) {
        self.max = new_max;
        // Evict oldest entries if needed.
        while self.data.len() > self.max {
            if let Some((&oldest_key, _)) = self.data.iter().next() {
                self.data.remove(&oldest_key);
            } else {
                break;
            }
        }
    }

    /// Remove and return the payload from the head (smallest position).
    fn remove_head(&mut self) -> Option<T> {
        let key = *self.data.keys().next()?;
        self.data.remove(&key)
    }

    /// Remove and return the payload at the given position.
    fn remove(&mut self, pos: u64) -> Option<T> {
        self.data.remove(&pos)
    }
}

// ---------------------------------------------------------------------------
// Mock payload for testing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockPayload {
    id: i32,
}

// ---------------------------------------------------------------------------
// Tests: Buffer creation (port of buffer_create)
// ---------------------------------------------------------------------------

#[test]
fn test_buffer_create() {
    let buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    assert_eq!(buffer.count(), 0);
    assert_eq!(buffer.max(), 10);
}

#[test]
fn test_buffer_create_different_sizes() {
    let b1: DataBuffer<MockPayload> = DataBuffer::new(1);
    assert_eq!(b1.max(), 1);

    let b2: DataBuffer<MockPayload> = DataBuffer::new(100);
    assert_eq!(b2.max(), 100);

    let b3: DataBuffer<MockPayload> = DataBuffer::new(1000);
    assert_eq!(b3.max(), 1000);
}

// ---------------------------------------------------------------------------
// Tests: Buffer put (port of buffer_put)
// ---------------------------------------------------------------------------

#[test]
fn test_buffer_put_single() {
    let mut buffer = DataBuffer::new(2);
    let ret = buffer.put(2, MockPayload { id: 2 });
    assert_eq!(ret, 0);
    assert_eq!(buffer.count(), 1);
}

#[test]
fn test_buffer_put_and_get() {
    let mut buffer = DataBuffer::new(2);
    buffer.put(2, MockPayload { id: 2 });

    let payload = buffer.get(2).unwrap();
    assert_eq!(payload.id, 2);
}

#[test]
fn test_buffer_put_duplicate_position() {
    let mut buffer = DataBuffer::new(2);
    buffer.put(2, MockPayload { id: 2 });
    buffer.put(2, MockPayload { id: 2 }); // Duplicate position.

    assert_eq!(buffer.count(), 1, "Duplicate put should not increase count");
}

#[test]
fn test_buffer_put_two_items() {
    let mut buffer = DataBuffer::new(2);
    buffer.put(2, MockPayload { id: 2 });
    buffer.put(1, MockPayload { id: 1 });

    assert_eq!(buffer.count(), 2);

    let p1 = buffer.get(1).unwrap();
    assert_eq!(p1.id, 1);

    let p2 = buffer.get(2).unwrap();
    assert_eq!(p2.id, 2);
}

#[test]
fn test_buffer_put_overflow() {
    let mut buffer = DataBuffer::new(2);
    buffer.put(1, MockPayload { id: 1 });
    buffer.put(2, MockPayload { id: 2 });
    buffer.put(3, MockPayload { id: 3 });

    // Should have evicted position 1 (oldest).
    assert!(buffer.count() <= 2);
    assert!(buffer.get(3).is_some());
    assert_eq!(buffer.get(3).unwrap().id, 3);
    assert_eq!(buffer.get(2).unwrap().id, 2);
}

// ---------------------------------------------------------------------------
// Tests: Buffer resize (port of buffer_resize)
// ---------------------------------------------------------------------------

#[test]
fn test_buffer_resize_same_size() {
    let mut buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    buffer.resize(10);
    assert_eq!(buffer.max(), 10);
}

#[test]
fn test_buffer_resize_increase() {
    let mut buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    buffer.resize(12);
    assert_eq!(buffer.max(), 12);
}

#[test]
fn test_buffer_resize_decrease() {
    let mut buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    buffer.resize(1);
    assert_eq!(buffer.max(), 1);
}

#[test]
fn test_buffer_resize_evicts_excess() {
    let mut buffer = DataBuffer::new(5);
    for i in 1..=5 {
        buffer.put(i, MockPayload { id: i as i32 });
    }
    assert_eq!(buffer.count(), 5);

    buffer.resize(3);
    assert_eq!(buffer.max(), 3);
    assert!(buffer.count() <= 3);
}

// ---------------------------------------------------------------------------
// Tests: Nominal usage (port of buffer_nominal)
// ---------------------------------------------------------------------------

#[test]
fn test_buffer_nominal_fill() {
    let max = 10;
    let mut buffer = DataBuffer::new(max);

    for i in 1..=max {
        let ret = buffer.put(i as u64, MockPayload { id: i as i32 });
        assert_eq!(ret, 0);
    }
    assert_eq!(buffer.count(), max);
}

#[test]
fn test_buffer_nominal_get_all() {
    let max = 10;
    let mut buffer = DataBuffer::new(max);

    for i in 1..=max {
        buffer.put(i as u64, MockPayload { id: i as i32 });
    }

    for i in 1..=max {
        let payload = buffer.get(i as u64).unwrap();
        assert_eq!(payload.id, i as i32);
    }
}

#[test]
fn test_buffer_nominal_replace_all() {
    let max = 10usize;
    let mut buffer = DataBuffer::new(max);

    // Fill with positions 1..=10.
    for i in 1..=max {
        buffer.put(i as u64, MockPayload { id: 0 });
    }

    // Fill with positions 11..=20 (replaces 1..=10).
    for i in 1..=max {
        let pos = (i + max) as u64;
        buffer.put(pos, MockPayload { id: i as i32 });
    }

    assert_eq!(buffer.count(), max);

    // Old positions should be gone.
    for i in 1..=max {
        assert!(buffer.get(i as u64).is_none());
    }

    // New positions should be present.
    for i in 1..=max {
        let pos = (i + max) as u64;
        let payload = buffer.get(pos).unwrap();
        assert_eq!(payload.id, i as i32);
    }
}

#[test]
fn test_buffer_nominal_remove_head() {
    let max = 10usize;
    let mut buffer = DataBuffer::new(max);

    for i in 1..=max {
        buffer.put((i + max) as u64, MockPayload { id: i as i32 });
    }

    let removed = buffer.remove_head().unwrap();
    assert_eq!(removed.id, 1);
    assert_eq!(buffer.count(), max - 1);
}

#[test]
fn test_buffer_nominal_remove_by_position() {
    let max = 10usize;
    let mut buffer = DataBuffer::new(max);

    for i in 1..=max {
        buffer.put((i + max) as u64, MockPayload { id: i as i32 });
    }

    let last_pos = (max * 2) as u64;
    let removed = buffer.remove(last_pos).unwrap();
    assert_eq!(removed.id, max as i32);
    assert_eq!(buffer.count(), max - 2 + 1); // Removed head earlier? No, fresh buffer.
    // Actually, we only removed one.
    assert_eq!(buffer.count(), max - 1);
}

#[test]
fn test_buffer_remove_nonexistent() {
    let mut buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    buffer.put(1, MockPayload { id: 1 });
    assert!(buffer.remove(999).is_none());
    assert_eq!(buffer.count(), 1);
}

#[test]
fn test_buffer_remove_head_empty() {
    let mut buffer: DataBuffer<MockPayload> = DataBuffer::new(10);
    assert!(buffer.remove_head().is_none());
}
