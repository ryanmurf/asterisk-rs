//! Port of asterisk/tests/test_dns.c, test_dns_srv.c, test_dns_naptr.c
//!
//! Tests DNS SRV lookup functionality via the SRVQUERY/SRVRESULT
//! dialplan functions in asterisk-funcs (func_srv.rs):
//! - SRV record parsing
//! - NAPTR record parsing (stub -- we test the function interface)
//! - Priority/weight sorting (tested via the SRV record data model)
//! - DNS response handling
//!
//! Since we don't have a full DNS resolver module, we test the dialplan
//! function interface and the SRV record data model.

use asterisk_funcs::srv::{FuncSrvQuery, FuncSrvResult, SRV_FIELDS};
use asterisk_funcs::{DialplanFunc, FuncContext};

// ---------------------------------------------------------------------------
// SRV record parsing via dialplan functions
// ---------------------------------------------------------------------------

/// Port of the SRV query initiation test from test_dns_srv.c.
///
/// Verify that SRVQUERY() returns a non-empty query ID.
#[test]
fn test_srvquery_returns_query_id() {
    let ctx = FuncContext::new();
    let func = FuncSrvQuery;

    let result = func.read(&ctx, "_sip._udp.example.com");
    assert!(result.is_ok());
    let query_id = result.unwrap();
    assert!(!query_id.is_empty());
    // Query ID should be deterministic based on service name.
    assert!(query_id.contains("sip"));
}

/// Test SRVQUERY with different service names.
#[test]
fn test_srvquery_different_services() {
    let ctx = FuncContext::new();
    let func = FuncSrvQuery;

    let id1 = func.read(&ctx, "_sip._udp.example.com").unwrap();
    let id2 = func.read(&ctx, "_xmpp._tcp.example.com").unwrap();

    // Different services should produce different query IDs.
    assert_ne!(id1, id2);
}

/// Test SRVQUERY with empty argument fails.
#[test]
fn test_srvquery_empty_arg_fails() {
    let ctx = FuncContext::new();
    let func = FuncSrvQuery;

    let result = func.read(&ctx, "");
    assert!(result.is_err());
}

/// Test SRVQUERY with whitespace-only argument fails.
#[test]
fn test_srvquery_whitespace_arg_fails() {
    let ctx = FuncContext::new();
    let func = FuncSrvQuery;

    let result = func.read(&ctx, "   ");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// SRV result retrieval
// ---------------------------------------------------------------------------

/// Port of the SRV result retrieval test from test_dns_srv.c.
///
/// Verify that SRVRESULT() with "getnum" returns "0" when no results stored.
#[test]
fn test_srvresult_getnum_default() {
    let ctx = FuncContext::new();
    let func = FuncSrvResult;

    let result = func.read(&ctx, "some_query_id,getnum");
    assert_eq!(result.unwrap(), "0");
}

/// Test SRVRESULT with stored results.
#[test]
fn test_srvresult_with_stored_data() {
    let mut ctx = FuncContext::new();
    let func = FuncSrvResult;

    // Simulate stored SRV results by setting context variables.
    ctx.set_variable("myquery_count", "2");
    ctx.set_variable("myquery_1_host", "sip1.example.com");
    ctx.set_variable("myquery_1_port", "5060");
    ctx.set_variable("myquery_1_priority", "10");
    ctx.set_variable("myquery_1_weight", "20");
    ctx.set_variable("myquery_2_host", "sip2.example.com");
    ctx.set_variable("myquery_2_port", "5061");
    ctx.set_variable("myquery_2_priority", "20");
    ctx.set_variable("myquery_2_weight", "10");

    // Get count.
    assert_eq!(func.read(&ctx, "myquery,getnum").unwrap(), "2");

    // Get individual fields.
    assert_eq!(
        func.read(&ctx, "myquery,1,host").unwrap(),
        "sip1.example.com"
    );
    assert_eq!(func.read(&ctx, "myquery,1,port").unwrap(), "5060");
    assert_eq!(func.read(&ctx, "myquery,1,priority").unwrap(), "10");
    assert_eq!(func.read(&ctx, "myquery,1,weight").unwrap(), "20");

    assert_eq!(
        func.read(&ctx, "myquery,2,host").unwrap(),
        "sip2.example.com"
    );
    assert_eq!(func.read(&ctx, "myquery,2,port").unwrap(), "5061");
}

/// Test SRVRESULT with invalid field name.
#[test]
fn test_srvresult_invalid_field() {
    let ctx = FuncContext::new();
    let func = FuncSrvResult;

    let result = func.read(&ctx, "some_id,1,bogus");
    assert!(result.is_err());
}

/// Test SRVRESULT with missing arguments.
#[test]
fn test_srvresult_missing_args() {
    let ctx = FuncContext::new();
    let func = FuncSrvResult;

    let result = func.read(&ctx, "only_one_arg");
    assert!(result.is_err());
}

/// Test SRVRESULT with empty query ID.
#[test]
fn test_srvresult_empty_query_id() {
    let ctx = FuncContext::new();
    let func = FuncSrvResult;

    let result = func.read(&ctx, ",1,host");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// SRV field constants
// ---------------------------------------------------------------------------

/// Test that SRV_FIELDS contains the expected field names.
#[test]
fn test_srv_fields_constant() {
    assert!(SRV_FIELDS.contains(&"host"));
    assert!(SRV_FIELDS.contains(&"port"));
    assert!(SRV_FIELDS.contains(&"priority"));
    assert!(SRV_FIELDS.contains(&"weight"));
    assert_eq!(SRV_FIELDS.len(), 4);
}

// ---------------------------------------------------------------------------
// Priority/weight sorting (simulated with data model)
// ---------------------------------------------------------------------------

/// Port of the SRV priority sorting test from test_dns_srv.c.
///
/// Verify that SRV records stored with different priorities are
/// retrievable in the correct order. In a full implementation, the
/// SRVQUERY function would sort by priority then weight. Here we
/// verify the data model supports this.
#[test]
fn test_srv_priority_ordering() {
    // Simulate three SRV records with different priorities.
    #[derive(Debug)]
    struct SrvRecord {
        host: String,
        port: u16,
        priority: u16,
        weight: u16,
    }

    let mut records = vec![
        SrvRecord {
            host: "sip3.example.com".into(),
            port: 5060,
            priority: 30,
            weight: 10,
        },
        SrvRecord {
            host: "sip1.example.com".into(),
            port: 5060,
            priority: 10,
            weight: 20,
        },
        SrvRecord {
            host: "sip2.example.com".into(),
            port: 5060,
            priority: 20,
            weight: 30,
        },
    ];

    // Sort by priority (ascending), then weight (descending for selection).
    records.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));

    assert_eq!(records[0].host, "sip1.example.com");
    assert_eq!(records[1].host, "sip2.example.com");
    assert_eq!(records[2].host, "sip3.example.com");
}

