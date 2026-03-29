//! astcanary - Watchdog process for Asterisk realtime priority monitoring
//!
//! When Asterisk runs at realtime priority (-p), this canary process runs at
//! normal priority. It periodically touches a monitor file. If a runaway
//! realtime thread consumes all CPU, the canary won't get scheduled and the
//! file timestamp goes stale, allowing Asterisk to detect the problem and
//! deprioritize itself.
//!
//! Port of asterisk/utils/astcanary.c

use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

/// Asterisk canary watchdog process
#[derive(Parser, Debug)]
#[command(name = "astcanary", about = "Asterisk canary watchdog process")]
struct Args {
    /// Path to the monitor file whose mtime is checked by Asterisk
    monitor_file: PathBuf,

    /// Parent PID to watch (exit when parent dies)
    ppid: u32,
}

/// Explanation written into the canary file when it is created.
const EXPLANATION: &str = "\
This file is created when Asterisk is run with a realtime priority (-p).  It\n\
must continue to exist, and the astcanary process must be allowed to continue\n\
running, or else the Asterisk process will, within a short period of time,\n\
slow itself down to regular priority.\n\
\n\
The technical explanation for this file is to provide an assurance to Asterisk\n\
that there are no threads that have gone into runaway mode, thus hogging the\n\
CPU, and making the Asterisk machine seem to be unresponsive.  When that\n\
happens, the astcanary process will be unable to update the timestamp on this\n\
file, and Asterisk will notice within 120 seconds and react.  Slowing the\n\
Asterisk process down to regular priority will permit an administrator to\n\
intervene, thus avoiding a need to reboot the entire machine.\n";

/// Return the current parent PID.
///
/// On Unix systems, when the original parent exits, the child is reparented
/// (usually to PID 1). We detect that change to know the parent died.
///
/// We use std::process::Command to call `ps` rather than requiring the libc crate.
fn get_ppid() -> u32 {
    #[cfg(unix)]
    {
        // Read /proc/self/stat on Linux, or use sysctl on macOS
        // Simplest cross-unix approach: read ppid from /proc or ps command
        if let Ok(stat) = fs::read_to_string("/proc/self/stat") {
            // Format: pid (comm) state ppid ...
            // Find the closing paren first to handle comm fields with spaces
            if let Some(paren_end) = stat.rfind(')') {
                let after_comm = &stat[paren_end + 2..]; // skip ") "
                let fields: Vec<&str> = after_comm.split_whitespace().collect();
                // fields[0] = state, fields[1] = ppid
                if fields.len() > 1 {
                    if let Ok(ppid) = fields[1].parse::<u32>() {
                        return ppid;
                    }
                }
            }
        }

        // Fallback: use ps command (works on macOS and BSDs)
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
            .output()
        {
            if let Ok(ppid_str) = String::from_utf8(output.stdout) {
                if let Ok(ppid) = ppid_str.trim().parse::<u32>() {
                    return ppid;
                }
            }
        }

        0
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Touch the file (update mtime) or recreate it if missing.
fn touch_or_create(path: &PathBuf) -> Result<(), std::io::Error> {
    // Try to update modification time by setting it to "now"
    if path.exists() {
        // Use filetime-style touch: open and immediately close to update mtime
        // Actually, we need to use utime/utimensat. Simplest: write nothing, just
        // re-set the modification time by opening for append and closing.
        let file = fs::OpenOptions::new().append(true).open(path);
        match file {
            Ok(f) => {
                // Set mtime to now by using set_len to current length (no-op but touches metadata)
                let meta = f.metadata()?;
                f.set_len(meta.len())?;
                return Ok(());
            }
            Err(_) => {
                // Fall through to recreate
            }
        }
    }

    // Recreate the file
    let mut f = fs::File::create(path)?;
    f.write_all(EXPLANATION.as_bytes())?;
    Ok(())
}

fn main() {
    let args = Args::parse();

    // Run at normal priority (best effort - ignore errors on platforms that
    // don't support it). Use `nice` or `renice` command instead of libc.
    #[cfg(unix)]
    {
        let pid = std::process::id().to_string();
        let _ = std::process::Command::new("renice")
            .args(["0", "-p", &pid])
            .output();
    }

    // Loop while our original parent is still alive
    loop {
        if get_ppid() != args.ppid {
            // Parent died, exit gracefully
            break;
        }

        if let Err(e) = touch_or_create(&args.monitor_file) {
            eprintln!("astcanary: failed to update monitor file: {e}");
            process::exit(1);
        }

        thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_touch_or_create_new_file() {
        let path = PathBuf::from("/tmp/astcanary_test_file");
        let _ = fs::remove_file(&path);

        assert!(touch_or_create(&path).is_ok());
        assert!(path.exists());

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("realtime priority"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_touch_or_create_existing_file() {
        let path = PathBuf::from("/tmp/astcanary_test_file2");
        fs::write(&path, "existing").unwrap();

        assert!(touch_or_create(&path).is_ok());
        assert!(path.exists());

        let _ = fs::remove_file(&path);
    }
}
