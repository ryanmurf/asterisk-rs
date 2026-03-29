//! Port of asterisk/tests/test_func_file.c
//!
//! Tests FILE() function read and write operations:
//! - Read with offset and length (byte mode)
//! - Read with offset and length (line mode)
//! - Write/replace operations (byte and line mode)
//! - Negative offset handling
//! - Line counting

use std::io::{Read, Write};

// ---------------------------------------------------------------------------
// Helper: FILE() read emulation
// ---------------------------------------------------------------------------

/// Emulate Asterisk's FILE() read function with offset and length.
///
/// Supports byte mode and line mode ('l' flag).
/// Negative offsets count from the end.
/// Negative lengths count from the end position.
fn file_read(contents: &str, args: &str) -> String {
    let parts: Vec<&str> = args.splitn(4, ',').collect();
    let line_mode = parts.get(2).map_or(false, |f| f.contains('l'));

    if line_mode {
        file_read_line_mode(contents, &parts)
    } else {
        file_read_byte_mode(contents, &parts)
    }
}

fn file_read_byte_mode(contents: &str, parts: &[&str]) -> String {
    let len = contents.len() as isize;

    let offset_str = parts.first().copied().unwrap_or("");
    let length_str = parts.get(1).copied().unwrap_or("");

    if offset_str.is_empty() && length_str.is_empty() {
        return contents.to_string();
    }

    let mut offset: isize = offset_str.parse().unwrap_or(0);
    if offset < 0 {
        offset += len;
    }
    if offset < 0 {
        offset = 0;
    }
    if offset >= len {
        return String::new();
    }

    if length_str.is_empty() {
        return contents[offset as usize..].to_string();
    }

    let mut length: isize = length_str.parse().unwrap_or(0);
    if length < 0 {
        // Negative length: end position is (offset + length + len_of_remaining).
        // Actually: negative length means "end position = offset + length" from absolute.
        length += len - offset;
    }
    if length <= 0 {
        return String::new();
    }

    let start = offset as usize;
    let end = std::cmp::min(start + length as usize, contents.len());
    contents[start..end].to_string()
}

fn file_read_line_mode(contents: &str, parts: &[&str]) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let total = lines.len() as isize;

    let offset_str = parts.first().copied().unwrap_or("");
    let length_str = parts.get(1).copied().unwrap_or("");

    let mut offset: isize = offset_str.parse().unwrap_or(0);
    if offset < 0 {
        offset += total;
    }
    if offset < 0 {
        offset = 0;
    }
    if offset >= total {
        return String::new();
    }

    if length_str.is_empty() {
        return lines[offset as usize..].join("");
    }

    let mut length: isize = length_str.parse().unwrap_or(0);
    if length < 0 {
        length += total - offset;
    }
    if length <= 0 {
        return String::new();
    }

    let start = offset as usize;
    let end = std::cmp::min(start + length as usize, lines.len());
    lines[start..end].join("")
}

// ---------------------------------------------------------------------------
// Helper: FILE() write emulation
// ---------------------------------------------------------------------------

fn file_write(contents: &str, args: &str, value: &str) -> String {
    let parts: Vec<&str> = args.splitn(4, ',').collect();
    let flags = parts.get(2).copied().unwrap_or("");
    let line_mode = flags.contains('l');

    if line_mode {
        file_write_line_mode(contents, &parts, value, flags)
    } else {
        file_write_byte_mode(contents, &parts, value)
    }
}

fn file_write_byte_mode(contents: &str, parts: &[&str], value: &str) -> String {
    let len = contents.len() as isize;

    let offset_str = parts.first().copied().unwrap_or("");
    let length_str = parts.get(1).copied().unwrap_or("");

    if offset_str.is_empty() && length_str.is_empty() {
        return value.to_string();
    }

    let mut offset: isize = offset_str.parse().unwrap_or(0);
    if offset < 0 {
        offset += len;
    }
    if offset < 0 {
        offset = 0;
    }

    if length_str.is_empty() {
        // Truncate at offset and append value.
        let prefix = &contents[..std::cmp::min(offset as usize, contents.len())];
        return format!("{}{}", prefix, value);
    }

    let mut length: isize = length_str.parse().unwrap_or(0);
    if length < 0 {
        length += len - offset;
    }
    if length < 0 {
        length = 0;
    }

    let start = offset as usize;
    let end = std::cmp::min(start + length as usize, contents.len());
    let prefix = &contents[..start];
    let suffix = if end < contents.len() {
        &contents[end..]
    } else {
        ""
    };
    format!("{}{}{}", prefix, value, suffix)
}

fn file_write_line_mode(contents: &str, parts: &[&str], value: &str, flags: &str) -> String {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    let total = lines.len() as isize;
    let append_newline = !flags.contains('d');

    let offset_str = parts.first().copied().unwrap_or("");
    let length_str = parts.get(1).copied().unwrap_or("");

    if offset_str.is_empty() && length_str.is_empty() {
        if append_newline && !value.ends_with('\n') {
            return format!("{}\n", value);
        }
        return value.to_string();
    }

    let mut offset: isize = offset_str.parse().unwrap_or(0);
    if offset < 0 {
        offset += total;
    }
    if offset < 0 {
        offset = 0;
    }

    if length_str.is_empty() {
        let prefix: String = lines[..std::cmp::min(offset as usize, lines.len())].join("");
        if append_newline && !value.ends_with('\n') {
            return format!("{}{}\n", prefix, value);
        }
        return format!("{}{}", prefix, value);
    }

    let mut length: isize = length_str.parse().unwrap_or(0);
    if length < 0 {
        length += total - offset;
    }
    if length <= 0 {
        return contents.to_string();
    }

    let start = offset as usize;
    let end = std::cmp::min(start + length as usize, lines.len());
    let prefix: String = lines[..start].join("");
    let suffix: String = lines[end..].join("");
    let val = if append_newline && !value.ends_with('\n') {
        format!("{}\n", value)
    } else {
        value.to_string()
    };
    format!("{}{}{}", prefix, val, suffix)
}

