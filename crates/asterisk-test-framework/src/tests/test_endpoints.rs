//! Port of asterisk/tests/test_endpoints.c
//!
//! Tests endpoint creation, default snapshot values, and setters:
//! - Creating endpoints with valid/invalid parameters
//! - Default state (UNKNOWN), max_channels (-1), num_channels (0)
//! - Setting state and max_channels

use std::fmt;

// ---------------------------------------------------------------------------
// Endpoint state enum mirroring AST_ENDPOINT_*
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointState {
    Unknown,
    Offline,
    Online,
}

impl fmt::Display for EndpointState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointState::Unknown => write!(f, "unknown"),
            EndpointState::Offline => write!(f, "offline"),
            EndpointState::Online => write!(f, "online"),
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EndpointSnapshot {
    id: String,
    tech: String,
    resource: String,
    state: EndpointState,
    max_channels: i32,
    num_channels: usize,
}

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

struct Endpoint {
    tech: String,
    resource: String,
    state: EndpointState,
    max_channels: i32,
    channels: Vec<String>,
}

impl Endpoint {
    /// Create a new endpoint. Returns None if tech or resource is empty.
    fn create(tech: &str, resource: &str) -> Option<Self> {
        if tech.is_empty() || resource.is_empty() {
            return None;
        }
        Some(Self {
            tech: tech.to_string(),
            resource: resource.to_string(),
            state: EndpointState::Unknown,
            max_channels: -1,
            channels: Vec::new(),
        })
    }

    fn tech(&self) -> &str {
        &self.tech
    }

    fn resource(&self) -> &str {
        &self.resource
    }

    fn id(&self) -> String {
        format!("{}/{}", self.tech, self.resource)
    }

    fn set_state(&mut self, state: EndpointState) {
        self.state = state;
    }

    fn set_max_channels(&mut self, max: i32) {
        self.max_channels = max;
    }

    fn snapshot(&self) -> EndpointSnapshot {
        EndpointSnapshot {
            id: self.id(),
            tech: self.tech.clone(),
            resource: self.resource.clone(),
            state: self.state,
            max_channels: self.max_channels,
            num_channels: self.channels.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: creation
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(create) from test_endpoints.c.
///
/// Verifies that invalid parameter combinations return None and
/// a valid (tech, resource) pair creates a working endpoint.
#[test]
fn test_endpoint_create_invalid() {
    assert!(Endpoint::create("", "").is_none());
    assert!(Endpoint::create("TEST", "").is_none());
    assert!(Endpoint::create("", "test_res").is_none());
}

#[test]
fn test_endpoint_create_valid() {
    let ep = Endpoint::create("TEST", "test_res").unwrap();
    assert_eq!(ep.tech(), "TEST");
    assert_eq!(ep.resource(), "test_res");
    assert_eq!(ep.id(), "TEST/test_res");
}

// ---------------------------------------------------------------------------
// Tests: defaults
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(defaults) from test_endpoints.c.
///
/// Verify that a newly created endpoint snapshot has the expected defaults.
#[test]
fn test_endpoint_defaults() {
    let ep = Endpoint::create("TEST", "test_res").unwrap();
    let snap = ep.snapshot();

    assert_eq!(snap.id, "TEST/test_res");
    assert_eq!(snap.tech, "TEST");
    assert_eq!(snap.resource, "test_res");
    assert_eq!(snap.state, EndpointState::Unknown);
    assert_eq!(snap.max_channels, -1);
    assert_eq!(snap.num_channels, 0);
}

// ---------------------------------------------------------------------------
// Tests: setters
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(setters) from test_endpoints.c.
///
/// Verify that state and max_channels setters are reflected in the snapshot.
#[test]
fn test_endpoint_setters() {
    let mut ep = Endpoint::create("TEST", "test_res").unwrap();

    ep.set_state(EndpointState::Online);
    ep.set_max_channels(314159);

    let snap = ep.snapshot();
    assert_eq!(snap.state, EndpointState::Online);
    assert_eq!(snap.max_channels, 314159);
}

/// Test setting state to offline.
#[test]
fn test_endpoint_set_offline() {
    let mut ep = Endpoint::create("TEST", "test_res").unwrap();
    ep.set_state(EndpointState::Offline);
    assert_eq!(ep.snapshot().state, EndpointState::Offline);
}
