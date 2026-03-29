//! REALTIME() function - query realtime backend.
//!
//! Port of func_realtime.c from Asterisk C.
//!
//! Provides:
//! - REALTIME(family,fieldmatch,matchvalue[,delim1[,delim2]]) - read from realtime
//! - REALTIME(family,fieldmatch,matchvalue,field)= value     - write to realtime
//! - REALTIME_STORE(family,field1,field2,...) = val1,val2,... - store new record
//! - REALTIME_DESTROY(family,fieldmatch,matchvalue[,delim1[,delim2]]) - delete
//! - REALTIME_FIELD(family,fieldmatch,matchvalue,fieldname)  - read single field
//! - REALTIME_HASH(family,fieldmatch,matchvalue)             - read as hash
//!
//! This is a stub: actual realtime backend lookups are not implemented.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// REALTIME() function.
pub struct FuncRealtime;

impl FuncRealtime {
    fn parse_args(args: &str) -> Result<Vec<&str>, FuncError> {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "REALTIME requires at least family and fieldmatch arguments".into(),
            ));
        }
        Ok(parts)
    }
}

impl DialplanFunc for FuncRealtime {
    fn name(&self) -> &str {
        "REALTIME"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts = Self::parse_args(args)?;
        let _family = parts[0].trim();
        let _field_match = parts[1].trim();
        let _match_value = parts.get(2).map(|s| s.trim()).unwrap_or("");
        let _delim1 = parts.get(3).map(|s| s.trim()).unwrap_or(",");
        let _delim2 = parts.get(4).map(|s| s.trim()).unwrap_or("=");

        // Stub: would query the realtime backend (database, LDAP, etc.)
        Ok(String::new())
    }

    fn write(&self, _ctx: &mut FuncContext, args: &str, _value: &str) -> Result<(), FuncError> {
        let parts = Self::parse_args(args)?;
        let _family = parts[0].trim();
        let _field_match = parts[1].trim();
        // Stub: would write to the realtime backend
        Ok(())
    }
}

/// REALTIME_FIELD() function - read a single field.
pub struct FuncRealtimeField;

impl DialplanFunc for FuncRealtimeField {
    fn name(&self) -> &str {
        "REALTIME_FIELD"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 4 {
            return Err(FuncError::InvalidArgument(
                "REALTIME_FIELD requires family, fieldmatch, matchvalue, and fieldname".into(),
            ));
        }
        // Stub: would query realtime for a specific field
        Ok(String::new())
    }
}

/// REALTIME_HASH() function - read as hash (name=value pairs).
pub struct FuncRealtimeHash;

impl DialplanFunc for FuncRealtimeHash {
    fn name(&self) -> &str {
        "REALTIME_HASH"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "REALTIME_HASH requires family, fieldmatch, and matchvalue".into(),
            ));
        }
        // Stub: would query realtime and return as hash
        Ok(String::new())
    }
}

/// REALTIME_STORE() function - store a new record.
pub struct FuncRealtimeStore;

impl DialplanFunc for FuncRealtimeStore {
    fn name(&self) -> &str {
        "REALTIME_STORE"
    }

    fn read(&self, _ctx: &FuncContext, _args: &str) -> FuncResult {
        Err(FuncError::ReadOnly("REALTIME_STORE".into()))
    }

    fn write(&self, _ctx: &mut FuncContext, _args: &str, _value: &str) -> Result<(), FuncError> {
        // Stub: would store a new record in realtime backend
        Ok(())
    }
}

/// REALTIME_DESTROY() function - delete a record.
pub struct FuncRealtimeDestroy;

impl DialplanFunc for FuncRealtimeDestroy {
    fn name(&self) -> &str {
        "REALTIME_DESTROY"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "REALTIME_DESTROY requires family, fieldmatch, and matchvalue".into(),
            ));
        }
        // Stub: would delete from realtime backend, returns count
        Ok("0".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_realtime_read() {
        let ctx = FuncContext::new();
        let func = FuncRealtime;
        let result = func.read(&ctx, "sippeers,name,100");
        assert!(result.is_ok());
    }

    #[test]
    fn test_realtime_missing_args() {
        let ctx = FuncContext::new();
        let func = FuncRealtime;
        assert!(func.read(&ctx, "sippeers").is_err());
    }
}
