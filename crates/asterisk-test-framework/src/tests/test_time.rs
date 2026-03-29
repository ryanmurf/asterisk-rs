//! Port of asterisk/tests/test_time.c
//!
//! Tests time handling operations:
//! - Time unit string parsing (ns, us, ms, s, m, h, d, w, mo, y)
//! - Time creation by unit value
//! - Time creation by unit string
//! - Timeval to microseconds conversion
//! - Duration arithmetic
//! - Epoch conversions

use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Time unit enum mirroring Asterisk's TIME_UNIT_*
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeUnit {
    Nanosecond,
    Microsecond,
    Millisecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// Parse a time unit string (case-insensitive).
///
/// Port of ast_time_str_to_unit from Asterisk.
fn time_str_to_unit(s: &str) -> Option<TimeUnit> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "ns" | "nsec" | "nanosecond" | "nanoseconds" => Some(TimeUnit::Nanosecond),
        "us" | "usec" | "microsecond" | "microseconds" => Some(TimeUnit::Microsecond),
        "ms" | "msec" | "millisecond" | "milliseconds" => Some(TimeUnit::Millisecond),
        "s" | "sec" | "second" | "seconds" => Some(TimeUnit::Second),
        "m" | "min" | "minute" | "minutes" => Some(TimeUnit::Minute),
        "h" | "hr" | "hour" | "hours" => Some(TimeUnit::Hour),
        "d" | "day" | "days" => Some(TimeUnit::Day),
        "w" | "wk" | "week" | "weeks" => Some(TimeUnit::Week),
        "mo" | "mth" | "month" | "months" => Some(TimeUnit::Month),
        "y" | "yr" | "year" | "years" => Some(TimeUnit::Year),
        _ => None,
    }
}

/// Timeval equivalent (seconds + microseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimeVal {
    tv_sec: i64,
    tv_usec: i64,
}

impl TimeVal {
    fn new(sec: i64, usec: i64) -> Self {
        // Normalize: ensure usec is in [0, 999999].
        let total_usec = sec * 1_000_000 + usec;
        let norm_sec = total_usec / 1_000_000;
        let norm_usec = total_usec % 1_000_000;
        Self {
            tv_sec: norm_sec,
            tv_usec: norm_usec,
        }
    }

    fn to_usec(&self) -> i64 {
        self.tv_sec * 1_000_000 + self.tv_usec
    }
}

/// Create a timeval from a value and unit.
///
/// Port of ast_time_create_by_unit from Asterisk.
fn time_create_by_unit(value: i64, unit: TimeUnit) -> TimeVal {
    let usec = match unit {
        TimeUnit::Nanosecond => value / 1000,
        TimeUnit::Microsecond => value,
        TimeUnit::Millisecond => value * 1_000,
        TimeUnit::Second => value * 1_000_000,
        TimeUnit::Minute => value * 60 * 1_000_000,
        TimeUnit::Hour => value * 3600 * 1_000_000,
        TimeUnit::Day => value * 86400 * 1_000_000,
        TimeUnit::Week => value * 604800 * 1_000_000,
        TimeUnit::Month => value * 2629746 * 1_000_000,
        TimeUnit::Year => value * 31556952 * 1_000_000,
    };
    TimeVal::new(usec / 1_000_000, usec % 1_000_000)
}

/// Create a timeval from a value and unit string.
///
/// Port of ast_time_create_by_unit_str from Asterisk.
fn time_create_by_unit_str(value: i64, unit_str: &str) -> TimeVal {
    let unit = time_str_to_unit(unit_str).expect("Invalid time unit string");
    time_create_by_unit(value, unit)
}

// ---------------------------------------------------------------------------
// Tests: Time unit string parsing (port of test_time_str_to_unit)
// ---------------------------------------------------------------------------

#[test]
fn test_time_str_to_unit_abbreviations() {
    assert_eq!(time_str_to_unit("ns"), Some(TimeUnit::Nanosecond));
    assert_eq!(time_str_to_unit("us"), Some(TimeUnit::Microsecond));
    assert_eq!(time_str_to_unit("ms"), Some(TimeUnit::Millisecond));
    assert_eq!(time_str_to_unit("s"), Some(TimeUnit::Second));
    assert_eq!(time_str_to_unit("m"), Some(TimeUnit::Minute));
    assert_eq!(time_str_to_unit("h"), Some(TimeUnit::Hour));
    assert_eq!(time_str_to_unit("d"), Some(TimeUnit::Day));
    assert_eq!(time_str_to_unit("w"), Some(TimeUnit::Week));
    assert_eq!(time_str_to_unit("mo"), Some(TimeUnit::Month));
    assert_eq!(time_str_to_unit("y"), Some(TimeUnit::Year));
}

