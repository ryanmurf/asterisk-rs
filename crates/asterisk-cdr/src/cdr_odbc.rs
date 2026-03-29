//! ODBC CDR backend (stub SQL interface).
//!
//! Port of cdr/cdr_odbc.c from Asterisk C.
//!
//! Writes CDR records to any ODBC-compatible database. The actual ODBC
//! driver calls are stubbed with a trait interface.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// Configuration for the ODBC CDR backend.
#[derive(Debug, Clone)]
pub struct OdbcCdrConfig {
    /// ODBC DSN (Data Source Name)
    pub dsn: String,
    /// Database table name
    pub table: String,
    /// Username (if not in DSN)
    pub username: String,
    /// Password (if not in DSN)
    pub password: String,
    /// Whether to log unanswered calls
    pub log_unanswered: bool,
    /// Whether to log high-resolution time
    pub high_resolution_time: bool,
}

impl Default for OdbcCdrConfig {
    fn default() -> Self {
        Self {
            dsn: "asterisk-cdr".to_string(),
            table: "cdr".to_string(),
            username: String::new(),
            password: String::new(),
            log_unanswered: true,
            high_resolution_time: false,
        }
    }
}

/// ODBC CDR backend.
///
/// Stub implementation that generates SQL INSERT statements for CDR records.
pub struct OdbcCdrBackend {
    config: OdbcCdrConfig,
    last_sql: parking_lot::Mutex<Option<String>>,
}

impl OdbcCdrBackend {
    pub fn new() -> Self {
        Self {
            config: OdbcCdrConfig::default(),
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    pub fn with_config(config: OdbcCdrConfig) -> Self {
        Self {
            config,
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    /// Generate the INSERT SQL for a CDR record.
    pub fn build_insert_sql(&self, cdr: &Cdr) -> String {
        format!(
            "INSERT INTO {} (accountcode,src,dst,dcontext,clid,channel,dstchannel,lastapp,lastdata,duration,billsec,disposition,amaflags,uniqueid,userfield) VALUES ('{}','{}','{}','{}','{}','{}','{}','{}','{}',{},{},'{}','{}','{}','{}')",
            self.config.table,
            cdr.account_code, cdr.src, cdr.dst, cdr.dst_context,
            cdr.caller_id, cdr.channel, cdr.dst_channel,
            cdr.last_app, cdr.last_data, cdr.duration, cdr.billsec,
            cdr.disposition.as_str(), cdr.ama_flags.as_str(),
            cdr.unique_id, cdr.user_field,
        )
    }

    pub fn last_sql(&self) -> Option<String> {
        self.last_sql.lock().clone()
    }
}

impl Default for OdbcCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for OdbcCdrBackend {
    fn name(&self) -> &str {
        "odbc"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        if !self.config.log_unanswered && cdr.disposition == crate::CdrDisposition::NoAnswer {
            return Ok(());
        }
        let sql = self.build_insert_sql(cdr);
        debug!("CDR ODBC [{}]: {}", self.config.dsn, sql);
        *self.last_sql.lock() = Some(sql);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_insert_sql() {
        let backend = OdbcCdrBackend::new();
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid-1".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.duration = 60;
        let sql = backend.build_insert_sql(&cdr);
        assert!(sql.starts_with("INSERT INTO cdr"));
        assert!(sql.contains("5551234"));
    }

    #[test]
    fn test_odbc_log() {
        let backend = OdbcCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        backend.log(&cdr).unwrap();
        assert!(backend.last_sql().is_some());
    }
}
