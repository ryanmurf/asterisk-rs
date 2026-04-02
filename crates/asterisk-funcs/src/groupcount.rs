//! GROUP/GROUP_COUNT functions - channel grouping and counting.
//!
//! Port of func_groupcount.c from Asterisk C.
//!
//! Provides:
//! - GROUP(category) - set/get group membership for a channel
//! - GROUP_COUNT(group@category) - count channels in a group
//! - GROUP_LIST(category) - list all groups in a category
//! - GROUP_MATCH_COUNT(pattern@category) - count matching groups

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// GROUP() function.
///
/// Sets or gets the group that this channel belongs to in a given category.
///
/// Read usage:  GROUP([category]) - get current group name
/// Write usage: Set(GROUP(category)=groupname) - set group membership
///
/// The default category is "" (empty string).
pub struct FuncGroup;

impl DialplanFunc for FuncGroup {
    fn name(&self) -> &str {
        "GROUP"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let category = if args.trim().is_empty() {
            ""
        } else {
            args.trim()
        };

        let key = group_var_name(category);
        Ok(ctx.get_variable(&key).cloned().unwrap_or_default())
    }

    fn write(&self, ctx: &mut FuncContext, args: &str, value: &str) -> Result<(), FuncError> {
        let category = if args.trim().is_empty() {
            ""
        } else {
            args.trim()
        };

        let key = group_var_name(category);
        if value.trim().is_empty() {
            // Removing from group
            ctx.variables.remove(&key);
        } else {
            ctx.set_variable(&key, value.trim());
        }

        // Also maintain a list of all groups in this category
        let list_key = group_list_var_name(category);
        let mut groups = parse_group_list(ctx.get_variable(&list_key));
        let group_name = value.trim().to_string();
        if !group_name.is_empty() && !groups.contains(&group_name) {
            groups.push(group_name);
        }
        ctx.set_variable(&list_key, &groups.join(","));

        Ok(())
    }
}

/// GROUP_COUNT() function.
///
/// Returns the number of channels in the specified group@category.
///
/// Usage: GROUP_COUNT(group[@category])
///
/// In a single-channel context (no global state), this returns 1 if
/// the current channel is in the specified group, 0 otherwise.
/// A full implementation would query across all active channels.
pub struct FuncGroupCount;

impl DialplanFunc for FuncGroupCount {
    fn name(&self) -> &str {
        "GROUP_COUNT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (group, category) = parse_group_at_category(args);

        // Check the stored count variable first (for testing / multi-channel scenarios)
        let count_key = format!("__GROUP_COUNT_{}@{}", group, category);
        if let Some(count) = ctx.get_variable(&count_key) {
            return Ok(count.clone());
        }

        // Fall back to checking if this channel is in the group
        let key = group_var_name(&category);
        let current_group = ctx.get_variable(&key).cloned().unwrap_or_default();
        if current_group == group {
            Ok("1".to_string())
        } else {
            Ok("0".to_string())
        }
    }
}

/// GROUP_LIST() function.
///
/// Returns a space-separated list of all group@category pairs for this channel.
///
/// Usage: GROUP_LIST([category])
pub struct FuncGroupList;

impl DialplanFunc for FuncGroupList {
    fn name(&self) -> &str {
        "GROUP_LIST"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let filter_category = args.trim();

        // Collect all __GROUP_<cat> variables
        let mut result = Vec::new();
        for (key, value) in &ctx.variables {
            if let Some(cat) = key.strip_prefix("__GROUP_") {
                if cat.starts_with("LIST_") {
                    continue; // Skip list metadata
                }
                if (filter_category.is_empty() || cat == filter_category)
                    && !value.is_empty() {
                        result.push(format!("{}@{}", value, cat));
                    }
            }
        }

        Ok(result.join(" "))
    }
}

/// GROUP_MATCH_COUNT() function.
///
/// Returns the count of channels matching a group pattern within a category.
///
/// Usage: GROUP_MATCH_COUNT(pattern[@category])
///
/// The pattern supports simple glob matching (* and ?).
pub struct FuncGroupMatchCount;

impl DialplanFunc for FuncGroupMatchCount {
    fn name(&self) -> &str {
        "GROUP_MATCH_COUNT"
    }

    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult {
        let (pattern, category) = parse_group_at_category(args);

        // Check stored match count first
        let count_key = format!("__GROUP_MATCH_COUNT_{}@{}", pattern, category);
        if let Some(count) = ctx.get_variable(&count_key) {
            return Ok(count.clone());
        }

        // Fall back to checking this channel's group against the pattern
        let key = group_var_name(&category);
        let current_group = ctx.get_variable(&key).cloned().unwrap_or_default();

        if glob_match(&pattern, &current_group) {
            Ok("1".to_string())
        } else {
            Ok("0".to_string())
        }
    }
}

// ---- helpers ----

/// Build the variable name for storing a channel's group in a category.
fn group_var_name(category: &str) -> String {
    if category.is_empty() {
        "__GROUP_".to_string()
    } else {
        format!("__GROUP_{}", category)
    }
}

/// Build the variable name for storing the list of groups in a category.
fn group_list_var_name(category: &str) -> String {
    if category.is_empty() {
        "__GROUP_LIST_".to_string()
    } else {
        format!("__GROUP_LIST_{}", category)
    }
}

/// Parse a comma-separated group list.
fn parse_group_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse "group@category" notation.
/// Returns (group, category) where category defaults to "".
fn parse_group_at_category(args: &str) -> (String, String) {
    let args = args.trim();
    if let Some(at_pos) = args.find('@') {
        let group = args[..at_pos].trim().to_string();
        let category = args[at_pos + 1..].trim().to_string();
        (group, category)
    } else {
        (args.to_string(), String::new())
    }
}

/// Simple glob-style pattern matching supporting * and ?.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_set_and_get() {
        let mut ctx = FuncContext::new();
        let func = FuncGroup;
        func.write(&mut ctx, "lines", "line1").unwrap();
        assert_eq!(func.read(&ctx, "lines").unwrap(), "line1");
    }

    #[test]
    fn test_group_count() {
        let mut ctx = FuncContext::new();
        let group = FuncGroup;
        let count = FuncGroupCount;
        group.write(&mut ctx, "lines", "line1").unwrap();
        assert_eq!(count.read(&ctx, "line1@lines").unwrap(), "1");
        assert_eq!(count.read(&ctx, "line2@lines").unwrap(), "0");
    }

    #[test]
    fn test_group_list() {
        let mut ctx = FuncContext::new();
        let group = FuncGroup;
        let list = FuncGroupList;
        group.write(&mut ctx, "lines", "line1").unwrap();
        let result = list.read(&ctx, "").unwrap();
        assert!(result.contains("line1@lines"));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("line*", "line1"));
        assert!(glob_match("line*", "line99"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("line?", "line1"));
        assert!(!glob_match("line?", "line12"));
        assert!(glob_match("l*1", "line1"));
        assert!(!glob_match("line*", "other"));
    }

    #[test]
    fn test_group_match_count() {
        let mut ctx = FuncContext::new();
        let group = FuncGroup;
        let match_count = FuncGroupMatchCount;
        group.write(&mut ctx, "lines", "line1").unwrap();
        assert_eq!(match_count.read(&ctx, "line*@lines").unwrap(), "1");
        assert_eq!(match_count.read(&ctx, "other*@lines").unwrap(), "0");
    }
}
