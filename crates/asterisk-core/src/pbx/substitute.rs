//! Variable substitution engine for Asterisk dialplan.
//!
//! This implements `${varname}`, `${varname:offset:length}`, `${FUNC(args)}`,
//! `${FUNC(args):offset:length}`, nested `${${varname}}`, and `$[expr]` expression
//! substitution, mirroring the C `pbx_substitute_variables_helper_full` and
//! `ast_str_substitute_variables_full2` functions.

use crate::channel::Channel;
use crate::pbx::expression::evaluate_expression;
use crate::pbx::func_registry::FUNC_REGISTRY;
use std::collections::HashMap;

/// Maximum recursion depth for variable substitution (prevents infinite loops).
const MAX_SUBSTITUTE_DEPTH: usize = 15;

/// Substitute variables and expressions in a string, using a channel for lookups.
///
/// This resolves:
/// - `${varname}` -- channel variable, then global variable lookup
/// - `${varname:offset:length}` -- substring of a variable
/// - `${FUNC(args)}` -- dialplan function read
/// - `${FUNC(args):offset:length}` -- function call with substring
/// - `${${varname}}` -- nested substitution (inner resolved first)
/// - `$[expr]` -- expression evaluation
pub fn substitute_variables(channel: &Channel, input: &str) -> String {
    substitute_variables_full(Some(channel), None, input)
}

/// Substitute variables with optional channel and/or external variable map.
///
/// This is the full substitution function that can work with or without a channel.
/// When `headp` is provided, it is searched as a secondary variable source.
pub fn substitute_variables_full(
    channel: Option<&Channel>,
    headp: Option<&HashMap<String, String>>,
    input: &str,
) -> String {
    substitute_recursive(channel, headp, input, 0)
}

