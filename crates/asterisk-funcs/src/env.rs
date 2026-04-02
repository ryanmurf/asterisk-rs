//! ENV, STAT, FILE, and related environment/filesystem functions.
//!
//! Port of func_env.c from Asterisk C.
//!
//! Provides:
//! - ENV(name) - read/write environment variables
//! - STAT(flag,filename) - file stat information
//! - FILE(filename,offset,length) - read file contents
//! - FILE_COUNT_LINE(filename) - count lines in file
//! - FILE_FORMAT(filename) - detect audio file format

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

/// ENV() function.
///
/// Reads or writes OS environment variables.
///
/// Read usage:  ENV(HOME) -> "/home/user"
/// Write usage: Set(ENV(MY_VAR)=value)
pub struct FuncEnv;

impl DialplanFunc for FuncEnv {
    fn name(&self) -> &str {
        "ENV"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let name = args.trim();
        if name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "ENV: variable name is required".to_string(),
            ));
        }
        Ok(std::env::var(name).unwrap_or_default())
    }

    fn write(&self, _ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let name = args.trim();
        if name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "ENV: variable name is required".to_string(),
            ));
        }
        if value.is_empty() {
            std::env::remove_var(name);
        } else {
            std::env::set_var(name, value);
        }
        Ok(())
    }
}

/// STAT() function.
///
/// Returns information about a file.
///
/// Usage: STAT(flag,filename)
///
/// Flags:
///   d - 1 if directory, 0 otherwise
///   e - 1 if exists, 0 otherwise
///   f - 1 if regular file, 0 otherwise
///   l - 1 if symlink, 0 otherwise
///   s - file size in bytes
///   A - last access time (Unix timestamp)
///   m - last modification time (Unix timestamp)
pub struct FuncStat;

impl DialplanFunc for FuncStat {
    fn name(&self) -> &str {
        "STAT"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "STAT: syntax is STAT(flag,filename)".to_string(),
            ));
        }
        let flag = parts[0].trim();
        let filename = parts[1].trim();

        if filename.is_empty() {
            return Err(FuncError::InvalidArgument(
                "STAT: filename is required".to_string(),
            ));
        }

        match flag {
            "e" => {
                let exists = std::path::Path::new(filename).exists();
                Ok(if exists { "1" } else { "0" }.to_string())
            }
            "d" => {
                let is_dir = std::path::Path::new(filename).is_dir();
                Ok(if is_dir { "1" } else { "0" }.to_string())
            }
            "f" => {
                let is_file = std::path::Path::new(filename).is_file();
                Ok(if is_file { "1" } else { "0" }.to_string())
            }
            "l" => {
                let is_symlink = fs::symlink_metadata(filename)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);
                Ok(if is_symlink { "1" } else { "0" }.to_string())
            }
            "s" => {
                let size = fs::metadata(filename)
                    .map(|m| m.len())
                    .unwrap_or(0);
                Ok(size.to_string())
            }
            "A" | "a" => {
                let time = fs::metadata(filename)
                    .and_then(|m| m.accessed())
                    .and_then(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .map_err(|e| std::io::Error::other(e))
                    })
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Ok(time.to_string())
            }
            "m" | "M" => {
                let time = fs::metadata(filename)
                    .and_then(|m| m.modified())
                    .and_then(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .map_err(|e| std::io::Error::other(e))
                    })
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Ok(time.to_string())
            }
            other => Err(FuncError::InvalidArgument(format!(
                "STAT: unknown flag '{}', expected e|d|f|l|s|A|m",
                other
            ))),
        }
    }
}

/// FILE() function.
///
/// Reads the contents of a file.
///
/// Usage: FILE(filename[,offset[,length]])
///
/// offset: byte offset to start reading (default 0)
/// length: maximum bytes to read (default: entire file)
pub struct FuncFile;

