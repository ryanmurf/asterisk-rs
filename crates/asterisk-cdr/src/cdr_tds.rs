//! FreeTDS/MSSQL CDR backend (stub).
//!
//! Port of cdr/cdr_tds.c from Asterisk C.
//!
//! Writes CDR records to a Microsoft SQL Server database via the FreeTDS
//! library (Tabular Data Stream protocol).

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// Configuration for the FreeTDS CDR backend.
#[derive(Debug, Clone)]
pub struct TdsCdrConfig {
    /// MSSQL server hostname
    pub hostname: String,
    /// Server port
    pub port: u16,
    /// Database name
    pub dbname: String,
    /// Database user
    pub user: String,
    /// Database password
    pub password: String,
    /// Table name
    pub table: String,
    /// Character set
    pub charset: String,
    /// TDS protocol version
    pub tds_version: String,
}

impl Default for TdsCdrConfig {
    fn default() -> Self {
        Self {
            hostname: "localhost".to_string(),
            port: 1433,
            dbname: "asterisk".to_string(),
            user: "sa".to_string(),
            password: String::new(),
            table: "cdr".to_string(),
            charset: "UTF-8".to_string(),
            tds_version: "7.2".to_string(),
        }
    }
}

/// FreeTDS/MSSQL CDR backend.
pub struct TdsCdrBackend {
    config: TdsCdrConfig,
    last_sql: parking_lot::Mutex<Option<String>>,
}

impl TdsCdrBackend {
    pub fn new() -> Self {
        Self {
            config: TdsCdrConfig::default(),
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    pub fn with_config(config: TdsCdrConfig) -> Self {
        Self {
            config,
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    /// Build T-SQL INSERT statement.
    pub fn build_insert_sql(&self, cdr: &Cdr) -> String {
        format!(
            "INSERT INTO {} (accountcode,src,dst,dcontext,clid,channel,dstchannel,lastapp,lastdata,duration,billsec,disposition,amaflags,uniqueid,userfield) VALUES (N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',N'{}',{},{},N'{}',N'{}',N'{}',N'{}')",
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

impl Default for TdsCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for TdsCdrBackend {
    fn name(&self) -> &str {
        "tds"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let sql = self.build_insert_sql(cdr);
        debug!("CDR TDS [{}]: {}", self.config.dbname, sql);
        *self.last_sql.lock() = Some(sql);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_insert_sql() {
        let backend = TdsCdrBackend::new();
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid".to_string());
        cdr.src = "5551234".to_string();
        let sql = backend.build_insert_sql(&cdr);
        assert!(sql.contains("INSERT INTO cdr"));
        assert!(sql.contains("N'5551234'")); // NVARCHAR literals
    }

    #[test]
    fn test_tds_log() {
        let backend = TdsCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        backend.log(&cdr).unwrap();
        assert!(backend.last_sql().is_some());
    }

    #[test]
    fn test_default_config() {
        let config = TdsCdrConfig::default();
        assert_eq!(config.port, 1433);
        assert_eq!(config.tds_version, "7.2");
    }
}
