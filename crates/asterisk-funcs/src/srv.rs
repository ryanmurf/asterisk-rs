//! SRVQUERY/SRVRESULT functions - DNS SRV record lookup.
//!
//! Port of func_srv.c from Asterisk C.
//!
//! Provides:
//! - SRVQUERY(service) - initiate an SRV lookup
//! - SRVRESULT(id,resultnum[,field]) - retrieve results
//!
//! Usage:
//!   Set(id=${SRVQUERY(_sip._udp.example.com)})
//!   Set(count=${SRVRESULT(${id},getnum)})
//!   Set(host=${SRVRESULT(${id},1,host)})
//!   Set(port=${SRVRESULT(${id},1,port)})
//!   Set(priority=${SRVRESULT(${id},1,priority)})
//!   Set(weight=${SRVRESULT(${id},1,weight)})

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// SRV record result fields.
pub const SRV_FIELDS: &[&str] = &["host", "port", "priority", "weight"];

/// SRVQUERY() function - initiate a DNS SRV lookup.
pub struct FuncSrvQuery;

impl DialplanFunc for FuncSrvQuery {
    fn name(&self) -> &str {
        "SRVQUERY"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let service = args.trim();
        if service.is_empty() {
            return Err(FuncError::InvalidArgument(
                "SRVQUERY requires a service name argument".into(),
            ));
        }

        // Stub: would perform actual DNS SRV lookup
        // Store a query ID in channel variables
        let query_id = format!("srv_{}", service.replace('.', "_"));
        // Store 0 results by default
        let count_var = format!("{}_count", query_id);
        // We can't mutate ctx here (read-only), so just return the ID
        // In production, results would be stored in a channel datastore
        Ok(query_id)
    }
}

/// SRVRESULT() function - retrieve SRV lookup results.
pub struct FuncSrvResult;

impl DialplanFunc for FuncSrvResult {
    fn name(&self) -> &str {
        "SRVRESULT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.split(',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "SRVRESULT requires at least id and resultnum arguments".into(),
            ));
        }

        let query_id = parts[0].trim();
        let result_num = parts[1].trim();
        let field = parts.get(2).map(|s| s.trim()).unwrap_or("host");

        if query_id.is_empty() {
            return Err(FuncError::InvalidArgument(
                "SRVRESULT: query ID cannot be empty".into(),
            ));
        }

        if result_num == "getnum" {
            // Return total number of results
            let count_var = format!("{}_count", query_id);
            return Ok(ctx.get_variable(&count_var).cloned().unwrap_or_else(|| "0".into()));
        }

        // Validate field name
        if !SRV_FIELDS.contains(&field) {
            return Err(FuncError::InvalidArgument(format!(
                "SRVRESULT: unknown field '{}', valid fields: {:?}", field, SRV_FIELDS
            )));
        }

        // Stub: would look up result from stored SRV data
        let var_name = format!("{}_{}_{}",query_id, result_num, field);
        Ok(ctx.get_variable(&var_name).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srvquery() {
        let ctx = FuncContext::new();
        let func = FuncSrvQuery;
        let result = func.read(&ctx, "_sip._udp.example.com");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_srvquery_empty() {
        let ctx = FuncContext::new();
        let func = FuncSrvQuery;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_srvresult_getnum() {
        let ctx = FuncContext::new();
        let func = FuncSrvResult;
        let result = func.read(&ctx, "some_id,getnum");
        assert_eq!(result.unwrap(), "0");
    }

    #[test]
    fn test_srvresult_invalid_field() {
        let ctx = FuncContext::new();
        let func = FuncSrvResult;
        assert!(func.read(&ctx, "some_id,1,invalid").is_err());
    }
}
