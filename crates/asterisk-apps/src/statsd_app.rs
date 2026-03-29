//! StatsD metrics from dialplan.
//!
//! Port of app_statsd.c from Asterisk C. Sends StatsD metrics
//! (gauges, counters, timers, sets) from the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// StatsD metric type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsdMetricType {
    /// Gauge: set to a specific value.
    Gauge,
    /// Counter: increment/decrement.
    Counter,
    /// Timer: duration in milliseconds.
    Timer,
    /// Set: count unique values.
    Set,
}

impl StatsdMetricType {
    /// Parse from string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "g" | "gauge" => Some(Self::Gauge),
            "c" | "counter" => Some(Self::Counter),
            "ms" | "timer" => Some(Self::Timer),
            "s" | "set" => Some(Self::Set),
            _ => None,
        }
    }

    /// StatsD wire format type character.
    pub fn type_char(&self) -> &'static str {
        match self {
            Self::Gauge => "g",
            Self::Counter => "c",
            Self::Timer => "ms",
            Self::Set => "s",
        }
    }
}

/// The StatsD() dialplan application.
///
/// Usage: StatsD(metric_type,statistic_name,value)
///
/// Sends a metric to the StatsD server.
///
/// metric_type: g (gauge), c (counter), ms (timer), s (set)
/// statistic_name: the metric name (e.g. "calls.answered")
/// value: the metric value
pub struct AppStatsd;

impl DialplanApp for AppStatsd {
    fn name(&self) -> &str {
        "StatsD"
    }

    fn description(&self) -> &str {
        "Send StatsD metrics from dialplan"
    }
}

impl AppStatsd {
    /// Execute the StatsD application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.split(',').collect();

        if parts.len() < 3 {
            warn!("StatsD: requires metric_type,name,value arguments");
            return PbxExecResult::Failed;
        }

        let metric_type = match StatsdMetricType::from_str_opt(parts[0].trim()) {
            Some(t) => t,
            None => {
                warn!("StatsD: unknown metric type '{}'", parts[0]);
                return PbxExecResult::Failed;
            }
        };

        let name = parts[1].trim();
        let value = parts[2].trim();

        info!(
            "StatsD: channel '{}' {}:{}|{}",
            channel.name, name, value, metric_type.type_char(),
        );

        // In a real implementation:
        // Send UDP packet to StatsD server:
        //   "<name>:<value>|<type>"

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_type_parse() {
        assert_eq!(StatsdMetricType::from_str_opt("g"), Some(StatsdMetricType::Gauge));
        assert_eq!(StatsdMetricType::from_str_opt("c"), Some(StatsdMetricType::Counter));
        assert_eq!(StatsdMetricType::from_str_opt("ms"), Some(StatsdMetricType::Timer));
        assert_eq!(StatsdMetricType::from_str_opt("s"), Some(StatsdMetricType::Set));
        assert_eq!(StatsdMetricType::from_str_opt("x"), None);
    }

    #[test]
    fn test_metric_type_char() {
        assert_eq!(StatsdMetricType::Gauge.type_char(), "g");
        assert_eq!(StatsdMetricType::Counter.type_char(), "c");
    }

    #[tokio::test]
    async fn test_statsd_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppStatsd::exec(&mut channel, "c,calls.total,1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_statsd_exec_bad_type() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppStatsd::exec(&mut channel, "x,calls.total,1").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
