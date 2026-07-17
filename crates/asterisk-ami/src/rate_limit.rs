//! AMI login rate limiting (issue #130).
//!
//! Bounds the VOLUME of failed `Login` attempts from a single source address.
//! The constant-time credential compare (PR #123) already closes the per-guess
//! timing oracle; this closes the remaining hole where an attacker can grind
//! unlimited online password guesses bounded only by network throughput.
//!
//! This mirrors the approach of the in-tree SIP `rate_limit.rs`: per-source
//! tracking in a `DashMap`, a sliding failure window, and a temporary block once
//! a threshold is exceeded. A successful login clears the source's state so a
//! legitimate user who mistypes a few times is never permanently locked out.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::warn;

/// Default: block a source after this many failed logins within the window.
const DEFAULT_MAX_FAILURES: u32 = 5;
/// Default sliding window over which failures are counted.
const DEFAULT_WINDOW_SECS: u64 = 60;
/// Default block duration once the threshold is tripped.
const DEFAULT_BLOCK_SECS: u64 = 60;

/// Configuration for the AMI login rate limiter.
#[derive(Debug, Clone)]
pub struct LoginRateLimitConfig {
    /// Failed logins from one source within `window` before it is blocked.
    pub max_failures: u32,
    /// Sliding window over which failures accumulate.
    pub window: Duration,
    /// How long a source stays blocked once it trips the threshold.
    pub block_duration: Duration,
    /// Whether rate limiting is enforced at all.
    pub enabled: bool,
}

impl Default for LoginRateLimitConfig {
    fn default() -> Self {
        Self {
            max_failures: DEFAULT_MAX_FAILURES,
            window: Duration::from_secs(DEFAULT_WINDOW_SECS),
            block_duration: Duration::from_secs(DEFAULT_BLOCK_SECS),
            enabled: true,
        }
    }
}

#[derive(Debug)]
struct Attempts {
    failures: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// Per-source failed-`Login` tracker with temporary blocking.
#[derive(Debug)]
pub struct LoginRateLimiter {
    config: LoginRateLimitConfig,
    by_ip: DashMap<IpAddr, Attempts>,
}

impl LoginRateLimiter {
    /// Create a limiter with default thresholds.
    pub fn new() -> Self {
        Self::with_config(LoginRateLimitConfig::default())
    }

    /// Create a limiter with an explicit configuration.
    pub fn with_config(config: LoginRateLimitConfig) -> Self {
        Self {
            config,
            by_ip: DashMap::new(),
        }
    }

    /// Check whether a source may attempt a `Login` right now.
    ///
    /// Returns `Err(retry_after)` if the source is currently blocked. Expired
    /// blocks are cleared on read.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        if !self.config.enabled {
            return Ok(());
        }
        if let Some(mut entry) = self.by_ip.get_mut(&ip) {
            if let Some(until) = entry.blocked_until {
                let now = Instant::now();
                if now < until {
                    return Err(until - now);
                }
                // Block expired: reset so the source gets a fresh window.
                entry.failures = 0;
                entry.window_start = now;
                entry.blocked_until = None;
            }
        }
        Ok(())
    }

    /// Record a failed `Login` from a source, tripping a block at the threshold.
    pub fn record_failure(&self, ip: IpAddr) {
        if !self.config.enabled {
            return;
        }
        let now = Instant::now();
        let mut entry = self.by_ip.entry(ip).or_insert(Attempts {
            failures: 0,
            window_start: now,
            blocked_until: None,
        });

        // Slide the window: stale failures don't accumulate forever.
        if now.duration_since(entry.window_start) > self.config.window {
            entry.failures = 0;
            entry.window_start = now;
            entry.blocked_until = None;
        }

        entry.failures += 1;
        if entry.failures >= self.config.max_failures {
            entry.blocked_until = Some(now + self.config.block_duration);
            warn!(
                "AMI: login rate limit tripped for {} after {} failures; blocking for {}s",
                ip,
                entry.failures,
                self.config.block_duration.as_secs()
            );
        }
    }

    /// Clear a source's failure state after a successful login.
    pub fn record_success(&self, ip: IpAddr) {
        self.by_ip.remove(&ip);
    }

    /// Whether a source is currently blocked (expired blocks are cleared).
    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        self.check(ip).is_err()
    }
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, n))
    }

    #[test]
    fn blocks_after_threshold_failures() {
        let limiter = LoginRateLimiter::with_config(LoginRateLimitConfig {
            max_failures: 3,
            window: Duration::from_secs(60),
            block_duration: Duration::from_secs(60),
            enabled: true,
        });
        let src = ip(10);
        assert!(limiter.check(src).is_ok());
        limiter.record_failure(src);
        limiter.record_failure(src);
        assert!(limiter.check(src).is_ok(), "still under threshold");
        limiter.record_failure(src); // third -> trips
        assert!(limiter.check(src).is_err(), "blocked after threshold failures");
    }

    #[test]
    fn success_clears_failures() {
        let limiter = LoginRateLimiter::with_config(LoginRateLimitConfig {
            max_failures: 2,
            ..LoginRateLimitConfig::default()
        });
        let src = ip(11);
        limiter.record_failure(src);
        limiter.record_success(src);
        // A fresh failure after success must not immediately block.
        limiter.record_failure(src);
        assert!(limiter.check(src).is_ok());
    }

    #[test]
    fn disabled_never_blocks() {
        let limiter = LoginRateLimiter::with_config(LoginRateLimitConfig {
            max_failures: 1,
            enabled: false,
            ..LoginRateLimitConfig::default()
        });
        let src = ip(12);
        for _ in 0..10 {
            limiter.record_failure(src);
        }
        assert!(limiter.check(src).is_ok());
    }
}
