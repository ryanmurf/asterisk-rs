//! SYSINFO() function - system information.
//!
//! Port of func_sysinfo.c from Asterisk C.
//!
//! Provides:
//! - SYSINFO(parameter) - return system information
//!
//! Parameters:
//! - loadavg   - system load average (1-minute)
//! - numcalls  - number of active calls
//! - uptime    - system uptime in hours
//! - totalram  - total RAM in bytes
//! - freeram   - free RAM in bytes
//! - bufferram - buffered RAM in bytes
//! - totalswap - total swap in bytes
//! - freeswap  - free swap in bytes
//! - numprocs  - number of processes
//! - numcpus   - number of CPU cores

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Known SYSINFO parameter names.
pub const SYSINFO_PARAMS: &[&str] = &[
    "loadavg", "numcalls", "uptime", "totalram", "freeram",
    "bufferram", "totalswap", "freeswap", "numprocs", "numcpus",
];

/// SYSINFO() function.
pub struct FuncSysInfo;

impl DialplanFunc for FuncSysInfo {
    fn name(&self) -> &str {
        "SYSINFO"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let param = args.trim().to_lowercase();

        match param.as_str() {
            "loadavg" => {
                // System load average; platform-dependent
                Ok("0.00".to_string())
            }
            "numcalls" => {
                // Number of active calls (stub)
                Ok("0".to_string())
            }
            "uptime" => {
                // System uptime in hours (stub)
                Ok("0".to_string())
            }
            "totalram" => {
                // Total RAM (stub)
                Ok("0".to_string())
            }
            "freeram" => {
                // Free RAM (stub)
                Ok("0".to_string())
            }
            "bufferram" => {
                // Buffer RAM (stub)
                Ok("0".to_string())
            }
            "totalswap" => {
                // Total swap (stub)
                Ok("0".to_string())
            }
            "freeswap" => {
                // Free swap (stub)
                Ok("0".to_string())
            }
            "numprocs" => {
                // Number of processes (stub)
                Ok("0".to_string())
            }
            "numcpus" => {
                // Number of CPU cores - this we can actually provide
                Ok(std::thread::available_parallelism()
                    .map(|n| n.get().to_string())
                    .unwrap_or_else(|_| "1".to_string()))
            }
            "" => Err(FuncError::InvalidArgument(
                "SYSINFO requires a parameter argument".into(),
            )),
            _ => Err(FuncError::InvalidArgument(format!(
                "SYSINFO: unknown parameter '{}', valid: {:?}", param, SYSINFO_PARAMS
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sysinfo_numcpus() {
        let ctx = FuncContext::new();
        let func = FuncSysInfo;
        let result = func.read(&ctx, "numcpus").unwrap();
        let cpus: u32 = result.parse().unwrap();
        assert!(cpus >= 1);
    }

    #[test]
    fn test_sysinfo_loadavg() {
        let ctx = FuncContext::new();
        let func = FuncSysInfo;
        let result = func.read(&ctx, "loadavg").unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_sysinfo_numcalls() {
        let ctx = FuncContext::new();
        let func = FuncSysInfo;
        let result = func.read(&ctx, "numcalls").unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn test_sysinfo_empty() {
        let ctx = FuncContext::new();
        let func = FuncSysInfo;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_sysinfo_invalid() {
        let ctx = FuncContext::new();
        let func = FuncSysInfo;
        assert!(func.read(&ctx, "invalid").is_err());
    }
}
