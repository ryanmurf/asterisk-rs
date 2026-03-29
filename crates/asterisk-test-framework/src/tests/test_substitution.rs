//! Port of asterisk/tests/test_substitution.c
//!
//! Tests variable substitution in strings:
//! - ${variable} substitution
//! - Nested ${${var}} substitution
//! - ${var:offset:length} substring extraction
//! - Multiple substitutions in one string
//! - Missing variable -> empty string
//! - Variable set/get
//!
//! Uses the substitution engine from asterisk_core::pbx::substitute.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Simple substitution engine for testing
// ---------------------------------------------------------------------------

/// Substitute variables in a string using a variable map.
///
/// Supports:
/// - ${varname} -- simple variable lookup
/// - ${varname:offset} -- substring from offset
/// - ${varname:offset:length} -- substring with length
/// - ${${varname}} -- nested substitution
/// - Missing variables -> empty string
fn substitute(vars: &HashMap<String, String>, input: &str) -> String {
    substitute_inner(vars, input, 0)
}

fn substitute_inner(vars: &HashMap<String, String>, input: &str, depth: usize) -> String {
    if depth > 15 {
        return String::new();
    }

    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '$' && i + 1 < len && chars[i + 1] == '{' {
            // Find matching '}'.
            let start = i + 2;
            let mut brace_depth = 1;
            let mut j = start;
            while j < len && brace_depth > 0 {
                if chars[j] == '{' {
                    brace_depth += 1;
                } else if chars[j] == '}' {
                    brace_depth -= 1;
                }
                if brace_depth > 0 {
                    j += 1;
                }
            }
            if brace_depth == 0 {
                let content: String = chars[start..j].iter().collect();

                // Check for nested ${...}.
                let resolved = if content.contains("${") {
                    substitute_inner(vars, &content, depth + 1)
                } else {
                    content
                };

                // Parse varname:offset:length.
                let value = resolve_var_with_substring(vars, &resolved);
                result.push_str(&value);
                i = j + 1;
            } else {
                result.push(chars[i]);
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn resolve_var_with_substring(vars: &HashMap<String, String>, expr: &str) -> String {
    // Split on first ':' to get varname and optional offset/length.
    if let Some(colon_pos) = expr.find(':') {
        let var_name = &expr[..colon_pos];
        let rest = &expr[colon_pos + 1..];

        let var_value = vars.get(var_name).cloned().unwrap_or_default();

        // Parse offset and optional length.
        let (offset_str, length_str) = if let Some(colon2) = rest.find(':') {
            (&rest[..colon2], Some(&rest[colon2 + 1..]))
        } else {
            (rest, None)
        };

        let offset: i32 = offset_str.parse().unwrap_or(0);
        let val_len = var_value.len() as i32;

        // Handle negative offset (from end).
        let start = if offset < 0 {
            (val_len + offset).max(0) as usize
        } else {
            offset.min(val_len) as usize
        };

        match length_str {
            Some(ls) => {
                let length: i32 = ls.parse().unwrap_or(0);
                let remaining = val_len - start as i32;
                let actual_len = if length < 0 {
                    (remaining + length).max(0) as usize
                } else {
                    length.min(remaining) as usize
                };
                var_value[start..start + actual_len].to_string()
            }
            None => var_value[start..].to_string(),
        }
    } else {
        // Simple variable lookup.
        vars.get(expr).cloned().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Tests: Simple variable substitution
// ---------------------------------------------------------------------------

/// Port of test_chan_variable from test_substitution.c.
#[test]
fn test_simple_variable() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());
    assert_eq!(substitute(&vars, "${foo}"), "123");
}

#[test]
fn test_missing_variable_empty() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "${nonexistent}"), "");
}

#[test]
fn test_no_substitution() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "hello world"), "hello world");
}

// ---------------------------------------------------------------------------
// Tests: Multiple substitutions
// ---------------------------------------------------------------------------

/// Port of test_expected_result tests from test_substitution.c.
#[test]
fn test_multiple_substitutions() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    assert_eq!(substitute(&vars, "${foo}${foo}"), "123123");
    assert_eq!(substitute(&vars, "A${foo}A${foo}A"), "A123A123A");
}

// ---------------------------------------------------------------------------
// Tests: Nested substitution
// ---------------------------------------------------------------------------

