//! Prometheus metrics endpoint.
//!
//! Port of `res/res_prometheus.c`. Provides Prometheus-compatible metrics
//! collection (counters, gauges, histograms) and an HTTP `/metrics` endpoint
//! serving the Prometheus text exposition format.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum PrometheusError {
    #[error("metric not found: {0}")]
    MetricNotFound(String),
    #[error("metric already registered: {0}")]
    AlreadyRegistered(String),
    #[error("prometheus error: {0}")]
    Other(String),
}

pub type PrometheusResult<T> = Result<T, PrometheusError>;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Prometheus module configuration (from `prometheus.conf`).
#[derive(Debug, Clone)]
pub struct PrometheusConfig {
    /// Whether Prometheus metrics are enabled.
    pub enabled: bool,
    /// Whether core system metrics are enabled.
    pub core_metrics_enabled: bool,
    /// HTTP URI path to serve metrics (default: "metrics").
    pub uri: String,
    /// Optional Basic Auth username.
    pub auth_username: String,
    /// Optional Basic Auth password.
    pub auth_password: String,
    /// Auth realm for Basic Auth.
    pub auth_realm: String,
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            core_metrics_enabled: true,
            uri: "metrics".to_string(),
            auth_username: String::new(),
            auth_password: String::new(),
            auth_realm: "Asterisk Prometheus Metrics".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Metric types
// ---------------------------------------------------------------------------

/// Label set for a metric (key-value pairs).
pub type Labels = Vec<(String, String)>;

/// Format labels for Prometheus text format.
fn format_labels(labels: &Labels) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// A Prometheus counter (monotonically increasing).
#[derive(Debug)]
pub struct Counter {
    pub name: String,
    pub help: String,
    values: RwLock<HashMap<String, AtomicU64Wrapper>>,
}

/// Wrapper to make AtomicU64 Debug-friendly in HashMap.
#[derive(Debug)]
struct AtomicU64Wrapper(AtomicU64);

impl AtomicU64Wrapper {
    fn new(v: u64) -> Self {
        Self(AtomicU64::new(v))
    }
    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn inc(&self, v: u64) {
        self.0.fetch_add(v, Ordering::Relaxed);
    }
}

impl Counter {
    pub fn new(name: &str, help: &str) -> Self {
        Self {
            name: name.to_string(),
            help: help.to_string(),
            values: RwLock::new(HashMap::new()),
        }
    }

    /// Increment the counter for the given label set.
    pub fn inc(&self, labels: &Labels) {
        self.inc_by(labels, 1);
    }

    /// Increment the counter by a specific amount.
    pub fn inc_by(&self, labels: &Labels, amount: u64) {
        let key = format_labels(labels);
        let values = self.values.read();
        if let Some(v) = values.get(&key) {
            v.inc(amount);
            return;
        }
        drop(values);
        self.values
            .write()
            .entry(key)
            .or_insert_with(|| AtomicU64Wrapper::new(0))
            .inc(amount);
    }

    /// Get the current value for a label set.
    pub fn get(&self, labels: &Labels) -> u64 {
        let key = format_labels(labels);
        self.values
            .read()
            .get(&key)
            .map(|v| v.get())
            .unwrap_or(0)
    }

    /// Format as Prometheus text.
    fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} counter\n", self.name));
        let values = self.values.read();
        if values.is_empty() {
            out.push_str(&format!("{} 0\n", self.name));
        } else {
            for (labels, value) in values.iter() {
                out.push_str(&format!("{}{} {}\n", self.name, labels, value.get()));
            }
        }
        out
    }
}

/// A Prometheus gauge (can go up or down).
#[derive(Debug)]
pub struct Gauge {
    pub name: String,
    pub help: String,
    values: RwLock<HashMap<String, AtomicI64Wrapper>>,
}

#[derive(Debug)]
struct AtomicI64Wrapper(AtomicI64);

impl AtomicI64Wrapper {
    fn new(v: i64) -> Self {
        Self(AtomicI64::new(v))
    }
    fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, v: i64) {
        self.0.store(v, Ordering::Relaxed);
    }
    fn add(&self, v: i64) {
        self.0.fetch_add(v, Ordering::Relaxed);
    }
}

impl Gauge {
    pub fn new(name: &str, help: &str) -> Self {
        Self {
            name: name.to_string(),
            help: help.to_string(),
            values: RwLock::new(HashMap::new()),
        }
    }

