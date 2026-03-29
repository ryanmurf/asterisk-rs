//! JSON parsing/encoding functions.
//!
//! Port of func_json.c from Asterisk C.
//!
//! Provides:
//! - JSON_DECODE(varname,path[,separator[,options]]) - extract value from JSON
//! - JSON_ENCODE(var1,var2,...) - encode variables as JSON object
//!
//! Since we do not have a full JSON parser as a dependency, we implement
//! a minimal JSON parser sufficient for the dialplan function interface.
//! The JSON string is read from the context variable named by the first argument.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// Minimal JSON value representation.
#[derive(Debug, Clone)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Serialize to a JSON string.
    pub fn to_json_string(&self) -> String {
        match self {
            JsonValue::Null => "null".to_string(),
            JsonValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            JsonValue::Number(n) => {
                if *n == (*n as i64) as f64 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            JsonValue::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            JsonValue::Array(arr) => {
                let items: Vec<String> = arr.iter().map(|v| v.to_json_string()).collect();
                format!("[{}]", items.join(","))
            }
            JsonValue::Object(obj) => {
                let items: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("\"{}\":{}", k.replace('\\', "\\\\").replace('"', "\\\""), v.to_json_string()))
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
    }

    /// Get value as displayable string (without quotes for strings).
    pub fn display_value(&self) -> String {
        match self {
            JsonValue::Null => String::new(),
            JsonValue::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            JsonValue::Number(n) => {
                if *n == (*n as i64) as f64 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    // Format with enough precision, trim trailing zeros
                    let s = format!("{}", n);
                    s
                }
            }
            JsonValue::Str(s) => s.clone(),
            JsonValue::Array(_) | JsonValue::Object(_) => self.to_json_string(),
        }
    }

    /// Navigate to a nested value by dot-separated (or custom separator) path.
    pub fn get_path(&self, path: &str, sep: &str) -> Option<&JsonValue> {
        if path.is_empty() {
            return Some(self);
        }

        let (key, rest) = if let Some(pos) = path.find(sep) {
            (&path[..pos], &path[pos + sep.len()..])
        } else {
            (path, "")
        };

        match self {
            JsonValue::Object(obj) => {
                for (k, v) in obj {
                    if k == key {
                        if rest.is_empty() {
                            return Some(v);
                        }
                        return v.get_path(rest, sep);
                    }
                }
                None
            }
            JsonValue::Array(arr) => {
                if let Ok(index) = key.parse::<usize>() {
                    if let Some(v) = arr.get(index) {
                        if rest.is_empty() {
                            return Some(v);
                        }
                        return v.get_path(rest, sep);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Get array length if this is an array, None otherwise.
    pub fn array_len(&self) -> Option<usize> {
        match self {
            JsonValue::Array(arr) => Some(arr.len()),
            _ => None,
        }
    }
}

/// Minimal JSON parser.
pub struct JsonParser;

impl JsonParser {
    pub fn parse(input: &str) -> Result<JsonValue, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("empty input".to_string());
        }
        let (val, _) = Self::parse_value(input)?;
        Ok(val)
    }

    fn parse_value(input: &str) -> Result<(JsonValue, &str), String> {
        let input = Self::skip_ws(input);
        if input.is_empty() {
            return Err("unexpected end of input".to_string());
        }

        match input.as_bytes()[0] {
            b'"' => Self::parse_string(input),
            b'{' => Self::parse_object(input),
            b'[' => Self::parse_array(input),
            b't' | b'f' => Self::parse_bool(input),
            b'n' => Self::parse_null(input),
            _ => Self::parse_number(input),
        }
    }

    fn skip_ws(input: &str) -> &str {
        input.trim_start()
    }

    fn parse_string(input: &str) -> Result<(JsonValue, &str), String> {
        if !input.starts_with('"') {
            return Err("expected '\"'".to_string());
        }
        let bytes = input.as_bytes();
        let mut i = 1;
        let mut s = String::new();
        while i < bytes.len() {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'"' => { s.push('"'); i += 2; }
                    b'\\' => { s.push('\\'); i += 2; }
                    b'/' => { s.push('/'); i += 2; }
                    b'n' => { s.push('\n'); i += 2; }
                    b'r' => { s.push('\r'); i += 2; }
                    b't' => { s.push('\t'); i += 2; }
                    _ => { s.push(bytes[i] as char); i += 1; }
                }
            } else if bytes[i] == b'"' {
                return Ok((JsonValue::Str(s), &input[i + 1..]));
            } else {
                s.push(bytes[i] as char);
                i += 1;
            }
        }
        Err("unterminated string".to_string())
    }

    fn parse_number(input: &str) -> Result<(JsonValue, &str), String> {
        let mut end = 0;
        let bytes = input.as_bytes();
        if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
            end += 1;
        }
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end < bytes.len() && bytes[end] == b'.' {
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
            end += 1;
            if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                end += 1;
            }
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        if end == 0 {
            return Err(format!("expected number at '{}'", &input[..input.len().min(20)]));
        }
        let num_str = &input[..end];
        let num: f64 = num_str
            .parse()
            .map_err(|_| format!("invalid number: {}", num_str))?;
        Ok((JsonValue::Number(num), &input[end..]))
    }

    fn parse_bool(input: &str) -> Result<(JsonValue, &str), String> {
        if input.starts_with("true") {
            Ok((JsonValue::Bool(true), &input[4..]))
        } else if input.starts_with("false") {
            Ok((JsonValue::Bool(false), &input[5..]))
        } else {
            Err("expected boolean".to_string())
        }
    }

    fn parse_null(input: &str) -> Result<(JsonValue, &str), String> {
        if input.starts_with("null") {
            Ok((JsonValue::Null, &input[4..]))
        } else {
            Err("expected null".to_string())
        }
    }

    fn parse_array(input: &str) -> Result<(JsonValue, &str), String> {
        let mut rest = Self::skip_ws(&input[1..]);
        let mut items = Vec::new();

        if rest.starts_with(']') {
            return Ok((JsonValue::Array(items), &rest[1..]));
        }

        loop {
            let (val, r) = Self::parse_value(rest)?;
            items.push(val);
            rest = Self::skip_ws(r);
            if rest.starts_with(']') {
                return Ok((JsonValue::Array(items), &rest[1..]));
            }
            if !rest.starts_with(',') {
                return Err("expected ',' or ']' in array".to_string());
            }
            rest = Self::skip_ws(&rest[1..]);
        }
    }

    fn parse_object(input: &str) -> Result<(JsonValue, &str), String> {
        let mut rest = Self::skip_ws(&input[1..]);
        let mut entries = Vec::new();

        if rest.starts_with('}') {
            return Ok((JsonValue::Object(entries), &rest[1..]));
        }

        loop {
            // Parse key
            let (key_val, r) = Self::parse_string(rest)?;
            let key = match key_val {
                JsonValue::Str(s) => s,
                _ => return Err("expected string key".to_string()),
            };
            rest = Self::skip_ws(r);
            if !rest.starts_with(':') {
                return Err("expected ':'".to_string());
            }
            rest = Self::skip_ws(&rest[1..]);

            // Parse value
            let (val, r) = Self::parse_value(rest)?;
            entries.push((key, val));
            rest = Self::skip_ws(r);

            if rest.starts_with('}') {
                return Ok((JsonValue::Object(entries), &rest[1..]));
            }
            if !rest.starts_with(',') {
                return Err("expected ',' or '}' in object".to_string());
            }
            rest = Self::skip_ws(&rest[1..]);
        }
    }
}

