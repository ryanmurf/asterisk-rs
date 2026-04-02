//! System resource limits.
//!
//! Port of `res/res_limit.c`. Provides functions for querying and setting
//! system resource limits (file descriptors, core dump size, etc.), and
//! logging current limit values for diagnostic purposes.

use std::fmt;

use thiserror::Error;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum LimitError {
    #[error("failed to get resource limit: {0}")]
    GetFailed(String),
    #[error("failed to set resource limit: {0}")]
    SetFailed(String),
    #[error("insufficient privileges to set limit")]
    InsufficientPrivileges,
}

pub type LimitResult<T> = Result<T, LimitError>;

// ---------------------------------------------------------------------------
// Resource limit types
// ---------------------------------------------------------------------------

/// Unix resource limit types (correspond to RLIMIT_* constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    /// Maximum number of open file descriptors.
    OpenFiles,
    /// Maximum size of core dumps.
    CoreSize,
    /// Maximum size of the process's virtual memory.
    VirtualMemory,
    /// Maximum data segment size.
    DataSize,
    /// Maximum stack size.
    StackSize,
    /// Maximum number of processes per user.
    NumProcesses,
}

impl ResourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenFiles => "open files",
            Self::CoreSize => "core file size",
            Self::VirtualMemory => "virtual memory",
            Self::DataSize => "data segment size",
            Self::StackSize => "stack size",
            Self::NumProcesses => "max user processes",
        }
    }

    /// Convert to the libc `rlimit` resource constant.
    #[cfg(unix)]
    fn to_rlimit_resource(self) -> libc::c_int {
        match self {
            Self::OpenFiles => libc::RLIMIT_NOFILE as libc::c_int,
            Self::CoreSize => libc::RLIMIT_CORE as libc::c_int,
            Self::DataSize => libc::RLIMIT_DATA as libc::c_int,
            Self::StackSize => libc::RLIMIT_STACK as libc::c_int,
            Self::NumProcesses => libc::RLIMIT_NPROC as libc::c_int,
            // RLIMIT_AS may not exist on all platforms, fallback to DATA.
            Self::VirtualMemory => {
                #[cfg(target_os = "linux")]
                { libc::RLIMIT_AS as libc::c_int }
                #[cfg(not(target_os = "linux"))]
                { libc::RLIMIT_DATA as libc::c_int }
            }
        }
    }
}

impl fmt::Display for ResourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Resource limit value
// ---------------------------------------------------------------------------

/// A resource limit (soft, hard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimit {
    /// Current (soft) limit.
    pub soft: u64,
    /// Maximum (hard) limit.
    pub hard: u64,
}

impl ResourceLimit {
    /// Whether the soft limit is "unlimited" (u64::MAX).
    pub fn soft_is_unlimited(&self) -> bool {
        self.soft == u64::MAX
    }

    /// Whether the hard limit is "unlimited" (u64::MAX).
    pub fn hard_is_unlimited(&self) -> bool {
        self.hard == u64::MAX
    }

    /// Format the limit for display.
    pub fn display_value(val: u64) -> String {
        if val == u64::MAX {
            "unlimited".to_string()
        } else {
            val.to_string()
        }
    }
}

impl fmt::Display for ResourceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "soft={}, hard={}",
            Self::display_value(self.soft),
            Self::display_value(self.hard)
        )
    }
}

// ---------------------------------------------------------------------------
// Platform-specific implementations
// ---------------------------------------------------------------------------

/// Get the current resource limit for the given type.
#[cfg(unix)]
pub fn get_resource_limit(resource: ResourceType) -> LimitResult<ResourceLimit> {
    use std::mem::MaybeUninit;

    let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
    let ret =
        unsafe { libc::getrlimit(resource.to_rlimit_resource() as _, rlim.as_mut_ptr()) };
    if ret != 0 {
        return Err(LimitError::GetFailed(format!(
            "{}: errno {}",
            resource,
            std::io::Error::last_os_error()
        )));
    }
    let rlim = unsafe { rlim.assume_init() };
    let soft = if rlim.rlim_cur == libc::RLIM_INFINITY {
        u64::MAX
    } else {
        rlim.rlim_cur
    };
    let hard = if rlim.rlim_max == libc::RLIM_INFINITY {
        u64::MAX
    } else {
        rlim.rlim_max
    };
    Ok(ResourceLimit { soft, hard })
}

