//! ODBC() function - execute ODBC queries from dialplan.
//!
//! Port of func_odbc.c from Asterisk C.
//!
//! Provides dynamically-created dialplan functions that execute SQL
//! queries via ODBC. Functions are defined in func_odbc.conf:
//!
//!   [MYQUERY]
//!   dsn=asterisk
//!   readsql=SELECT name FROM users WHERE id='${ARG1}'
//!   writesql=UPDATE users SET name='${VALUE}' WHERE id='${ARG1}'
//!
//! This generates a function ODBC_MYQUERY() that can be used in dialplan.
//!
//! This is a stub interface. Actual ODBC connectivity requires an ODBC
//! driver manager (unixODBC/iODBC) and database drivers.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// ODBC() function (stub interface).
///
/// In a full implementation, ODBC functions would be dynamically registered
/// from func_odbc.conf. This stub provides the base ODBC_FETCH function.
pub struct FuncOdbc;

impl DialplanFunc for FuncOdbc {
    fn name(&self) -> &str {
        "ODBC_FETCH"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        if args.trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "ODBC_FETCH requires a fetch ID argument".into(),
            ));
        }
        // Stub: would fetch next row from a previous ODBC query
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_odbc_fetch_stub() {
        let ctx = FuncContext::new();
        let func = FuncOdbc;
        let result = func.read(&ctx, "some_id");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_odbc_fetch_no_args() {
        let ctx = FuncContext::new();
        let func = FuncOdbc;
        assert!(func.read(&ctx, "").is_err());
    }
}