/// JSON_DECODE() function.
///
/// Extracts a value from a JSON string using a dot-notation path.
///
/// Usage: JSON_DECODE(varname,key[,separator[,options]])
///
/// In this port, `varname` is the name of a context variable containing JSON.
/// `key` is a path like "path.to.key" or "array.0.field".
/// `separator` defaults to "." but can be changed (e.g., "/").
/// `options`: "c" to return array count instead of value.
pub struct FuncJsonDecode;

impl DialplanFunc for FuncJsonDecode {
    fn name(&self) -> &str {
        "JSON_DECODE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(4, ',').collect();
        if parts.len() < 2 {
            return Err(FuncError::InvalidArgument(
                "JSON_DECODE: requires varname,key arguments".to_string(),
            ));
        }

        let varname = parts[0].trim();
        let key = parts[1].trim();
        let mut separator = ".";
        let mut count_mode = false;

        if parts.len() > 2 {
            let opt_str = parts[2].trim();
            // Options may contain separator and/or 'c' flag
            if opt_str.contains('c') {
                count_mode = true;
            }
            // If there's a single non-c character, that's the separator
            let non_c: String = opt_str.chars().filter(|&c| c != 'c').collect();
            if non_c.len() == 1 {
                // Separator is from the original arg, extract it
                // We need to keep the reference valid
            }
        }
        if parts.len() > 2 {
            let sep_str = parts[2].trim();
            let sep_char: String = sep_str.chars().filter(|&c| c != 'c').collect();
            if sep_char.len() == 1 {
                // Use a leaked string for lifetime (safe in this context as it's one char)
                // Actually, just match on it
            }
        }

        // Re-parse separator properly
        if parts.len() > 2 {
            let raw = parts[2].trim();
            if raw.contains('c') {
                count_mode = true;
            }
            let sep_chars: Vec<char> = raw.chars().filter(|&c| c != 'c').collect();
            if sep_chars.len() == 1 {
                separator = match sep_chars[0] {
                    '/' => "/",
                    '.' => ".",
                    '-' => "-",
                    '_' => "_",
                    ':' => ":",
                    '|' => "|",
                    _ => ".",
                };
            }
        }
        // Check 4th arg for options too
        if parts.len() > 3 {
            let opts = parts[3].trim();
            if opts.contains('c') {
                count_mode = true;
            }
        }

        if varname.is_empty() {
            return Err(FuncError::InvalidArgument(
                "JSON_DECODE: varname is required".to_string(),
            ));
        }
        if key.is_empty() {
            return Ok(String::new());
        }

        // Get JSON from variable
        let json_str = ctx.get_variable(varname).cloned().unwrap_or_default();
        if json_str.is_empty() {
            return Ok(String::new());
        }

        let json = JsonParser::parse(&json_str).map_err(|e| {
            FuncError::Internal(format!("JSON_DECODE: failed to parse JSON: {}", e))
        })?;

        // Navigate to the path
        match json.get_path(key, separator) {
            Some(val) => {
                if count_mode {
                    if let Some(len) = val.array_len() {
                        return Ok(len.to_string());
                    }
                }
                Ok(val.display_value())
            }
            None => Ok(String::new()),
        }
    }
}

