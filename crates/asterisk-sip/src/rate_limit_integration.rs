//! Example integration of SIP rate limiting with event handler
//!
//! This demonstrates how to use the SipRateLimiter with the SipEventHandler
//! for comprehensive abuse detection and prevention.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use crate::rate_limit::{SipRateLimiter, RateLimitConfig};
use crate::parser::SipMessage;

/// Example of how to set up rate limiting in a SIP server
pub async fn setup_rate_limited_sip_server() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Create a custom rate limit configuration
    let rate_config = RateLimitConfig {
        invite_rate_limit: 20,        // 20 INVITE/sec per IP
        scanner_threshold: 50,        // Block after 50 sequential REGISTER/OPTIONS
        block_duration_secs: 600,     // Block for 10 minutes
        enabled: true,
    };

    // 2. Create the rate limiter
    let rate_limiter = Arc::new(SipRateLimiter::with_config(rate_config));

    // 3. Start the cleanup task (important for memory management)
    let _cleanup_handle = rate_limiter.start_cleanup_task();

    // 4. Create event handler with rate limiter integration
    // (This would typically use your actual dialplan and transport)
    // let dialplan = Arc::new(your_dialplan);
    // let transport = Arc::new(your_transport);
    // let handler = SipEventHandler::with_rate_limiter(dialplan, transport, rate_limiter.clone());

    // 5. Example of manual rate checking before processing messages
    let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 5060);
    let invite_message = create_invite_message();

    match rate_limiter.check_message(&invite_message, test_addr) {
        Ok(()) => {
            println!("Message allowed, processing...");
            // Process the SIP message normally
        }
        Err(reason) => {
            println!("Message blocked: {}", reason);
            // Send appropriate SIP error response (503 Service Unavailable)
        }
    }

    // 6. Monitor rate limiter statistics
    tokio::spawn({
        let rate_limiter = rate_limiter.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let stats = rate_limiter.get_stats();
                println!("Rate limiter stats: {}", stats);

                // Log currently blocked IPs
                let blocked_ips = rate_limiter.get_blocked_ips();
                if !blocked_ips.is_empty() {
                    println!("Currently blocked IPs:");
                    for (ip, reason, remaining) in blocked_ips {
                        match remaining {
                            Some(duration) => {
                                println!("  {}: {} ({}s remaining)", ip, reason, duration.as_secs())
                            }
                            None => println!("  {}: {}", ip, reason),
                        }
                    }
                }
            }
        }
    });

    // 7. Example of administrative controls
    
    // Manually block a problematic IP
    rate_limiter.block_ip(
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        "Manually blocked by admin".to_string(),
        3600, // 1 hour
    );

    // Check if an IP is blocked
    if rate_limiter.is_blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))) {
        println!("IP 10.0.0.1 is currently blocked");
    }

    // Manually unblock an IP
    if rate_limiter.unblock_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))) {
        println!("Successfully unblocked IP 10.0.0.1");
    }

    // Update configuration at runtime
    let new_config = RateLimitConfig {
        invite_rate_limit: 30,        // Increased limit
        scanner_threshold: 100,       // More tolerant scanner detection
        block_duration_secs: 300,     // Shorter blocks
        enabled: true,
    };
    rate_limiter.update_config(new_config);

    // Emergency: clear all blocks
    // rate_limiter.clear_all_blocks();

    Ok(())
}

/// Create a sample INVITE message for testing
fn create_invite_message() -> SipMessage {
    let raw_message = "\
INVITE sip:test@example.com SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.100:5060;branch=z9hG4bK776asdhds\r\n\
Max-Forwards: 70\r\n\
To: <sip:test@example.com>\r\n\
From: Alice <sip:alice@atlanta.com>;tag=1928301774\r\n\
Call-ID: a84b4c76e66710@pc33.atlanta.com\r\n\
CSeq: 314159 INVITE\r\n\
Contact: <sip:alice@pc33.atlanta.com>\r\n\
Content-Type: application/sdp\r\n\
Content-Length: 142\r\n\
\r\n\
v=0\r\n\
o=alice 53655765 2353687637 IN IP4 pc33.atlanta.com\r\n\
s=-\r\n\
c=IN IP4 pc33.atlanta.com\r\n\
t=0 0\r\n\
m=audio 3456 RTP/AVP 0\r\n\
a=rtpmap:0 PCMU/8000\r\n";

    SipMessage::parse(raw_message.as_bytes()).expect("Failed to parse test INVITE")
}

/// Example of how to integrate rate limiting in different scenarios
pub mod scenarios {
    use super::*;

    /// High-traffic SIP trunk scenario
    pub fn high_traffic_trunk_config() -> RateLimitConfig {
        RateLimitConfig {
            invite_rate_limit: 100,      // High-capacity trunk
            scanner_threshold: 200,      // More tolerance for legitimate traffic
            block_duration_secs: 1800,   // 30-minute blocks
            enabled: true,
        }
    }

    /// Residential/small business scenario
    pub fn residential_config() -> RateLimitConfig {
        RateLimitConfig {
            invite_rate_limit: 10,       // Lower expected traffic
            scanner_threshold: 20,       // More sensitive to scanning
            block_duration_secs: 300,    // 5-minute blocks
            enabled: true,
        }
    }

    /// Development/testing scenario
    pub fn development_config() -> RateLimitConfig {
        RateLimitConfig {
            invite_rate_limit: 1000,     // Very high limits for testing
            scanner_threshold: 1000,     // Avoid blocking during tests
            block_duration_secs: 60,     // Short blocks
            enabled: false,              // Can be disabled for debugging
        }
    }

    /// Demonstration of progressive rate limiting based on time of day.
    ///
    /// Uses wall-clock UTC hour to switch between business-hours and
    /// off-hours profiles. No external crate (e.g. chrono) is required.
    pub async fn time_based_rate_limiting(rate_limiter: Arc<SipRateLimiter>) {
        loop {
            // Derive current UTC hour from SystemTime without chrono.
            let secs_since_epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let current_hour = ((secs_since_epoch % 86400) / 3600) as u32;

            let config = match current_hour {
                // Business hours (9 AM - 5 PM UTC): Higher limits
                9..=17 => RateLimitConfig {
                    invite_rate_limit: 50,
                    scanner_threshold: 100,
                    block_duration_secs: 600,
                    enabled: true,
                },
                // Off hours: Lower limits, more sensitive
                _ => RateLimitConfig {
                    invite_rate_limit: 20,
                    scanner_threshold: 50,
                    block_duration_secs: 1200,
                    enabled: true,
                },
            };

            rate_limiter.update_config(config);

            // Check again in an hour
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_integration() {
        let rate_limiter = Arc::new(SipRateLimiter::new());
        let test_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200)), 5060);
        let invite_msg = create_invite_message();

        // First message should be allowed
        assert!(rate_limiter.check_message(&invite_msg, test_addr).is_ok());

        let stats = rate_limiter.get_stats();
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.blocked_requests, 0);
    }

    #[tokio::test]
    async fn test_configuration_update() {
        let rate_limiter = Arc::new(SipRateLimiter::new());
        
        // Update to very restrictive config
        let strict_config = RateLimitConfig {
            invite_rate_limit: 1,
            scanner_threshold: 2,
            block_duration_secs: 60,
            enabled: true,
        };
        rate_limiter.update_config(strict_config);

        // Verify config was updated
        let current_config = rate_limiter.get_config();
        assert_eq!(current_config.invite_rate_limit, 1);
        assert_eq!(current_config.scanner_threshold, 2);
    }
}