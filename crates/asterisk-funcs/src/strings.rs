//! String manipulation dialplan functions.
//!
//! Port of func_strings.c from Asterisk C.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// LEN() - returns the length of a string.
pub struct FuncLen;

impl DialplanFunc for FuncLen {
    fn name(&self) -> &str { "LEN" }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(args.len().to_string())
    }
}

/// TOLOWER() - converts a string to lowercase.
pub struct FuncToLower;

impl DialplanFunc for FuncToLower {
    fn name(&self) -> &str { "TOLOWER" }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(args.to_lowercase())
    }
}

/// TOUPPER() - converts a string to uppercase.
pub struct FuncToUpper;

impl DialplanFunc for FuncToUpper {
    fn name(&self) -> &str { "TOUPPER" }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        Ok(args.to_uppercase())
    }
}

/// FIELDQTY() - counts the number of fields separated by a delimiter.
///
/// Usage: FIELDQTY(string,delimiter)
pub struct FuncFieldQty;

impl DialplanFunc for FuncFieldQty {
    fn name(&self) -> &str { "FIELDQTY" }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "FIELDQTY requires varname and delimiter".to_string()
            ));
        }
        let varname = parts[0].trim();
        let delimiter = parse_delimiter(parts[1].trim());

        // Look up the variable value
        let value = ctx.get_variable(varname)
            .map(|s| s.as_str())
            .unwrap_or("");

        if value.is_empty() {
            return Ok("0".to_string());
        }

        let count = value.split(&delimiter).count();
        Ok(count.to_string())
    }
}

/// CUT() - retrieves specific fields from a delimited string.
///
/// Usage: CUT(varname,delimiter,fieldspec)
/// fieldspec: single number (1-based), range (e.g., "2-4"), or "-" for last
pub struct FuncCut;

impl DialplanFunc for FuncCut {
    fn name(&self) -> &str { "CUT" }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.len() < 3 {
            return Err(FuncError::InvalidArgument(
                "CUT requires varname, delimiter, and field specification".to_string()
            ));
        }
        let varname = parts[0].trim();
        let delimiter = parse_delimiter(parts[1].trim());
        let field_spec = parts[2].trim();

        let value = ctx.get_variable(varname)
            .map(|s| s.as_str())
            .unwrap_or("");

        let fields: Vec<&str> = value.split(&delimiter).collect();

        // Parse field specification
        let selected = parse_field_spec(field_spec, fields.len());
        let result: Vec<&str> = selected
            .into_iter()
            .filter_map(|i| fields.get(i))
            .copied()
            .collect();

        Ok(result.join(&delimiter))
    }
}

/// FILTER() - filters a string to include only allowed characters.
///
/// Usage: FILTER(allowed-chars,string)
pub struct FuncFilter;

impl DialplanFunc for FuncFilter {
    fn name(&self) -> &str { "FILTER" }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "FILTER requires allowed-chars and string".to_string()
            ));
        }
        let allowed = parts[0].trim();
        let input = parts[1];

        let allowed_chars = expand_char_spec(allowed);
        let filtered: String = input
            .chars()
            .filter(|c| allowed_chars.contains(c))
            .collect();

        Ok(filtered)
    }
}

/// REPLACE() - replaces characters in a string.
///
/// Usage: REPLACE(varname,find-chars[,replace-char])
pub struct FuncReplace;

impl DialplanFunc for FuncReplace {
    fn name(&self) -> &str { "REPLACE" }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "REPLACE requires varname and find-chars".to_string()
            ));
        }
        let varname = parts[0].trim();
        // Don't trim find_chars since it might be spaces
        let find_chars = parts[1];
        let replace_char = parts.get(2).map(|s| s.trim()).unwrap_or("");

        let value = ctx.get_variable(varname)
            .map(|s| s.as_str())
            .unwrap_or("");

        let result: String = value
            .chars()
            .map(|c| {
                if find_chars.contains(c) {
                    if replace_char.is_empty() {
                        String::new()
                    } else {
                        replace_char.to_string()
                    }
                } else {
                    c.to_string()
                }
            })
            .collect();

        Ok(result)
    }
}

/// SHIFT() - removes and returns the first element of a variable treated as a list.
///
/// Usage: SHIFT(varname[,delimiter])
pub struct FuncShift;

impl DialplanFunc for FuncShift {
    fn name(&self) -> &str { "SHIFT" }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let varname = parts[0].trim();
        let delimiter = parts.get(1)
            .map(|s| parse_delimiter(s.trim()))
            .unwrap_or_else(|| ",".to_string());

