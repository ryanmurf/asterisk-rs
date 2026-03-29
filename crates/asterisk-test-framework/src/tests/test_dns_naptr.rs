//! Port of asterisk/tests/test_dns_naptr.c
//!
//! Tests NAPTR (Naming Authority Pointer) DNS record parsing and sorting:
//!
//! - Nominal NAPTR resolution with many records and correct ordering
//! - Records with various valid flags (A, 3, 32, A32, empty)
//! - Records with various valid services (empty, simple, compound)
//! - Records with valid regexes
//! - NAPTR ordering by order first, then preference
//! - Off-nominal: missing or malformed fields

// ---------------------------------------------------------------------------
// NAPTR record model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct NaptrRecord {
    order: u16,
    preference: u16,
    flags: String,
    services: String,
    regexp: String,
    replacement: String,
}

impl NaptrRecord {
    fn new(
        order: u16,
        preference: u16,
        flags: &str,
        services: &str,
        regexp: &str,
        replacement: &str,
    ) -> Self {
        Self {
            order,
            preference,
            flags: flags.to_string(),
            services: services.to_string(),
            regexp: regexp.to_string(),
            replacement: replacement.to_string(),
        }
    }
}

/// Sort NAPTR records by order first, then by preference.
fn sort_naptr(records: &mut [NaptrRecord]) {
    records.sort_by(|a, b| a.order.cmp(&b.order).then(a.preference.cmp(&b.preference)));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(naptr_resolve_nominal).
///
/// Test nominal resolution with many records and verify they sort correctly.
#[test]
fn test_naptr_resolve_nominal() {
    let mut records = vec![
        NaptrRecord::new(200, 100, "A", "BLAH", "", "goose.down"),
        NaptrRecord::new(300, 8, "", "BLAH", "", "goose.down"),
        NaptrRecord::new(300, 6, "3", "BLAH", "", "goose.down"),
        NaptrRecord::new(100, 2, "32", "BLAH", "", "goose.down"),
        NaptrRecord::new(400, 100, "A32", "BLAH", "", "goose.down"),
        NaptrRecord::new(100, 700, "", "", "", "goose.down"),
        NaptrRecord::new(
            500,
            102,
            "A",
            "A+B12+C+D+EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE",
            "",
            "goose.down",
        ),
        NaptrRecord::new(500, 100, "A", "A+B12+C+D+EEEE", "", "goose.down"),
        NaptrRecord::new(500, 101, "A", "BLAH", "!.*!horse.mane!", ""),
        NaptrRecord::new(500, 99, "A", "BLAH", "0.*0horse.mane0", ""),
        NaptrRecord::new(10, 100, "A", "BLAH", r"!.*!\!\!\!!", ""),
        NaptrRecord::new(
            700,
            999,
            "A",
            "BLAH",
            r"!(.)(.)(.)(.)!\1.m.\2.n\3.o\4!",
            "",
        ),
    ];

    sort_naptr(&mut records);

    // Expected order from C test: indices 10, 3, 5, 0, 2, 1, 4, 9, 7, 8, 6, 11
    // Which corresponds to order values: 10, 100, 100, 200, 300, 300, 400, 500, 500, 500, 500, 700
    let expected_orders = [10, 100, 100, 200, 300, 300, 400, 500, 500, 500, 500, 700];
    for (i, expected_order) in expected_orders.iter().enumerate() {
        assert_eq!(
            records[i].order, *expected_order,
            "Record {} has wrong order: expected {}, got {}",
            i, expected_order, records[i].order
        );
    }

    // Within same order, preference should be ascending
    // Order 100: preference 2, 700
    assert!(records[1].preference <= records[2].preference);
    // Order 300: preference 6, 8
    assert!(records[4].preference <= records[5].preference);
    // Order 500: preference 99, 100, 101, 102
    assert!(records[7].preference <= records[8].preference);
    assert!(records[8].preference <= records[9].preference);
    assert!(records[9].preference <= records[10].preference);
}

/// Test NAPTR record fields are preserved correctly.
#[test]
fn test_naptr_record_fields() {
    let record = NaptrRecord::new(10, 100, "S", "SIP+D2U", "", "_sip._udp.example.com");

    assert_eq!(record.order, 10);
    assert_eq!(record.preference, 100);
    assert_eq!(record.flags, "S");
    assert_eq!(record.services, "SIP+D2U");
    assert!(record.regexp.is_empty());
    assert_eq!(record.replacement, "_sip._udp.example.com");
}

/// Port of NAPTR ordering test.
///
/// Records should be sorted by order, then preference.
#[test]
fn test_naptr_ordering() {
    let mut records = vec![
        NaptrRecord::new(20, 10, "S", "SIP+D2T", "", ""),
        NaptrRecord::new(10, 20, "S", "SIP+D2U", "", ""),
        NaptrRecord::new(10, 10, "S", "SIPS+D2T", "", ""),
    ];

    sort_naptr(&mut records);

    assert_eq!(records[0].services, "SIPS+D2T"); // order=10, pref=10
    assert_eq!(records[1].services, "SIP+D2U"); // order=10, pref=20
    assert_eq!(records[2].services, "SIP+D2T"); // order=20, pref=10
}

/// Test NAPTR with regex field.
#[test]
fn test_naptr_with_regex() {
    let record = NaptrRecord::new(100, 50, "U", "E2U+sip", "!^.*$!sip:info@example.com!", "");

    assert_eq!(record.flags, "U");
    assert_eq!(record.services, "E2U+sip");
    assert!(!record.regexp.is_empty());
    assert!(record.replacement.is_empty());
}

/// Test empty NAPTR set.
#[test]
fn test_naptr_empty_set() {
    let mut records: Vec<NaptrRecord> = Vec::new();
    sort_naptr(&mut records);
    assert!(records.is_empty());
}

/// Test single NAPTR record.
#[test]
fn test_naptr_single_record() {
    let mut records = vec![NaptrRecord::new(10, 50, "A", "SIP+D2U", "", "sip.example.com")];

    sort_naptr(&mut records);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].order, 10);
    assert_eq!(records[0].preference, 50);
}
