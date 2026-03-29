//! Port of asterisk/tests/test_file.c
//!
//! Tests file operations: directory reading, path handling, file format
//! detection, and temporary file creation.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a temporary directory for testing.
fn create_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{}_{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("Failed to create temp dir");
    dir
}

/// Create temporary files in a directory.
fn create_temp_files(dir: &Path, count: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for i in 0..count {
        let path = dir.join(format!("test_file_{}.txt", i));
        let mut f = fs::File::create(&path).expect("Failed to create temp file");
        write!(f, "content of file {}", i).unwrap();
        files.push(path);
    }
    files
}

/// Clean up a temporary directory and its contents.
fn cleanup_temp_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Directory reading
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(read_dirs_test) from test_file.c.
///
/// Test reading a directory's contents and finding a specific file.
#[test]
fn test_read_directory() {
    let dir = create_temp_dir("test_read_dir");
    let files = create_temp_files(&dir, 5);

    // Read directory entries.
    let entries: Vec<String> = fs::read_dir(&dir)
        .expect("Failed to read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert_eq!(entries.len(), 5);

    // Find a specific file.
    let target = files[2].file_name().unwrap().to_string_lossy().to_string();
    assert!(entries.contains(&target));

    cleanup_temp_dir(&dir);
}

/// Test reading a directory with subdirectories.
#[test]
fn test_read_directory_with_subdirs() {
    let dir = create_temp_dir("test_read_subdir");
    create_temp_files(&dir, 3);

    let subdir = dir.join("subdir");
    fs::create_dir(&subdir).unwrap();
    create_temp_files(&subdir, 2);

    // Read top-level entries.
    let entries: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    // Should have 3 files + 1 subdir = 4 entries.
    assert_eq!(entries.len(), 4);
    assert!(entries.contains(&"subdir".to_string()));

    cleanup_temp_dir(&dir);
}

/// Test reading an empty directory.
#[test]
fn test_read_empty_directory() {
    let dir = create_temp_dir("test_empty_dir");

    let entries: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(entries.is_empty());

    cleanup_temp_dir(&dir);
}

// ---------------------------------------------------------------------------
// Path handling
// ---------------------------------------------------------------------------

/// Test path component extraction.
#[test]
fn test_path_components() {
    let path = Path::new("/usr/share/asterisk/sounds/en/hello.gsm");

    assert_eq!(path.file_name().unwrap().to_str().unwrap(), "hello.gsm");
    assert_eq!(
        path.parent().unwrap().to_str().unwrap(),
        "/usr/share/asterisk/sounds/en"
    );
    assert_eq!(path.extension().unwrap().to_str().unwrap(), "gsm");
    assert_eq!(path.file_stem().unwrap().to_str().unwrap(), "hello");
}

/// Test path joining.
#[test]
fn test_path_joining() {
    let base = Path::new("/var/spool/asterisk");
    let joined = base.join("voicemail/default/1234/INBOX");

    assert_eq!(
        joined.to_str().unwrap(),
        "/var/spool/asterisk/voicemail/default/1234/INBOX"
    );
}

/// Test relative path handling.
#[test]
fn test_relative_paths() {
    let path = Path::new("sounds/en/hello.gsm");
    assert!(path.is_relative());
    assert!(!path.is_absolute());

    let abs = Path::new("/etc/asterisk/asterisk.conf");
    assert!(abs.is_absolute());
    assert!(!abs.is_relative());
}

// ---------------------------------------------------------------------------
// File format detection
// ---------------------------------------------------------------------------

/// Test extension-based format detection.
#[test]
fn test_file_format_detection() {
    fn detect_format(filename: &str) -> &str {
        match Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("wav") | Some("WAV") => "wav",
            Some("gsm") => "gsm",
            Some("ulaw") | Some("ul") => "ulaw",
            Some("alaw") | Some("al") => "alaw",
            Some("sln") | Some("slin") | Some("raw") => "slin",
            Some("g729") => "g729",
            Some("g722") => "g722",
            Some("ogg") => "ogg_vorbis",
            Some("mp3") => "mp3",
            _ => "unknown",
        }
    }

    assert_eq!(detect_format("hello.wav"), "wav");
    assert_eq!(detect_format("hello.WAV"), "wav");
    assert_eq!(detect_format("hello.gsm"), "gsm");
    assert_eq!(detect_format("hello.ulaw"), "ulaw");
    assert_eq!(detect_format("hello.alaw"), "alaw");
    assert_eq!(detect_format("hello.sln"), "slin");
    assert_eq!(detect_format("hello.g729"), "g729");
    assert_eq!(detect_format("hello.ogg"), "ogg_vorbis");
    assert_eq!(detect_format("hello.xyz"), "unknown");
    assert_eq!(detect_format("hello"), "unknown");
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

/// Test creating and reading a temporary file.
#[test]
fn test_temp_file_create_read() {
    let dir = create_temp_dir("test_file_ops");
    let path = dir.join("test.txt");

    // Write.
    {
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "Hello, Asterisk!").unwrap();
    }

    // Read.
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, "Hello, Asterisk!");

    cleanup_temp_dir(&dir);
}

/// Test file existence checking.
#[test]
fn test_file_exists() {
    let dir = create_temp_dir("test_file_exists");
    let path = dir.join("exists.txt");

    assert!(!path.exists());

    fs::File::create(&path).unwrap();
    assert!(path.exists());

    fs::remove_file(&path).unwrap();
    assert!(!path.exists());

    cleanup_temp_dir(&dir);
}

/// Test file metadata (size).
#[test]
fn test_file_size() {
    let dir = create_temp_dir("test_file_size");
    let path = dir.join("sized.txt");

    {
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "12345678901234567890").unwrap(); // 20 bytes
    }

    let metadata = fs::metadata(&path).unwrap();
    assert_eq!(metadata.len(), 20);

    cleanup_temp_dir(&dir);
}

/// Test finding files by iterating directory entries.
#[test]
fn test_find_file_in_directory() {
    let dir = create_temp_dir("test_find_file");
    create_temp_files(&dir, 10);

    let target = "test_file_5.txt";
    let found = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy() == target);

    assert!(found, "Should find {} in directory", target);

    cleanup_temp_dir(&dir);
}

/// Test that all created files are unique.
#[test]
fn test_unique_file_names() {
    let dir = create_temp_dir("test_unique");
    create_temp_files(&dir, 20);

    let names: HashSet<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert_eq!(names.len(), 20);

    cleanup_temp_dir(&dir);
}