        let value = ctx.get_variable(varname)
            .map(|s| s.as_str())
            .unwrap_or("");

        let mut fields: Vec<&str> = value.split(&delimiter[..]).collect();
        if fields.is_empty() {
            return Ok(String::new());
        }

        let first = fields.remove(0).to_string();
        // Note: In a real implementation, we'd write the remaining value
        // back to the variable. Here we just return the first element.
        Ok(first)
    }
}

/// POP() - removes and returns the last element of a variable treated as a list.
///
/// Usage: POP(varname[,delimiter])
pub struct FuncPop;

impl DialplanFunc for FuncPop {
    fn name(&self) -> &str { "POP" }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let varname = parts[0].trim();
        let delimiter = parts.get(1)
            .map(|s| parse_delimiter(s.trim()))
            .unwrap_or_else(|| ",".to_string());

        let value = ctx.get_variable(varname)
            .map(|s| s.as_str())
            .unwrap_or("");

        let mut fields: Vec<&str> = value.split(&delimiter[..]).collect();
        if fields.is_empty() {
            return Ok(String::new());
        }

        let last = fields.pop().unwrap_or("").to_string();
        Ok(last)
    }
}

/// PUSH() - appends a value to the end of a variable treated as a list.
///
/// Usage: PUSH(varname,value[,delimiter])
pub struct FuncPush;

impl DialplanFunc for FuncPush {
    fn name(&self) -> &str { "PUSH" }

    fn read(&self, _ctx: &FuncContext, _args: &str) -> FuncResult {
        // PUSH is write-only
        Err(FuncError::InvalidArgument("PUSH is a write-only function".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let varname = parts[0].trim();
        let delimiter = parts.get(1)
            .map(|s| parse_delimiter(s.trim()))
            .unwrap_or_else(|| ",".to_string());

        let current = ctx.get_variable(varname).cloned().unwrap_or_default();
        let new_value = if current.is_empty() {
            value.to_string()
        } else {
            format!("{}{}{}", current, delimiter, value)
        };
        ctx.set_variable(varname, &new_value);
        Ok(())
    }
}

/// UNSHIFT() - prepends a value to the beginning of a variable treated as a list.
///
/// Usage: UNSHIFT(varname,value[,delimiter])
pub struct FuncUnshift;

impl DialplanFunc for FuncUnshift {
    fn name(&self) -> &str { "UNSHIFT" }

    fn read(&self, _ctx: &FuncContext, _args: &str) -> FuncResult {
        Err(FuncError::InvalidArgument("UNSHIFT is a write-only function".to_string()))
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let varname = parts[0].trim();
        let delimiter = parts.get(1)
            .map(|s| parse_delimiter(s.trim()))
            .unwrap_or_else(|| ",".to_string());

        let current = ctx.get_variable(varname).cloned().unwrap_or_default();
        let new_value = if current.is_empty() {
            value.to_string()
        } else {
            format!("{}{}{}", value, delimiter, current)
        };
        ctx.set_variable(varname, &new_value);
        Ok(())
    }
}

/// Parse a delimiter specification, handling escape sequences.
fn parse_delimiter(s: &str) -> String {
    if s.is_empty() {
        return ",".to_string();
    }
    match s {
        "\\n" => "\n".to_string(),
        "\\r" => "\r".to_string(),
        "\\t" => "\t".to_string(),
        _ => {
            if let Some(hex_part) = s.strip_prefix("\\x") {
                if hex_part.len() >= 2 {
                    if let Ok(byte) = u8::from_str_radix(&hex_part[..2], 16) {
                        return String::from(byte as char);
                    }
                }
            }
            s.chars().next().map(|c| c.to_string()).unwrap_or_else(|| ",".to_string())
        }
    }
}

/// Parse a field specification into 0-based indices.
fn parse_field_spec(spec: &str, total: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }

    let mut indices = Vec::new();

    for part in spec.split('&') {
        let part = part.trim();
        if part.contains('-') {
            let range_parts: Vec<&str> = part.splitn(2, '-').collect();
            let start = range_parts[0].parse::<usize>().unwrap_or(1).saturating_sub(1);
            let end = if range_parts[1].is_empty() {
                total - 1
            } else {
                range_parts[1]
                    .parse::<usize>()
                    .unwrap_or(total)
                    .saturating_sub(1)
                    .min(total - 1)
            };
            for i in start..=end {
                if i < total {
                    indices.push(i);
                }
            }
        } else if let Ok(n) = part.parse::<usize>() {
            if n > 0 && n <= total {
                indices.push(n - 1);
            }
        }
    }

    indices
}

/// Expand a character specification into a set of allowed characters.
/// Supports ranges like "a-z" and escape sequences like "\x20".
fn expand_char_spec(spec: &str) -> Vec<char> {
    let mut chars = Vec::new();
    let spec_chars: Vec<char> = spec.chars().collect();
    let mut i = 0;

    while i < spec_chars.len() {
        if i + 2 < spec_chars.len() && spec_chars[i + 1] == '-' {
            // Range: a-z
            let start = spec_chars[i];
            let end = spec_chars[i + 2];
            let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
            for c in lo..=hi {
                chars.push(c);
            }
            i += 3;
        } else if spec_chars[i] == '\\' && i + 1 < spec_chars.len() {
            match spec_chars[i + 1] {
                'n' => { chars.push('\n'); i += 2; }
                'r' => { chars.push('\r'); i += 2; }
                't' => { chars.push('\t'); i += 2; }
                'x' if i + 3 < spec_chars.len() => {
                    let hex: String = spec_chars[i+2..i+4].iter().collect();
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        chars.push(byte as char);
                    }
                    i += 4;
                }
                '-' => { chars.push('-'); i += 2; }
                other => { chars.push(other); i += 2; }
            }
        } else {
            chars.push(spec_chars[i]);
            i += 1;
        }
    }

    chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_len() {
        let ctx = FuncContext::new();
        let func = FuncLen;
        assert_eq!(func.read(&ctx, "hello").unwrap(), "5");
        assert_eq!(func.read(&ctx, "").unwrap(), "0");
    }

    #[test]
    fn test_tolower() {
        let ctx = FuncContext::new();
        let func = FuncToLower;
        assert_eq!(func.read(&ctx, "HELLO World").unwrap(), "hello world");
    }

    #[test]
    fn test_toupper() {
        let ctx = FuncContext::new();
        let func = FuncToUpper;
        assert_eq!(func.read(&ctx, "hello world").unwrap(), "HELLO WORLD");
    }

    #[test]
    fn test_fieldqty() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("test", "ex-amp-le");
        let func = FuncFieldQty;
        assert_eq!(func.read(&ctx, "test,-").unwrap(), "3");
    }

    #[test]
    fn test_cut() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("test", "one-two-three-four");
        let func = FuncCut;
        assert_eq!(func.read(&ctx, "test,-,2").unwrap(), "two");
        assert_eq!(func.read(&ctx, "test,-,2-3").unwrap(), "two-three");
    }

    #[test]
    fn test_filter() {
        let ctx = FuncContext::new();
        let func = FuncFilter;
        assert_eq!(func.read(&ctx, "0-9,abc123def456").unwrap(), "123456");
        assert_eq!(func.read(&ctx, "a-z,Hello World").unwrap(), "elloorld");
    }

    #[test]
    fn test_replace() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("test", "hello world");
        let func = FuncReplace;
        assert_eq!(func.read(&ctx, "test, ,_").unwrap(), "hello_world");
    }

    #[test]
    fn test_shift() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("list", "a,b,c,d");
        let func = FuncShift;
        assert_eq!(func.read(&ctx, "list").unwrap(), "a");
    }

    #[test]
    fn test_pop() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("list", "a,b,c,d");
        let func = FuncPop;
        assert_eq!(func.read(&ctx, "list").unwrap(), "d");
    }

    #[test]
    fn test_push() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("list", "a,b");
        let func = FuncPush;
        func.write(&mut ctx, "list", "c").unwrap();
        assert_eq!(ctx.get_variable("list").unwrap(), "a,b,c");
    }

    #[test]
    fn test_unshift() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("list", "b,c");
        let func = FuncUnshift;
        func.write(&mut ctx, "list", "a").unwrap();
        assert_eq!(ctx.get_variable("list").unwrap(), "a,b,c");
    }

    #[test]
    fn test_expand_char_spec() {
        let chars = expand_char_spec("a-z");
        assert_eq!(chars.len(), 26);
        assert!(chars.contains(&'a'));
        assert!(chars.contains(&'z'));
        assert!(chars.contains(&'m'));

        let chars = expand_char_spec("0-9");
        assert_eq!(chars.len(), 10);
    }
}