/// Non-Unix fallback: always returns placeholder values.
#[cfg(not(unix))]
pub fn get_resource_limit(resource: ResourceType) -> LimitResult<ResourceLimit> {
    Ok(ResourceLimit {
        soft: u64::MAX,
        hard: u64::MAX,
    })
}

/// Set the soft resource limit for the given type.
#[cfg(unix)]
pub fn set_resource_limit(resource: ResourceType, new_soft: u64) -> LimitResult<()> {
    let current = get_resource_limit(resource)?;
    if new_soft > current.hard && current.hard != u64::MAX {
        return Err(LimitError::SetFailed(format!(
            "requested soft limit {} exceeds hard limit {}",
            new_soft, current.hard
        )));
    }

    let rlim = libc::rlimit {
        rlim_cur: if new_soft == u64::MAX {
            libc::RLIM_INFINITY
        } else {
            new_soft as libc::rlim_t
        },
        rlim_max: if current.hard == u64::MAX {
            libc::RLIM_INFINITY
        } else {
            current.hard as libc::rlim_t
        },
    };

    let ret = unsafe { libc::setrlimit(resource.to_rlimit_resource() as _, &rlim) };
    if ret != 0 {
        return Err(LimitError::SetFailed(format!(
            "{}: errno {}",
            resource,
            std::io::Error::last_os_error()
        )));
    }
    info!(
        resource = %resource,
        new_soft = ResourceLimit::display_value(new_soft).as_str(),
        "Resource limit updated"
    );
    Ok(())
}

/// Non-Unix fallback: no-op.
#[cfg(not(unix))]
pub fn set_resource_limit(_resource: ResourceType, _new_soft: u64) -> LimitResult<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

/// Get the file descriptor limit.
pub fn get_file_limit() -> LimitResult<(u64, u64)> {
    let rl = get_resource_limit(ResourceType::OpenFiles)?;
    Ok((rl.soft, rl.hard))
}

/// Set the file descriptor soft limit.
pub fn set_file_limit(new_limit: u64) -> LimitResult<()> {
    set_resource_limit(ResourceType::OpenFiles, new_limit)
}

/// Get the core dump size limit.
pub fn get_core_limit() -> LimitResult<(u64, u64)> {
    let rl = get_resource_limit(ResourceType::CoreSize)?;
    Ok((rl.soft, rl.hard))
}

/// Log all resource limits for diagnostics.
pub fn log_resource_limits() {
    let resources = [
        ResourceType::OpenFiles,
        ResourceType::CoreSize,
        ResourceType::VirtualMemory,
        ResourceType::DataSize,
        ResourceType::StackSize,
        ResourceType::NumProcesses,
    ];

    for resource in &resources {
        match get_resource_limit(*resource) {
            Ok(limit) => {
                info!(
                    resource = %resource,
                    soft = ResourceLimit::display_value(limit.soft).as_str(),
                    hard = ResourceLimit::display_value(limit.hard).as_str(),
                    "Resource limit"
                );
            }
            Err(e) => {
                warn!(resource = %resource, error = %e, "Failed to get resource limit");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_type_display() {
        assert_eq!(ResourceType::OpenFiles.as_str(), "open files");
        assert_eq!(ResourceType::CoreSize.as_str(), "core file size");
    }

    #[test]
    fn test_resource_limit_display() {
        let rl = ResourceLimit {
            soft: 1024,
            hard: u64::MAX,
        };
        let s = format!("{}", rl);
        assert!(s.contains("1024"));
        assert!(s.contains("unlimited"));
    }

    #[test]
    fn test_get_file_limit() {
        let result = get_file_limit();
        assert!(result.is_ok());
        let (soft, hard) = result.unwrap();
        assert!(soft > 0);
        assert!(hard >= soft);
    }

    #[test]
    fn test_get_core_limit() {
        let result = get_core_limit();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_resource_limit() {
        for resource in &[
            ResourceType::OpenFiles,
            ResourceType::CoreSize,
            ResourceType::StackSize,
        ] {
            let result = get_resource_limit(*resource);
            assert!(result.is_ok(), "Failed for {:?}", resource);
        }
    }

    #[test]
    fn test_unlimited_check() {
        let rl = ResourceLimit {
            soft: u64::MAX,
            hard: u64::MAX,
        };
        assert!(rl.soft_is_unlimited());
        assert!(rl.hard_is_unlimited());

        let rl2 = ResourceLimit {
            soft: 1024,
            hard: 4096,
        };
        assert!(!rl2.soft_is_unlimited());
        assert!(!rl2.hard_is_unlimited());
    }
}
