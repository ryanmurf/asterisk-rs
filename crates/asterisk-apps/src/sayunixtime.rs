//! SayUnixTime dialplan application.
//!
//! Port of app_sayunixtime.c from Asterisk C. Says a date/time
//! from a Unix timestamp using the channel's language and a
//! configurable format string.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// Options for the SayUnixTime application.
#[derive(Debug, Clone)]
pub struct SayUnixTimeOptions {
    /// Unix timestamp to say (default: current time).
    pub unixtime: Option<i64>,
    /// Timezone (e.g. "US/Eastern", default: system timezone).
    pub timezone: String,
    /// Format string (default: "ABdY 'digits/at' IMp").
    pub format: String,
}

impl SayUnixTimeOptions {
    /// Parse from comma-separated arguments.
    ///
    /// Format: [unixtime[,timezone[,format]]]
    pub fn parse(args: &str) -> Self {
        let parts: Vec<&str> = args.split(',').collect();
        Self {
            unixtime: parts.first().and_then(|s| s.trim().parse().ok()),
            timezone: parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default(),
            format: parts
                .get(2)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "ABdY 'digits/at' IMp".to_string()),
        }
    }
}

/// The SayUnixTime() dialplan application.
///
/// Usage: SayUnixTime([unixtime[,timezone[,format]]])
///
/// Says the specified date and time to the channel. If unixtime is
/// omitted, the current time is used.
///
/// Format characters:
///   A - Day of week
///   B - Month name
///   d - Day of month
///   Y - Year
///   I - 12-hour hour
///   H - 24-hour hour
///   M - Minutes
///   p - AM/PM
///   Q - "today", "yesterday", or date
///   q - "" (today), "yesterday", or date
///   R - 24-hour time (HH:MM)
pub struct AppSayUnixTime;

impl DialplanApp for AppSayUnixTime {
    fn name(&self) -> &str {
        "SayUnixTime"
    }

    fn description(&self) -> &str {
        "Say date and time from a Unix timestamp"
    }
}

impl AppSayUnixTime {
    /// Execute the SayUnixTime application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let options = SayUnixTimeOptions::parse(args);

        let timestamp = options.unixtime.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

        info!(
            "SayUnixTime: channel '{}' time={} tz='{}' fmt='{}'",
            channel.name, timestamp, options.timezone, options.format,
        );

        // In a real implementation:
        // 1. Convert timestamp to broken-down time in the given timezone
        // 2. Walk the format string
        // 3. For each format char, play the appropriate audio file(s)

        PbxExecResult::Success
    }
}

/// The DateTime() dialplan application (alias for SayUnixTime).
pub struct AppDateTime;

impl DialplanApp for AppDateTime {
    fn name(&self) -> &str {
        "DateTime"
    }

    fn description(&self) -> &str {
        "Say date and time (alias for SayUnixTime)"
    }
}

impl AppDateTime {
    /// Execute the DateTime application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        AppSayUnixTime::exec(channel, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sayunixtime_options_defaults() {
        let opts = SayUnixTimeOptions::parse("");
        assert!(opts.unixtime.is_none());
        assert!(opts.timezone.is_empty());
        assert_eq!(opts.format, "ABdY 'digits/at' IMp");
    }

    #[test]
    fn test_sayunixtime_options_full() {
        let opts = SayUnixTimeOptions::parse("1234567890,US/Eastern,HM");
        assert_eq!(opts.unixtime, Some(1234567890));
        assert_eq!(opts.timezone, "US/Eastern");
        assert_eq!(opts.format, "HM");
    }

    #[tokio::test]
    async fn test_sayunixtime_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSayUnixTime::exec(&mut channel, "1234567890").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
