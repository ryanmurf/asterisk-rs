//! Conditional logic dialplan functions.
//!
//! Port of func_logic.c from Asterisk C.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// IF() function.
///
/// Usage: IF(expression?true_value:false_value)
///
/// Returns true_value if expression is non-empty and non-zero,
/// otherwise returns false_value.
pub struct FuncIf;

impl DialplanFunc for FuncIf {
    fn name(&self) -> &str {
        "IF"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        // Parse: expression?true_value:false_value
        let (expression, rest) = match args.find('?') {
            Some(pos) => (&args[..pos], &args[pos + 1..]),
            None => {
                return Err(FuncError::InvalidArgument(
                    "IF: syntax is IF(expression?true:false)".to_string(),
                ));
            }
        };

        let (true_val, false_val) = match rest.find(':') {
            Some(pos) => (&rest[..pos], &rest[pos + 1..]),
            None => (rest, ""),
        };

        let expression = expression.trim();
        let is_true = is_truthy(expression);

        if is_true {
            Ok(true_val.to_string())
        } else {
            Ok(false_val.to_string())
        }
    }
}

/// IFTIME() function.
///
/// Usage: IFTIME(timespec?true_value:false_value)
///
/// Evaluates a time specification and returns true_value if the current
/// time matches, or false_value otherwise.
///
/// Timespec format: times,days_of_week,days_of_month,months
///   e.g., "*,*,*,*" matches always
///         "9:00-17:00,mon-fri,*,*" matches business hours
pub struct FuncIfTime;

impl DialplanFunc for FuncIfTime {
    fn name(&self) -> &str {
        "IFTIME"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let (timespec, rest) = match args.find('?') {
            Some(pos) => (&args[..pos], &args[pos + 1..]),
            None => {
                return Err(FuncError::InvalidArgument(
                    "IFTIME: syntax is IFTIME(timespec?true:false)".to_string(),
                ));
            }
        };

        let (true_val, false_val) = match rest.find(':') {
            Some(pos) => (&rest[..pos], &rest[pos + 1..]),
            None => (rest, ""),
        };

        let matches = evaluate_timespec(timespec.trim());

        if matches {
            Ok(true_val.to_string())
        } else {
            Ok(false_val.to_string())
        }
    }
}

/// SET() function.
///
/// Usage: SET(varname=value)
///
/// Sets a channel variable and returns the value.
pub struct FuncSet;

impl DialplanFunc for FuncSet {
    fn name(&self) -> &str {
        "SET"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        // Parse varname=value
        if let Some(eq_pos) = args.find('=') {
            let value = &args[eq_pos + 1..];
            Ok(value.to_string())
        } else {
            Err(FuncError::InvalidArgument(
                "SET: syntax is SET(varname=value)".to_string(),
            ))
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        // args is the variable name
        let varname = args.trim();
        if varname.is_empty() {
            return Err(FuncError::InvalidArgument(
                "SET: variable name is required".to_string(),
            ));
        }
        ctx.set_variable(varname, value);
        Ok(())
    }
}

/// EXISTS() function.
///
/// Usage: EXISTS(data)
///
/// Returns "1" if data is non-empty, "0" otherwise.
pub struct FuncExists;

impl DialplanFunc for FuncExists {
    fn name(&self) -> &str {
        "EXISTS"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        if args.is_empty() {
            Ok("0".to_string())
        } else {
            Ok("1".to_string())
        }
    }
}

/// ISNULL() function.
///
/// Usage: ISNULL(data)
///
/// Returns "1" if data is empty/null, "0" otherwise.
pub struct FuncIsNull;

impl DialplanFunc for FuncIsNull {
    fn name(&self) -> &str {
        "ISNULL"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        if args.is_empty() {
            Ok("1".to_string())
        } else {
            Ok("0".to_string())
        }
    }
}

/// Check if an expression string is "truthy".
///
/// A value is truthy if it is non-empty and not "0".
fn is_truthy(expr: &str) -> bool {
    let trimmed = expr.trim();
    !trimmed.is_empty() && trimmed != "0"
}

/// Evaluate a time specification against the current time.
///
/// Format: times,days_of_week,days_of_month,months
///
/// Each component can be:
///   "*" - matches any value
///   Specific values or ranges (e.g., "9:00-17:00", "mon-fri")
fn evaluate_timespec(timespec: &str) -> bool {
    let parts: Vec<&str> = timespec.split(',').collect();
    if parts.len() < 4 {
        // If wildcard "*" or incomplete spec, try simple evaluation
        if timespec.trim() == "*" || timespec.trim() == "*,*,*,*" {
            return true;
        }
        // Simplified: treat incomplete specs as no match for safety
        return false;
    }

    let time_range = parts[0].trim();
    let dow = parts[1].trim();
    let dom = parts[2].trim();
    let months = parts[3].trim();

    // If all components are wildcards, it always matches
    if time_range == "*" && dow == "*" && dom == "*" && months == "*" {
        return true;
    }

    // In a full implementation, we'd get the current time and check:
    // 1. Current time of day is within time_range
    // 2. Current day of week matches dow
    // 3. Current day of month matches dom
    // 4. Current month matches months
    //
    // For now, we implement a simplified version that handles wildcards
    // and common patterns.

    let now = chrono_compat::now();

    // Check time range
    if time_range != "*" && !check_time_range(time_range, now.hour, now.minute) {
        return false;
    }

    // Check day of week
    if dow != "*" && !check_day_of_week(dow, now.day_of_week) {
        return false;
    }

    // Check day of month
    if dom != "*" && !check_day_of_month(dom, now.day_of_month) {
        return false;
    }

    // Check month
    if months != "*" && !check_month(months, now.month) {
        return false;
    }

    true
}

/// Check if the current time falls within a time range like "9:00-17:00".
fn check_time_range(range: &str, hour: u32, minute: u32) -> bool {
    if let Some(dash_pos) = range.find('-') {
        let start = &range[..dash_pos];
        let end = &range[dash_pos + 1..];

        let start_minutes = parse_time_to_minutes(start);
        let end_minutes = parse_time_to_minutes(end);
        let current_minutes = hour * 60 + minute;

        if let (Some(start_m), Some(end_m)) = (start_minutes, end_minutes) {
            if start_m <= end_m {
                return current_minutes >= start_m && current_minutes <= end_m;
            } else {
                // Wraps around midnight
                return current_minutes >= start_m || current_minutes <= end_m;
            }
        }
    }
    true
}

/// Parse "HH:MM" to total minutes.
fn parse_time_to_minutes(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() == 2 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        Some(h * 60 + m)
    } else {
        None
    }
}

