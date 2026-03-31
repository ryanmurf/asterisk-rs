# SIP Rate Limiting and Abuse Detection

This module provides comprehensive protection against SIP-based attacks and abuse, including INVITE floods, scanner detection, and automatic IP blocking.

## Features

- **Per-IP INVITE rate tracking**: Monitors INVITE requests per second per IP address
- **Configurable thresholds**: Default 50 INVITE/sec, fully customizable
- **Automatic IP blocking**: IPs exceeding thresholds are automatically blocked
- **Scanner detection**: Detects sequential REGISTER/OPTIONS floods (SIP scanning behavior)
- **DashMap-based tracking**: High-performance concurrent IP tracking with automatic expiry
- **Integration ready**: Seamlessly integrates with the SIP event handler
- **Administrative controls**: Manual block/unblock capabilities
- **Real-time statistics**: Comprehensive metrics and monitoring

## Configuration

```rust
use asterisk_sip::rate_limit::{SipRateLimiter, RateLimitConfig};

// Default configuration
let rate_limiter = SipRateLimiter::new();

// Custom configuration
let config = RateLimitConfig {
    invite_rate_limit: 20,        // 20 INVITE/sec per IP
    scanner_threshold: 50,        // Block after 50 sequential REGISTER/OPTIONS
    block_duration_secs: 600,     // Block for 10 minutes
    enabled: true,
};
let rate_limiter = SipRateLimiter::with_config(config);
```

## Basic Usage

### Integration with Event Handler

The rate limiter integrates directly with the SIP event handler for automatic protection:

```rust
use asterisk_sip::{SipEventHandler, SipRateLimiter};

// Create event handler with rate limiting
let rate_limiter = Arc::new(SipRateLimiter::new());
let handler = SipEventHandler::with_rate_limiter(
    dialplan,
    transport,
    rate_limiter.clone(),
);

// The event handler will now automatically check rates for all incoming messages
```

### Manual Rate Checking

For custom integrations or additional checks:

```rust
use std::net::SocketAddr;

// Check a message against rate limits
match rate_limiter.check_message(&sip_message, remote_addr) {
    Ok(()) => {
        // Message allowed, process normally
        println!("Processing message");
    }
    Err(reason) => {
        // Message blocked, send error response
        println!("Blocked: {}", reason);
        // Send 503 Service Unavailable
    }
}
```

## Administrative Controls

### Manual IP Blocking

```rust
use std::net::{IpAddr, Ipv4Addr};

// Block an IP manually
let problem_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
rate_limiter.block_ip(
    problem_ip,
    "Manual block - suspicious activity".to_string(),
    3600, // Block for 1 hour
);

// Check if IP is blocked
if rate_limiter.is_blocked(problem_ip) {
    println!("IP is currently blocked");
}

// Unblock an IP
if rate_limiter.unblock_ip(problem_ip) {
    println!("IP unblocked successfully");
}
```

### Emergency Controls

```rust
// Clear all blocks (emergency use)
rate_limiter.clear_all_blocks();

// Get list of currently blocked IPs
let blocked = rate_limiter.get_blocked_ips();
for (ip, reason, remaining) in blocked {
    match remaining {
        Some(duration) => println!("{}: {} ({}s left)", ip, reason, duration.as_secs()),
        None => println!("{}: {}", ip, reason),
    }
}
```

## Runtime Configuration Updates

The rate limiter supports runtime configuration changes without restart:

```rust
// Update configuration at runtime
let new_config = RateLimitConfig {
    invite_rate_limit: 30,        // Increased limit
    scanner_threshold: 100,       // More tolerant
    block_duration_secs: 300,     // Shorter blocks
    enabled: true,
};
rate_limiter.update_config(new_config);

// Get current configuration
let current_config = rate_limiter.get_config();
println!("Current INVITE limit: {}", current_config.invite_rate_limit);
```

## Monitoring and Statistics

### Basic Statistics