#[test]
fn test_time_str_to_unit_plural() {
    assert_eq!(time_str_to_unit("nanoseconds"), Some(TimeUnit::Nanosecond));
    assert_eq!(time_str_to_unit("microseconds"), Some(TimeUnit::Microsecond));
    assert_eq!(time_str_to_unit("milliseconds"), Some(TimeUnit::Millisecond));
    assert_eq!(time_str_to_unit("seconds"), Some(TimeUnit::Second));
    assert_eq!(time_str_to_unit("minutes"), Some(TimeUnit::Minute));
    assert_eq!(time_str_to_unit("hours"), Some(TimeUnit::Hour));
    assert_eq!(time_str_to_unit("days"), Some(TimeUnit::Day));
    assert_eq!(time_str_to_unit("weeks"), Some(TimeUnit::Week));
    assert_eq!(time_str_to_unit("months"), Some(TimeUnit::Month));
    assert_eq!(time_str_to_unit("years"), Some(TimeUnit::Year));
}

#[test]
fn test_time_str_to_unit_case_insensitive() {
    assert_eq!(time_str_to_unit("Nsec"), Some(TimeUnit::Nanosecond));
    assert_eq!(time_str_to_unit("Usec"), Some(TimeUnit::Microsecond));
    assert_eq!(time_str_to_unit("Msec"), Some(TimeUnit::Millisecond));
    assert_eq!(time_str_to_unit("Sec"), Some(TimeUnit::Second));
    assert_eq!(time_str_to_unit("Min"), Some(TimeUnit::Minute));
    assert_eq!(time_str_to_unit("Hr"), Some(TimeUnit::Hour));
    assert_eq!(time_str_to_unit("Day"), Some(TimeUnit::Day));
    assert_eq!(time_str_to_unit("Wk"), Some(TimeUnit::Week));
    assert_eq!(time_str_to_unit("Mth"), Some(TimeUnit::Month));
    assert_eq!(time_str_to_unit("Yr"), Some(TimeUnit::Year));
}

#[test]
fn test_time_str_to_unit_invalid() {
    assert_eq!(time_str_to_unit("xyz"), None);
    assert_eq!(time_str_to_unit(""), None);
    assert_eq!(time_str_to_unit("millis"), None);
}

// ---------------------------------------------------------------------------
// Tests: Time creation by unit (port of test_time_create_by_unit)
// ---------------------------------------------------------------------------

#[test]
fn test_time_create_nanosecond() {
    let tv = time_create_by_unit(1000, TimeUnit::Nanosecond);
    assert_eq!(tv.tv_usec, 1);
}

#[test]
fn test_time_create_microsecond() {
    let tv = time_create_by_unit(1, TimeUnit::Microsecond);
    assert_eq!(tv.tv_usec, 1);
}

#[test]
fn test_time_create_millisecond() {
    let tv = time_create_by_unit(1, TimeUnit::Millisecond);
    assert_eq!(tv.tv_usec, 1000);
}

#[test]
fn test_time_create_second() {
    let tv = time_create_by_unit(1, TimeUnit::Second);
    assert_eq!(tv.tv_sec, 1);
}

#[test]
fn test_time_create_minute() {
    let tv = time_create_by_unit(1, TimeUnit::Minute);
    assert_eq!(tv.tv_sec, 60);
}

#[test]
fn test_time_create_hour() {
    let tv = time_create_by_unit(1, TimeUnit::Hour);
    assert_eq!(tv.tv_sec, 3600);
}

#[test]
fn test_time_create_day() {
    let tv = time_create_by_unit(1, TimeUnit::Day);
    assert_eq!(tv.tv_sec, 86400);
}

#[test]
fn test_time_create_week() {
    let tv = time_create_by_unit(1, TimeUnit::Week);
    assert_eq!(tv.tv_sec, 604800);
}

#[test]
fn test_time_create_month() {
    let tv = time_create_by_unit(1, TimeUnit::Month);
    assert_eq!(tv.tv_sec, 2629746);
}

#[test]
fn test_time_create_year() {
    let tv = time_create_by_unit(1, TimeUnit::Year);
    assert_eq!(tv.tv_sec, 31556952);
}

