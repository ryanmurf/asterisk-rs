//! Port of asterisk/tests/test_poll.c
//!
//! Tests poll/select I/O readiness detection. The C test uses pipes and
//! /dev/null to test poll behavior. In Rust we use mio or standard I/O
//! primitives:
//! - Write-ready detection (writing to a sink)
//! - Read-ready detection (reading from a source with data)
//! - Read-blocked detection (no data available)
//! - Timeout behavior
//! - Multiple fd readiness counting
//!
//! Since we don't want a libc dependency, we use std I/O and verify
//! the behavioral guarantees through standard Rust mechanisms.

use std::io::Write;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Pipe helper
// ---------------------------------------------------------------------------

fn create_pipe() -> (std::fs::File, std::fs::File) {
    let mut fds = [0i32; 2];
    let result = unsafe { libc_pipe(fds.as_mut_ptr()) };
    assert_eq!(result, 0, "pipe() failed");
    unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    }
}

/// Minimal pipe syscall wrapper (no libc crate needed).
unsafe fn libc_pipe(fds: *mut i32) -> i32 {
    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
    }
    unsafe { pipe(fds) }
}

/// Minimal poll syscall wrapper.
unsafe fn sys_poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32 {
    extern "C" {
        fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    }
    unsafe { poll(fds, nfds, timeout) }
}

const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// Wraps the poll syscall.
fn do_poll(fds: &mut [PollFd], timeout_ms: i32) -> i32 {
    unsafe { sys_poll(fds.as_mut_ptr(), fds.len() as u64, timeout_ms) }
}

// ---------------------------------------------------------------------------
// Tests: Write-ready detection
// ---------------------------------------------------------------------------

/// Port of poll_test from test_poll.c.
/// /dev/null is always writable.
#[test]
fn test_poll_devnull_writable() {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let fd = file.as_raw_fd();

    let mut fds = [PollFd {
        fd,
        events: POLLOUT,
        revents: 0,
    }];
    let result = do_poll(&mut fds, 0);

    assert!(result >= 1, "Expected /dev/null to be writable, got {}", result);
}

/// /dev/zero is always readable.
#[test]
fn test_poll_devzero_readable() {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/zero")
        .unwrap();
    let fd = file.as_raw_fd();

    let mut fds = [PollFd {
        fd,
        events: POLLIN,
        revents: 0,
    }];
    let result = do_poll(&mut fds, 0);

    assert!(result >= 1, "Expected /dev/zero to be readable, got {}", result);
}

// ---------------------------------------------------------------------------
// Tests: Pipe read blocking
// ---------------------------------------------------------------------------

/// An empty pipe should not be readable.
#[test]
fn test_poll_pipe_not_readable() {
    let (read_end, _write_end) = create_pipe();
    let fd = read_end.as_raw_fd();

    let mut fds = [PollFd {
        fd,
        events: POLLIN,
        revents: 0,
    }];
    let result = do_poll(&mut fds, 0); // immediate timeout

    assert_eq!(result, 0, "Empty pipe should not be readable");
}

/// A pipe with data should be readable.
#[test]
fn test_poll_pipe_readable_with_data() {
    let (read_end, mut write_end) = create_pipe();
    write_end.write_all(b"hello").unwrap();

    let fd = read_end.as_raw_fd();
    let mut fds = [PollFd {
        fd,
        events: POLLIN,
        revents: 0,
    }];
    let result = do_poll(&mut fds, 100);

    assert!(result >= 1, "Pipe with data should be readable");
}

// ---------------------------------------------------------------------------
// Tests: Multiple fd readiness
// ---------------------------------------------------------------------------

/// Port of the main poll_test that checks 3 fds:
/// - fd[0] = /dev/null (write) -> should be ready
/// - fd[1] = /dev/zero (read) -> should be ready
/// - fd[2] = empty pipe (read) -> should NOT be ready
/// Expected: 2 fds ready.
#[test]
fn test_poll_multiple_fds() {
    let null_file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let zero_file = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/zero")
        .unwrap();
    let (pipe_read, _pipe_write) = create_pipe();

    let mut fds = [
        PollFd {
            fd: null_file.as_raw_fd(),
            events: POLLOUT,
            revents: 0,
        },
        PollFd {
            fd: zero_file.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        },
        PollFd {
            fd: pipe_read.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        },
    ];

    let result = do_poll(&mut fds, 0);
    assert_eq!(result, 2, "Expected 2 ready fds, got {}", result);
}

/// Test with 1ms timeout (should still return 2 ready).
#[test]
fn test_poll_multiple_fds_with_timeout() {
    let null_file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let zero_file = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/zero")
        .unwrap();
    let (pipe_read, _pipe_write) = create_pipe();

    let mut fds = [
        PollFd {
            fd: null_file.as_raw_fd(),
            events: POLLOUT,
            revents: 0,
        },
        PollFd {
            fd: zero_file.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        },
        PollFd {
            fd: pipe_read.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        },
    ];

    let result = do_poll(&mut fds, 1);
    assert_eq!(result, 2, "Expected 2 ready fds with 1ms timeout, got {}", result);
}

// ---------------------------------------------------------------------------
// Tests: Timeout behavior
// ---------------------------------------------------------------------------

/// Test that poll with 0 timeout returns immediately.
#[test]
fn test_poll_zero_timeout() {
    let (pipe_read, _pipe_write) = create_pipe();

    let start = Instant::now();
    let mut fds = [PollFd {
        fd: pipe_read.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    }];
    let _ = do_poll(&mut fds, 0);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "Zero timeout should return quickly"
    );
}

/// Test that poll with short timeout returns within expected time.
#[test]
fn test_poll_short_timeout() {
    let (pipe_read, _pipe_write) = create_pipe();

    let start = Instant::now();
    let mut fds = [PollFd {
        fd: pipe_read.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    }];
    let result = do_poll(&mut fds, 10); // 10ms
    let elapsed = start.elapsed();

    assert_eq!(result, 0);
    assert!(
        elapsed < Duration::from_millis(500),
        "Short timeout took too long"
    );
}

// ---------------------------------------------------------------------------
// Tests: Revents checking
// ---------------------------------------------------------------------------

/// Verify that revents is set correctly for ready fds.
#[test]
fn test_poll_revents() {
    let zero_file = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/zero")
        .unwrap();

    let mut fds = [PollFd {
        fd: zero_file.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    }];

    let result = do_poll(&mut fds, 0);
    assert_eq!(result, 1);
    // revents should have some readable flag set.
    assert!(fds[0].revents != 0, "revents should be non-zero for ready fd");
}
