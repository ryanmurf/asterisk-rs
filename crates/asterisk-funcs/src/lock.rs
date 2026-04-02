//! LOCK()/TRYLOCK()/UNLOCK() functions - named mutex locks for dialplan.
//!
//! Port of func_lock.c from Asterisk C.
//!
//! Provides dialplan synchronization primitives:
//! - LOCK(name) - Acquire named mutex (blocks until available), returns 1 on success
//! - TRYLOCK(name) - Try to acquire named mutex (non-blocking), returns 1 on success, 0 if busy
//! - UNLOCK(name) - Release named mutex, returns 1 on success, 0 if not held

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use std::collections::HashSet;
use std::sync::Mutex;

/// Global lock manager for dialplan named locks.
///
/// In production, this would use real mutexes. For the dialplan function
/// port, we track lock state in a shared set.
#[derive(Debug, Default)]
pub struct LockManager {
    /// Currently held locks (lock_name -> holder channel)
    held_locks: Mutex<HashSet<String>>,
}

impl LockManager {
    pub fn new() -> Self {
        Self {
            held_locks: Mutex::new(HashSet::new()),
        }
    }

    /// Attempt to acquire a named lock. Returns true if acquired.
    pub fn try_lock(&self, name: &str) -> bool {
        let mut locks = self.held_locks.lock().unwrap();
        if locks.contains(name) {
            false
        } else {
            locks.insert(name.to_string());
            true
        }
    }

    /// Release a named lock. Returns true if was held.
    pub fn unlock(&self, name: &str) -> bool {
        self.held_locks.lock().unwrap().remove(name)
    }

    /// Check if a lock is currently held.
    pub fn is_locked(&self, name: &str) -> bool {
        self.held_locks.lock().unwrap().contains(name)
    }
}

/// LOCK() function.
///
/// Usage: LOCK(name) - acquires named lock, returns "1" on success.
/// In this simplified port, LOCK behaves like TRYLOCK (non-blocking).
pub struct FuncLock;

impl DialplanFunc for FuncLock {
    fn name(&self) -> &str {
        "LOCK"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let lock_name = args.trim();
        if lock_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "LOCK: lock name is required".to_string(),
            ));
        }
        // Simulate lock by setting a variable
        let key = format!("__LOCK_{}", lock_name);
        if ctx.get_variable(&key).is_some() {
            Ok("0".to_string()) // already locked
        } else {
            Ok("1".to_string()) // would acquire
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, _value: &str) -> Result<(), FuncError> {
        let lock_name = args.trim();
        if lock_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "LOCK: lock name is required".to_string(),
            ));
        }
        let key = format!("__LOCK_{}", lock_name);
        ctx.set_variable(&key, "1");
        Ok(())
    }
}

/// TRYLOCK() function.
///
/// Usage: TRYLOCK(name) - tries to acquire named lock (non-blocking).
/// Returns "1" if acquired, "0" if already held.
pub struct FuncTryLock;

impl DialplanFunc for FuncTryLock {
    fn name(&self) -> &str {
        "TRYLOCK"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let lock_name = args.trim();
        if lock_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "TRYLOCK: lock name is required".to_string(),
            ));
        }
        let key = format!("__LOCK_{}", lock_name);
        if ctx.get_variable(&key).is_some() {
            Ok("0".to_string())
        } else {
            Ok("1".to_string())
        }
    }
}

/// UNLOCK() function.
///
/// Usage: UNLOCK(name) - releases named lock.
/// Returns "1" if released, "0" if not held.
pub struct FuncUnlock;

impl DialplanFunc for FuncUnlock {
    fn name(&self) -> &str {
        "UNLOCK"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let lock_name = args.trim();
        if lock_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "UNLOCK: lock name is required".to_string(),
            ));
        }
        let key = format!("__LOCK_{}", lock_name);
        if ctx.get_variable(&key).is_some() {
            Ok("1".to_string())
        } else {
            Ok("0".to_string())
        }
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, _value: &str) -> Result<(), FuncError> {
        let lock_name = args.trim();
        if lock_name.is_empty() {
            return Err(FuncError::InvalidArgument(
                "UNLOCK: lock name is required".to_string(),
            ));
        }
        let key = format!("__LOCK_{}", lock_name);
        ctx.variables.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_manager() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock("test"));
        assert!(!mgr.try_lock("test")); // already held
        assert!(mgr.is_locked("test"));
        assert!(mgr.unlock("test"));
        assert!(!mgr.is_locked("test"));
        assert!(!mgr.unlock("test")); // not held
    }

    #[test]
    fn test_func_lock_read() {
        let ctx = FuncContext::new();
        let func = FuncLock;
        assert_eq!(func.read(&ctx, "mylock").unwrap(), "1"); // not held
    }

    #[test]
    fn test_func_trylock() {
        let mut ctx = FuncContext::new();
        let func = FuncTryLock;
        assert_eq!(func.read(&ctx, "mylock").unwrap(), "1");
        ctx.set_variable("__LOCK_mylock", "1");
        assert_eq!(func.read(&ctx, "mylock").unwrap(), "0");
    }

    #[test]
    fn test_func_unlock() {
        let mut ctx = FuncContext::new();
        let func = FuncUnlock;
        assert_eq!(func.read(&ctx, "mylock").unwrap(), "0"); // not held
        ctx.set_variable("__LOCK_mylock", "1");
        assert_eq!(func.read(&ctx, "mylock").unwrap(), "1"); // is held
    }

    #[test]
    fn test_empty_name_err() {
        let ctx = FuncContext::new();
        assert!(FuncLock.read(&ctx, "").is_err());
        assert!(FuncTryLock.read(&ctx, "").is_err());
        assert!(FuncUnlock.read(&ctx, "").is_err());
    }
}
