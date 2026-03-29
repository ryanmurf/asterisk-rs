//! SNMP agent.
//!
//! Port of `res/res_snmp.c`. Implements an SNMP sub-agent providing
//! ASTERISK-MIB objects (version, uptime, channel counts, etc.) via an
//! OID tree that can be walked and queried.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SnmpError {
    #[error("OID not found: {0}")]
    OidNotFound(String),
    #[error("SNMP error: {0}")]
    Other(String),
}

pub type SnmpResult<T> = Result<T, SnmpError>;

// ---------------------------------------------------------------------------
// OID representation
// ---------------------------------------------------------------------------

/// An SNMP Object Identifier, stored as a vector of integer components.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(pub Vec<u32>);

impl Oid {
    /// Parse an OID from dotted notation (e.g., "1.3.6.1.4.1.22736").
    pub fn parse(s: &str) -> Option<Self> {
        let components: Result<Vec<u32>, _> = s
            .split('.')
            .filter(|p| !p.is_empty())
            .map(|p| p.parse())
            .collect();
        components.ok().map(Oid)
    }

    /// Convert to dotted string notation.
    pub fn to_dotted(&self) -> String {
        self.0
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Check whether `self` is a prefix of `other`.
    pub fn is_prefix_of(&self, other: &Oid) -> bool {
        other.0.starts_with(&self.0)
    }

    /// Return the number of components.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the OID is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_dotted())
    }
}

// ---------------------------------------------------------------------------
// MIB value types
// ---------------------------------------------------------------------------

/// SNMP value types returned by MIB nodes.
#[derive(Debug, Clone, PartialEq)]
pub enum MibValueType {
    Integer,
    OctetString,
    Counter32,
    Counter64,
    Gauge32,
    TimeTicks,
    ObjectIdentifier,
}

/// An SNMP value.
#[derive(Debug, Clone)]
pub enum MibValue {
    Integer(i64),
    OctetString(String),
    Counter32(u32),
    Counter64(u64),
    Gauge32(u32),
    TimeTicks(u32),
    ObjectIdentifier(Oid),
}

impl fmt::Display for MibValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(v) => write!(f, "{}", v),
            Self::OctetString(v) => write!(f, "{}", v),
            Self::Counter32(v) => write!(f, "{}", v),
            Self::Counter64(v) => write!(f, "{}", v),
            Self::Gauge32(v) => write!(f, "{}", v),
            Self::TimeTicks(v) => write!(f, "{}", v),
            Self::ObjectIdentifier(v) => write!(f, "{}", v),
        }
    }
}

// ---------------------------------------------------------------------------
// MIB node
// ---------------------------------------------------------------------------

/// A node in the MIB tree.
pub struct MibNode {
    /// The OID for this node.
    pub oid: Oid,
    /// Human-readable name.
    pub name: String,
    /// Value type.
    pub value_type: MibValueType,
    /// Handler that returns the current value for this OID.
    pub handler: Box<dyn Fn() -> MibValue + Send + Sync>,
}

impl fmt::Debug for MibNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MibNode")
            .field("oid", &self.oid)
            .field("name", &self.name)
            .field("value_type", &self.value_type)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ASTERISK-MIB constants
// ---------------------------------------------------------------------------

/// Asterisk enterprise OID: 1.3.6.1.4.1.22736
pub const ASTERISK_ENTERPRISE_OID: &str = "1.3.6.1.4.1.22736";

/// Well-known sub-OIDs under the Asterisk enterprise.
pub mod asterisk_oids {
    pub const VERSION: &str = "1.3.6.1.4.1.22736.1.1.0";
    pub const UPTIME: &str = "1.3.6.1.4.1.22736.1.2.0";
    pub const RELOAD_COUNT: &str = "1.3.6.1.4.1.22736.1.3.0";
    pub const ACTIVE_CHANNELS: &str = "1.3.6.1.4.1.22736.1.4.0";
    pub const ACTIVE_CALLS: &str = "1.3.6.1.4.1.22736.1.5.0";
    pub const PROCESSED_CALLS: &str = "1.3.6.1.4.1.22736.1.6.0";
}