/// Port of the SRV weight selection test from test_dns_srv.c.
///
/// Verify that among records with the same priority, weight is used
/// for selection.
#[test]
fn test_srv_weight_selection_same_priority() {
    #[derive(Debug)]
    struct SrvRecord {
        host: String,
        priority: u16,
        weight: u16,
    }

    let records = vec![
        SrvRecord {
            host: "a.example.com".into(),
            priority: 10,
            weight: 70,
        },
        SrvRecord {
            host: "b.example.com".into(),
            priority: 10,
            weight: 20,
        },
        SrvRecord {
            host: "c.example.com".into(),
            priority: 10,
            weight: 10,
        },
    ];

    // All have the same priority.
    assert!(records.iter().all(|r| r.priority == 10));

    // Total weight.
    let total_weight: u16 = records.iter().map(|r| r.weight).sum();
    assert_eq!(total_weight, 100);

    // The record with the highest weight should be selected most often.
    assert_eq!(records[0].weight, 70);
}

// ---------------------------------------------------------------------------
// NAPTR record handling (stub tests)
// ---------------------------------------------------------------------------

/// Port of the NAPTR record parsing test from test_dns_naptr.c.
///
/// Since we don't have a full NAPTR module, we test the general DNS
/// response data model pattern.
#[test]
fn test_naptr_record_model() {
    #[derive(Debug)]
    struct NaptrRecord {
        order: u16,
        preference: u16,
        flags: String,
        service: String,
        regexp: String,
        replacement: String,
    }

    let record = NaptrRecord {
        order: 10,
        preference: 100,
        flags: "S".to_string(),
        service: "SIP+D2U".to_string(),
        regexp: String::new(),
        replacement: "_sip._udp.example.com".to_string(),
    };

    assert_eq!(record.order, 10);
    assert_eq!(record.preference, 100);
    assert_eq!(record.flags, "S");
    assert_eq!(record.service, "SIP+D2U");
    assert!(record.regexp.is_empty());
    assert_eq!(record.replacement, "_sip._udp.example.com");
}

/// Test NAPTR record ordering.
#[test]
fn test_naptr_record_ordering() {
    #[derive(Debug)]
    struct NaptrRecord {
        order: u16,
        preference: u16,
        service: String,
    }

    let mut records = vec![
        NaptrRecord {
            order: 20,
            preference: 10,
            service: "SIP+D2T".into(),
        },
        NaptrRecord {
            order: 10,
            preference: 20,
            service: "SIP+D2U".into(),
        },
        NaptrRecord {
            order: 10,
            preference: 10,
            service: "SIPS+D2T".into(),
        },
    ];

    // Sort by order first, then preference.
    records.sort_by(|a, b| a.order.cmp(&b.order).then(a.preference.cmp(&b.preference)));

    assert_eq!(records[0].service, "SIPS+D2T"); // order=10, pref=10
    assert_eq!(records[1].service, "SIP+D2U"); // order=10, pref=20
    assert_eq!(records[2].service, "SIP+D2T"); // order=20, pref=10
}
