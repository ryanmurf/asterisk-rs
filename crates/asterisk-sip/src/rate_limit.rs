//! SIP Rate Limiting and Abuse Detection
//!
//! This module provides protection against SIP-based attacks including:
//! - INVITE floods (calls per second)
//! - Scanner detection (sequential REGISTER/OPTIONS floods)
//! - Automatic IP blocking based on configurable thresholds
//! - Per-IP rate tracking with automatic expiry

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use parking_lot::RwLock;
use tracing::{debug, info, warn};

use crate::parser::{SipMessage, SipMethod};

/// Default INVITE rate limit (requests per second)
const DEFAULT_INVITE_RATE_LIMIT: u32 = 50;

/// Default scanner detection threshold (sequential requests)
const DEFAULT_SCANNER_THRESHOLD: u32 = 100;

/// Default block duration in seconds
const DEFAULT_BLOCK_DURATION_SECS: u64 = 300; // 5 minutes

/// How long to keep IP tracking data (seconds)
const IP_TRACKING_TTL_SECS: u64 = 60;

/// Configuration for rate limiting and abuse detection
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum INVITE requests per second per IP
    pub invite_rate_limit: u32,
    /// Maximum sequential REGISTER/OPTIONS before considering it scanning
    pub scanner_threshold: u32,
    /// How long to block IPs that exceed limits (seconds)
    pub block_duration_secs: u64,
    /// Enable/disable rate limiting
    pub enabled: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            invite_rate_limit: DEFAULT_INVITE_RATE_LIMIT,
            scanner_threshold: DEFAULT_SCANNER_THRESHOLD,
            block_duration_secs: DEFAULT_BLOCK_DURATION_SECS,
            enabled: true,
        }
    }
}

/// Per-IP tracking data for rate limiting
#[derive(Debug)]
struct IpTrackingData {
    /// Ring buffer of recent INVITE timestamps (last second)
    invite_timestamps: VecDeque<Instant>,
    /// Count of sequential REGISTER/OPTIONS requests
    sequential_scanner_requests: u32,
    /// Last request timestamp for cleanup
    last_activity: Instant,
    /// Last method seen (to detect sequential patterns)
    last_method: Option<SipMethod>,
}

impl IpTrackingData {
    fn new() -> Self {
        Self {
            invite_timestamps: VecDeque::new(),
            sequential_scanner_requests: 0,
            last_activity: Instant::now(),
            last_method: None,
        }
    }