// ---------------------------------------------------------------------------
// Tests: Timeval normalization
// ---------------------------------------------------------------------------

#[test]
fn test_time_normalize_nanoseconds() {
    let tv = time_create_by_unit(1500000000, TimeUnit::Nanosecond);
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);
}

#[test]
fn test_time_normalize_microseconds() {
    let tv = time_create_by_unit(1500000, TimeUnit::Microsecond);
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);
}

#[test]
fn test_time_normalize_milliseconds() {
    let tv = time_create_by_unit(1500, TimeUnit::Millisecond);
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);
}

// ---------------------------------------------------------------------------
// Tests: Time creation by unit string (port of test_time_create_by_unit_str)
// ---------------------------------------------------------------------------

#[test]
fn test_time_create_by_str_ns() {
    let tv = time_create_by_unit_str(1000, "ns");
    assert_eq!(tv.tv_usec, 1);
}

#[test]
fn test_time_create_by_str_us() {
    let tv = time_create_by_unit_str(1, "us");
    assert_eq!(tv.tv_usec, 1);
}

#[test]
fn test_time_create_by_str_ms() {
    let tv = time_create_by_unit_str(1, "ms");
    assert_eq!(tv.tv_usec, 1000);
}

#[test]
fn test_time_create_by_str_s() {
    let tv = time_create_by_unit_str(1, "s");
    assert_eq!(tv.tv_sec, 1);
}

#[test]
fn test_time_create_by_str_m() {
    let tv = time_create_by_unit_str(1, "m");
    assert_eq!(tv.tv_sec, 60);
}

#[test]
fn test_time_create_by_str_h() {
    let tv = time_create_by_unit_str(1, "h");
    assert_eq!(tv.tv_sec, 3600);
}

#[test]
fn test_time_create_by_str_d() {
    let tv = time_create_by_unit_str(1, "d");
    assert_eq!(tv.tv_sec, 86400);
}

#[test]
fn test_time_create_by_str_w() {
    let tv = time_create_by_unit_str(1, "w");
    assert_eq!(tv.tv_sec, 604800);
}

#[test]
fn test_time_create_by_str_mo() {
    let tv = time_create_by_unit_str(1, "mo");
    assert_eq!(tv.tv_sec, 2629746);
}

#[test]
fn test_time_create_by_str_yr() {
    let tv = time_create_by_unit_str(1, "yr");
    assert_eq!(tv.tv_sec, 31556952);
}

#[test]
fn test_time_create_by_str_normalize() {
    let tv = time_create_by_unit_str(1500000000, "ns");
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);

    let tv = time_create_by_unit_str(1500000, "us");
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);

    let tv = time_create_by_unit_str(1500, "ms");
    assert_eq!(tv.tv_sec, 1);
    assert_eq!(tv.tv_usec, 500000);
}

// ---------------------------------------------------------------------------
// Tests: Timeval to microseconds (port of test_time_tv_to_usec)
// ---------------------------------------------------------------------------

#[test]
fn test_time_tv_to_usec_zero() {
    let tv = TimeVal::new(0, 0);
    assert_eq!(tv.to_usec(), 0);
}

#[test]
fn test_time_tv_to_usec_one_usec() {
    let tv = TimeVal::new(0, 1);
    assert_eq!(tv.to_usec(), 1);
}

#[test]
fn test_time_tv_to_usec_one_sec() {
    let tv = TimeVal::new(1, 0);
    assert_eq!(tv.to_usec(), 1000000);
}

#[test]
fn test_time_tv_to_usec_combined() {
    let tv = TimeVal::new(1, 1);
    assert_eq!(tv.to_usec(), 1000001);
}

// ---------------------------------------------------------------------------
// Tests: Epoch conversions
// ---------------------------------------------------------------------------

#[test]
fn test_epoch_now() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap();
    assert!(now.as_secs() > 1_700_000_000); // After ~2023
}

#[test]
fn test_duration_arithmetic() {
    let d1 = Duration::from_secs(10);
    let d2 = Duration::from_secs(5);
    assert_eq!((d1 - d2).as_secs(), 5);
    assert_eq!((d1 + d2).as_secs(), 15);
}

#[test]
fn test_duration_from_millis() {
    let d = Duration::from_millis(1500);
    assert_eq!(d.as_secs(), 1);
    assert_eq!(d.subsec_millis(), 500);
}
