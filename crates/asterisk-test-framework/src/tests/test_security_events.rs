//! Port of asterisk/tests/test_security_events.c
//!
//! Tests security event creation and reporting:
//! - Event type enumeration
//! - Event field population
//! - Multiple security event types
//! - Address/transport field validation
//! - Event creation does not panic

use std::net::SocketAddr;

// ---------------------------------------------------------------------------
// Security event system mirroring Asterisk
// ---------------------------------------------------------------------------

/// Transport type for security events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Udp,
    Tcp,
    Tls,
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Udp => write!(f, "UDP"),
            Transport::Tcp => write!(f, "TCP"),
            Transport::Tls => write!(f, "TLS"),
        }
    }
}

/// Security event types, mirroring AST_SECURITY_EVENT_*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SecurityEventType {
    FailedAcl,
    InvalAcctId,
    SessionLimit,
    MemLimit,
    LoadAvg,
    ReqNoSupport,
    ReqNotAllowed,
    AuthMethodNotAllowed,
    ReqBadFormat,
    SuccessfulAuth,
    UnexpectedAddr,
    ChalRespFailed,
    InvalPassword,
    ChalSent,
    InvalTransport,
}

impl SecurityEventType {
    fn all() -> &'static [SecurityEventType] {
        &[
            SecurityEventType::FailedAcl,
            SecurityEventType::InvalAcctId,
            SecurityEventType::SessionLimit,
            SecurityEventType::MemLimit,
            SecurityEventType::LoadAvg,
            SecurityEventType::ReqNoSupport,
            SecurityEventType::ReqNotAllowed,
            SecurityEventType::AuthMethodNotAllowed,
            SecurityEventType::ReqBadFormat,
            SecurityEventType::SuccessfulAuth,
            SecurityEventType::UnexpectedAddr,
            SecurityEventType::ChalRespFailed,
            SecurityEventType::InvalPassword,
            SecurityEventType::ChalSent,
            SecurityEventType::InvalTransport,
        ]
    }
}

/// Common fields for all security events.
#[derive(Debug, Clone)]
struct SecurityEventCommon {
    event_type: SecurityEventType,
    service: String,
    #[allow(dead_code)]
    module: String,
    account_id: String,
    session_id: String,
    local_addr: SocketAddr,
    local_transport: Transport,
    remote_addr: SocketAddr,
    remote_transport: Transport,
}

/// A security event with common fields and optional extra fields.
#[derive(Debug, Clone)]
struct SecurityEvent {
    common: SecurityEventCommon,
    extra: std::collections::HashMap<String, String>,
}

impl SecurityEvent {
    fn new(common: SecurityEventCommon) -> Self {
        Self {
            common,
            extra: std::collections::HashMap::new(),
        }
    }

    fn with_extra(mut self, key: &str, value: &str) -> Self {
        self.extra.insert(key.to_string(), value.to_string());
        self
    }

    /// Validate that all required common fields are populated.
    fn is_valid(&self) -> bool {
        !self.common.service.is_empty()
            && !self.common.account_id.is_empty()
            && !self.common.session_id.is_empty()
    }
}

/// Generate a test event of the given type.
fn generate_event(event_type: SecurityEventType) -> SecurityEvent {
    let (local, remote, transport, account, session) = match event_type {
        SecurityEventType::FailedAcl => (
            "192.168.1.1:12121",
            "192.168.1.2:12345",
            Transport::Udp,
            "Username",
            "Session123",
        ),
        SecurityEventType::InvalAcctId => (
            "10.1.2.3:4321",
            "10.1.2.4:123",
            Transport::Tcp,
            "FakeUser",
            "Session456",
        ),
        SecurityEventType::SessionLimit => (
            "10.5.4.3:4444",
            "10.5.4.2:3333",
            Transport::Tls,
            "Jenny",
            "8675309",
        ),
        SecurityEventType::MemLimit => (
            "10.10.10.10:555",
            "10.10.10.12:5656",
            Transport::Udp,
            "Felix",
            "Session2604",
        ),
        SecurityEventType::LoadAvg => (
            "10.11.12.13:9876",
            "10.12.11.10:9825",
            Transport::Udp,
            "GuestAccount",
            "XYZ123",
        ),
        SecurityEventType::ReqNoSupport => (
            "10.110.120.130:9888",
            "10.120.110.100:9777",
            Transport::Udp,
            "George",
            "asdkl23478289lasdkf",
        ),
        SecurityEventType::ReqNotAllowed => (
            "10.110.120.130:9888",
            "10.120.110.100:9777",
            Transport::Udp,
            "George",
            "alksdjf023423h4lka0df",
        ),
        SecurityEventType::AuthMethodNotAllowed => (
            "10.110.120.135:8754",
            "10.120.110.105:8745",
            Transport::Tcp,
            "Bob",
            "010101010101",
        ),
        SecurityEventType::ReqBadFormat => (
            "10.110.120.130:9888",
            "10.120.110.100:9777",
            Transport::Tcp,
            "Larry",
            "838383fhfhf83hf8h3f8h",
        ),
        _ => (
            "10.0.0.1:5060",
            "10.0.0.2:5060",
            Transport::Udp,
            "TestUser",
            "TestSession",
        ),
    };

    let common = SecurityEventCommon {
        event_type,
        service: "TEST".to_string(),
        module: "test_security_events".to_string(),
        account_id: account.to_string(),
        session_id: session.to_string(),
        local_addr: local.parse().unwrap(),
        local_transport: transport,
        remote_addr: remote.parse().unwrap(),
        remote_transport: transport,
    };

    let mut event = SecurityEvent::new(common);

    // Add type-specific extra fields.
    match event_type {
        SecurityEventType::FailedAcl => {
            event = event.with_extra("acl_name", "TEST_ACL");
        }
        SecurityEventType::ReqNoSupport => {
            event = event.with_extra("request_type", "MakeMeDinner");
        }
        SecurityEventType::ReqNotAllowed => {
            event = event.with_extra("request_type", "MakeMeBreakfast");
            event = event.with_extra("request_params", "BACONNNN!");
        }
        SecurityEventType::AuthMethodNotAllowed => {
            event = event.with_extra("auth_method", "PlainText");
        }
        SecurityEventType::ReqBadFormat => {
            event = event.with_extra("request_type", "CheeseBurger");
            event = event.with_extra("request_params", "Onions,Swiss,MotorOil");
        }
        _ => {}
    }

    event
}