/// Check if a day of week matches a specification like "mon-fri" or "sat&sun".
fn check_day_of_week(spec: &str, dow: u32) -> bool {
    for part in spec.split('&') {
        let part = part.trim().to_lowercase();
        if let Some(dash_pos) = part.find('-') {
            let start = &part[..dash_pos];
            let end = &part[dash_pos + 1..];
            if let (Some(s), Some(e)) = (day_name_to_num(start), day_name_to_num(end)) {
                if s <= e {
                    if dow >= s && dow <= e { return true; }
                } else {
                    if dow >= s || dow <= e { return true; }
                }
            }
        } else if let Some(d) = day_name_to_num(&part) {
            if dow == d { return true; }
        }
    }
    false
}

fn day_name_to_num(name: &str) -> Option<u32> {
    match name.trim().to_lowercase().as_str() {
        "sun" | "sunday" => Some(0),
        "mon" | "monday" => Some(1),
        "tue" | "tuesday" => Some(2),
        "wed" | "wednesday" => Some(3),
        "thu" | "thursday" => Some(4),
        "fri" | "friday" => Some(5),
        "sat" | "saturday" => Some(6),
        _ => None,
    }
}

/// Check if a day of month matches a specification like "1-15" or "1&15&30".
fn check_day_of_month(spec: &str, dom: u32) -> bool {
    for part in spec.split('&') {
        let part = part.trim();
        if let Some(dash_pos) = part.find('-') {
            if let (Ok(s), Ok(e)) = (
                part[..dash_pos].trim().parse::<u32>(),
                part[dash_pos + 1..].trim().parse::<u32>(),
            ) {
                if dom >= s && dom <= e { return true; }
            }
        } else if let Ok(d) = part.parse::<u32>() {
            if dom == d { return true; }
        }
    }
    false
}

/// Check if a month matches a specification like "jan-jun" or "1-6".
fn check_month(spec: &str, month: u32) -> bool {
    for part in spec.split('&') {
        let part = part.trim().to_lowercase();
        if let Some(dash_pos) = part.find('-') {
            let start = &part[..dash_pos];
            let end = &part[dash_pos + 1..];
            if let (Some(s), Some(e)) = (month_to_num(start), month_to_num(end)) {
                if s <= e {
                    if month >= s && month <= e { return true; }
                } else {
                    if month >= s || month <= e { return true; }
                }
            }
        } else if let Some(m) = month_to_num(&part) {
            if month == m { return true; }
        }
    }
    false
}

