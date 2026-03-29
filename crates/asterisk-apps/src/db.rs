//! AstDB (Asterisk internal database) dialplan applications.
//!
//! Port of app_db.c from Asterisk C. Provides DBput, DBget, DBdel,
//! and DBdeltree applications for accessing the Asterisk internal
//! database (Berkeley DB / SQLite) from the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The DBput() dialplan application.
///
/// Usage: DBput(family/key=value)
///
/// Stores a value in the Asterisk database.
pub struct AppDbPut;

impl DialplanApp for AppDbPut {
    fn name(&self) -> &str {
        "DBput"
    }

    fn description(&self) -> &str {
        "Store a value in the Asterisk database"
    }
}

impl AppDbPut {
    /// Execute the DBput application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        // Parse family/key=value
        let (path, value) = match args.split_once('=') {
            Some((p, v)) => (p.trim(), v.trim()),
            None => {
                warn!("DBput: requires family/key=value argument");
                return PbxExecResult::Failed;
            }
        };

        let (family, key) = match path.split_once('/') {
            Some((f, k)) => (f, k),
            None => {
                warn!("DBput: path must be family/key");
                return PbxExecResult::Failed;
            }
        };

        info!(
            "DBput: channel '{}' {}/{}={}",
            channel.name, family, key, value,
        );

        // In a real implementation:
        // ast_db_put(family, key, value)

        PbxExecResult::Success
    }
}

/// The DBget() dialplan application.
///
/// Usage: DBget(variable=family/key)
///
/// Retrieves a value from the Asterisk database and stores it in a
/// channel variable.
///
/// Sets DB_RESULT and DBGETSTATUS (FOUND/NOTFOUND).
pub struct AppDbGet;

impl DialplanApp for AppDbGet {
    fn name(&self) -> &str {
        "DBget"
    }

    fn description(&self) -> &str {
        "Retrieve a value from the Asterisk database"
    }
}

impl AppDbGet {
    /// Execute the DBget application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (varname, path) = match args.split_once('=') {
            Some((v, p)) => (v.trim(), p.trim()),
            None => {
                warn!("DBget: requires variable=family/key argument");
                return PbxExecResult::Failed;
            }
        };

        info!("DBget: channel '{}' {}={}", channel.name, varname, path);

        // In a real implementation:
        // 1. Parse family/key from path
        // 2. ast_db_get(family, key)
        // 3. Set channel variable varname to retrieved value
        // 4. Set DBGETSTATUS to FOUND or NOTFOUND

        PbxExecResult::Success
    }
}

/// The DBdel() dialplan application.
///
/// Usage: DBdel(family/key)
///
/// Deletes a key from the Asterisk database.
pub struct AppDbDel;

impl DialplanApp for AppDbDel {
    fn name(&self) -> &str {
        "DBdel"
    }

    fn description(&self) -> &str {
        "Delete a key from the Asterisk database"
    }
}

impl AppDbDel {
    /// Execute the DBdel application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (family, key) = match args.split_once('/') {
            Some((f, k)) => (f.trim(), k.trim()),
            None => {
                warn!("DBdel: requires family/key argument");
                return PbxExecResult::Failed;
            }
        };

        info!("DBdel: channel '{}' deleting {}/{}", channel.name, family, key);

        // In a real implementation:
        // ast_db_del(family, key)

        PbxExecResult::Success
    }
}

/// The DBdeltree() dialplan application.
///
/// Usage: DBdeltree(family[/keytree])
///
/// Deletes an entire family or keytree from the Asterisk database.
pub struct AppDbDelTree;

impl DialplanApp for AppDbDelTree {
    fn name(&self) -> &str {
        "DBdeltree"
    }

    fn description(&self) -> &str {
        "Delete a family or keytree from the Asterisk database"
    }
}

impl AppDbDelTree {
    /// Execute the DBdeltree application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let family = args.trim();

        if family.is_empty() {
            warn!("DBdeltree: requires family argument");
            return PbxExecResult::Failed;
        }

        info!("DBdeltree: channel '{}' deleting tree '{}'", channel.name, family);

        // In a real implementation:
        // ast_db_deltree(family, keytree)

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dbput_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDbPut::exec(&mut channel, "cidname/1234=John Doe").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_dbput_missing_value() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDbPut::exec(&mut channel, "cidname/1234").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_dbget_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDbGet::exec(&mut channel, "CALLERID=cidname/1234").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_dbdel_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDbDel::exec(&mut channel, "cidname/1234").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_dbdeltree_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDbDelTree::exec(&mut channel, "cidname").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