// ---------------------------------------------------------------------------
// SNMP Agent
// ---------------------------------------------------------------------------

/// An SNMP agent that serves ASTERISK-MIB data.
///
/// Uses a `BTreeMap` keyed by OID for efficient ordered walks.
pub struct SnmpAgent {
    /// MIB tree keyed by OID for lexicographic ordering (GET-NEXT support).
    mib_tree: RwLock<BTreeMap<Oid, Arc<MibNode>>>,
}

impl SnmpAgent {
    /// Create a new empty SNMP agent.
    pub fn new() -> Self {
        Self {
            mib_tree: RwLock::new(BTreeMap::new()),
        }
    }

    /// Create an agent pre-populated with the standard ASTERISK-MIB objects.
    pub fn with_asterisk_mib(
        version: String,
        start_time: Instant,
    ) -> Self {
        let agent = Self::new();

        // version
        let ver = version.clone();
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::VERSION).unwrap(),
            name: "astVersionString".into(),
            value_type: MibValueType::OctetString,
            handler: Box::new(move || MibValue::OctetString(ver.clone())),
        });

        // uptime (in hundredths of a second / TimeTicks)
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::UPTIME).unwrap(),
            name: "astConfigUpTime".into(),
            value_type: MibValueType::TimeTicks,
            handler: Box::new(move || {
                let elapsed = start_time.elapsed();
                MibValue::TimeTicks((elapsed.as_millis() / 10) as u32)
            }),
        });

        // reload count (starts at 0)
        let reload_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rc = Arc::clone(&reload_count);
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::RELOAD_COUNT).unwrap(),
            name: "astConfigReloadCount".into(),
            value_type: MibValueType::Counter32,
            handler: Box::new(move || {
                MibValue::Counter32(rc.load(std::sync::atomic::Ordering::Relaxed) as u32)
            }),
        });

        // active channels (placeholder returning 0)
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::ACTIVE_CHANNELS).unwrap(),
            name: "astNumChannels".into(),
            value_type: MibValueType::Gauge32,
            handler: Box::new(|| MibValue::Gauge32(0)),
        });

        // active calls
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::ACTIVE_CALLS).unwrap(),
            name: "astNumCalls".into(),
            value_type: MibValueType::Gauge32,
            handler: Box::new(|| MibValue::Gauge32(0)),
        });

        // processed calls
        agent.register(MibNode {
            oid: Oid::parse(asterisk_oids::PROCESSED_CALLS).unwrap(),
            name: "astNumCallsProcessed".into(),
            value_type: MibValueType::Counter64,
            handler: Box::new(|| MibValue::Counter64(0)),
        });

        agent
    }

    /// Register a MIB node.
    pub fn register(&self, node: MibNode) {
        debug!(oid = %node.oid, name = %node.name, "SNMP MIB node registered");
        self.mib_tree.write().insert(node.oid.clone(), Arc::new(node));
    }

    /// GET: retrieve the value of an exact OID.
    pub fn get(&self, oid: &Oid) -> SnmpResult<MibValue> {
        let tree = self.mib_tree.read();
        let node = tree
            .get(oid)
            .ok_or_else(|| SnmpError::OidNotFound(oid.to_dotted()))?;
        Ok((node.handler)())
    }

    /// GET by string OID.
    pub fn get_by_str(&self, oid_str: &str) -> SnmpResult<MibValue> {
        let oid = Oid::parse(oid_str)
            .ok_or_else(|| SnmpError::OidNotFound(oid_str.to_string()))?;
        self.get(&oid)
    }

    /// GET-NEXT: return the next OID and its value after the given OID.
    pub fn get_next(&self, oid: &Oid) -> SnmpResult<(Oid, MibValue)> {
        let tree = self.mib_tree.read();
        // Find the first OID strictly greater than the given one.
        let mut iter = tree.range::<Oid, _>((std::ops::Bound::Excluded(oid), std::ops::Bound::Unbounded));
        if let Some((next_oid, node)) = iter.next() {
            Ok((next_oid.clone(), (node.handler)()))
        } else {
            Err(SnmpError::OidNotFound(format!("no OID after {}", oid)))
        }
    }

    /// Walk: return all OID/value pairs under a given prefix OID.
    pub fn walk(&self, prefix: &Oid) -> Vec<(Oid, MibValue)> {
        let tree = self.mib_tree.read();
        tree.range::<Oid, _>(prefix..)
            .take_while(|(oid, _)| prefix.is_prefix_of(oid))
            .map(|(oid, node)| (oid.clone(), (node.handler)()))
            .collect()
    }

    /// List all registered OIDs.
    pub fn oid_list(&self) -> Vec<Oid> {
        self.mib_tree.read().keys().cloned().collect()
    }

    /// Number of registered MIB nodes.
    pub fn node_count(&self) -> usize {
        self.mib_tree.read().len()
    }
}

