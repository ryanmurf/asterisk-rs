//! Hash functions - hash tables and keypad hash.
//!
//! Port of func_hash.c concepts from Asterisk C.
//!
//! Provides:
//! - HASH(hashname,key) - read/write hash table entries stored in channel vars
//! - HASHKEYS(hashname) - list keys in a hash table
//! - KEYPADHASH(string) - convert letters to telephone keypad digits

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// HASH() function.
///
/// Implements hash table entries stored as channel variables.
///
/// Read:  HASH(hashname,key) -> value
/// Write: Set(HASH(hashname,key)=value)
///
/// The hash tables are stored as __HASH_<hashname>_<key> variables
/// with a key list in __HASH_KEYS_<hashname>.
pub struct FuncHash;

impl FuncHash {
    fn parse_args(args: &str) -> Result<(String, String), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.is_empty() || parts[0].trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "HASH: hashname is required".to_string(),
            ));
        }
        let hashname = parts[0].trim().to_string();
        let key = if parts.len() > 1 {
            parts[1].trim().to_string()
        } else {
            String::new()
        };
        Ok((hashname, key))
    }

    fn var_name(hashname: &str, key: &str) -> String {
        format!("__HASH_{}_{}", hashname, key)
    }

    fn keys_var(hashname: &str) -> String {
        format!("__HASH_KEYS_{}", hashname)
    }
}

impl DialplanFunc for FuncHash {
    fn name(&self) -> &str {
        "HASH"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (hashname, key) = Self::parse_args(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "HASH: key is required for read".to_string(),
            ));
        }
        let var = Self::var_name(&hashname, &key);
        ctx.get_variable(&var)
            .cloned()
            .ok_or_else(|| FuncError::DataNotAvailable(format!("HASH: key '{}' not found in '{}'", key, hashname)))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let (hashname, key) = Self::parse_args(args)?;
        if key.is_empty() {
            return Err(FuncError::InvalidArgument(
                "HASH: key is required for write".to_string(),
            ));
        }
        let var = Self::var_name(&hashname, &key);
        ctx.set_variable(&var, value);

        // Track key in the hash key list
        let keys_var = Self::keys_var(&hashname);
        let mut keys: Vec<String> = ctx
            .get_variable(&keys_var)
            .map(|v| {
                v.split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        if !keys.contains(&key) {
            keys.push(key);
        }
        ctx.set_variable(&keys_var, &keys.join(","));
        Ok(())
    }
}

/// HASHKEYS() function.
///
/// Returns a comma-separated list of keys in a hash table.
///
/// Usage: HASHKEYS(hashname) -> "key1,key2,key3"
pub struct FuncHashKeys;

impl DialplanFunc for FuncHashKeys {
    fn name(&self) -> &str {
        "HASHKEYS"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let hashname = args.trim();
        if hashname.is_empty() {
            return Err(FuncError::InvalidArgument(
                "HASHKEYS: hashname is required".to_string(),
            ));
        }
        let keys_var = FuncHash::keys_var(hashname);
        Ok(ctx.get_variable(&keys_var).cloned().unwrap_or_default())
    }
}

/// KEYPADHASH() function.
///
/// Converts letters in a string to their corresponding telephone
/// keypad digits:
///   ABC  -> 2
///   DEF  -> 3
///   GHI  -> 4
///   JKL  -> 5
///   MNO  -> 6
///   PQRS -> 7
///   TUV  -> 8
///   WXYZ -> 9
///
/// Digits pass through unchanged. Other characters are removed.
///
/// Usage: KEYPADHASH(Hello World) -> "43556096753"
pub struct FuncKeypadHash;

impl FuncKeypadHash {
    /// Convert a single character to its keypad digit.
    fn char_to_keypad(c: char) -> Option<char> {
        match c.to_ascii_uppercase() {
            'A' | 'B' | 'C' => Some('2'),
            'D' | 'E' | 'F' => Some('3'),
            'G' | 'H' | 'I' => Some('4'),
            'J' | 'K' | 'L' => Some('5'),
            'M' | 'N' | 'O' => Some('6'),
            'P' | 'Q' | 'R' | 'S' => Some('7'),
            'T' | 'U' | 'V' => Some('8'),
            'W' | 'X' | 'Y' | 'Z' => Some('9'),
            '0' => Some('0'),
            '1' => Some('1'),
            '2' => Some('2'),
            '3' => Some('3'),
            '4' => Some('4'),
            '5' => Some('5'),
            '6' => Some('6'),
            '7' => Some('7'),
            '8' => Some('8'),
            '9' => Some('9'),
            _ => None,
        }
    }

    /// Convert a string to keypad digits.
    pub fn keypad_hash(input: &str) -> String {
        input.chars().filter_map(Self::char_to_keypad).collect()
    }
}

impl DialplanFunc for FuncKeypadHash {
    fn name(&self) -> &str {
        "KEYPADHASH"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(Self::keypad_hash(args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_write_and_read() {
        let mut ctx = FuncContext::new();
        let func = FuncHash;
        func.write(&mut ctx, "colors,red", "#FF0000").unwrap();
        assert_eq!(func.read(&ctx, "colors,red").unwrap(), "#FF0000");
    }

    #[test]
    fn test_hash_not_found() {
        let ctx = FuncContext::new();
        let func = FuncHash;
        assert!(func.read(&ctx, "colors,blue").is_err());
    }

    #[test]
    fn test_hashkeys() {
        let mut ctx = FuncContext::new();
        let hash = FuncHash;
        let keys = FuncHashKeys;
        hash.write(&mut ctx, "colors,red", "#FF0000").unwrap();
        hash.write(&mut ctx, "colors,green", "#00FF00").unwrap();
        hash.write(&mut ctx, "colors,blue", "#0000FF").unwrap();
        let result = keys.read(&ctx, "colors").unwrap();
        assert!(result.contains("red"));
        assert!(result.contains("green"));
        assert!(result.contains("blue"));
    }

    #[test]
    fn test_hashkeys_empty() {
        let ctx = FuncContext::new();
        let func = FuncHashKeys;
        assert_eq!(func.read(&ctx, "nonexistent").unwrap(), "");
    }

    #[test]
    fn test_keypadhash_hello() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        assert_eq!(func.read(&ctx, "Hello").unwrap(), "43556");
    }

    #[test]
    fn test_keypadhash_digits_passthrough() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        assert_eq!(func.read(&ctx, "123ABC").unwrap(), "123222");
    }

    #[test]
    fn test_keypadhash_full_alphabet() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        // A-C=2, D-F=3, G-I=4, J-L=5, M-O=6, P-S=7, T-V=8, W-Z=9
        assert_eq!(
            func.read(&ctx, "ABCDEFGHIJKLMNOPQRSTUVWXYZ").unwrap(),
            "22233344455566677778889999"
        );
    }

    #[test]
    fn test_keypadhash_empty() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        assert_eq!(func.read(&ctx, "").unwrap(), "");
    }

    #[test]
    fn test_keypadhash_special_chars_removed() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        assert_eq!(func.read(&ctx, "Hi!@#").unwrap(), "44");
    }

    #[test]
    fn test_keypadhash_case_insensitive() {
        let ctx = FuncContext::new();
        let func = FuncKeypadHash;
        assert_eq!(
            func.read(&ctx, "abc").unwrap(),
            func.read(&ctx, "ABC").unwrap()
        );
    }
}
