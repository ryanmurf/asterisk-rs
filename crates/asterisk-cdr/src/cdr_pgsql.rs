//! PostgreSQL CDR backend (stub SQL interface).
//!
//! Port of cdr/cdr_pgsql.c from Asterisk C.
//!
//! Writes CDR records to a PostgreSQL database using parameterized queries.

use crate::{Cdr, CdrBackend, CdrError};
use tracing::debug;

/// Configuration for the PostgreSQL CDR backend.
#[derive(Debug, Clone)]
pub struct PgsqlCdrConfig {
    /// PostgreSQL host
    pub host: String,
    /// PostgreSQL port
    pub port: u16,
    /// Database name
    pub dbname: String,
    /// Database user
    pub user: String,
    /// Database password
    pub password: String,
    /// Table name
    pub table: String,
    /// Connection encoding
    pub encoding: String,
    /// Connection timezone
    pub timezone: String,
}

impl Default for PgsqlCdrConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5432,
            dbname: "asteriskcdrdb".to_string(),
            user: "asterisk".to_string(),
            password: String::new(),
            table: "cdr".to_string(),
            encoding: "UTF8".to_string(),
            timezone: "UTC".to_string(),
        }
    }
}

/// PostgreSQL CDR backend.
pub struct PgsqlCdrBackend {
    config: PgsqlCdrConfig,
    last_sql: parking_lot::Mutex<Option<String>>,
}

impl PgsqlCdrBackend {
    pub fn new() -> Self {
        Self {
            config: PgsqlCdrConfig::default(),
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    pub fn with_config(config: PgsqlCdrConfig) -> Self {
        Self {
            config,
            last_sql: parking_lot::Mutex::new(None),
        }
    }

    /// Build the PostgreSQL connection string.
    pub fn connection_string(&self) -> String {
        format!(
            "host={} port={} dbname={} user={} password={}",
            self.config.host, self.config.port, self.config.dbname,
            self.config.user, self.config.password,
        )
    }

    /// Generate parameterized INSERT.
    pub fn build_insert_sql(&self, cdr: &Cdr) -> String {
        format!(
            "INSERT INTO {} (accountcode,src,dst,dcontext,clid,channel,dstchannel,lastapp,lastdata,duration,billsec,disposition,amaflags,uniqueid,userfield,linkedid,peeraccount,sequence) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
            self.config.table,
        )
    }

    /// Get parameter values for a CDR.
    pub fn cdr_params(cdr: &Cdr) -> Vec<String> {
        vec![
            cdr.account_code.clone(),
            cdr.src.clone(),
            cdr.dst.clone(),
            cdr.dst_context.clone(),
            cdr.caller_id.clone(),
            cdr.channel.clone(),
            cdr.dst_channel.clone(),
            cdr.last_app.clone(),
            cdr.last_data.clone(),
            cdr.duration.to_string(),
            cdr.billsec.to_string(),
            cdr.disposition.as_str().to_string(),
            cdr.ama_flags.as_str().to_string(),
            cdr.unique_id.clone(),
            cdr.user_field.clone(),
            cdr.linked_id.clone(),
            cdr.peer_account.clone(),
            cdr.sequence.to_string(),
        ]
    }

    pub fn last_sql(&self) -> Option<String> {
        self.last_sql.lock().clone()
    }
}

impl Default for PgsqlCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for PgsqlCdrBackend {
    fn name(&self) -> &str {
        "pgsql"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let sql = self.build_insert_sql(cdr);
        debug!("CDR PgSQL [{}]: {}", self.config.dbname, sql);
        *self.last_sql.lock() = Some(sql);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_string() {
        let backend = PgsqlCdrBackend::new();
        let cs = backend.connection_string();
        assert!(cs.contains("host=localhost"));
        assert!(cs.contains("port=5432"));
    }

    #[test]
    fn test_build_insert() {
        let backend = PgsqlCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        let sql = backend.build_insert_sql(&cdr);
        assert!(sql.contains("INSERT INTO cdr"));
        assert!(sql.contains("$1"));
    }

    #[test]
    fn test_cdr_params() {
        let mut cdr = Cdr::new("SIP/alice".to_string(), "uid".to_string());
        cdr.src = "5551234".to_string();
        let params = PgsqlCdrBackend::cdr_params(&cdr);
        assert_eq!(params.len(), 18);
        assert_eq!(params[1], "5551234"); // src
    }

    #[test]
    fn test_pgsql_log() {
        let backend = PgsqlCdrBackend::new();
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        backend.log(&cdr).unwrap();
        assert!(backend.last_sql().is_some());
    }
}