impl Default for SnmpAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SnmpAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnmpAgent")
            .field("nodes", &self.mib_tree.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oid_parse() {
        let oid = Oid::parse("1.3.6.1.4.1.22736").unwrap();
        assert_eq!(oid.0, vec![1, 3, 6, 1, 4, 1, 22736]);
        assert_eq!(oid.to_dotted(), "1.3.6.1.4.1.22736");
    }

    #[test]
    fn test_oid_prefix() {
        let parent = Oid::parse("1.3.6.1").unwrap();
        let child = Oid::parse("1.3.6.1.4.1").unwrap();
        let other = Oid::parse("1.3.7.1").unwrap();
        assert!(parent.is_prefix_of(&child));
        assert!(!parent.is_prefix_of(&other));
    }

    #[test]
    fn test_agent_register_and_get() {
        let agent = SnmpAgent::new();
        agent.register(MibNode {
            oid: Oid::parse("1.3.6.1.4.1.22736.1.1.0").unwrap(),
            name: "test".into(),
            value_type: MibValueType::OctetString,
            handler: Box::new(|| MibValue::OctetString("hello".into())),
        });

        match agent.get_by_str("1.3.6.1.4.1.22736.1.1.0").unwrap() {
            MibValue::OctetString(v) => assert_eq!(v, "hello"),
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn test_agent_get_not_found() {
        let agent = SnmpAgent::new();
        assert!(agent.get_by_str("1.2.3").is_err());
    }

    #[test]
    fn test_agent_get_next() {
        let agent = SnmpAgent::new();
        agent.register(MibNode {
            oid: Oid::parse("1.1.0").unwrap(),
            name: "first".into(),
            value_type: MibValueType::Integer,
            handler: Box::new(|| MibValue::Integer(1)),
        });
        agent.register(MibNode {
            oid: Oid::parse("1.2.0").unwrap(),
            name: "second".into(),
            value_type: MibValueType::Integer,
            handler: Box::new(|| MibValue::Integer(2)),
        });

        let (next_oid, val) = agent.get_next(&Oid::parse("1.1.0").unwrap()).unwrap();
        assert_eq!(next_oid, Oid::parse("1.2.0").unwrap());
        match val {
            MibValue::Integer(v) => assert_eq!(v, 2),
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn test_asterisk_mib_agent() {
        let agent = SnmpAgent::with_asterisk_mib("21.0.0".to_string(), Instant::now());
        assert_eq!(agent.node_count(), 6);

        match agent.get_by_str(asterisk_oids::VERSION).unwrap() {
            MibValue::OctetString(v) => assert_eq!(v, "21.0.0"),
            _ => panic!("wrong type for version"),
        }

        // Uptime should be a small TimeTicks value since we just started.
        match agent.get_by_str(asterisk_oids::UPTIME).unwrap() {
            MibValue::TimeTicks(_) => {}
            _ => panic!("wrong type for uptime"),
        }
    }

    #[test]
    fn test_walk() {
        let agent = SnmpAgent::with_asterisk_mib("1.0".to_string(), Instant::now());
        let prefix = Oid::parse("1.3.6.1.4.1.22736.1").unwrap();
        let results = agent.walk(&prefix);
        assert_eq!(results.len(), 6);
    }
}