fn month_to_num(name: &str) -> Option<u32> {
    match name.trim().to_lowercase().as_str() {
        "jan" | "january" | "1" => Some(1),
        "feb" | "february" | "2" => Some(2),
        "mar" | "march" | "3" => Some(3),
        "apr" | "april" | "4" => Some(4),
        "may" | "5" => Some(5),
        "jun" | "june" | "6" => Some(6),
        "jul" | "july" | "7" => Some(7),
        "aug" | "august" | "8" => Some(8),
        "sep" | "september" | "9" => Some(9),
        "oct" | "october" | "10" => Some(10),
        "nov" | "november" | "11" => Some(11),
        "dec" | "december" | "12" => Some(12),
        _ => None,
    }
}

/// Minimal time access without pulling in the chrono crate.
mod chrono_compat {
    use std::time::{SystemTime, UNIX_EPOCH};

    pub struct SimpleTime {
        pub hour: u32,
        pub minute: u32,
        pub day_of_week: u32,
        pub day_of_month: u32,
        pub month: u32,
    }

    pub fn now() -> SimpleTime {
        // Get current time as Unix timestamp
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs() as i64;

        // Simple time decomposition (UTC)
        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;
        let hour = (time_of_day / 3600) as u32;
        let minute = ((time_of_day % 3600) / 60) as u32;

        // Day of week: Jan 1, 1970 was a Thursday (4)
        let day_of_week = ((days_since_epoch + 4) % 7) as u32;

        // Approximate month and day (good enough for basic timespec matching)
        let (month, day_of_month) = approximate_date(days_since_epoch);

        SimpleTime {
            hour,
            minute,
            day_of_week,
            day_of_month,
            month,
        }
    }

    fn approximate_date(days: i64) -> (u32, u32) {
        // Simplified Gregorian calendar calculation
        let mut remaining = days;
        let mut year: i64 = 1970;

        loop {
            let days_in_year = if is_leap(year) { 366 } else { 365 };
            if remaining < days_in_year {
                break;
            }
            remaining -= days_in_year;
            year += 1;
        }

        let leap = is_leap(year);
        let month_days: [i64; 12] = [
            31,
            if leap { 29 } else { 28 },
            31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
        ];

        let mut month = 0u32;
        for (i, &days) in month_days.iter().enumerate() {
            if remaining < days {
                month = i as u32 + 1;
                break;
            }
            remaining -= days;
        }
        if month == 0 {
            month = 12;
        }

        (month, remaining as u32 + 1)
    }

    fn is_leap(year: i64) -> bool {
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_if_true() {
        let ctx = FuncContext::new();
        let func = FuncIf;
        assert_eq!(func.read(&ctx, "1?yes:no").unwrap(), "yes");
    }

    #[test]
    fn test_if_false() {
        let ctx = FuncContext::new();
        let func = FuncIf;
        assert_eq!(func.read(&ctx, "0?yes:no").unwrap(), "no");
    }

    #[test]
    fn test_if_empty() {
        let ctx = FuncContext::new();
        let func = FuncIf;
        assert_eq!(func.read(&ctx, "?yes:no").unwrap(), "no");
    }

    #[test]
    fn test_if_nonempty() {
        let ctx = FuncContext::new();
        let func = FuncIf;
        assert_eq!(func.read(&ctx, "hello?yes:no").unwrap(), "yes");
    }

    #[test]
    fn test_exists() {
        let ctx = FuncContext::new();
        let func = FuncExists;
        assert_eq!(func.read(&ctx, "something").unwrap(), "1");
        assert_eq!(func.read(&ctx, "").unwrap(), "0");
    }

    #[test]
    fn test_isnull() {
        let ctx = FuncContext::new();
        let func = FuncIsNull;
        assert_eq!(func.read(&ctx, "").unwrap(), "1");
        assert_eq!(func.read(&ctx, "something").unwrap(), "0");
    }

    #[test]
    fn test_iftime_always() {
        let ctx = FuncContext::new();
        let func = FuncIfTime;
        assert_eq!(func.read(&ctx, "*,*,*,*?yes:no").unwrap(), "yes");
    }

    #[test]
    fn test_time_range_parsing() {
        assert_eq!(parse_time_to_minutes("9:00"), Some(540));
        assert_eq!(parse_time_to_minutes("17:30"), Some(1050));
    }

    #[test]
    fn test_day_name_to_num() {
        assert_eq!(day_name_to_num("mon"), Some(1));
        assert_eq!(day_name_to_num("fri"), Some(5));
        assert_eq!(day_name_to_num("sunday"), Some(0));
    }

    #[test]
    fn test_month_to_num() {
        assert_eq!(month_to_num("jan"), Some(1));
        assert_eq!(month_to_num("december"), Some(12));
        assert_eq!(month_to_num("6"), Some(6));
    }
}
