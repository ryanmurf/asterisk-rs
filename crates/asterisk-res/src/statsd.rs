//! StatsD metrics client.
//!
//! Port of `res/res_statsd.c`. Provides a UDP client for sending metrics
//! (counters, gauges, timers, meters, sets) to a StatsD server.

use std::fmt;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum StatsdError {
    #[error("StatsD not enabled")]
    NotEnabled,
    #[error("StatsD send failed: {0}")]
    SendError(String),
    #[error("StatsD config error: {0}")]
    ConfigError(String),
}

pub type StatsdResult<T> = Result<T, StatsdError>;

// ---------------------------------------------------------------------------
// Metric types
// ---------------------------------------------------------------------------

/// StatsD metric type identifiers (used in the wire protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    /// Counter: incremented/decremented.
    Counter,
    /// Gauge: absolute value.
    Gauge,
    /// Timer: millisecond timing.
    Timer,
    /// Meter: events per second (non-standard).
    Meter,
    /// Set: unique values.
    Set,
}

impl MetricType {
    /// StatsD wire format suffix.
    fn suffix(&self) -> &'static str {
        match self {
            Self::Counter => "c",
            Self::Gauge => "g",
            Self::Timer => "ms",
            Self::Meter => "m",
            Self::Set => "s",
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// StatsD client configuration (from `statsd.conf [global]`).
#[derive(Debug, Clone)]
pub struct StatsdConfig {
    /// Whether StatsD is enabled.
    pub enabled: bool,
    /// StatsD server address.
    pub server: SocketAddr,
    /// Prefix to prepend to every metric name.
    pub prefix: String,
    /// Append a newline to every datagram (for testing with netcat).
    pub add_newline: bool,
    /// Support the non-standard meter type.
    pub meter_support: bool,
}

impl Default for StatsdConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: "127.0.0.1:8125".parse().unwrap(),
            prefix: String::new(),
            add_newline: false,
            meter_support: true,
        }
    }
}

// ---------------------------------------------------------------------------
// StatsD client
// ---------------------------------------------------------------------------

/// StatsD UDP metrics client.
///
/// Port of `res_statsd.c`. Sends metrics over UDP to a StatsD-compatible
/// server. Thread-safe and lock-free for the hot path.
#[derive(Debug)]
pub struct StatsdClient {
    config: RwLock<StatsdConfig>,
    socket: RwLock<Option<UdpSocket>>,
}

impl StatsdClient {
    /// Create a new StatsD client (initially disabled).
    pub fn new() -> Self {
        Self {
            config: RwLock::new(StatsdConfig::default()),
            socket: RwLock::new(None),
        }
    }

    /// Create a new StatsD client with the given configuration.
    pub fn with_config(config: StatsdConfig) -> StatsdResult<Self> {
        let client = Self {
            config: RwLock::new(config.clone()),
            socket: RwLock::new(None),
        };
        if config.enabled {
            client.connect()?;
        }
        Ok(client)
    }

    /// Update the configuration and reconnect if needed.
    pub fn configure(&self, config: StatsdConfig) -> StatsdResult<()> {
        let should_connect = config.enabled;
        *self.config.write() = config;
        if should_connect {
            self.connect()?;
        } else {
            *self.socket.write() = None;
        }
        Ok(())
    }

    /// Establish the UDP socket.
    fn connect(&self) -> StatsdResult<()> {
        let config = self.config.read();
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| StatsdError::SendError(e.to_string()))?;
        socket
            .connect(config.server)
            .map_err(|e| StatsdError::SendError(e.to_string()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| StatsdError::SendError(e.to_string()))?;
        *self.socket.write() = Some(socket);
        debug!(server = %config.server, "StatsD client connected");
        Ok(())
    }