    /// Clean old INVITE timestamps (older than 1 second)
    fn clean_old_invites(&mut self, now: Instant) {
        let cutoff = now - Duration::from_secs(1);
        while let Some(&front_time) = self.invite_timestamps.front() {
            if front_time < cutoff {
                self.invite_timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Record an INVITE request and return current rate
    fn record_invite(&mut self, now: Instant) -> u32 {
        self.clean_old_invites(now);
        self.invite_timestamps.push_back(now);
        self.last_activity = now;
        self.invite_timestamps.len() as u32
    }

    /// Record a scanner-type request (REGISTER/OPTIONS) and return sequential count
    fn record_scanner_request(&mut self, method: SipMethod, now: Instant) -> u32 {
        self.last_activity = now;

        // If it's the same method as last time, increment sequential count
        if self.last_method == Some(method) {
            self.sequential_scanner_requests += 1;
        } else {
            // Different method or first request - reset count
            self.sequential_scanner_requests = 1;
        }

        self.last_method = Some(method);
        self.sequential_scanner_requests
    }

    /// Reset scanner detection (called when non-scanner requests are received)
    fn reset_scanner_detection(&mut self, method: SipMethod, now: Instant) {
        self.last_activity = now;
        self.last_method = Some(method);
        self.sequential_scanner_requests = 0;
    }

    /// Check if this IP data is stale and should be cleaned up
    fn is_stale(&self, now: Instant) -> bool {
        now.duration_since(self.last_activity) > Duration::from_secs(IP_TRACKING_TTL_SECS)
    }
}

/// Blocked IP information
#[derive(Debug)]
struct BlockedIp {
    /// When the block was applied
    blocked_at: SystemTime,
    /// Duration of the block
    block_duration: Duration,
    /// Reason for blocking
    reason: String,
}

impl BlockedIp {
    fn new(reason: String, duration: Duration) -> Self {
        Self {
            blocked_at: SystemTime::now(),
            block_duration: duration,
            reason,
        }
    }

    /// Check if this block has expired
    fn is_expired(&self) -> bool {
        if let Ok(elapsed) = self.blocked_at.elapsed() {
            elapsed >= self.block_duration
        } else {
            true // If we can't get the time, assume expired
        }
    }

    /// Time remaining on this block
    fn time_remaining(&self) -> Option<Duration> {
        if let Ok(elapsed) = self.blocked_at.elapsed() {
            self.block_duration.checked_sub(elapsed)
        } else {
            None
        }
    }
}

/// SIP rate limiter and abuse detector
pub struct SipRateLimiter {
    /// Configuration
    config: RwLock<RateLimitConfig>,
    /// Per-IP tracking data (Arc-wrapped so the cleanup task can reference it)
    ip_tracking: Arc<DashMap<IpAddr, IpTrackingData>>,
    /// Currently blocked IPs (Arc-wrapped so the cleanup task can reference it)
    blocked_ips: Arc<DashMap<IpAddr, BlockedIp>>,
    /// Statistics
    total_requests: AtomicU64,
    blocked_requests: AtomicU64,
    rate_limited_requests: AtomicU64,
    scanner_detections: AtomicU64,
}

impl SipRateLimiter {
    /// Create a new rate limiter with default configuration
    pub fn new() -> Self {
        Self::with_config(RateLimitConfig::default())
    }

    /// Create a new rate limiter with specific configuration
    pub fn with_config(config: RateLimitConfig) -> Self {
        Self {
            config: RwLock::new(config),
            ip_tracking: Arc::new(DashMap::new()),
            blocked_ips: Arc::new(DashMap::new()),
            total_requests: AtomicU64::new(0),
            blocked_requests: AtomicU64::new(0),
            rate_limited_requests: AtomicU64::new(0),
            scanner_detections: AtomicU64::new(0),
        }
    }

    /// Update the configuration
    pub fn update_config(&self, config: RateLimitConfig) {
        *self.config.write() = config;
    }

    /// Get current configuration (copy)
    pub fn get_config(&self) -> RateLimitConfig {
        self.config.read().clone()
    }

    /// Check if a SIP message should be allowed through
    ///
    /// Returns `Ok(())` if allowed, `Err(reason)` if blocked
    pub fn check_message(&self, message: &SipMessage, remote_addr: SocketAddr) -> Result<(), String> {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let config = self.config.read().clone();
        if !config.enabled {
            return Ok(());
        }

        let ip = remote_addr.ip();
        let now = Instant::now();

        // First check if IP is currently blocked
        if let Some(blocked) = self.blocked_ips.get(&ip) {
            if blocked.is_expired() {
                // Block expired, remove it
                drop(blocked);
                self.blocked_ips.remove(&ip);
                info!(ip = %ip, "IP block expired, removing from blocked list");
            } else {
                // Still blocked
                self.blocked_requests.fetch_add(1, Ordering::Relaxed);
                if let Some(remaining) = blocked.time_remaining() {
                    return Err(format!(
                        "IP blocked for {} ({}s remaining)",
                        blocked.reason,
                        remaining.as_secs()
                    ));
                } else {
                    return Err(format!("IP blocked for {}", blocked.reason));
                }
            }
        }

        // Get or create tracking data for this IP
        let mut tracking = self.ip_tracking.entry(ip).or_insert_with(IpTrackingData::new);

        // Check the message based on method
        let method = message.method();
        match method {
            Some(SipMethod::Invite) => {
                let current_rate = tracking.record_invite(now);
                if current_rate > config.invite_rate_limit {
                    self.rate_limited_requests.fetch_add(1, Ordering::Relaxed);
                    let reason = format!("INVITE rate limit exceeded: {}/s", current_rate);
                    self.block_ip(ip, reason.clone(), config.block_duration_secs);
                    return Err(reason);
                }
            }
            Some(SipMethod::Register) | Some(SipMethod::Options) => {
                let sequential_count = tracking.record_scanner_request(method.unwrap(), now);
                if sequential_count >= config.scanner_threshold {
                    self.scanner_detections.fetch_add(1, Ordering::Relaxed);
                    let reason = format!(
                        "Scanner detected: {} sequential {} requests",
                        sequential_count,
                        method.unwrap()
                    );
                    self.block_ip(ip, reason.clone(), config.block_duration_secs);
                    return Err(reason);
                }
            }
            _ => {
                // Reset scanner detection for other methods
                if let Some(method) = method {
                    tracking.reset_scanner_detection(method, now);
                }
            }
        }

        Ok(())
    }

    /// Manually block an IP address
    pub fn block_ip(&self, ip: IpAddr, reason: String, duration_secs: u64) {
        let duration = Duration::from_secs(duration_secs);
        let blocked = BlockedIp::new(reason.clone(), duration);
        
        warn!(
            ip = %ip,
            reason = %reason,
            duration_secs = duration_secs,
            "Blocking IP address"
        );

        self.blocked_ips.insert(ip, blocked);
        
        // Also remove from tracking to free memory
        self.ip_tracking.remove(&ip);
    }

    /// Manually unblock an IP address
    pub fn unblock_ip(&self, ip: IpAddr) -> bool {
        if self.blocked_ips.remove(&ip).is_some() {
            info!(ip = %ip, "Manually unblocked IP address");
            true
        } else {
            false
        }
    }

    /// Check if an IP is currently blocked
    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        if let Some(blocked) = self.blocked_ips.get(&ip) {
            if blocked.is_expired() {
                // Clean up expired block
                drop(blocked);
                self.blocked_ips.remove(&ip);
                false
            } else {
                true
            }
        } else {
            false
        }
    }

    /// Get list of currently blocked IPs
    pub fn get_blocked_ips(&self) -> Vec<(IpAddr, String, Option<Duration>)> {
        let mut result = Vec::new();
        
        // Clean up expired blocks while building the list
        let mut expired_ips = Vec::new();
        
        for item in self.blocked_ips.iter() {
            let ip = item.key();
            let blocked = item.value();
            if blocked.is_expired() {
                expired_ips.push(*ip);
            } else {
                result.push((*ip, blocked.reason.clone(), blocked.time_remaining()));
            }
        }
        
        // Remove expired blocks
        for ip in expired_ips {
            self.blocked_ips.remove(&ip);
        }
        
        result
    }

    /// Periodic cleanup of stale IP tracking data
    pub fn cleanup_stale_data(&self) {
        let now = Instant::now();
        let mut stale_ips = Vec::new();

        // Find stale tracking data
        for item in self.ip_tracking.iter() {
            let ip = item.key();
            let tracking = item.value();
            if tracking.is_stale(now) {
                stale_ips.push(*ip);
            }
        }

        // Remove stale data
        for ip in stale_ips {
            if self.ip_tracking.remove(&ip).is_some() {
                debug!(ip = %ip, "Cleaned up stale IP tracking data");
            }
        }

        // Clean up expired blocks
        let mut expired_blocks = Vec::new();
        for item in self.blocked_ips.iter() {
            let ip = item.key();
            let blocked = item.value();
            if blocked.is_expired() {
                expired_blocks.push(*ip);
            }
        }

        for ip in expired_blocks {
            if self.blocked_ips.remove(&ip).is_some() {
                info!(ip = %ip, "Cleaned up expired IP block");
            }
        }
    }

    /// Get rate limiter statistics
    pub fn get_stats(&self) -> RateLimiterStats {
        RateLimiterStats {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            blocked_requests: self.blocked_requests.load(Ordering::Relaxed),
            rate_limited_requests: self.rate_limited_requests.load(Ordering::Relaxed),
            scanner_detections: self.scanner_detections.load(Ordering::Relaxed),
            active_ip_tracking: self.ip_tracking.len(),
            blocked_ips_count: self.blocked_ips.len(),
        }
    }

    /// Reset all statistics
    pub fn reset_stats(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.blocked_requests.store(0, Ordering::Relaxed);
        self.rate_limited_requests.store(0, Ordering::Relaxed);
        self.scanner_detections.store(0, Ordering::Relaxed);
    }

    /// Clear all blocked IPs (emergency unblock)
    pub fn clear_all_blocks(&self) {
        let count = self.blocked_ips.len();
        self.blocked_ips.clear();
        warn!(count = count, "Cleared all IP blocks");
    }

    /// Start a background cleanup task
    pub fn start_cleanup_task(&self) -> tokio::task::JoinHandle<()> {
        // Clone the DashMaps for the background task
        let ip_tracking = self.ip_tracking.clone();
        let blocked_ips = self.blocked_ips.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            
            loop {
                interval.tick().await;
                
                let now = Instant::now();
                let mut stale_ips = Vec::new();
                let mut expired_blocks = Vec::new();

                // Find stale tracking data
                for item in ip_tracking.iter() {
                    let ip = item.key();
                    let tracking = item.value();
                    if tracking.is_stale(now) {
                        stale_ips.push(*ip);
                    }
                }

                // Find expired blocks
                for item in blocked_ips.iter() {
                    let ip = item.key();
                    let blocked = item.value();
                    if blocked.is_expired() {
                        expired_blocks.push(*ip);
                    }
                }

                // Clean up stale data
                let mut cleaned_tracking = 0;
                let mut cleaned_blocks = 0;

                for ip in stale_ips {
                    if ip_tracking.remove(&ip).is_some() {
                        cleaned_tracking += 1;
                    }
                }

                for ip in expired_blocks {
                    if blocked_ips.remove(&ip).is_some() {
                        cleaned_blocks += 1;
                    }
                }

                if cleaned_tracking > 0 || cleaned_blocks > 0 {
                    debug!(
                        cleaned_tracking = cleaned_tracking,
                        cleaned_blocks = cleaned_blocks,
                        "Rate limiter cleanup completed"
                    );
                }
            }
        })
    }
}