/// JSON_ENCODE() function.
///
/// Encodes context variables as a JSON object.
///
/// Usage: JSON_ENCODE(var1,var2,...) -> {"var1":"value1","var2":"value2"}
///
/// Each argument is a variable name. The function reads the values
/// from the context and produces a JSON object.
pub struct FuncJsonEncode;

impl DialplanFunc for FuncJsonEncode {
    fn name(&self) -> &str {
        "JSON_ENCODE"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let varnames: Vec<&str> = args.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if varnames.is_empty() {
            return Err(FuncError::InvalidArgument(
                "JSON_ENCODE: at least one variable name is required".to_string(),
            ));
        }

        let mut entries = Vec::new();
        for name in varnames {
            let value = ctx.get_variable(name).cloned().unwrap_or_default();
            entries.push((name.to_string(), JsonValue::Str(value)));
        }

        let obj = JsonValue::Object(entries);
        Ok(obj.to_json_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_parse_string() {
        let val = JsonParser::parse(r#""hello""#).unwrap();
        assert_eq!(val.display_value(), "hello");
    }

    #[test]
    fn test_json_parse_number() {
        let val = JsonParser::parse("42").unwrap();
        assert_eq!(val.display_value(), "42");
    }

    #[test]
    fn test_json_parse_float() {
        let val = JsonParser::parse("3.14").unwrap();
        assert!(val.display_value().starts_with("3.14"));
    }

    #[test]
    fn test_json_parse_bool() {
        let val = JsonParser::parse("true").unwrap();
        assert_eq!(val.display_value(), "1");
        let val = JsonParser::parse("false").unwrap();
        assert_eq!(val.display_value(), "0");
    }

    #[test]
    fn test_json_parse_null() {
        let val = JsonParser::parse("null").unwrap();
        assert_eq!(val.display_value(), "");
    }

    #[test]
    fn test_json_parse_object() {
        let val = JsonParser::parse(r#"{"city":"Anytown","state":"USA"}"#).unwrap();
        assert_eq!(val.get_path("city", ".").unwrap().display_value(), "Anytown");
        assert_eq!(val.get_path("state", ".").unwrap().display_value(), "USA");
    }

    #[test]
    fn test_json_parse_nested() {
        let val = JsonParser::parse(r#"{"path":{"to":{"elem":"someVar"}}}"#).unwrap();
        assert_eq!(
            val.get_path("path.to.elem", ".").unwrap().display_value(),
            "someVar"
        );
    }

    #[test]
    fn test_json_parse_array() {
        let val = JsonParser::parse(r#"[0, 1, 2]"#).unwrap();
        assert_eq!(val.get_path("0", ".").unwrap().display_value(), "0");
        assert_eq!(val.get_path("1", ".").unwrap().display_value(), "1");
    }

    #[test]
    fn test_json_array_count() {
        let val = JsonParser::parse(r#"[1, 2, 3]"#).unwrap();
        assert_eq!(val.array_len(), Some(3));
    }

    #[test]
    fn test_json_decode_function() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("mydata", r#"{"city":"Anytown","state":"USA"}"#);
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "mydata,city").unwrap(), "Anytown");
        assert_eq!(func.read(&ctx, "mydata,state").unwrap(), "USA");
        assert_eq!(func.read(&ctx, "mydata,blah").unwrap(), "");
    }

    #[test]
    fn test_json_decode_nested_with_separator() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("data", r#"{"path":{"to":{"elem":"val"}}}"#);
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "data,path/to/elem,/").unwrap(), "val");
    }

    #[test]
    fn test_json_decode_integer() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("data", r#"{"key1":123}"#);
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "data,key1").unwrap(), "123");
    }

    #[test]
    fn test_json_decode_boolean() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("data", r#"{"myboolean":true}"#);
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "data,myboolean").unwrap(), "1");
    }

    #[test]
    fn test_json_encode() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("name", "Alice");
        ctx.set_variable("age", "30");
        let func = FuncJsonEncode;
        let result = func.read(&ctx, "name,age").unwrap();
        assert!(result.contains("\"name\":\"Alice\""));
        assert!(result.contains("\"age\":\"30\""));
    }

    #[test]
    fn test_json_decode_empty_var() {
        let ctx = FuncContext::new();
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "nosuchvar,key").unwrap(), "");
    }

    #[test]
    fn test_json_nested_array() {
        let mut ctx = FuncContext::new();
        ctx.set_variable("data", r#"{"arr":["item0","item1"]}"#);
        let func = FuncJsonDecode;
        assert_eq!(func.read(&ctx, "data,arr.1").unwrap(), "item1");
    }
}
