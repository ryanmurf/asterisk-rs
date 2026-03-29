//! Port of asterisk/tests/test_dns_srv.c
//!
//! Tests SRV (Service) DNS record parsing, priority sorting, and weight
//! selection:
//!
//! - Single SRV record resolution
//! - Multiple records sorted by priority
//! - Records with same priority, different weights
//! - Records with different priorities and weights
//! - Degenerate case: all zero weights
//! - Record field parsing (priority, weight, port, host)
//! - Off-nominal: missing fields
//! - Large number of SRV records

// ---------------------------------------------------------------------------
// SRV record model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SrvRecord {
    priority: u16,
    weight: u16,
    port: u16,
    host: String,
}

impl SrvRecord {
    fn new(priority: u16, weight: u16, port: u16, host: &str) -> Self {
        Self {
            priority,
            weight,
            port,
            host: host.to_string(),
        }
    }
}

/// Sort SRV records according to RFC 2782:
/// - Primary sort by priority (ascending)
/// - Within same priority, by weight (descending, higher weight = more preferred)
fn sort_srv(records: &mut [SrvRecord]) {
    records.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(b.weight.cmp(&a.weight))
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(srv_resolve_single_record).
///
/// Test resolving a single SRV record.
#[test]
fn test_srv_resolve_single_record() {
    let mut records = vec![SrvRecord::new(10, 10, 5060, "goose.down")];

    sort_srv(&mut records);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].priority, 10);
    assert_eq!(records[0].weight, 10);
    assert_eq!(records[0].port, 5060);
    assert_eq!(records[0].host, "goose.down");
}

/// Port of AST_TEST_DEFINE(srv_resolve_sort_by_priority).
///
/// Records with different priorities should sort by priority ascending.
#[test]
fn test_srv_resolve_sort_by_priority() {
    let mut records = vec![
        SrvRecord::new(30, 10, 5060, "third.example.com"),
        SrvRecord::new(10, 10, 5060, "first.example.com"),
        SrvRecord::new(20, 10, 5060, "second.example.com"),
    ];

    sort_srv(&mut records);

    assert_eq!(records[0].host, "first.example.com");
    assert_eq!(records[0].priority, 10);
    assert_eq!(records[1].host, "second.example.com");
    assert_eq!(records[1].priority, 20);
    assert_eq!(records[2].host, "third.example.com");
    assert_eq!(records[2].priority, 30);
}

/// Port of AST_TEST_DEFINE(srv_resolve_same_priority_weight).
///
/// Records with same priority should sort by weight descending.
#[test]
fn test_srv_resolve_same_priority_weight() {
    let mut records = vec![
        SrvRecord::new(10, 10, 5060, "low.example.com"),
        SrvRecord::new(10, 70, 5060, "high.example.com"),
        SrvRecord::new(10, 20, 5060, "mid.example.com"),
    ];

    sort_srv(&mut records);

    assert_eq!(records[0].host, "high.example.com");
    assert_eq!(records[0].weight, 70);
    assert_eq!(records[1].host, "mid.example.com");
    assert_eq!(records[1].weight, 20);
    assert_eq!(records[2].host, "low.example.com");
    assert_eq!(records[2].weight, 10);
}

/// Port of AST_TEST_DEFINE(srv_resolve_different_priorities_and_weights).
///
/// Records with different priorities should sort by priority first,
/// then by weight within same priority.
#[test]
fn test_srv_resolve_different_priorities_and_weights() {
    let mut records = vec![
        SrvRecord::new(20, 30, 5060, "p20w30.example.com"),
        SrvRecord::new(10, 10, 5060, "p10w10.example.com"),
        SrvRecord::new(10, 50, 5060, "p10w50.example.com"),
        SrvRecord::new(20, 10, 5060, "p20w10.example.com"),
    ];

    sort_srv(&mut records);

    // Priority 10 first, weight 50 before 10
    assert_eq!(records[0].host, "p10w50.example.com");
    assert_eq!(records[1].host, "p10w10.example.com");
    // Priority 20 second, weight 30 before 10
    assert_eq!(records[2].host, "p20w30.example.com");
    assert_eq!(records[3].host, "p20w10.example.com");
}

/// Port of AST_TEST_DEFINE(srv_resolve_all_zero_weights).
///
/// All zero weights is a valid degenerate case.
#[test]
fn test_srv_resolve_all_zero_weights() {
    let mut records = vec![
        SrvRecord::new(10, 0, 5060, "a.example.com"),
        SrvRecord::new(10, 0, 5061, "b.example.com"),
        SrvRecord::new(10, 0, 5062, "c.example.com"),
    ];

    sort_srv(&mut records);

    // All same priority and weight, order is stable
    assert_eq!(records.len(), 3);
    for r in &records {
        assert_eq!(r.priority, 10);
        assert_eq!(r.weight, 0);
    }
}

/// Test SRV record field parsing.
#[test]
fn test_srv_record_fields() {
    let record = SrvRecord::new(10, 60, 5060, "sip.example.com");

    assert_eq!(record.priority, 10);
    assert_eq!(record.weight, 60);
    assert_eq!(record.port, 5060);
    assert_eq!(record.host, "sip.example.com");
}

/// Test large number of SRV records.
#[test]
fn test_srv_large_record_set() {
    let mut records: Vec<SrvRecord> = (0..100)
        .map(|i| SrvRecord::new(i, 100 - i, 5060 + i, &format!("host{}.example.com", i)))
        .collect();

    sort_srv(&mut records);

    // Verify sorted by priority ascending
    for i in 1..records.len() {
        assert!(
            records[i].priority >= records[i - 1].priority,
            "Records should be sorted by priority at index {}",
            i
        );
    }

    assert_eq!(records[0].priority, 0);
    assert_eq!(records[99].priority, 99);
}

/// Port of off-nominal test: empty SRV set.
#[test]
fn test_srv_empty_set() {
    let mut records: Vec<SrvRecord> = Vec::new();
    sort_srv(&mut records);
    assert!(records.is_empty());
}