impl Default for SipRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Rate limiter statistics
#[derive(Debug, Clone)]
pub struct RateLimiterStats {
    /// Total requests processed
    pub total_requests: u64,
    /// Requests blocked due to IP blocks
    pub blocked_requests: u64,
    /// Requests blocked due to rate limiting
    pub rate_limited_requests: u64,
    /// Scanner detection events
    pub scanner_detections: u64,
    /// Number of IPs being actively tracked
    pub active_ip_tracking: usize,
    /// Number of currently blocked IPs
    pub blocked_ips_count: usize,
}

impl std::fmt::Display for RateLimiterStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Total: {}, Blocked: {}, Rate Limited: {}, Scanners: {}, Tracking: {} IPs, Blocked: {} IPs",
            self.total_requests,
            self.blocked_requests,
            self.rate_limited_requests,
            self.scanner_detections,
            self.active_ip_tracking,
            self.blocked_ips_count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SipMessage;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn create_test_message(method: &str) -> SipMessage {
        let raw = format!(
            "{} sip:test@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.168.1.100:5060\r\n\
             From: <sip:caller@example.com>;tag=123\r\n\
             To: <sip:test@example.com>\r\n\
             Call-ID: test-call-id\r\n\
             CSeq: 1 {}\r\n\
             Content-Length: 0\r\n\r\n",
            method, method
        );
        SipMessage::parse(raw.as_bytes()).expect("Failed to parse test message")
    }

    #[test]
    fn test_invite_rate_limiting() {
        let config = RateLimitConfig {
            invite_rate_limit: 2, // Very low for testing
            scanner_threshold: 10,
            block_duration_secs: 60,
            enabled: true,
        };
        
        let limiter = SipRateLimiter::with_config(config);
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5060);
        let invite_msg = create_test_message("INVITE");

        // First two INVITEs should pass
        assert!(limiter.check_message(&invite_msg, test_addr).is_ok());
        assert!(limiter.check_message(&invite_msg, test_addr).is_ok());

        // Third should be blocked (rate limit exceeded)
        assert!(limiter.check_message(&invite_msg, test_addr).is_err());
        
        // IP should now be blocked
        assert!(limiter.is_blocked(test_addr.ip()));
    }

    #[test]
    fn test_scanner_detection() {
        let config = RateLimitConfig {
            invite_rate_limit: 100,
            scanner_threshold: 3, // Very low for testing
            block_duration_secs: 60,
            enabled: true,
        };
        
        let limiter = SipRateLimiter::with_config(config);
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 101)), 5060);
        let register_msg = create_test_message("REGISTER");

        // First two REGISTERs should pass
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());

        // Third should trigger scanner detection
        assert!(limiter.check_message(&register_msg, test_addr).is_err());
        
        // IP should now be blocked
        assert!(limiter.is_blocked(test_addr.ip()));
    }

    #[test]
    fn test_scanner_reset_on_different_method() {
        let config = RateLimitConfig {
            invite_rate_limit: 100,
            scanner_threshold: 3,
            block_duration_secs: 60,
            enabled: true,
        };
        
        let limiter = SipRateLimiter::with_config(config);
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 102)), 5060);
        let register_msg = create_test_message("REGISTER");
        let invite_msg = create_test_message("INVITE");

        // Two REGISTERs
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());

        // INVITE should reset scanner detection
        assert!(limiter.check_message(&invite_msg, test_addr).is_ok());

        // Now we can send more REGISTERs without immediate blocking
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());
        assert!(limiter.check_message(&register_msg, test_addr).is_ok());
    }

    #[test]
    fn test_disabled_rate_limiting() {
        let config = RateLimitConfig {
            invite_rate_limit: 1,
            scanner_threshold: 1,
            block_duration_secs: 60,
            enabled: false, // Disabled
        };
        
        let limiter = SipRateLimiter::with_config(config);
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 103)), 5060);
        let invite_msg = create_test_message("INVITE");

        // Should allow unlimited requests when disabled
        for _ in 0..10 {
            assert!(limiter.check_message(&invite_msg, test_addr).is_ok());
        }
    }

    #[test]
    fn test_manual_block_unblock() {
        let limiter = SipRateLimiter::new();
        let test_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 104));

        // Block IP manually
        limiter.block_ip(test_ip, "Test block".to_string(), 60);
        assert!(limiter.is_blocked(test_ip));

        // Unblock IP manually
        assert!(limiter.unblock_ip(test_ip));
        assert!(!limiter.is_blocked(test_ip));
    }

    #[test]
    fn test_statistics() {
        let limiter = SipRateLimiter::new();
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 105)), 5060);
        let invite_msg = create_test_message("INVITE");

        let stats_before = limiter.get_stats();
        assert_eq!(stats_before.total_requests, 0);

        // Process some messages
        let _ = limiter.check_message(&invite_msg, test_addr);
        let _ = limiter.check_message(&invite_msg, test_addr);

        let stats_after = limiter.get_stats();
        assert_eq!(stats_after.total_requests, 2);
    }
}