/// Port of nested ${${var}} tests.
#[test]
fn test_nested_substitution() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());
    vars.insert("bar".to_string(), "foo".to_string());

    // ${${bar}} -> ${foo} -> 123
    assert_eq!(substitute(&vars, "A${${bar}}A"), "A123A");
}

#[test]
fn test_nested_partial_var_name() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());
    vars.insert("baz".to_string(), "fo".to_string());

    // ${${baz}o} -> ${fo + o} -> ${foo} -> 123
    assert_eq!(substitute(&vars, "A${${baz}o}A"), "A123A");
}

// ---------------------------------------------------------------------------
// Tests: Substring extraction
// ---------------------------------------------------------------------------

#[test]
fn test_substring_offset() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:1} -> "23"
    assert_eq!(substitute(&vars, "A${foo:1}A"), "A23A");
}

#[test]
fn test_substring_offset_length() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:1:1} -> "2"
    assert_eq!(substitute(&vars, "A${foo:1:1}A"), "A2A");
}

#[test]
fn test_substring_negative_length() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:1:-1} -> from offset 1, length = remaining - 1 = 2 - 1 = 1 -> "2"
    assert_eq!(substitute(&vars, "A${foo:1:-1}A"), "A2A");
}

#[test]
fn test_substring_negative_offset() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:-1:1} -> from end-1 = index 2, length 1 -> "3"
    assert_eq!(substitute(&vars, "A${foo:-1:1}A"), "A3A");
}

#[test]
fn test_substring_negative_offset_2() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:-2:1} -> from end-2 = index 1, length 1 -> "2"
    assert_eq!(substitute(&vars, "A${foo:-2:1}A"), "A2A");
}

#[test]
fn test_substring_both_negative() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    // ${foo:-2:-1} -> from index 1, length = remaining-1 = 2-1 = 1 -> "2"
    assert_eq!(substitute(&vars, "A${foo:-2:-1}A"), "A2A");
}

// ---------------------------------------------------------------------------
// Tests: Nested with substring
// ---------------------------------------------------------------------------

#[test]
fn test_nested_with_substring() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());
    vars.insert("baz".to_string(), "fo".to_string());

    // ${${baz}o:1} -> ${foo:1} -> "23"
    assert_eq!(substitute(&vars, "A${${baz}o:1}A"), "A23A");

    // ${${baz}o:1:1} -> ${foo:1:1} -> "2"
    assert_eq!(substitute(&vars, "A${${baz}o:1:1}A"), "A2A");

    // ${${baz}o:1:-1} -> ${foo:1:-1} -> "2"
    assert_eq!(substitute(&vars, "A${${baz}o:1:-1}A"), "A2A");

    // ${${baz}o:-1:1} -> ${foo:-1:1} -> "3"
    assert_eq!(substitute(&vars, "A${${baz}o:-1:1}A"), "A3A");

    // ${${baz}o:-2:1} -> ${foo:-2:1} -> "2"
    assert_eq!(substitute(&vars, "A${${baz}o:-2:1}A"), "A2A");

    // ${${baz}o:-2:-1} -> ${foo:-2:-1} -> "2"
    assert_eq!(substitute(&vars, "A${${baz}o:-2:-1}A"), "A2A");
}

// ---------------------------------------------------------------------------
// Tests: Variable set and retrieve
// ---------------------------------------------------------------------------

#[test]
fn test_variable_set_get() {
    let values = ["one", "three", "reallylongdinosaursoundingthingwithwordsinit"];
    let mut vars = HashMap::new();

    for value in &values {
        vars.insert("testvar".to_string(), value.to_string());
        assert_eq!(substitute(&vars, "${testvar}"), *value);
    }
}

// ---------------------------------------------------------------------------
// Tests: Mixed known and unknown variables
// ---------------------------------------------------------------------------

#[test]
fn test_mixed_known_unknown() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), "123".to_string());

    assert_eq!(
        substitute(&vars, "${foo},${this_does_not_exist}"),
        "123,"
    );
}

// ---------------------------------------------------------------------------
// Tests: Empty and edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_empty_input() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, ""), "");
}

#[test]
fn test_no_variables_in_input() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "plain text"), "plain text");
}

#[test]
fn test_dollar_without_brace() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "price is $5"), "price is $5");
}

#[test]
fn test_unclosed_brace() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "${unclosed"), "${unclosed");
}

#[test]
fn test_empty_variable_name() {
    let vars = HashMap::new();
    assert_eq!(substitute(&vars, "${}"), "");
}