impl DialplanFunc for FuncFile {
    fn name(&self) -> &str {
        "FILE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        let filename = parts
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                FuncError::InvalidArgument("FILE: filename is required".to_string())
            })?;

        let offset: u64 = parts
            .get(1)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        let max_length: Option<usize> = parts.get(2).and_then(|s| s.trim().parse().ok());

        let mut file = fs::File::open(filename).map_err(|e| {
            FuncError::Internal(format!("FILE: cannot open '{}': {}", filename, e))
        })?;

        if offset > 0 {
            file.seek(SeekFrom::Start(offset)).map_err(|e| {
                FuncError::Internal(format!("FILE: seek failed: {}", e))
            })?;
        }

        let mut contents = String::new();
        if let Some(len) = max_length {
            let mut buf = vec![0u8; len];
            let n = file.read(&mut buf).map_err(|e| {
                FuncError::Internal(format!("FILE: read failed: {}", e))
            })?;
            contents = String::from_utf8_lossy(&buf[..n]).to_string();
        } else {
            file.read_to_string(&mut contents).map_err(|e| {
                FuncError::Internal(format!("FILE: read failed: {}", e))
            })?;
        }

        Ok(contents)
    }
}

/// FILE_COUNT_LINE() function.
///
/// Counts the number of lines in a file.
///
/// Usage: FILE_COUNT_LINE(filename) -> line count as string
pub struct FuncFileCountLine;

impl DialplanFunc for FuncFileCountLine {
    fn name(&self) -> &str {
        "FILE_COUNT_LINE"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let filename = args.trim();
        if filename.is_empty() {
            return Err(FuncError::InvalidArgument(
                "FILE_COUNT_LINE: filename is required".to_string(),
            ));
        }

        let file = fs::File::open(filename).map_err(|e| {
            FuncError::Internal(format!(
                "FILE_COUNT_LINE: cannot open '{}': {}",
                filename, e
            ))
        })?;
        let reader = BufReader::new(file);
        let count = reader.lines().count();
        Ok(count.to_string())
    }
}

/// FILE_FORMAT() function.
///
/// Detects the audio file format from a filename's extension.
///
/// Usage: FILE_FORMAT(filename) -> format name (e.g., "wav", "gsm", "sln")
pub struct FuncFileFormat;

impl DialplanFunc for FuncFileFormat {
    fn name(&self) -> &str {
        "FILE_FORMAT"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let filename = args.trim();
        if filename.is_empty() {
            return Err(FuncError::InvalidArgument(
                "FILE_FORMAT: filename is required".to_string(),
            ));
        }

        let path = std::path::Path::new(filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // Map extensions to format names
        let format = match ext.to_lowercase().as_str() {
            "wav" | "wav49" => "wav",
            "gsm" => "gsm",
            "sln" | "raw" | "slin" => "sln",
            "pcm" | "ul" | "ulaw" => "pcm",
            "alaw" | "al" => "alaw",
            "g729" => "g729",
            "g723" | "g723sf" => "g723",
            "g726" | "g726-16" | "g726-24" | "g726-32" | "g726-40" => "g726",
            "h263" => "h263",
            "h264" => "h264",
            "ogg" | "opus" => "ogg_opus",
            "vox" => "vox",
            _ => "unknown",
        };

        Ok(format.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_read() {
        let ctx = FuncContext::new();
        let func = FuncEnv;
        // PATH should exist on any system
        let result = func.read(&ctx, "PATH").unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_env_nonexistent() {
        let ctx = FuncContext::new();
        let func = FuncEnv;
        let result = func.read(&ctx, "AST_RS_NONEXISTENT_VAR_12345").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_stat_nonexistent() {
        let ctx = FuncContext::new();
        let func = FuncStat;
        assert_eq!(func.read(&ctx, "e,/nonexistent/file/xyz").unwrap(), "0");
    }

    #[test]
    fn test_stat_exists() {
        let ctx = FuncContext::new();
        let func = FuncStat;
        // /tmp should exist
        assert_eq!(func.read(&ctx, "e,/tmp").unwrap(), "1");
        assert_eq!(func.read(&ctx, "d,/tmp").unwrap(), "1");
    }

    #[test]
    fn test_file_format_detection() {
        let ctx = FuncContext::new();
        let func = FuncFileFormat;
        assert_eq!(func.read(&ctx, "test.wav").unwrap(), "wav");
        assert_eq!(func.read(&ctx, "prompt.gsm").unwrap(), "gsm");
        assert_eq!(func.read(&ctx, "audio.sln").unwrap(), "sln");
        assert_eq!(func.read(&ctx, "recording.g729").unwrap(), "g729");
        assert_eq!(func.read(&ctx, "video.h264").unwrap(), "h264");
    }
}