    /// Set the gauge to a specific value.
    pub fn set(&self, labels: &Labels, value: i64) {
        let key = format_labels(labels);
        let values = self.values.read();
        if let Some(v) = values.get(&key) {
            v.set(value);
            return;
        }
        drop(values);
        self.values
            .write()
            .entry(key)
            .or_insert_with(|| AtomicI64Wrapper::new(value))
            .set(value);
    }

    /// Increment the gauge.
    pub fn inc(&self, labels: &Labels) {
        let key = format_labels(labels);
        let values = self.values.read();
        if let Some(v) = values.get(&key) {
            v.add(1);
            return;
        }
        drop(values);
        self.values
            .write()
            .entry(key)
            .or_insert_with(|| AtomicI64Wrapper::new(0))
            .add(1);
    }

    /// Decrement the gauge.
    pub fn dec(&self, labels: &Labels) {
        let key = format_labels(labels);
        let values = self.values.read();
        if let Some(v) = values.get(&key) {
            v.add(-1);
            return;
        }
        drop(values);
        self.values
            .write()
            .entry(key)
            .or_insert_with(|| AtomicI64Wrapper::new(0))
            .add(-1);
    }

    /// Get the current value.
    pub fn get(&self, labels: &Labels) -> i64 {
        let key = format_labels(labels);
        self.values
            .read()
            .get(&key)
            .map(|v| v.get())
            .unwrap_or(0)
    }

    fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} gauge\n", self.name));
        let values = self.values.read();
        if values.is_empty() {
            out.push_str(&format!("{} 0\n", self.name));
        } else {
            for (labels, value) in values.iter() {
                out.push_str(&format!("{}{} {}\n", self.name, labels, value.get()));
            }
        }
        out
    }
}

/// A Prometheus histogram (distribution of observations).
#[derive(Debug)]
pub struct Histogram {
    pub name: String,
    pub help: String,
    /// Bucket boundaries (upper bounds).
    pub buckets: Vec<f64>,
    /// Accumulated bucket counts and sum per label set.
    observations: RwLock<HashMap<String, HistogramData>>,
}

#[derive(Debug, Clone)]
struct HistogramData {
    bucket_counts: Vec<u64>,
    count: u64,
    sum: f64,
}

impl Histogram {
    /// Create a histogram with the given bucket boundaries.
    pub fn new(name: &str, help: &str, buckets: Vec<f64>) -> Self {
        Self {
            name: name.to_string(),
            help: help.to_string(),
            buckets,
            observations: RwLock::new(HashMap::new()),
        }
    }

    /// Create with default buckets (.005, .01, .025, .05, .1, .25, .5, 1, 2.5, 5, 10).
    pub fn with_default_buckets(name: &str, help: &str) -> Self {
        Self::new(
            name,
            help,
            vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
        )
    }

    /// Observe a value.
    pub fn observe(&self, labels: &Labels, value: f64) {
        let key = format_labels(labels);
        let mut obs = self.observations.write();
        let data = obs.entry(key).or_insert_with(|| HistogramData {
            bucket_counts: vec![0; self.buckets.len()],
            count: 0,
            sum: 0.0,
        });
        data.count += 1;
        data.sum += value;
        for (i, bound) in self.buckets.iter().enumerate() {
            if value <= *bound {
                data.bucket_counts[i] += 1;
            }
        }
    }

