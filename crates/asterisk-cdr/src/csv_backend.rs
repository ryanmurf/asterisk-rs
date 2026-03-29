//! CSV CDR backend - writes CDR records to CSV files.
//!
//! Port of cdr/cdr_csv.c from Asterisk C.

use crate::{Cdr, CdrBackend, CdrError};
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

/// Date format for CDR timestamps.
/// Date format matching Asterisk C's CDR date output.
#[allow(dead_code)]
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// CSV CDR backend configuration.
#[derive(Debug, Clone)]
pub struct CsvCdrConfig {
    /// Directory where CSV files are written
    pub log_dir: PathBuf,
    /// Master CSV filename
    pub master_file: String,
    /// Whether to use GMT/UTC for timestamps
    pub use_gmt: bool,
    /// Whether to create per-account log files
    pub account_logs: bool,
    /// Whether to log the unique ID field
    pub log_unique_id: bool,
    /// Whether to log the user field
    pub log_user_field: bool,
    /// Whether to include new CDR columns (peeraccount, linkedid)
    pub new_cdr_columns: bool,
}

impl Default for CsvCdrConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("/var/log/asterisk/cdr-csv"),
            master_file: "Master.csv".to_string(),
            use_gmt: false,
            account_logs: true,
            log_unique_id: false,
            log_user_field: false,
            new_cdr_columns: false,
        }
    }
}

/// CSV CDR backend.
///
/// Writes CDR records to CSV files in the configured log directory.
/// The master file contains all CDRs, and optionally per-account
/// files are created for each unique account code.
pub struct CsvCdrBackend {
    config: CsvCdrConfig,
    /// Lock for file writing to prevent interleaved writes
    write_lock: Mutex<()>,
}

impl CsvCdrBackend {
    /// Create a new CSV CDR backend with default configuration.
    pub fn new() -> Self {
        Self {
            config: CsvCdrConfig::default(),
            write_lock: Mutex::new(()),
        }
    }

    /// Create a new CSV CDR backend with the given configuration.
    pub fn with_config(config: CsvCdrConfig) -> Self {
        Self {
            config,
            write_lock: Mutex::new(()),
        }
    }

    /// Format a CDR as a CSV line.
    fn format_csv(&self, cdr: &Cdr) -> String {
        let mut buf = String::with_capacity(512);

        // Standard fields in the traditional Asterisk CDR CSV order:
        // accountcode, src, dst, dcontext, clid, channel, dstchannel,
        // lastapp, lastdata, start, answer, end, duration, billsec,
        // disposition, amaflags
        append_csv_string(&mut buf, &cdr.account_code);
        append_csv_string(&mut buf, &cdr.src);
        append_csv_string(&mut buf, &cdr.dst);
        append_csv_string(&mut buf, &cdr.dst_context);
        append_csv_string(&mut buf, &cdr.caller_id);
        append_csv_string(&mut buf, &cdr.channel);
        append_csv_string(&mut buf, &cdr.dst_channel);
        append_csv_string(&mut buf, &cdr.last_app);
        append_csv_string(&mut buf, &cdr.last_data);
        append_csv_date(&mut buf, cdr.start, self.config.use_gmt);
        append_csv_date_opt(&mut buf, cdr.answer, self.config.use_gmt);
        append_csv_date(&mut buf, cdr.end, self.config.use_gmt);
        append_csv_int(&mut buf, cdr.duration);
        append_csv_int(&mut buf, cdr.billsec);
        append_csv_string(&mut buf, cdr.disposition.as_str());
        append_csv_string(&mut buf, cdr.ama_flags.as_str());

        // Optional fields
        if self.config.log_unique_id {
            append_csv_string(&mut buf, &cdr.unique_id);
        }
        if self.config.log_user_field {
            append_csv_string(&mut buf, &cdr.user_field);
        }
        if self.config.new_cdr_columns {
            append_csv_string(&mut buf, &cdr.peer_account);
            append_csv_string(&mut buf, &cdr.linked_id);
            append_csv_int(&mut buf, cdr.sequence as i64);
        }

        // Remove trailing comma and add newline
        if buf.ends_with(',') {
            buf.pop();
        }
        buf.push('\n');

        buf
    }

