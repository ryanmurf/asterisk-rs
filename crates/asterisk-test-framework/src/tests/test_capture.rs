//! Port of asterisk/tests/test_capture.c
//!
//! Tests process output capture:
//!
//! - Capturing exit code from `true` (exit 0)
//! - Capturing exit code from `false` (exit 1)
//! - Capturing stdout from a stdin-transforming command (base64)
//! - Capturing stdout from a command with dynamic arguments
//! - Capturing both stdout and stderr separately
//!
//! Uses std::process::Command for actual process execution.

use std::process::Command;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(test_capture_true).
///
/// The `true` command should exit with code 0 and produce no output.
#[test]
fn test_capture_true() {
    let output = Command::new("true").output().expect("Failed to run true");

    assert!(output.stdout.is_empty(), "unexpected stdout from true");
    assert!(output.stderr.is_empty(), "unexpected stderr from true");
    assert!(output.status.success());
    assert_eq!(output.status.code(), Some(0));
}

/// Port of AST_TEST_DEFINE(test_capture_false).
///
/// The `false` command should exit with code 1 and produce no output.
#[test]
fn test_capture_false() {
    let output = Command::new("false").output().expect("Failed to run false");

    assert!(output.stdout.is_empty(), "unexpected stdout from false");
    assert!(output.stderr.is_empty(), "unexpected stderr from false");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
}

/// Port of AST_TEST_DEFINE(test_capture_with_stdin).
///
/// Feed "Mary had a little lamb." to base64 and verify the output.
#[test]
fn test_capture_with_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("base64")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn base64");

    let data = b"Mary had a little lamb.";
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(data)
        .expect("Failed to write stdin");
    // Close stdin to signal EOF
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("Failed to wait for base64");

    let expected = "TWFyeSBoYWQgYSBsaXR0bGUgbGFtYi4=";
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout_str.trim();
    assert_eq!(trimmed, expected, "base64 output mismatch");
    assert!(output.stderr.is_empty(), "unexpected stderr");
    assert!(output.status.success());
}

/// Port of AST_TEST_DEFINE(test_capture_with_dynamic).
///
/// Run `printf` with a dynamic argument and verify stdout.
#[test]
fn test_capture_with_dynamic() {
    let test_string = "hello_dynamic_test";
    let output = Command::new("printf")
        .arg("%s")
        .arg(test_string)
        .output()
        .expect("Failed to run printf");

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout_str.as_ref(), test_string);
    assert!(output.status.success());
}

/// Port of AST_TEST_DEFINE(test_capture_stdout_stderr).
///
/// Capture both stdout and stderr from a shell command.
/// Note: we use printf instead of echo -n for macOS compatibility.
#[test]
fn test_capture_stdout_stderr() {
    let output = Command::new("sh")
        .arg("-c")
        .arg("printf 'foo' >&2 ; printf 'zzz' >&1 ; printf 'bar' >&2")
        .output()
        .expect("Failed to run sh");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "zzz",
        "unexpected stdout"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foobar",
        "unexpected stderr"
    );
    assert!(output.status.success());
}
