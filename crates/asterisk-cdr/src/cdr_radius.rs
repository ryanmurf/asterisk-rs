//! RADIUS accounting CDR backend (stub RADIUS client).
//!
//! Port of cdr/cdr_radius.c from Asterisk C.
//!
//! Sends CDR records as RADIUS Accounting-Request messages to a
//! RADIUS server. Uses standard RADIUS AVPs plus Asterisk vendor-specific
//! attributes.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// RADIUS AVP (Attribute-Value Pair) types used for CDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RadiusAvpType {
    /// User-Name (RFC 2865)
    UserName = 1,
    /// Acct-Status-Type (RFC 2866)
    AcctStatusType = 40,
    /// Acct-Session-Id (RFC 2866)
    AcctSessionId = 44,
    /// Acct-Session-Time (RFC 2866)
    AcctSessionTime = 46,
    /// Calling-Station-Id (RFC 2865)
    CallingStationId = 31,
    /// Called-Station-Id (RFC 2865)
    CalledStationId = 30,
    /// Acct-Terminate-Cause (RFC 2866)
    AcctTerminateCause = 49,
    /// NAS-IP-Address (RFC 2865)
    NasIpAddress = 4,
    /// NAS-Port (RFC 2865)
    NasPort = 5,
}

/// RADIUS accounting status types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AcctStatusType {
    Start = 1,
    Stop = 2,
    InterimUpdate = 3,
}

/// A single RADIUS AVP.
#[derive(Debug, Clone)]
pub struct RadiusAvp {
    pub avp_type: u8,
    pub value: RadiusAvpValue,
}

/// AVP value types.
#[derive(Debug, Clone)]
pub enum RadiusAvpValue {
    String(String),
    Integer(u32),
    IpAddr([u8; 4]),
}

impl RadiusAvp {
    pub fn string(avp_type: u8, value: &str) -> Self {
        Self {
            avp_type,
            value: RadiusAvpValue::String(value.to_string()),
        }
    }

    pub fn integer(avp_type: u8, value: u32) -> Self {
        Self {
            avp_type,
            value: RadiusAvpValue::Integer(value),
        }
    }
}

/// Asterisk vendor-specific RADIUS attributes (vendor ID 22736).
pub mod vendor_avp {
    pub const ASTERISK_VENDOR_ID: u32 = 22736;
    pub const AST_ACCT_CODE: u8 = 1;
    pub const AST_SRC: u8 = 2;
    pub const AST_DST: u8 = 3;
    pub const AST_DST_CTX: u8 = 4;
    pub const AST_CLID: u8 = 5;
    pub const AST_CHAN: u8 = 6;
    pub const AST_DST_CHAN: u8 = 7;
    pub const AST_LAST_APP: u8 = 8;
    pub const AST_LAST_DATA: u8 = 9;
    pub const AST_DURATION: u8 = 10;
    pub const AST_BILLSEC: u8 = 11;
    pub const AST_DISPOSITION: u8 = 12;
    pub const AST_AMA_FLAGS: u8 = 13;
    pub const AST_UNIQUE_ID: u8 = 14;
    pub const AST_USER_FIELD: u8 = 15;
}

/// Configuration for the RADIUS CDR backend.
#[derive(Debug, Clone)]
pub struct RadiusCdrConfig {
    /// RADIUS server address
    pub server: String,
    /// RADIUS server port (accounting)
    pub port: u16,
    /// Shared secret
    pub secret: String,
    /// Timeout in seconds
    pub timeout: u32,
    /// Retries
    pub retries: u32,
}

impl Default for RadiusCdrConfig {
    fn default() -> Self {
        Self {
            server: "127.0.0.1".to_string(),
            port: 1813,
            secret: String::new(),
            timeout: 3,
            retries: 3,
        }
    }
}

/// RADIUS CDR backend.
pub struct RadiusCdrBackend {
    config: RadiusCdrConfig,
    last_avps: parking_lot::Mutex<Option<Vec<RadiusAvp>>>,
}

impl RadiusCdrBackend {
    pub fn new() -> Self {
        Self {
            config: RadiusCdrConfig::default(),
            last_avps: parking_lot::Mutex::new(None),
        }
    }

    pub fn with_config(config: RadiusCdrConfig) -> Self {
        Self {
            config,
            last_avps: parking_lot::Mutex::new(None),
        }
    }

    /// Build RADIUS AVPs from a CDR record.
    pub fn build_avps(cdr: &Cdr) -> Vec<RadiusAvp> {
        vec![
            RadiusAvp::integer(RadiusAvpType::AcctStatusType as u8, AcctStatusType::Stop as u32),
            RadiusAvp::string(RadiusAvpType::AcctSessionId as u8, &cdr.unique_id),
            RadiusAvp::integer(RadiusAvpType::AcctSessionTime as u8, cdr.duration as u32),
            RadiusAvp::string(RadiusAvpType::CallingStationId as u8, &cdr.src),
            RadiusAvp::string(RadiusAvpType::CalledStationId as u8, &cdr.dst),
            RadiusAvp::string(RadiusAvpType::UserName as u8, &cdr.account_code),
        ]
    }

    pub fn last_avps(&self) -> Option<Vec<RadiusAvp>> {
        self.last_avps.lock().clone()
    }
}

impl Default for RadiusCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for RadiusCdrBackend {
    fn name(&self) -> &str {
        "radius"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let avps = Self::build_avps(cdr);
        debug!(
            "CDR RADIUS: sending {} AVPs to {}:{}",
            avps.len(),
            self.config.server,
            self.config.port,
        );
        *self.last_avps.lock() = Some(avps);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_avps() {
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid-1".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.duration = 60;
        let avps = RadiusCdrBackend::build_avps(&cdr);
        assert!(!avps.is_empty());
        assert_eq!(avps[0].avp_type, RadiusAvpType::AcctStatusType as u8);
    }

    #[test]
    fn test_radius_log() {
        let backend = RadiusCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        backend.log(&cdr).unwrap();
        assert!(backend.last_avps().is_some());
    }

    #[test]
    fn test_default_config() {
        let config = RadiusCdrConfig::default();
        assert_eq!(config.port, 1813);
    }
}
