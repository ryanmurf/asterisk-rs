//! ast_coredumper - Asterisk core dump analysis helper
//!
//! Assists in analyzing Asterisk core dumps by:
//! - Locating core files in standard locations
//! - Running GDB to extract backtraces
//! - Collecting relevant Asterisk configuration for bug reports
//! - Packaging results into tarballs for submission
//!
//! Port of asterisk/contrib/scripts/ast_coredumper (originally a bash script).

use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// Asterisk core dump analysis helper
#[derive(Parser, Debug)]
#[command(
    name = "ast_coredumper",
    about = "Analyze Asterisk core dumps and extract backtraces"
)]
struct Args {
    /// Core dump file(s) to analyze. If not specified, searches standard locations.
    #[arg()]
    coredumps: Vec<PathBuf>,

    /// Output directory for results
    #[arg(short, long, default_value = "/tmp")]
    output_dir: PathBuf,

    /// Path to GDB binary
    #[arg(long, default_value = "gdb")]
    gdb: String,

    /// Delete core dumps after processing
    #[arg(long)]
    delete_coredumps_after: bool,

    /// Create tarball of results
    #[arg(long)]
    tarball_results: bool,

    /// Include Asterisk config in tarball
    #[arg(long)]
    tarball_config: bool,

    /// Asterisk modules directory
    #[arg(long)]
    moddir: Option<PathBuf>,

    /// Asterisk lib directory
    #[arg(long)]
    libdir: Option<PathBuf>,

    /// Asterisk etc directory (configuration)
    #[arg(long, default_value = "/etc/asterisk")]
    etcdir: PathBuf,

    /// Analyze a running Asterisk process instead of a core file
    #[arg(long)]
    running: bool,

    /// Dry run: show what would be done without actually doing it
    #[arg(long)]
    dry_run: bool,

    /// Find the latest core dump in standard locations
    #[arg(long)]
    latest: bool,
}

/// Standard locations to search for core dump files.
const CORE_SEARCH_PATHS: &[&str] = &[
    "/tmp",
    "/var/lib/asterisk",
    "/var/spool/asterisk",
    "/var/log/asterisk",
    ".",
];

