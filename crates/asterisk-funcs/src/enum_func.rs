//! ENUM lookup functions - DNS ENUM queries (E.164 to URI).
//!
//! Port of func_enum.c from Asterisk C.
//!
//! Provides:
//! - ENUMLOOKUP(number,method,options,zone) - DNS ENUM lookup
//! - ENUMQUERY(number,zone) - store ENUM result
//! - ENUMRESULT(id,field) - retrieve stored result fields

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// A single ENUM/NAPTR result record.
#[derive(Debug, Clone)]
pub struct EnumResult {
    /// Order field from NAPTR
    pub order: u16,
    /// Preference field from NAPTR
    pub preference: u16,
    /// Service (e.g., "E2U+sip", "E2U+iax")
    pub service: String,
    /// The resulting URI
    pub uri: String,
}

/// ENUMLOOKUP() function.
///
/// Performs a DNS ENUM lookup converting an E.164 phone number to a URI
/// via NAPTR record queries.
///
/// Usage: ENUMLOOKUP(number[,method[,options[,zone]]])
///
/// - number: E.164 phone number (e.g., +15551234567)
/// - method: Service type to filter (default: "pjsip")
/// - options: "c" to return count of matches
/// - zone: DNS zone suffix (default: "e164.arpa")
///
/// Since actual DNS queries are not performed in this Rust port,
/// the lookup is simulated by storing/retrieving results from context variables.
pub struct FuncEnumLookup;

impl FuncEnumLookup {
    /// Convert an E.164 number to the ENUM domain name.
    /// E.g., +15551234567 -> 7.6.5.4.3.2.1.5.5.5.1.e164.arpa
    pub fn number_to_domain(number: &str, zone: &str) -> String {
        let digits: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
        let reversed: String = digits
            .chars()
            .rev()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(".");
        format!("{}.{}", reversed, zone)
    }
}

impl DialplanFunc for FuncEnumLookup {
    fn name(&self) -> &str {
        "ENUMLOOKUP"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(4, ',').collect();
        if parts.is_empty() || parts[0].trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "ENUMLOOKUP: number argument is required".to_string(),
            ));
        }

        let number = parts[0].trim();
        let _method = if parts.len() > 1 && !parts[1].trim().is_empty() {
            parts[1].trim()
        } else {
            "pjsip"
        };
        let options = if parts.len() > 2 { parts[2].trim() } else { "" };
        let zone = if parts.len() > 3 && !parts[3].trim().is_empty() {
            parts[3].trim()
        } else {
            "e164.arpa"
        };

        let domain = Self::number_to_domain(number, zone);

        // Check for stored ENUM results (from test setup or previous queries)
        let result_var = format!("__ENUM_RESULT_{}", domain);
        if let Some(stored) = ctx.get_variable(&result_var) {
            if options.contains('c') {
                // Return count
                let count = stored.split('|').filter(|s| !s.is_empty()).count();
                return Ok(count.to_string());
            }
            // Return first result
            return Ok(stored.split('|').next().unwrap_or("").to_string());
        }

        // No results - return empty for real DNS lookup scenario
        if options.contains('c') {
            Ok("0".to_string())
        } else {
            Ok(String::new())
        }
    }
}

/// ENUMQUERY() function.
///
/// Initiates an ENUM lookup and stores the results for later retrieval
/// with ENUMRESULT().
///
/// Usage: ENUMQUERY(number[,method[,zone]])
/// Returns: A query ID for use with ENUMRESULT
pub struct FuncEnumQuery {
    next_id: std::sync::atomic::AtomicU64,
}

impl FuncEnumQuery {
    pub fn new() -> Self {
        Self {
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl Default for FuncEnumQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl DialplanFunc for FuncEnumQuery {
    fn name(&self) -> &str {
        "ENUMQUERY"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.is_empty() || parts[0].trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "ENUMQUERY: number argument is required".to_string(),
            ));
        }

        // In production, this would perform DNS NAPTR lookups.
        // Return a query ID for subsequent ENUMRESULT calls.
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(id.to_string())
    }
}

/// ENUMRESULT() function.
///
/// Retrieves results from a previous ENUMQUERY().
///
/// Usage: ENUMRESULT(id,resultnum)
///   - id: query ID from ENUMQUERY
///   - resultnum: 1-based result index, or "getnum" for total count
pub struct FuncEnumResult;

impl DialplanFunc for FuncEnumResult {
    fn name(&self) -> &str {
        "ENUMRESULT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "ENUMRESULT: requires id,resultnum arguments".to_string(),
            ));
        }

        let id = parts[0].trim();
        let result_num = parts[1].trim();

        let var_name = format!("__ENUMQUERY_{}", id);
        let stored = ctx.get_variable(&var_name).cloned().unwrap_or_default();

        if result_num.eq_ignore_ascii_case("getnum") {
            let count = if stored.is_empty() {
                0
            } else {
                stored.split('|').filter(|s| !s.is_empty()).count()
            };
            return Ok(count.to_string());
        }

        let index: usize = result_num.parse().map_err(|_| {
            FuncError::InvalidArgument(format!(
                "ENUMRESULT: invalid result number '{}'",
                result_num
            ))
        })?;

        if index == 0 {
            return Err(FuncError::InvalidArgument(
                "ENUMRESULT: result numbers start at 1".to_string(),
            ));
        }

        let results: Vec<&str> = stored.split('|').filter(|s| !s.is_empty()).collect();
        if index > results.len() {
            Ok(String::new())
        } else {
            Ok(results[index - 1].to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_number_to_domain() {
        let domain = FuncEnumLookup::number_to_domain("+15551234567", "e164.arpa");
        assert_eq!(domain, "7.6.5.4.3.2.1.5.5.5.1.e164.arpa");
    }

    #[test]
    fn test_enumlookup_no_results() {
        let ctx = FuncContext::new();
        let func = FuncEnumLookup;
        let result = func.read(&ctx, "+15551234567").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_enumlookup_count_no_results() {
        let ctx = FuncContext::new();
        let func = FuncEnumLookup;
        let result = func.read(&ctx, "+15551234567,,c").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_enumlookup_with_stored_results() {
        let mut ctx = FuncContext::new();
        let domain = FuncEnumLookup::number_to_domain("+15551234567", "e164.arpa");
        let var = format!("__ENUM_RESULT_{}", domain);
        ctx.set_variable(&var, "sip:5551234567@example.com|iax2:5551234567@example.com");
        let func = FuncEnumLookup;
        let result = func.read(&ctx, "+15551234567").unwrap();
        assert_eq!(result, "sip:5551234567@example.com");
    }

    #[test]
    fn test_enumquery_returns_id() {
        let ctx = FuncContext::new();
        let func = FuncEnumQuery::new();
        let id1 = func.read(&ctx, "+15551234567").unwrap();
        let id2 = func.read(&ctx, "+15551234567").unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_enumresult_getnum() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__ENUMQUERY_1", "sip:foo@bar.com|iax2:foo@bar.com");
        let func = FuncEnumResult;
        let count = func.read(&ctx, "1,getnum").unwrap();
        assert_eq!(count, "2");
    }

    #[test]
    fn test_enumresult_by_index() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("__ENUMQUERY_1", "sip:foo@bar.com|iax2:foo@bar.com");
        let func = FuncEnumResult;
        assert_eq!(func.read(&ctx, "1,1").unwrap(), "sip:foo@bar.com");
        assert_eq!(func.read(&ctx, "1,2").unwrap(), "iax2:foo@bar.com");
        assert_eq!(func.read(&ctx, "1,3").unwrap(), "");
    }
}