/// Internal recursive substitution with depth tracking.
fn substitute_recursive(
    channel: Option<&Channel>,
    headp: Option<&HashMap<String, String>>,
    input: &str,
    depth: usize,
) -> String {
    if depth >= MAX_SUBSTITUTE_DEPTH {
        tracing::error!(
            "Exceeded maximum variable substitution recursion depth ({}) - possible infinite recursion in dialplan?",
            MAX_SUBSTITUTE_DEPTH
        );
        return String::new();
    }

    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '$' && i + 1 < len {
            match chars[i + 1] {
                '{' => {
                    // Variable substitution: ${...}
                    let start = i + 2;
                    if let Some((content, end_pos)) = find_matching_brace(&chars, start) {
                        // Check if the content contains nested ${ or $[ that need substitution
                        let needs_sub = content.contains("${") || content.contains("$[");

                        let resolved_content = if needs_sub {
                            substitute_recursive(channel, headp, &content, depth + 1)
                        } else {
                            content
                        };

                        // Parse variable name with optional :offset:length
                        let (var_name, offset, length) =
                            parse_variable_name_with_substring(&resolved_content);

                        // Look up the value
                        let value = lookup_variable(channel, headp, &var_name);

                        // Apply substring if needed
                        let final_value = if offset.is_some() || length.is_some() {
                            apply_substring(
                                &value,
                                offset.unwrap_or(0),
                                length.unwrap_or(i32::MAX),
                            )
                        } else {
                            value
                        };

                        result.push_str(&final_value);
                        i = end_pos + 1; // skip past closing '}'
                    } else {
                        // Unmatched '${' -- copy literally
                        tracing::warn!("Error in extension logic (missing '}}')");
                        result.push('$');
                        result.push('{');
                        i += 2;
                    }
                }
                '[' => {
                    // Expression substitution: $[...]
                    let start = i + 2;
                    if let Some((content, end_pos)) = find_matching_bracket(&chars, start) {
                        // First substitute variables within the expression
                        let needs_sub = content.contains("${") || content.contains("$[");
                        let resolved = if needs_sub {
                            substitute_recursive(channel, headp, &content, depth + 1)
                        } else {
                            content
                        };

                        // Evaluate the expression
                        match evaluate_expression(&resolved) {
                            Ok(expr_result) => result.push_str(&expr_result),
                            Err(e) => {
                                tracing::warn!("Expression evaluation error: {}", e);
                                result.push('0');
                            }
                        }

                        i = end_pos + 1; // skip past closing ']'
                    } else {
                        tracing::warn!("Error in extension logic (missing ']')");
                        result.push('$');
                        result.push('[');
                        i += 2;
                    }
                }
                '$' => {
                    // Escaped $$ -> literal $
                    result.push('$');
                    i += 2;
                }
                _ => {
                    // '$' not followed by '{' or '[' -- just a literal '$'
                    result.push('$');
                    i += 1;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Find the matching closing '}' for a '${...}' construct, handling nesting.
/// Returns the content between braces and the position of the closing '}'.
///
/// Only `${` sequences count as nested brace levels.  Standalone `{`
/// characters do NOT increment the brace depth -- they are literal
/// characters (e.g. in function arguments or regex patterns).
fn find_matching_brace(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut brackets = 1;
    let mut pos = start;

    while pos < chars.len() && brackets > 0 {
        if pos + 1 < chars.len() && chars[pos] == '$' && chars[pos + 1] == '{' {
            brackets += 1;
            pos += 2; // skip both '$' and '{'
            continue;
        } else if pos + 1 < chars.len() && chars[pos] == '$' && chars[pos + 1] == '[' {
            pos += 2; // skip both '$' and '['
            continue;
        } else if chars[pos] == '}' {
            brackets -= 1;
            if brackets == 0 {
                let content: String = chars[start..pos].iter().collect();
                return Some((content, pos));
            }
        }
        // NOTE: standalone '{' is NOT counted as a brace level.
        // Only '${' opens a nested variable reference.
        pos += 1;
    }

    None
}

/// Find the matching closing ']' for a '$[...]' construct, handling nesting.
///
/// Only `$[` sequences count as nested expression brackets.  Standalone
/// `[` characters (e.g. inside regex character classes like `[0-9]`) do
/// NOT increment the bracket depth -- they are literal characters in the
/// expression and will be handled by the expression parser.
fn find_matching_bracket(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut brackets = 1;
    let mut pos = start;
    let mut in_quotes = false;

    while pos < chars.len() && brackets > 0 {
        let ch = chars[pos];

        // Track double-quoted strings so brackets inside quotes are literal.
        if ch == '"' {
            in_quotes = !in_quotes;
            pos += 1;
            continue;
        }

        if in_quotes {
            pos += 1;
            continue;
        }

        if pos + 1 < chars.len() && ch == '$' && chars[pos + 1] == '[' {
            brackets += 1;
            pos += 2; // skip both '$' and '['
            continue;
        } else if pos + 1 < chars.len() && ch == '$' && chars[pos + 1] == '{' {
            pos += 2; // skip both '$' and '{'
            continue;
        } else if ch == ']' {
            brackets -= 1;
            if brackets == 0 {
                let content: String = chars[start..pos].iter().collect();
                return Some((content, pos));
            }
        }
        // NOTE: standalone '[' is NOT counted as a bracket level.
        // Only '$[' opens a nested expression bracket.
        pos += 1;
    }

    None
}

/// Parse a variable name, extracting optional `:offset:length` suffix.
///
/// Handles both plain variables like `FOO` and functions like `FUNC(args)`.
/// For functions, the `:offset:length` is only parsed outside the parentheses.
///
/// Returns `(name, Option<offset>, Option<length>)`.
fn parse_variable_name_with_substring(input: &str) -> (String, Option<i32>, Option<i32>) {
    let mut parens = 0;
    let mut colon_pos = None;

    for (i, ch) in input.char_indices() {
        match ch {
            '(' => parens += 1,
            ')' => {
                if parens > 0 {
                    parens -= 1;
                }
            }
            ':' if parens == 0 => {
                colon_pos = Some(i);
                break;
            }
            _ => {}
        }
    }

    if let Some(cp) = colon_pos {
        let var_name = input[..cp].to_string();
        let substr_spec = &input[cp + 1..];

        // Parse offset:length
        let parts: Vec<&str> = substr_spec.splitn(2, ':').collect();
        let offset = parts.first().and_then(|s| s.parse::<i32>().ok());
        let length = parts.get(1).and_then(|s| s.parse::<i32>().ok());

        (var_name, offset, length)
    } else {
        (input.to_string(), None, None)
    }
}

/// Check if a variable name is a function call (contains parentheses).
fn is_function_call(name: &str) -> bool {
    let mut depth = 0;
    for ch in name.chars() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
        }
    }
    // A function call has at least one '(' and the parens are balanced
    name.contains('(') && depth == 0
}

/// Split a function-call string into name and args: "FUNC(args)" -> ("FUNC", "args").
fn split_function_call(name: &str) -> Option<(&str, &str)> {
    let paren_pos = name.find('(')?;
    let func_name = &name[..paren_pos];
    // Strip the trailing ')'
    let rest = &name[paren_pos + 1..];
    let args = rest.strip_suffix(')')?;
    Some((func_name, args))
}

/// Look up a variable or function value.
///
/// Order of lookup:
/// 1. Built-in channel variables (EXTEN, CONTEXT, PRIORITY, CHANNEL, UNIQUEID, CALLERID(...), etc.)
/// 2. Channel variables (from channel->variables map)
/// 3. Global/external variables (from headp)
/// 4. If the name looks like a function call `FUNC(args)`, try the function registry
fn lookup_variable(
    channel: Option<&Channel>,
    headp: Option<&HashMap<String, String>>,
    name: &str,
) -> String {
    // Built-in channel variables (checked first, before function registry)
    if let Some(ch) = channel {
        match name {
            "EXTEN" => return ch.exten.clone(),
            "CONTEXT" => return ch.context.clone(),
            "PRIORITY" => return ch.priority.to_string(),
            "CHANNEL" => return ch.name.clone(),
            "UNIQUEID" => return ch.unique_id.as_str().to_string(),
            "HANGUPCAUSE" => return format!("{}", ch.hangup_cause as u32),
            "CALLERID(num)" | "CALLERID(number)" => {
                return ch.caller.id.number.number.clone();
            }
            "CALLERID(name)" => {
                return ch.caller.id.name.name.clone();
            }
            "CALLERID(all)" => {
                let cid_name = &ch.caller.id.name.name;
                let num = &ch.caller.id.number.number;
                if cid_name.is_empty() {
                    return num.clone();
                }
                return format!("\"{}\" <{}>", cid_name, num);
            }
            _ => {}
        }

        // Channel variables
        if let Some(val) = ch.variables.get(name) {
            return val.clone();
        }
    }

    // External variable map (globals)
    if let Some(vars) = headp {
        if let Some(val) = vars.get(name) {
            return val.clone();
        }
    }

    // If the name looks like a function call, try the function registry
    if is_function_call(name) {
        if let Some((func_name, args)) = split_function_call(name) {
            // Try the function registry
            if let Some(func) = FUNC_REGISTRY.find(func_name) {
                if let Some(ch) = channel {
                    match try_call_function_sync(&*func, ch, args) {
                        Some(val) => return val,
                        None => {
                            tracing::debug!("Function {} returned no result", func_name);
                        }
                    }
                } else {
                    tracing::debug!(
                        "Function {} called without a channel context",
                        func_name
                    );
                }
            } else {
                tracing::debug!("Unknown function: {}", func_name);
            }
        }
        return String::new();
    }

    // Not found -- return empty string
    String::new()
}

/// Try to call a dialplan function synchronously.
///
/// This creates a small tokio runtime block for the async call. In production,
/// the PBX exec loop is already async, so this is mainly for variable substitution
/// happening in a sync context.
fn try_call_function_sync(
    func: &dyn crate::pbx::DialplanFunction,
    channel: &Channel,
    args: &str,
) -> Option<String> {
    // If we're already inside a tokio runtime, use block_in_place + block_on
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let result = tokio::task::block_in_place(|| handle.block_on(func.read(channel, args)));
        match result {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::debug!("Function {} error: {}", func.name(), e);
                None
            }
        }
    } else {
        // No runtime -- try creating a temporary one
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .ok()?;
        match rt.block_on(func.read(channel, args)) {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::debug!("Function {} error: {}", func.name(), e);
                None
            }
        }
    }
}