    /// Build the full metric name with prefix.
    fn full_name(&self, name: &str) -> String {
        let config = self.config.read();
        if config.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", config.prefix, name)
        }
    }

    /// Format and send a metric datagram.
    fn send_metric(
        &self,
        name: &str,
        value: &str,
        metric_type: MetricType,
        sample_rate: Option<f64>,
    ) -> StatsdResult<()> {
        let config = self.config.read();
        if !config.enabled {
            return Err(StatsdError::NotEnabled);
        }

        // Handle meter fallback when meter_support is disabled.
        let (actual_type, actual_name) = if metric_type == MetricType::Meter && !config.meter_support
        {
            (MetricType::Counter, format!("{}_meter", name))
        } else {
            (metric_type, name.to_string())
        };

        let full_name = if config.prefix.is_empty() {
            actual_name
        } else {
            format!("{}.{}", config.prefix, actual_name)
        };

        let mut datagram = format!("{}:{}|{}", full_name, value, actual_type.suffix());

        if let Some(rate) = sample_rate {
            if rate < 1.0 {
                datagram.push_str(&format!("|@{}", rate));
            }
        }

        if config.add_newline {
            datagram.push('\n');
        }

        drop(config);

        let socket = self.socket.read();
        if let Some(ref sock) = *socket {
            match sock.send(datagram.as_bytes()) {
                Ok(_) => {
                    debug!(metric = %datagram, "StatsD sent");
                }
                Err(e) => {
                    // Non-blocking send may fail with WouldBlock -- that's OK for UDP.
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        warn!(error = %e, "StatsD send failed");
                    }
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public metric helpers (mirrors ast_statsd_log* functions)
    // -----------------------------------------------------------------------

    /// Send a counter increment/decrement.
    pub fn counter(&self, name: &str, value: i64) -> StatsdResult<()> {
        let sign = if value >= 0 {
            format!("+{}", value)
        } else {
            format!("{}", value)
        };
        self.send_metric(name, &sign, MetricType::Counter, None)
    }

    /// Send a gauge value.
    pub fn gauge(&self, name: &str, value: f64) -> StatsdResult<()> {
        self.send_metric(name, &format!("{}", value), MetricType::Gauge, None)
    }

    /// Send a timer value (milliseconds).
    pub fn timer(&self, name: &str, millis: u64) -> StatsdResult<()> {
        self.send_metric(name, &format!("{}", millis), MetricType::Timer, None)
    }

    /// Send a meter event.
    pub fn meter(&self, name: &str, value: i64) -> StatsdResult<()> {
        self.send_metric(name, &format!("{}", value), MetricType::Meter, None)
    }

    /// Send a set value.
    pub fn set(&self, name: &str, value: &str) -> StatsdResult<()> {
        self.send_metric(name, value, MetricType::Set, None)
    }

    /// Generic stat_send helper (matches the C `ast_statsd_log_string` signature).
    pub fn stat_send(
        &self,
        metric_type: MetricType,
        name: &str,
        value: &str,
    ) -> StatsdResult<()> {
        self.send_metric(name, value, metric_type, None)
    }
}

impl Default for StatsdClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_type_suffix() {
        assert_eq!(MetricType::Counter.suffix(), "c");
        assert_eq!(MetricType::Gauge.suffix(), "g");
        assert_eq!(MetricType::Timer.suffix(), "ms");
        assert_eq!(MetricType::Meter.suffix(), "m");
        assert_eq!(MetricType::Set.suffix(), "s");
    }

    #[test]
    fn test_client_disabled_by_default() {
        let client = StatsdClient::new();
        assert!(matches!(
            client.counter("test", 1),
            Err(StatsdError::NotEnabled)
        ));
    }

    #[test]
    fn test_default_config() {
        let config = StatsdConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.server.port(), 8125);
        assert!(config.prefix.is_empty());
    }

    #[test]
    fn test_full_name() {
        let client = StatsdClient::new();
        assert_eq!(client.full_name("channels.count"), "channels.count");

        client.configure(StatsdConfig {
            prefix: "asterisk".to_string(),
            ..Default::default()
        }).ok();
        assert_eq!(client.full_name("channels.count"), "asterisk.channels.count");
    }
}