    fn format(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} histogram\n", self.name));
        let obs = self.observations.read();
        for (labels, data) in obs.iter() {
            let mut cumulative = 0u64;
            for (i, bound) in self.buckets.iter().enumerate() {
                cumulative += data.bucket_counts[i];
                out.push_str(&format!(
                    "{}_bucket{{le=\"{}\"{}}} {}\n",
                    self.name,
                    bound,
                    if labels.is_empty() {
                        String::new()
                    } else {
                        format!(",{}", &labels[1..labels.len() - 1])
                    },
                    cumulative,
                ));
            }
            out.push_str(&format!(
                "{}_bucket{{le=\"+Inf\"{}}} {}\n",
                self.name,
                if labels.is_empty() {
                    String::new()
                } else {
                    format!(",{}", &labels[1..labels.len() - 1])
                },
                data.count,
            ));
            out.push_str(&format!("{}_sum{} {}\n", self.name, labels, data.sum));
            out.push_str(&format!("{}_count{} {}\n", self.name, labels, data.count));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Metric registry and /metrics endpoint
// ---------------------------------------------------------------------------

/// Registered metric kind for the registry.
#[derive(Debug)]
enum RegisteredMetric {
    Counter(Arc<Counter>),
    Gauge(Arc<Gauge>),
    Histogram(Arc<Histogram>),
}

/// Prometheus metrics registry.
///
/// Collects all registered metrics and renders the `/metrics` endpoint
/// in Prometheus text exposition format.
#[derive(Debug)]
pub struct MetricsRegistry {
    pub config: RwLock<PrometheusConfig>,
    metrics: RwLock<Vec<RegisteredMetric>>,
}

impl MetricsRegistry {
    pub fn new(config: PrometheusConfig) -> Self {
        Self {
            config: RwLock::new(config),
            metrics: RwLock::new(Vec::new()),
        }
    }

    /// Register a counter.
    pub fn register_counter(&self, counter: Arc<Counter>) {
        self.metrics
            .write()
            .push(RegisteredMetric::Counter(counter));
    }

    /// Register a gauge.
    pub fn register_gauge(&self, gauge: Arc<Gauge>) {
        self.metrics.write().push(RegisteredMetric::Gauge(gauge));
    }

    /// Register a histogram.
    pub fn register_histogram(&self, histogram: Arc<Histogram>) {
        self.metrics
            .write()
            .push(RegisteredMetric::Histogram(histogram));
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let metrics = self.metrics.read();
        let mut output = String::new();
        for metric in metrics.iter() {
            match metric {
                RegisteredMetric::Counter(c) => output.push_str(&c.format()),
                RegisteredMetric::Gauge(g) => output.push_str(&g.format()),
                RegisteredMetric::Histogram(h) => output.push_str(&h.format()),
            }
        }
        output
    }

    /// Check Basic Auth credentials against config.
    pub fn check_auth(&self, username: &str, password: &str) -> bool {
        let config = self.config.read();
        if config.auth_username.is_empty() {
            return true; // No auth configured.
        }
        config.auth_username == username && config.auth_password == password
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_labels() {
        assert_eq!(format_labels(&vec![]), "");
        assert_eq!(
            format_labels(&vec![("method".to_string(), "INVITE".to_string())]),
            "{method=\"INVITE\"}"
        );
    }

    #[test]
    fn test_counter() {
        let counter = Counter::new("sip_requests_total", "Total SIP requests");
        let labels = vec![("method".to_string(), "INVITE".to_string())];
        counter.inc(&labels);
        counter.inc(&labels);
        counter.inc_by(&labels, 3);
        assert_eq!(counter.get(&labels), 5);
    }

    #[test]
    fn test_gauge() {
        let gauge = Gauge::new("active_channels", "Active channels");
        let labels = vec![];
        gauge.set(&labels, 10);
        assert_eq!(gauge.get(&labels), 10);
        gauge.inc(&labels);
        assert_eq!(gauge.get(&labels), 11);
        gauge.dec(&labels);
        assert_eq!(gauge.get(&labels), 10);
    }

    #[test]
    fn test_histogram() {
        let hist = Histogram::new(
            "request_duration_seconds",
            "Request duration",
            vec![0.1, 0.5, 1.0, 5.0],
        );
        let labels = vec![];
        hist.observe(&labels, 0.05);
        hist.observe(&labels, 0.3);
        hist.observe(&labels, 2.0);

        let formatted = hist.format();
        assert!(formatted.contains("request_duration_seconds_count"));
        assert!(formatted.contains("request_duration_seconds_sum"));
    }

    #[test]
    fn test_counter_format() {
        let counter = Counter::new("test_counter", "A test counter");
        counter.inc(&vec![]);
        let formatted = counter.format();
        assert!(formatted.contains("# HELP test_counter A test counter"));
        assert!(formatted.contains("# TYPE test_counter counter"));
        assert!(formatted.contains("test_counter 1"));
    }

    #[test]
    fn test_registry_render() {
        let registry = MetricsRegistry::new(PrometheusConfig::default());
        let counter = Arc::new(Counter::new("total", "Total"));
        counter.inc(&vec![]);
        registry.register_counter(counter);

        let output = registry.render();
        assert!(output.contains("# TYPE total counter"));
        assert!(output.contains("total 1"));
    }

    #[test]
    fn test_auth_check() {
        let registry = MetricsRegistry::new(PrometheusConfig {
            auth_username: "admin".to_string(),
            auth_password: "secret".to_string(),
            ..Default::default()
        });
        assert!(registry.check_auth("admin", "secret"));
        assert!(!registry.check_auth("admin", "wrong"));

        // No auth configured = always pass.
        let open_registry = MetricsRegistry::new(PrometheusConfig::default());
        assert!(open_registry.check_auth("", ""));
    }
}