// ---------------------------------------------------------------------------
// Read tests (byte mode)
// ---------------------------------------------------------------------------

/// Port of read_tests from test_func_file.c -- byte mode tests.
#[test]
fn test_file_read_first_char() {
    assert_eq!(file_read("123456789", "0,1"), "1");
    assert_eq!(file_read("123456789", "0,-8"), "1");
    assert_eq!(file_read("123456789", "-9,1"), "1");
    assert_eq!(file_read("123456789", "-9,-8"), "1");
}

#[test]
fn test_file_read_zero_length() {
    assert_eq!(file_read("123456789", "0,0"), "");
    assert_eq!(file_read("123456789", "-9,0"), "");
    assert_eq!(file_read("123456789", "-9,-9"), "");
}

#[test]
fn test_file_read_no_length() {
    assert_eq!(file_read("123456789", "-5"), "56789");
    assert_eq!(file_read("123456789", "4"), "56789");
}

#[test]
fn test_file_read_past_end() {
    assert_eq!(file_read("123456789", "8,10"), "9");
    assert_eq!(file_read("123456789", "10,1"), "");
}

#[test]
fn test_file_read_middle() {
    assert_eq!(file_read("123456789", "2,5"), "34567");
    assert_eq!(file_read("123456789", "-7,5"), "34567");
}

// ---------------------------------------------------------------------------
// Read tests (line mode)
// ---------------------------------------------------------------------------

/// Port of read_tests from test_func_file.c -- line mode tests.
#[test]
fn test_file_read_line_first() {
    assert_eq!(file_read("123\n456\n789\n", "0,1,l"), "123\n");
    assert_eq!(file_read("123\n456\n789\n", "-3,1,l"), "123\n");
    assert_eq!(file_read("123\n456\n789\n", "0,-2,l"), "123\n");
    assert_eq!(file_read("123\n456\n789\n", "-3,-2,l"), "123\n");
}

#[test]
fn test_file_read_line_zero_length() {
    assert_eq!(file_read("123\n456\n789\n", "0,0,l"), "");
    assert_eq!(file_read("123\n456\n789\n", "-3,0,l"), "");
    assert_eq!(file_read("123\n456\n789\n", "-3,-3,l"), "");
}

#[test]
fn test_file_read_line_no_length() {
    assert_eq!(file_read("123\n456\n789\n", "1,,l"), "456\n789\n");
    assert_eq!(file_read("123\n456\n789\n", "-2,,l"), "456\n789\n");
}

// ---------------------------------------------------------------------------
// Write tests (byte mode)
// ---------------------------------------------------------------------------

/// Port of write_tests from test_func_file.c -- byte mode tests.
#[test]
fn test_file_write_single_char_replace() {
    assert_eq!(file_write("123456789", "0,1", "a"), "a23456789");
    assert_eq!(file_write("123456789", "5,1", "b"), "12345b789");
}

#[test]
fn test_file_write_replace_two_with_one() {
    assert_eq!(file_write("123456789", "0,2", "c"), "c3456789");
    assert_eq!(file_write("123456789", "4,2", "d"), "1234d789");
}

#[test]
fn test_file_write_truncate() {
    assert_eq!(file_write("123456789", "5", "e"), "12345e");
    assert_eq!(file_write("123456789", "5", ""), "12345");
}

#[test]
fn test_file_write_replace_one_with_two() {
    assert_eq!(file_write("123456789", "0,1", "fg"), "fg23456789");
}

#[test]
fn test_file_write_overwrite_entire() {
    assert_eq!(file_write("123456789", "", "h"), "h");
}

// ---------------------------------------------------------------------------
// Write tests (line mode)
// ---------------------------------------------------------------------------

#[test]
fn test_file_write_line_replace_same() {
    assert_eq!(
        file_write("123\n456\n789\n", "0,1,l", "abc"),
        "abc\n456\n789\n"
    );
    assert_eq!(
        file_write("123\n456\n789\n", "1,1,l", "abc"),
        "123\nabc\n789\n"
    );
}

#[test]
fn test_file_write_line_replace_shorter() {
    assert_eq!(
        file_write("123\n456\n789\n", "0,1,l", "ab"),
        "ab\n456\n789\n"
    );
}

#[test]
fn test_file_write_line_replace_longer() {
    assert_eq!(
        file_write("123\n456\n789\n", "0,1,l", "abcd"),
        "abcd\n456\n789\n"
    );
}

// ---------------------------------------------------------------------------
// Line counting
// ---------------------------------------------------------------------------

/// Test line counting in different content.
#[test]
fn test_line_counting() {
    let count_lines = |s: &str| -> usize { s.split('\n').count() - if s.ends_with('\n') { 1 } else { 0 } };

    assert_eq!(count_lines(""), 1); // Empty string has one "line".
    assert_eq!(count_lines("hello"), 1);
    assert_eq!(count_lines("hello\n"), 1);
    assert_eq!(count_lines("a\nb\nc\n"), 3);
    assert_eq!(count_lines("a\nb\nc"), 3);
}