/// Apply substring operation on a value, mimicking the C `substring()` function.
///
/// - `offset` can be negative (count from end).
/// - `length` can be negative (leave that many off the end).
/// - `length` of `i32::MAX` means "take the rest".
fn apply_substring(value: &str, offset: i32, length: i32) -> String {
    let lr = value.len() as i32;

    // Quick check: if no modification needed
    if offset == 0 && length >= lr {
        return value.to_string();
    }

    // Translate negative offset
    let offset = if offset < 0 {
        let o = lr + offset;
        if o < 0 { 0 } else { o }
    } else {
        offset
    };

    // Too large offset -> empty string
    if offset >= lr {
        return String::new();
    }

    let start = offset as usize;
    let remaining = lr - offset;

    if length >= 0 && length < remaining {
        // Truncate to length
        let end = start + length as usize;
        value[start..end].to_string()
    } else if length < 0 {
        // Negative length means leave that many off the end
        let effective_len = remaining + length;
        if effective_len > 0 {
            let end = start + effective_len as usize;
            value[start..end].to_string()
        } else {
            String::new()
        }
    } else {
        // Take the rest
        value[start..].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_substitution() {
        assert_eq!(substitute_variables_full(None, None, "hello world"), "hello world");
    }

    #[test]
    fn test_simple_channel_variable() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("FOO", "bar");
        assert_eq!(substitute_variables(&ch, "${FOO}"), "bar");
    }

    #[test]
    fn test_missing_variable() {
        let ch = Channel::new("Test/1");
        assert_eq!(substitute_variables(&ch, "${MISSING}"), "");
    }

    #[test]
    fn test_builtin_variables() {
        let mut ch = Channel::new("SIP/alice-001");
        ch.context = "mycontext".to_string();
        ch.exten = "100".to_string();
        ch.priority = 3;

        assert_eq!(substitute_variables(&ch, "${EXTEN}"), "100");
        assert_eq!(substitute_variables(&ch, "${CONTEXT}"), "mycontext");
        assert_eq!(substitute_variables(&ch, "${PRIORITY}"), "3");
        assert_eq!(substitute_variables(&ch, "${CHANNEL}"), "SIP/alice-001");
    }

    #[test]
    fn test_mixed_text_and_variables() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("NAME", "world");
        assert_eq!(
            substitute_variables(&ch, "hello ${NAME}!"),
            "hello world!"
        );
    }

    #[test]
    fn test_multiple_variables() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("A", "1");
        ch.set_variable("B", "2");
        assert_eq!(substitute_variables(&ch, "${A} + ${B}"), "1 + 2");
    }

    #[test]
    fn test_nested_substitution() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("VARNAME", "TARGET");
        ch.set_variable("TARGET", "success");
        assert_eq!(substitute_variables(&ch, "${${VARNAME}}"), "success");
    }

    #[test]
    fn test_substring_offset_length() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("STR", "abcdefgh");

        // offset 2, length 3 -> "cde"
        assert_eq!(substitute_variables(&ch, "${STR:2:3}"), "cde");

        // offset 0, length 3 -> "abc"
        assert_eq!(substitute_variables(&ch, "${STR:0:3}"), "abc");

        // negative offset -> from end
        assert_eq!(substitute_variables(&ch, "${STR:-3:3}"), "fgh");
    }

    #[test]
    fn test_substring_negative_length() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("STR", "abcdefgh");

        // offset 0, length -2 -> remove last 2 chars -> "abcdef"
        assert_eq!(substitute_variables(&ch, "${STR:0:-2}"), "abcdef");
    }

    #[test]
    fn test_expression_substitution() {
        let ch = Channel::new("Test/1");
        assert_eq!(substitute_variables(&ch, "$[1 + 2]"), "3");
    }

    #[test]
    fn test_expression_with_variable() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("COUNT", "7");
        assert_eq!(substitute_variables(&ch, "$[${COUNT} > 5]"), "1");
        assert_eq!(substitute_variables(&ch, "$[${COUNT} > 10]"), "0");
    }

    #[test]
    fn test_dollar_dollar_escape() {
        let ch = Channel::new("Test/1");
        assert_eq!(substitute_variables(&ch, "$$"), "$");
    }

    #[test]
    fn test_dollar_not_substitution() {
        let ch = Channel::new("Test/1");
        assert_eq!(substitute_variables(&ch, "$5.00"), "$5.00");
    }

    #[test]
    fn test_headp_variables() {
        let mut vars = HashMap::new();
        vars.insert("GLOBAL_VAR".to_string(), "global_value".to_string());
        assert_eq!(
            substitute_variables_full(None, Some(&vars), "${GLOBAL_VAR}"),
            "global_value"
        );
    }

    #[test]
    fn test_channel_overrides_headp() {
        let mut ch = Channel::new("Test/1");
        ch.set_variable("VAR", "channel_value");
        let mut globals = HashMap::new();
        globals.insert("VAR".to_string(), "global_value".to_string());
        assert_eq!(
            substitute_variables_full(Some(&ch), Some(&globals), "${VAR}"),
            "channel_value"
        );
    }

    #[test]
    fn test_apply_substring() {
        assert_eq!(apply_substring("abcdefgh", 0, i32::MAX), "abcdefgh");
        assert_eq!(apply_substring("abcdefgh", 2, 3), "cde");
        assert_eq!(apply_substring("abcdefgh", -3, 3), "fgh");
        assert_eq!(apply_substring("abcdefgh", 0, -2), "abcdef");
        assert_eq!(apply_substring("abcdefgh", 2, -2), "cdef");
        assert_eq!(apply_substring("abcdefgh", 100, 3), "");
    }

    #[test]
    fn test_parse_variable_name_with_substring() {
        let (name, offset, length) = parse_variable_name_with_substring("FOO");
        assert_eq!(name, "FOO");
        assert!(offset.is_none());
        assert!(length.is_none());

        let (name, offset, length) = parse_variable_name_with_substring("FOO:2:3");
        assert_eq!(name, "FOO");
        assert_eq!(offset, Some(2));
        assert_eq!(length, Some(3));

        let (name, offset, length) = parse_variable_name_with_substring("FUNC(arg1,arg2):0:5");
        assert_eq!(name, "FUNC(arg1,arg2)");
        assert_eq!(offset, Some(0));
        assert_eq!(length, Some(5));
    }

    #[test]
    fn test_unmatched_brace() {
        let ch = Channel::new("Test/1");
        // Should not panic; just copy literally
        let result = substitute_variables(&ch, "${MISSING");
        assert!(result.contains("${MISSING"));
    }

    #[test]
    fn test_callerid_builtins() {
        let mut ch = Channel::new("Test/1");
        ch.caller.id.number.number = "5551234".to_string();
        ch.caller.id.name.name = "Alice".to_string();

        assert_eq!(substitute_variables(&ch, "${CALLERID(num)}"), "5551234");
        assert_eq!(substitute_variables(&ch, "${CALLERID(name)}"), "Alice");
        assert_eq!(
            substitute_variables(&ch, "${CALLERID(all)}"),
            "\"Alice\" <5551234>"
        );
    }
}