/// Find core dump files in standard locations.
fn find_coredumps(latest_only: bool) -> Vec<PathBuf> {
    let mut cores = Vec::new();

    for search_path in CORE_SEARCH_PATHS {
        let path = Path::new(search_path);
        if !path.is_dir() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("core") && !name_str.ends_with(".txt") {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() && meta.len() > 0 {
                            cores.push(entry.path());
                        }
                    }
                }
            }
        }
    }

    // Sort by modification time, newest first
    cores.sort_by(|a, b| {
        let a_time = fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let b_time = fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    if latest_only {
        cores.truncate(1);
    }

    cores
}

/// GDB commands to extract useful backtrace information.
const GDB_BACKTRACE_COMMANDS: &str = "\
set pagination off
set width 0
thread apply all bt full
info threads
quit
";

/// Run GDB on a core file and extract backtrace.
fn extract_backtrace(
    gdb: &str,
    core_path: &Path,
    output_dir: &Path,
    dry_run: bool,
) -> Result<PathBuf, String> {
    let core_name = core_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let output_file = output_dir.join(format!("{core_name}-backtrace.txt"));

    if dry_run {
        println!("Would run: {gdb} -batch -ex 'thread apply all bt full' on {}", core_path.display());
        println!("Output would go to: {}", output_file.display());
        return Ok(output_file);
    }

    // Write GDB commands to a temp file
    let cmd_file = output_dir.join(format!(".gdb_commands_{}", std::process::id()));
    fs::write(&cmd_file, GDB_BACKTRACE_COMMANDS)
        .map_err(|e| format!("Failed to write GDB command file: {e}"))?;

    // Try to find the asterisk binary
    let asterisk_bins = [
        "/usr/sbin/asterisk",
        "/usr/local/sbin/asterisk",
        "/opt/asterisk/sbin/asterisk",
    ];

    let asterisk_bin = asterisk_bins
        .iter()
        .find(|p| Path::new(p).exists())
        .copied()
        .unwrap_or("asterisk");

    println!("Running GDB on {} ...", core_path.display());

    let result = Command::new(gdb)
        .args([
            "-batch",
            "-x",
            &cmd_file.to_string_lossy(),
            asterisk_bin,
            &core_path.to_string_lossy(),
        ])
        .output();

    // Clean up command file
    let _ = fs::remove_file(&cmd_file);

    match result {
        Ok(output) => {
            let mut content = String::from("=== Asterisk Core Dump Analysis ===\n");
            content.push_str(&format!("Core file: {}\n", core_path.display()));
            content.push_str(&format!(
                "Date: {}\n",
                chrono_lite_now()
            ));
            content.push_str("\n=== GDB Output ===\n");
            content.push_str(&String::from_utf8_lossy(&output.stdout));

            if !output.stderr.is_empty() {
                content.push_str("\n=== GDB Errors ===\n");
                content.push_str(&String::from_utf8_lossy(&output.stderr));
            }

            fs::write(&output_file, &content)
                .map_err(|e| format!("Failed to write output: {e}"))?;

            println!("Backtrace saved to: {}", output_file.display());
            Ok(output_file)
        }
        Err(e) => Err(format!("Failed to run GDB: {e}")),
    }
}

/// Simple timestamp without requiring the chrono crate.
fn chrono_lite_now() -> String {
    // Use the system date command as a simple approach
    Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Create a tarball of results.
fn create_tarball(
    output_dir: &Path,
    files: &[PathBuf],
    include_config: bool,
    etcdir: &Path,
    dry_run: bool,
) -> Result<PathBuf, String> {
    let tarball_path = output_dir.join(format!(
        "asterisk-coredump-{}.tar.gz",
        std::process::id()
    ));

    if dry_run {
        println!("Would create tarball: {}", tarball_path.display());
        return Ok(tarball_path);
    }

    let mut args = vec![
        "czf".to_string(),
        tarball_path.to_string_lossy().to_string(),
    ];

    for f in files {
        args.push(f.to_string_lossy().to_string());
    }

    if include_config && etcdir.is_dir() {
        args.push(etcdir.to_string_lossy().to_string());
    }

    let result = Command::new("tar").args(&args).output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("Tarball created: {}", tarball_path.display());
                Ok(tarball_path)
            } else {
                Err(format!(
                    "tar failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        }
        Err(e) => Err(format!("Failed to run tar: {e}")),
    }
}

fn main() {
    let args = Args::parse();

    // Ensure output directory exists
    if !args.output_dir.is_dir() {
        if let Err(e) = fs::create_dir_all(&args.output_dir) {
            eprintln!("Cannot create output directory: {e}");
            process::exit(1);
        }
    }

    // Find core dumps
    let coredumps = if args.coredumps.is_empty() {
        let found = find_coredumps(args.latest);
        if found.is_empty() {
            eprintln!("No core dump files found in standard locations.");
            eprintln!("Searched: {:?}", CORE_SEARCH_PATHS);
            eprintln!("Specify core dump path(s) on the command line.");
            process::exit(1);
        }
        println!("Found {} core dump(s)", found.len());
        found
    } else {
        // Validate provided paths
        for path in &args.coredumps {
            if !path.exists() {
                eprintln!("Core dump not found: {}", path.display());
                process::exit(1);
            }
        }
        args.coredumps.clone()
    };

    let mut result_files = Vec::new();
    let mut errors = 0;

    for core_path in &coredumps {
        println!("\n--- Processing: {} ---", core_path.display());

        match extract_backtrace(&args.gdb, core_path, &args.output_dir, args.dry_run) {
            Ok(output_file) => {
                result_files.push(output_file);
            }
            Err(e) => {
                eprintln!("Error processing {}: {e}", core_path.display());
                errors += 1;
            }
        }

        if args.delete_coredumps_after && !args.dry_run {
            if let Err(e) = fs::remove_file(core_path) {
                eprintln!(
                    "Warning: could not delete {}: {e}",
                    core_path.display()
                );
            } else {
                println!("Deleted: {}", core_path.display());
            }
        }
    }

    // Create tarball if requested
    if args.tarball_results && !result_files.is_empty() {
        match create_tarball(
            &args.output_dir,
            &result_files,
            args.tarball_config,
            &args.etcdir,
            args.dry_run,
        ) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error creating tarball: {e}");
                errors += 1;
            }
        }
    }

    // Summary
    println!("\n=== Summary ===");
    println!("Core dumps processed: {}", coredumps.len());
    println!("Results generated: {}", result_files.len());
    if errors > 0 {
        println!("Errors: {errors}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_coredumps_no_crash() {
        // Should not panic even if directories don't exist
        let _cores = find_coredumps(false);
    }

    #[test]
    fn test_chrono_lite_now() {
        let ts = chrono_lite_now();
        // Should return something non-empty (may be "unknown" if date command fails)
        assert!(!ts.is_empty());
    }
}