```rust
let stats = rate_limiter.get_stats();
println!("Rate Limiter Stats:");
println!("  Total requests: {}", stats.total_requests);
println!("  Blocked requests: {}", stats.blocked_requests);
println!("  Rate limited: {}", stats.rate_limited_requests);
println!("  Scanner detections: {}", stats.scanner_detections);
println!("  Active tracking: {} IPs", stats.active_ip_tracking);
println!("  Blocked IPs: {}", stats.blocked_ips_count);
```

### Continuous Monitoring

```rust
use tokio::time::Duration;

tokio::spawn({
    let rate_limiter = rate_limiter.clone();
    async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let stats = rate_limiter.get_stats();
            
            if stats.blocked_requests > 0 {
                tracing::warn!("Rate limiter active: {}", stats);
            }
            
            // Log blocked IPs
            let blocked = rate_limiter.get_blocked_ips();
            if !blocked.is_empty() {
                tracing::info!("Currently blocking {} IPs", blocked.len());
            }
        }
    }
});
```

## Protection Scenarios

### INVITE Flood Protection

The rate limiter tracks INVITE requests per IP per second:

- Default threshold: 50 INVITE/sec per IP
- Sliding window: 1-second window with automatic cleanup
- Action: Automatic IP block when threshold exceeded
- Response: 503 Service Unavailable with Retry-After header

### Scanner Detection

Detects SIP scanning patterns:

- Monitors sequential REGISTER/OPTIONS requests
- Default threshold: 100 sequential requests
- Resets on different message types (normal behavior)
- Common scanner tools detected: SIPVicious, SIPScan, etc.

### Memory Management

- Automatic cleanup of stale IP tracking data (60-second TTL)
- Automatic expiry of IP blocks
- Background cleanup task runs every 30 seconds
- Uses DashMap for high-performance concurrent access

## Configuration Presets

### High-Traffic Trunk

```rust
let config = RateLimitConfig {
    invite_rate_limit: 100,      // High capacity
    scanner_threshold: 200,      // More tolerance
    block_duration_secs: 1800,   // 30-minute blocks
    enabled: true,
};
```

### Residential/Small Business

```rust
let config = RateLimitConfig {
    invite_rate_limit: 10,       // Lower expected traffic
    scanner_threshold: 20,       // More sensitive
    block_duration_secs: 300,    // 5-minute blocks
    enabled: true,
};
```

### Development/Testing

```rust
let config = RateLimitConfig {
    invite_rate_limit: 1000,     // High limits
    scanner_threshold: 1000,     // Avoid blocking
    block_duration_secs: 60,     // Short blocks
    enabled: false,              // Can disable for debugging
};
```

## Integration Notes

### Event Handler Integration

When using with `SipEventHandler`, rate checking occurs automatically:

1. All incoming messages are checked before processing
2. Blocked messages receive appropriate SIP error responses
3. Statistics are updated automatically
4. No additional code required in message handlers

### Response Handling

Blocked requests receive standardized responses:

- **503 Service Unavailable**: For rate limited requests
- **Retry-After header**: Suggests backoff period
- **Proper Call-ID correlation**: Maintains SIP compliance
- **Logging**: All blocks logged with IP and reason

### Performance Considerations

- DashMap provides lock-free concurrent access
- O(1) lookups for IP tracking and blocking
- Automatic memory cleanup prevents unbounded growth
- Background tasks use minimal CPU
- Suitable for high-traffic production environments

## Security Best Practices

1. **Monitor statistics regularly** to detect attack patterns
2. **Tune thresholds** based on your traffic patterns
3. **Use appropriate block durations** (too short = ineffective, too long = DoS)
4. **Combine with firewall rules** for persistent attackers
5. **Log all blocks** for security analysis
6. **Consider geographic restrictions** for additional protection
7. **Regular security reviews** of blocked IPs and patterns

## Error Handling

The rate limiter is designed to fail safely:

- Invalid configurations use safe defaults
- Network errors don't affect rate limiting
- Memory pressure triggers aggressive cleanup
- Disabled mode allows all traffic (failsafe)

This ensures your SIP service remains available even if the rate limiter encounters issues.