    /// Write a CSV line to the master file.
    fn write_to_master(&self, line: &str) -> Result<(), CdrError> {
        let path = self.config.log_dir.join(&self.config.master_file);
        self.write_to_file(&path, line)
    }

    /// Write a CSV line to a per-account file.
    fn write_to_account(&self, account: &str, line: &str) -> Result<(), CdrError> {
        let filename = format!("{}.csv", account);
        let path = self.config.log_dir.join(&filename);
        self.write_to_file(&path, line)
    }

    /// Write a line to a file, creating the directory and file if needed.
    fn write_to_file(&self, path: &Path, line: &str) -> Result<(), CdrError> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open file in append mode
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        file.write_all(line.as_bytes())?;
        file.flush()?;

        Ok(())
    }
}

impl Default for CsvCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for CsvCdrBackend {
    fn name(&self) -> &str {
        "csv"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let _lock = self.write_lock.lock();

        let csv_line = self.format_csv(cdr);

        debug!("CDR CSV: writing record for '{}'", cdr.channel);

        // Write to master file
        self.write_to_master(&csv_line)?;

        // Write to per-account file if configured and account code is set
        if self.config.account_logs && !cdr.account_code.is_empty() {
            self.write_to_account(&cdr.account_code, &csv_line)?;
        }

        Ok(())
    }
}

/// Append a properly escaped CSV string field.
fn append_csv_string(buf: &mut String, s: &str) {
    buf.push('"');
    for ch in s.chars() {
        if ch == '"' {
            buf.push('"'); // Escape double quote by doubling it
        }
        buf.push(ch);
    }
    buf.push('"');
    buf.push(',');
}

/// Append an integer CSV field.
fn append_csv_int(buf: &mut String, n: i64) {
    buf.push_str(&n.to_string());
    buf.push(',');
}

/// Append a timestamp as a CSV field.
fn append_csv_date(buf: &mut String, time: SystemTime, _use_gmt: bool) {
    let formatted = format_system_time(time);
    append_csv_string(buf, &formatted);
}

/// Append an optional timestamp as a CSV field.
fn append_csv_date_opt(buf: &mut String, time: Option<SystemTime>, use_gmt: bool) {
    match time {
        Some(t) => append_csv_date(buf, t, use_gmt),
        None => append_csv_string(buf, ""),
    }
}

/// Format a SystemTime as a date/time string.
/// Uses a simple UTC formatting since we don't depend on chrono.
fn format_system_time(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let total_secs = duration.as_secs();

    // Decompose into date/time components (UTC)
    let days = total_secs / 86400;
    let remaining_secs = total_secs % 86400;
    let hours = remaining_secs / 3600;
    let minutes = (remaining_secs % 3600) / 60;
    let seconds = remaining_secs % 60;

    // Compute year, month, day from days since epoch
    let (year, month, day) = days_to_date(days as i64);

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    let mut year: i64 = 1970;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];

    let mut month = 1u32;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }

    (year, month, days as u32 + 1)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CdrDisposition;

    #[test]
    fn test_csv_format() {
        let backend = CsvCdrBackend::new();
        let mut cdr = Cdr::new("SIP/alice-001".to_string(), "uid-123".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.dst_context = "default".to_string();
        cdr.caller_id = "Alice <5551234>".to_string();
        cdr.disposition = CdrDisposition::Answered;
        cdr.duration = 120;
        cdr.billsec = 90;

        let csv = backend.format_csv(&cdr);
        assert!(csv.contains("\"5551234\""));
        assert!(csv.contains("\"100\""));
        assert!(csv.contains("\"SIP/alice-001\""));
        assert!(csv.contains("\"ANSWERED\""));
        assert!(csv.contains("120"));
        assert!(csv.contains("90"));
    }

    #[test]
    fn test_csv_escaping() {
        let mut buf = String::new();
        append_csv_string(&mut buf, "hello \"world\"");
        assert_eq!(buf, "\"hello \"\"world\"\"\",");
    }

    #[test]
    fn test_format_system_time() {
        // Test epoch time
        let epoch = UNIX_EPOCH;
        let formatted = format_system_time(epoch);
        assert_eq!(formatted, "1970-01-01 00:00:00");
    }

    #[test]
    fn test_days_to_date() {
        assert_eq!(days_to_date(0), (1970, 1, 1));
        assert_eq!(days_to_date(365), (1971, 1, 1));
    }
}