// ---------------------------------------------------------------------------
// Tests: Event type enumeration
// ---------------------------------------------------------------------------

/// Verify all security event types are defined.
#[test]
fn test_security_event_type_count() {
    assert_eq!(SecurityEventType::all().len(), 15);
}

/// Verify event types are distinct.
#[test]
fn test_security_event_types_distinct() {
    let types = SecurityEventType::all();
    for (i, t1) in types.iter().enumerate() {
        for (j, t2) in types.iter().enumerate() {
            if i != j {
                assert_ne!(t1, t2);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: Event creation and validation
// ---------------------------------------------------------------------------

/// Port of the security event generation tests from test_security_events.c.
/// Each event type should be creatable and valid.
#[test]
fn test_generate_failed_acl() {
    let event = generate_event(SecurityEventType::FailedAcl);
    assert!(event.is_valid());
    assert_eq!(event.common.event_type, SecurityEventType::FailedAcl);
    assert_eq!(event.common.account_id, "Username");
    assert_eq!(event.extra.get("acl_name"), Some(&"TEST_ACL".to_string()));
}

#[test]
fn test_generate_inval_acct_id() {
    let event = generate_event(SecurityEventType::InvalAcctId);
    assert!(event.is_valid());
    assert_eq!(event.common.event_type, SecurityEventType::InvalAcctId);
    assert_eq!(event.common.account_id, "FakeUser");
    assert_eq!(event.common.local_transport, Transport::Tcp);
}

#[test]
fn test_generate_session_limit() {
    let event = generate_event(SecurityEventType::SessionLimit);
    assert!(event.is_valid());
    assert_eq!(event.common.session_id, "8675309");
    assert_eq!(event.common.local_transport, Transport::Tls);
}

#[test]
fn test_generate_mem_limit() {
    let event = generate_event(SecurityEventType::MemLimit);
    assert!(event.is_valid());
    assert_eq!(event.common.account_id, "Felix");
}

#[test]
fn test_generate_load_avg() {
    let event = generate_event(SecurityEventType::LoadAvg);
    assert!(event.is_valid());
    assert_eq!(event.common.account_id, "GuestAccount");
}

#[test]
fn test_generate_req_no_support() {
    let event = generate_event(SecurityEventType::ReqNoSupport);
    assert!(event.is_valid());
    assert_eq!(
        event.extra.get("request_type"),
        Some(&"MakeMeDinner".to_string())
    );
}

#[test]
fn test_generate_req_not_allowed() {
    let event = generate_event(SecurityEventType::ReqNotAllowed);
    assert!(event.is_valid());
    assert_eq!(
        event.extra.get("request_type"),
        Some(&"MakeMeBreakfast".to_string())
    );
    assert_eq!(
        event.extra.get("request_params"),
        Some(&"BACONNNN!".to_string())
    );
}

#[test]
fn test_generate_auth_method_not_allowed() {
    let event = generate_event(SecurityEventType::AuthMethodNotAllowed);
    assert!(event.is_valid());
    assert_eq!(
        event.extra.get("auth_method"),
        Some(&"PlainText".to_string())
    );
}

#[test]
fn test_generate_req_bad_format() {
    let event = generate_event(SecurityEventType::ReqBadFormat);
    assert!(event.is_valid());
    assert_eq!(
        event.extra.get("request_type"),
        Some(&"CheeseBurger".to_string())
    );
}

/// Generate all event types and verify none panic.
#[test]
fn test_generate_all_event_types() {
    for event_type in SecurityEventType::all() {
        let event = generate_event(*event_type);
        assert!(event.is_valid(), "Event {:?} is not valid", event_type);
        assert_eq!(event.common.event_type, *event_type);
        assert_eq!(event.common.service, "TEST");
    }
}

// ---------------------------------------------------------------------------
// Tests: Address field validation
// ---------------------------------------------------------------------------

#[test]
fn test_security_event_addresses() {
    let event = generate_event(SecurityEventType::FailedAcl);
    assert_eq!(
        event.common.local_addr,
        "192.168.1.1:12121".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        event.common.remote_addr,
        "192.168.1.2:12345".parse::<SocketAddr>().unwrap()
    );
}

#[test]
fn test_security_event_transport() {
    let event = generate_event(SecurityEventType::FailedAcl);
    assert_eq!(event.common.local_transport, Transport::Udp);
    assert_eq!(event.common.remote_transport, Transport::Udp);

    let event2 = generate_event(SecurityEventType::InvalAcctId);
    assert_eq!(event2.common.local_transport, Transport::Tcp);

    let event3 = generate_event(SecurityEventType::SessionLimit);
    assert_eq!(event3.common.local_transport, Transport::Tls);
}
