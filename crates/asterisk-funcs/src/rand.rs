//! RAND() function - random number generation.
//!
//! Port of func_rand.c from Asterisk C.
//!
//! Usage: RAND([min[,max]])
//!
//! Returns a random integer between min (default 0) and max (default RAND_MAX).

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};
use rand::Rng;

/// RAND() function.
///
/// Generates a random integer within a specified range.
///
/// Usage:
///   RAND()       -> random number 0..2147483647
///   RAND(max)    -> random number 0..max (inclusive)
///   RAND(min,max) -> random number min..max (inclusive)
pub struct FuncRand;

impl DialplanFunc for FuncRand {
    fn name(&self) -> &str {
        "RAND"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let args = args.trim();

        let (min, max) = if args.is_empty() {
            (0i64, i32::MAX as i64)
        } else {
            let parts: Vec<&str> = args.splitn(2, ',').collect();
            match parts.len() {
                1 => {
                    let max_val: i64 = parts[0].trim().parse().map_err(|_| {
                        FuncError::InvalidArgument(format!(
                            "RAND: invalid max value '{}'",
                            parts[0].trim()
                        ))
                    })?;
                    (0, max_val)
                }
                2 => {
                    let min_val: i64 = parts[0].trim().parse().map_err(|_| {
                        FuncError::InvalidArgument(format!(
                            "RAND: invalid min value '{}'",
                            parts[0].trim()
                        ))
                    })?;
                    let max_val: i64 = parts[1].trim().parse().map_err(|_| {
                        FuncError::InvalidArgument(format!(
                            "RAND: invalid max value '{}'",
                            parts[1].trim()
                        ))
                    })?;
                    (min_val, max_val)
                }
                _ => (0, i32::MAX as i64),
            }
        };

        if min > max {
            return Err(FuncError::InvalidArgument(format!(
                "RAND: min ({}) must be <= max ({})",
                min, max
            )));
        }

        let mut rng = rand::thread_rng();
        let value = rng.gen_range(min..=max);
        Ok(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rand_range() {
        let ctx = FuncContext::new();
        let func = FuncRand;
        for _ in 0..100 {
            let result = func.read(&ctx, "1,10").unwrap();
            let val: i64 = result.parse().unwrap();
            assert!(val >= 1 && val <= 10, "RAND(1,10) = {} out of range", val);
        }
    }

    #[test]
    fn test_rand_single_arg() {
        let ctx = FuncContext::new();
        let func = FuncRand;
        for _ in 0..100 {
            let result = func.read(&ctx, "5").unwrap();
            let val: i64 = result.parse().unwrap();
            assert!(val >= 0 && val <= 5, "RAND(5) = {} out of range", val);
        }
    }

    #[test]
    fn test_rand_no_args() {
        let ctx = FuncContext::new();
        let func = FuncRand;
        let result = func.read(&ctx, "").unwrap();
        let val: i64 = result.parse().unwrap();
        assert!(val >= 0);
    }

    #[test]
    fn test_rand_min_greater_than_max() {
        let ctx = FuncContext::new();
        let func = FuncRand;
        assert!(func.read(&ctx, "10,1").is_err());
    }

    #[test]
    fn test_rand_same_min_max() {
        let ctx = FuncContext::new();
        let func = FuncRand;
        let result = func.read(&ctx, "42,42").unwrap();
        assert_eq!(result, "42");
    }
}
